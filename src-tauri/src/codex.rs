//! Chat handled by OpenAI's Codex CLI, for people who use Codex instead of Claude Code.
//!
//! Each message runs `codex exec` once, resuming the saved thread, with the app's tools over the
//! same localhost MCP server Claude uses. Codex's own tools (shell, browser, image generation…) are
//! turned off and its sandbox is read-only, so it can only work through the app's tools. There is
//! no calendar connector for Codex, so schedules are saved in the app only.
//!
//! Not yet tried with a real Codex account: the event format and flags follow Codex CLI 0.154.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::claude::{status_for, stamped, Turn, SYSTEM_PROMPT};
use crate::mcp::Endpoint;
use crate::tools::SERVER_NAME;

const TURN_TIMEOUT: Duration = Duration::from_secs(300);
const ONE_SHOT_TIMEOUT: Duration = Duration::from_secs(180);
/// Where the thread id is kept, so the conversation continues after the app restarts.
const THREAD_FILE: &str = "thread-id";
/// The day the saved thread started; a new day starts a new thread.
const THREAD_DATE_FILE: &str = "thread-date";
/// The environment variable Codex reads the app's MCP token from.
const TOKEN_VARIABLE: &str = "SUK_MCP_TOKEN";
/// Codex tools that have nothing to do with this app.
const DISABLED_FEATURES: &[&str] = &[
    "shell_tool",
    "unified_exec",
    "browser_use",
    "browser_use_external",
    "computer_use",
    "in_app_browser",
    "image_generation",
    "view_image",
    "apps",
    "plugins",
    "multi_agent",
    "hooks",
    "memories",
    "skill_search",
    "tool_suggest",
    "goals",
];
/// Added to the shared instructions: Codex has no calendar.
const CODEX_NOTE: &str = "\n\nThis app is running on Codex, which has no Google Calendar tools here. When planning, say Google Calendar isn't connected (it needs Claude Code), and still propose blocks with propose_schedule; confirmed blocks are saved in the app.";

#[derive(Debug, Clone)]
pub struct Setup {
    pub binary: PathBuf,
    /// An empty directory to run in, so no project files or instructions are picked up.
    pub workdir: PathBuf,
    pub mcp: Endpoint,
    /// Added to the instructions: what kind of work the user does.
    pub about_user: String,
}

#[derive(Default)]
pub struct Codex {
    /// Held for a whole turn: one conversation, one turn at a time.
    thread: Mutex<Option<String>>,
}

impl Codex {
    /// Forgets the current thread, so the next message starts a new conversation.
    pub fn start_over(&self, setup: &Setup) {
        *self.thread.lock().unwrap_or_else(|e| e.into_inner()) = None;
        let _ = std::fs::remove_file(setup.workdir.join(THREAD_FILE));
    }

    /// Starts a new thread on a new day. Returns whether the next message begins a new thread.
    pub fn begins_fresh(&self, setup: &Setup) -> bool {
        let mut thread = self.thread.lock().unwrap_or_else(|e| e.into_inner());
        let saved = thread.is_some() || setup.workdir.join(THREAD_FILE).exists();
        let today = chrono::Local::now().format("%Y-%m-%d").to_string();
        let started = std::fs::read_to_string(setup.workdir.join(THREAD_DATE_FILE)).unwrap_or_default();
        if saved && started.trim() != today {
            *thread = None;
            let _ = std::fs::remove_file(setup.workdir.join(THREAD_FILE));
            return true;
        }
        !saved
    }

    /// Sends a message and waits for Codex's reply, reporting progress through `on_status`.
    pub fn send(&self, setup: &Setup, message: &str, on_status: &mut dyn FnMut(&str)) -> Result<Turn, String> {
        let mut thread = self.thread.lock().unwrap_or_else(|e| e.into_inner());
        std::fs::create_dir_all(&setup.workdir).map_err(|e| e.to_string())?;
        if thread.is_none() {
            *thread = std::fs::read_to_string(setup.workdir.join(THREAD_FILE)).ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
        }
        let content = stamped(message);
        let result = run_turn(setup, thread.as_deref(), &content, on_status);
        match result {
            Ok((id, text)) => {
                if let Some(id) = id.filter(|id| thread.as_deref() != Some(id)) {
                    let _ = std::fs::write(setup.workdir.join(THREAD_FILE), &id);
                    let _ = std::fs::write(setup.workdir.join(THREAD_DATE_FILE), chrono::Local::now().format("%Y-%m-%d").to_string());
                    *thread = Some(id);
                }
                Ok(Turn { text, created_events: Vec::new(), usage: Default::default() })
            }
            // A saved thread that can't be resumed: start a new one once.
            Err(e) if thread.is_some() && e.contains("thread") => {
                eprintln!("codex: couldn't resume the saved thread ({e}); starting a new one");
                *thread = None;
                let _ = std::fs::remove_file(setup.workdir.join(THREAD_FILE));
                let (id, text) = run_turn(setup, None, &content, on_status)?;
                if let Some(id) = id {
                    let _ = std::fs::write(setup.workdir.join(THREAD_FILE), &id);
                    let _ = std::fs::write(setup.workdir.join(THREAD_DATE_FILE), chrono::Local::now().format("%Y-%m-%d").to_string());
                    *thread = Some(id);
                }
                Ok(Turn { text, created_events: Vec::new(), usage: Default::default() })
            }
            Err(e) => Err(e),
        }
    }
}

fn run_turn(setup: &Setup, thread: Option<&str>, content: &str, on_status: &mut dyn FnMut(&str)) -> Result<(Option<String>, String), String> {
    let mut child = Command::new(&setup.binary)
        .args(arguments(setup, thread))
        .current_dir(&setup.workdir)
        .env(TOKEN_VARIABLE, &setup.mcp.token)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("couldn't start Codex: {e}"))?;
    let mut stdin = child.stdin.take().ok_or("no stdin")?;
    stdin.write_all(content.as_bytes()).map_err(|e| e.to_string())?;
    drop(stdin);
    let stdout = child.stdout.take().ok_or("no stdout")?;
    let (sender, events) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if sender.send(line).is_err() {
                break;
            }
        }
    });
    let deadline = Instant::now() + TURN_TIMEOUT;
    let mut reader = EventReader::default();
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        match events.recv_timeout(remaining) {
            Ok(line) => {
                if let Some(status) = reader.read(&line) {
                    on_status(status);
                }
            }
            Err(RecvTimeoutError::Timeout) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("Codex took too long to answer".into());
            }
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }
    let exit = child.wait().map_err(|e| e.to_string())?;
    reader.finish(exit.success())
}

/// Reads `codex exec --json` events: the thread id, tool calls for progress, and the reply.
#[derive(Default, Debug)]
struct EventReader {
    thread: Option<String>,
    texts: Vec<String>,
    failure: Option<String>,
    completed: bool,
    last_error: Option<String>,
}

impl EventReader {
    /// Takes one line of output; returns a status to show, if any.
    fn read(&mut self, line: &str) -> Option<&'static str> {
        let event: Value = serde_json::from_str(line).ok()?;
        match event["type"].as_str().unwrap_or_default() {
            "thread.started" => self.thread = event["thread_id"].as_str().map(String::from),
            "item.started" if event["item"]["type"] == "mcp_tool_call" => {
                let tool = event["item"]["tool"].as_str().unwrap_or_default();
                return Some(status_for(&format!("mcp__{SERVER_NAME}__{tool}")));
            }
            "item.completed" if event["item"]["type"] == "agent_message" => {
                if let Some(text) = event["item"]["text"].as_str().map(str::trim).filter(|t| !t.is_empty()) {
                    self.texts.push(text.to_string());
                }
            }
            "turn.completed" => self.completed = true,
            "turn.failed" => {
                self.failure = Some(event["error"]["message"].as_str().unwrap_or("the turn failed").to_string());
            }
            "error" => self.last_error = event["message"].as_str().map(String::from),
            _ => {}
        }
        None
    }

    fn finish(self, exited_ok: bool) -> Result<(Option<String>, String), String> {
        if let Some(failure) = self.failure {
            return Err(format!("Codex couldn't answer: {failure}"));
        }
        if !self.completed || !exited_ok {
            let detail = self.last_error.unwrap_or_else(|| "it stopped unexpectedly".into());
            return Err(format!("Codex couldn't answer: {detail}"));
        }
        Ok((self.thread, self.texts.join("\n\n")))
    }
}

/// A TOML string for `-c key=value`; JSON string escapes are valid TOML.
fn toml_string(value: &str) -> String {
    serde_json::to_string(value).unwrap_or_default()
}

fn shared_flags(workdir: &Path) -> Vec<String> {
    let mut args: Vec<String> = [
        "--json",
        "--skip-git-repo-check",
        "--ignore-user-config",
        "--ignore-rules",
        "--sandbox",
        "read-only",
        "--color",
        "never",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    args.extend(["--cd".into(), workdir.display().to_string()]);
    args.extend(["-c".into(), "approval_policy=\"never\"".into()]);
    for feature in DISABLED_FEATURES {
        args.extend(["--disable".into(), feature.to_string()]);
    }
    args
}

fn arguments(setup: &Setup, thread: Option<&str>) -> Vec<String> {
    let mut args = vec!["exec".to_string()];
    args.extend(shared_flags(&setup.workdir));
    let server = format!("mcp_servers.{SERVER_NAME}");
    args.extend([
        "-c".into(),
        format!("developer_instructions={}", toml_string(&format!("{SYSTEM_PROMPT}{CODEX_NOTE}{}", setup.about_user))),
        "-c".into(),
        format!("{server}.url={}", toml_string(&setup.mcp.url)),
        "-c".into(),
        format!("{server}.bearer_token_env_var={}", toml_string(TOKEN_VARIABLE)),
        "-c".into(),
        format!("{server}.default_tools_approval_mode=\"approve\""),
        "-c".into(),
        format!("{server}.tool_timeout_sec=120"),
    ]);
    if let Some(thread) = thread {
        args.extend(["resume".into(), thread.into()]);
    }
    // The message comes on stdin.
    args.push("-".into());
    args
}

/// Asks Codex one question whose answer must match `schema`, without the app's tools or a saved
/// thread. Used for background work such as judging updates.
pub fn ask_json(binary: &Path, workdir: &Path, system: &str, prompt: &str, schema: &Value) -> Result<Value, String> {
    std::fs::create_dir_all(workdir).map_err(|e| e.to_string())?;
    let schema_file = workdir.join("schema.json");
    let answer_file = workdir.join("answer.json");
    std::fs::write(&schema_file, schema.to_string()).map_err(|e| e.to_string())?;
    let _ = std::fs::remove_file(&answer_file);
    let mut args = vec!["exec".to_string()];
    args.extend(shared_flags(workdir));
    args.extend([
        "--ephemeral".into(),
        "-c".into(),
        format!("developer_instructions={}", toml_string(system)),
        "--output-schema".into(),
        schema_file.display().to_string(),
        "--output-last-message".into(),
        answer_file.display().to_string(),
        "-".into(),
    ]);
    let mut child = Command::new(binary)
        .args(&args)
        .current_dir(workdir)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("couldn't start Codex: {e}"))?;
    let mut stdin = child.stdin.take().ok_or("no stdin")?;
    stdin.write_all(prompt.as_bytes()).map_err(|e| e.to_string())?;
    drop(stdin);
    let deadline = Instant::now() + ONE_SHOT_TIMEOUT;
    let exit = loop {
        if let Some(exit) = child.try_wait().map_err(|e| e.to_string())? {
            break exit;
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err("Codex took too long to answer".into());
        }
        std::thread::sleep(Duration::from_millis(200));
    };
    let answer = std::fs::read_to_string(&answer_file).map_err(|_| format!("Codex couldn't answer (exit {exit})"))?;
    serde_json::from_str(answer.trim()).map_err(|_| "Codex didn't give a structured answer".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup() -> Setup {
        Setup {
            binary: "codex".into(),
            workdir: "/tmp/suk-codex".into(),
            mcp: Endpoint { url: "http://127.0.0.1:4000/mcp".into(), token: "secret".into() },
            about_user: String::new(),
        }
    }

    #[test]
    fn codex_runs_locked_down_with_only_the_app_tools() {
        let args = arguments(&setup(), None);
        let joined = args.join(" ");
        assert_eq!(args[0], "exec");
        assert_eq!(args.last().unwrap(), "-");
        for expected in ["--json", "--sandbox read-only", "--ignore-user-config", "approval_policy=\"never\"", "--disable shell_tool", "--disable browser_use"] {
            assert!(joined.contains(expected), "{expected}");
        }
        assert!(joined.contains("mcp_servers.suk.url=\"http://127.0.0.1:4000/mcp\""));
        assert!(joined.contains("mcp_servers.suk.bearer_token_env_var=\"SUK_MCP_TOKEN\""));
        // The token itself never appears on the command line.
        assert!(!joined.contains("secret"));
        let instructions = args.iter().find(|a| a.starts_with("developer_instructions=")).unwrap();
        let prompt: String = serde_json::from_str(&instructions["developer_instructions=".len()..]).unwrap();
        assert!(prompt.starts_with("You are the assistant inside Suk") && prompt.contains("no Google Calendar tools"));
        assert!(!joined.contains(" resume "));

        let resumed = arguments(&setup(), Some("01a0af2b-a5f9"));
        let at = resumed.iter().position(|a| a == "resume").unwrap();
        assert_eq!((resumed[at + 1].as_str(), resumed[at + 2].as_str()), ("01a0af2b-a5f9", "-"));
        assert!(resumed[..at].contains(&"--json".to_string()), "flags come before resume");
    }

    /// Runs the real Codex CLI with the app's arguments against a signed-out CODEX_HOME, checking
    /// every flag, feature and config key is accepted (`--strict-config`) and the turn fails only
    /// for lack of sign-in. `CODEX_BIN=/path/to/codex cargo test --lib real_codex_accepts -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn real_codex_accepts_the_app_arguments() {
        let binary = std::env::var("CODEX_BIN").expect("set CODEX_BIN");
        let home = std::env::temp_dir().join(format!("suk-codex-home-{}", std::process::id()));
        let workdir = std::env::temp_dir().join(format!("suk-codex-work-{}", std::process::id()));
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(&workdir).unwrap();
        // A stand-in MCP server that records the Authorization header Codex sends.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let (sender, headers) = mpsc::channel();
        std::thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut buffer = [0u8; 8192];
                let n = std::io::Read::read(&mut stream, &mut buffer).unwrap_or(0);
                let _ = sender.send(String::from_utf8_lossy(&buffer[..n]).to_string());
                let _ = stream.write_all(b"HTTP/1.1 500 Internal Server Error\r\ncontent-length: 0\r\n\r\n");
            }
        });
        let setup = Setup { binary: binary.into(), workdir: workdir.clone(), mcp: Endpoint { url: format!("http://127.0.0.1:{port}/mcp"), token: "t".into() }, about_user: String::new() };
        let mut args = arguments(&setup, None);
        args.insert(1, "--strict-config".into());
        let out = Command::new(&setup.binary)
            .args(&args)
            .env("CODEX_HOME", &home)
            .env(TOKEN_VARIABLE, "token-from-env")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .and_then(|mut child| {
                child.stdin.take().unwrap().write_all(b"hello")?;
                child.wait_with_output()
            })
            .unwrap();
        let stdout = String::from_utf8_lossy(&out.stdout);
        let stderr = String::from_utf8_lossy(&out.stderr);
        println!("exit {:?}\nstdout:\n{stdout}\nstderr (tail):\n{}", out.status.code(), stderr.lines().rev().take(15).collect::<Vec<_>>().join("\n"));
        let request = headers.recv_timeout(Duration::from_secs(1)).expect("Codex connected to the app's MCP server");
        println!("request to MCP server:\n{}", request.lines().take(12).collect::<Vec<_>>().join("\n"));
        assert!(request.to_lowercase().contains("authorization: bearer token-from-env"), "token sent");
        let mut reader = EventReader::default();
        for line in stdout.lines() {
            reader.read(line);
        }
        assert!(reader.thread.is_some(), "Codex started a thread");
        let error = reader.finish(out.status.success()).unwrap_err();
        assert!(error.contains("401") || error.to_lowercase().contains("auth") || error.to_lowercase().contains("log"), "{error}");
        let _ = std::fs::remove_dir_all(&home);
        let _ = std::fs::remove_dir_all(&workdir);
    }

    #[test]
    fn replies_tool_calls_and_failures_are_read_from_events() {
        let mut reader = EventReader::default();
        let lines = [
            r#"{"type":"thread.started","thread_id":"01a0af2b-a5f9-7bf2-aad8-383a88299a5e"}"#,
            r#"{"type":"turn.started"}"#,
            "2026-09-17T11:41:09Z ERROR not json",
            r#"{"type":"item.started","item":{"id":"1","type":"mcp_tool_call","server":"suk","tool":"save","arguments":{},"status":"in_progress"}}"#,
            r#"{"type":"item.completed","item":{"id":"1","type":"mcp_tool_call","server":"suk","tool":"save","status":"completed"}}"#,
            r#"{"type":"item.completed","item":{"id":"2","type":"agent_message","text":"Saved Amit as your PhD student."}}"#,
            r#"{"type":"turn.completed","usage":{"input_tokens":10,"output_tokens":5}}"#,
        ];
        let statuses: Vec<_> = lines.iter().filter_map(|l| reader.read(l)).collect();
        assert_eq!(statuses, vec!["Saving…"]);
        let (thread, text) = reader.finish(true).unwrap();
        assert_eq!(thread.as_deref(), Some("01a0af2b-a5f9-7bf2-aad8-383a88299a5e"));
        assert_eq!(text, "Saved Amit as your PhD student.");

        // Real output when not signed in: retries, then the turn fails.
        let mut reader = EventReader::default();
        for line in [
            r#"{"type":"thread.started","thread_id":"x"}"#,
            r#"{"type":"error","message":"Reconnecting... 2/5 (unexpected status 401 Unauthorized: Missing bearer or basic authentication in header)"}"#,
        ] {
            reader.read(line);
        }
        assert!(reader.finish(false).unwrap_err().contains("401 Unauthorized"));
        let mut reader = EventReader::default();
        reader.read(r#"{"type":"turn.failed","error":{"message":"usage limit reached"}}"#);
        assert_eq!(reader.finish(true).unwrap_err(), "Codex couldn't answer: usage limit reached");
    }
}
