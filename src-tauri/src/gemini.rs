//! Chat handled by Google's Gemini CLI, for people who use it instead of Claude Code or Codex.
//!
//! Each message runs `gemini -p` once in an empty folder of the app's own, with the app's tools
//! over the same localhost MCP server and Gemini's own tools (shell, files, web) switched off.
//! The instructions go in a file Gemini reads as its system prompt. There is no calendar
//! connector, so schedules are saved in the app only.
//!
//! Not yet tried with a real Gemini account: the flags and output shape follow Gemini CLI 0.60.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::claude::{stamped, Turn, SYSTEM_PROMPT};
use crate::mcp::Endpoint;
use crate::tools::SERVER_NAME;

const TURN_TIMEOUT: Duration = Duration::from_secs(300);
/// Where the session id is kept, so the conversation continues after the app restarts.
const SESSION_FILE: &str = "session-id";
/// The day the saved session started; a new day starts a new one.
const SESSION_DATE_FILE: &str = "session-date";
/// Gemini's own tools, which have nothing to do with this app.
const EXCLUDED_TOOLS: &[&str] = &[
    "run_shell_command",
    "write_file",
    "replace",
    "read_file",
    "read_many_files",
    "list_directory",
    "glob",
    "search_file_content",
    "web_fetch",
    "google_web_search",
    "save_memory",
];
/// Added to the shared instructions: Gemini has no calendar here.
const GEMINI_NOTE: &str = "\n\nThis app is running on Gemini, which has no Google Calendar tools here. When planning, say Google Calendar isn't connected (it needs Claude Code), and still propose blocks with propose_schedule; confirmed blocks are saved in the app.";

#[derive(Debug, Clone)]
pub struct Setup {
    pub binary: PathBuf,
    /// An empty directory to run in, holding the settings and instructions Gemini reads.
    pub workdir: PathBuf,
    pub mcp: Endpoint,
    /// Added to the instructions: what kind of work the user does.
    pub about_user: String,
}

#[derive(Default)]
pub struct Gemini {
    /// Held for a whole turn: one conversation, one turn at a time.
    session: Mutex<Option<String>>,
}

impl Gemini {
    /// Starts a new session on a new day. Returns whether the next message begins a new one.
    pub fn begins_fresh(&self, setup: &Setup) -> bool {
        let mut session = self.session.lock().unwrap_or_else(|e| e.into_inner());
        let saved = session.is_some() || setup.workdir.join(SESSION_FILE).exists();
        let today = chrono::Local::now().format("%Y-%m-%d").to_string();
        let started = std::fs::read_to_string(setup.workdir.join(SESSION_DATE_FILE)).unwrap_or_default();
        if saved && started.trim() != today {
            *session = None;
            let _ = std::fs::remove_file(setup.workdir.join(SESSION_FILE));
            return true;
        }
        !saved
    }

    /// Forgets the current session, so the next message starts a new conversation.
    pub fn start_over(&self, setup: &Setup) {
        *self.session.lock().unwrap_or_else(|e| e.into_inner()) = None;
        let _ = std::fs::remove_file(setup.workdir.join(SESSION_FILE));
    }

    /// Sends a message and waits for Gemini's reply.
    pub fn send(&self, setup: &Setup, message: &str, on_status: &mut dyn FnMut(&str)) -> Result<Turn, String> {
        let mut session = self.session.lock().unwrap_or_else(|e| e.into_inner());
        prepare(setup)?;
        if session.is_none() {
            *session = std::fs::read_to_string(setup.workdir.join(SESSION_FILE)).ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
        }
        let content = stamped(message);
        on_status("Thinking…");
        let started = session.clone();
        let (id, text) = run_turn(setup, started.as_deref(), &content)?;
        if let Some(id) = id.filter(|id| session.as_deref() != Some(id)) {
            let _ = std::fs::write(setup.workdir.join(SESSION_FILE), &id);
            let _ = std::fs::write(setup.workdir.join(SESSION_DATE_FILE), chrono::Local::now().format("%Y-%m-%d").to_string());
            *session = Some(id);
        }
        Ok(Turn { text, created_events: Vec::new(), usage: Default::default() })
    }
}

/// Writes the settings and instructions Gemini reads from the folder it runs in.
fn prepare(setup: &Setup) -> Result<(), String> {
    std::fs::create_dir_all(setup.workdir.join(".gemini")).map_err(|e| e.to_string())?;
    let settings = json!({
        "mcpServers": {
            SERVER_NAME: {
                "httpUrl": setup.mcp.url,
                "headers": { "Authorization": format!("Bearer {}", setup.mcp.token) },
                "trust": true,
            }
        },
        "excludeTools": EXCLUDED_TOOLS,
        "hideBanner": true,
        "usageStatisticsEnabled": false,
    });
    std::fs::write(setup.workdir.join(".gemini/settings.json"), settings.to_string()).map_err(|e| e.to_string())?;
    std::fs::write(instructions_file(&setup.workdir), format!("{SYSTEM_PROMPT}{GEMINI_NOTE}{}", setup.about_user)).map_err(|e| e.to_string())
}

fn instructions_file(workdir: &Path) -> PathBuf {
    workdir.join("instructions.md")
}

fn arguments(message: &str, session: Option<&str>, new_id: &str) -> Vec<String> {
    let mut args: Vec<String> = ["-p", message, "-o", "json", "--approval-mode", "yolo", "--skip-trust"].iter().map(|s| s.to_string()).collect();
    args.extend(["--allowed-mcp-server-names".into(), SERVER_NAME.to_string()]);
    match session {
        // Sessions belong to the folder Gemini runs in, which is the app's own, so the most
        // recent one there is this conversation.
        Some(_) => args.extend(["--resume".into(), "latest".into()]),
        None => args.extend(["--session-id".into(), new_id.to_string()]),
    }
    args
}

fn run_turn(setup: &Setup, session: Option<&str>, content: &str) -> Result<(Option<String>, String), String> {
    let new_id = uuid::Uuid::new_v4().to_string();
    let mut child = Command::new(&setup.binary)
        .args(arguments(content, session, &new_id))
        .current_dir(&setup.workdir)
        .env("GEMINI_SYSTEM_MD", instructions_file(&setup.workdir))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("couldn't start Gemini: {e}"))?;
    let mut stdout = child.stdout.take().ok_or("no stdout")?;
    let mut stderr = child.stderr.take().ok_or("no stderr")?;
    let reader = std::thread::spawn(move || {
        let mut out = String::new();
        let _ = stdout.read_to_string(&mut out);
        out
    });
    let errors = std::thread::spawn(move || {
        let mut out = String::new();
        let _ = stderr.read_to_string(&mut out);
        out
    });
    let deadline = Instant::now() + TURN_TIMEOUT;
    loop {
        match child.try_wait().map_err(|e| e.to_string())? {
            Some(_) => break,
            None if Instant::now() > deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("Gemini took too long to answer".into());
            }
            None => std::thread::sleep(Duration::from_millis(200)),
        }
    }
    let out = reader.join().unwrap_or_default();
    let errors = errors.join().unwrap_or_default();
    read_reply(&out, &errors, &new_id)
}

/// `gemini -o json` prints {session_id, response, stats} or {error: {message}}.
fn read_reply(stdout: &str, stderr: &str, new_id: &str) -> Result<(Option<String>, String), String> {
    let start = stdout.find('{');
    let parsed: Option<Value> = start.and_then(|i| serde_json::from_str(stdout[i..].trim()).ok());
    let Some(value) = parsed else {
        return Err(format!("Gemini couldn't answer: {}", complaint(stderr)));
    };
    if let Some(message) = value["error"]["message"].as_str() {
        return Err(format!("Gemini couldn't answer: {}", message.chars().take(300).collect::<String>()));
    }
    let text = value["response"].as_str().unwrap_or_default().trim().to_string();
    if text.is_empty() {
        return Err("Gemini couldn't answer: it replied with nothing".into());
    }
    let id = value["session_id"].as_str().unwrap_or(new_id).to_string();
    Ok((Some(id), text))
}

/// What went wrong, from Gemini's error output: the message it reported, or the last line that
/// says something.
fn complaint(stderr: &str) -> String {
    // Errors come back as JSON inside JSON; unescaping once makes the innermost message readable.
    let text = crate::assistant::strip_ansi(stderr).replace("\\n", "\n").replace("\\\"", "\"");
    let messages: Vec<String> = text
        .match_indices("\"message\"")
        .filter_map(|(i, _)| {
            let rest = &text[i..];
            let value = rest[rest.find(':')? + 1..].trim_start().strip_prefix('"')?;
            Some(value[..value.find('"').unwrap_or(value.len())].trim().to_string())
        })
        .filter(|m| m.len() > 8 && !m.starts_with('{'))
        .collect();
    let line = messages.last().cloned().or_else(|| {
        text.lines()
            .map(str::trim)
            .filter(|l| l.len() > 8 && !l.starts_with("at ") && !l.starts_with(['{', '}', '[', ']', '"']))
            .next_back()
            .map(String::from)
    });
    line.unwrap_or_else(|| "it stopped unexpectedly".into()).chars().take(300).collect()
}

/// Asks Gemini one question whose answer must be JSON matching `schema`. Gemini has no schema
/// flag, so the shape is asked for in the prompt and the answer is read out of the reply.
pub fn ask_json(binary: &Path, workdir: &Path, system: &str, prompt: &str, schema: &Value) -> Result<Value, String> {
    std::fs::create_dir_all(workdir).map_err(|e| e.to_string())?;
    let instructions = workdir.join("instructions.md");
    std::fs::write(&instructions, format!("{system}\n\nAnswer with JSON only, matching this schema, and nothing else:\n{schema}"))
        .map_err(|e| e.to_string())?;
    let out = Command::new(binary)
        .args(["-p", prompt, "-o", "json", "--approval-mode", "plan", "--skip-trust"])
        .current_dir(workdir)
        .env("GEMINI_SYSTEM_MD", &instructions)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("couldn't start Gemini: {e}"))?;
    let (_, text) = read_reply(&String::from_utf8_lossy(&out.stdout), &String::from_utf8_lossy(&out.stderr), "")?;
    let json = text.trim().trim_start_matches("```json").trim_start_matches("```").trim_end_matches("```").trim();
    let start = json.find('{').unwrap_or(0);
    serde_json::from_str(&json[start..]).map_err(|_| "Gemini didn't answer with JSON".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup(workdir: &Path) -> Setup {
        Setup {
            binary: "gemini".into(),
            workdir: workdir.to_path_buf(),
            mcp: Endpoint { url: "http://127.0.0.1:4000/mcp".into(), token: "secret".into() },
            about_user: "\n\nThe user is an academic.".into(),
        }
    }

    #[test]
    fn gemini_runs_with_only_the_app_tools_and_its_own_instructions() {
        let dir = std::env::temp_dir().join(format!("suk-gemini-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let setup = setup(&dir);
        prepare(&setup).unwrap();
        let settings: Value = serde_json::from_str(&std::fs::read_to_string(dir.join(".gemini/settings.json")).unwrap()).unwrap();
        assert_eq!(settings["mcpServers"]["suk"]["httpUrl"], "http://127.0.0.1:4000/mcp");
        assert_eq!(settings["mcpServers"]["suk"]["headers"]["Authorization"], "Bearer secret");
        assert!(settings["excludeTools"].as_array().unwrap().iter().any(|t| t == "run_shell_command"));
        let instructions = std::fs::read_to_string(instructions_file(&dir)).unwrap();
        assert!(instructions.starts_with("You are the assistant inside Suk"));
        assert!(instructions.ends_with("The user is an academic."));
        assert!(instructions.contains("no Google Calendar tools"));

        let args = arguments("hello", None, "new-id").join(" ");
        assert!(args.contains("-p hello") && args.contains("-o json") && args.contains("--approval-mode yolo"));
        assert!(args.contains("--allowed-mcp-server-names suk") && args.contains("--session-id new-id"));
        assert!(arguments("hello", Some("old-id"), "new-id").join(" ").contains("--resume latest"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Runs the real Gemini CLI with the app's arguments and an invalid key, checking the flags
    /// and settings are accepted and that it connects to the app's tools with the token.
    /// `GEMINI_BIN=/path/to/gemini cargo test --lib real_gemini_accepts -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn real_gemini_accepts_the_app_arguments() {
        use std::io::Write;
        let binary = std::env::var("GEMINI_BIN").expect("set GEMINI_BIN");
        let dir = std::env::temp_dir().join(format!("suk-gemini-real-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        // A stand-in for the app's MCP server, to see what Gemini sends it.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let (sender, request) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut buffer = [0u8; 8192];
                let n = std::io::Read::read(&mut stream, &mut buffer).unwrap_or(0);
                let _ = sender.send(String::from_utf8_lossy(&buffer[..n]).to_string());
                let _ = stream.write_all(b"HTTP/1.1 500 Internal Server Error\r\ncontent-length: 0\r\n\r\n");
            }
        });
        let setup = Setup {
            binary: binary.into(),
            workdir: dir.clone(),
            mcp: Endpoint { url: format!("http://127.0.0.1:{port}/mcp"), token: "token-for-gemini".into() },
            about_user: String::new(),
        };
        prepare(&setup).unwrap();
        let out = Command::new(&setup.binary)
            .args(arguments("hello", None, "11111111-2222-3333-4444-555555555555"))
            .current_dir(&dir)
            .env("GEMINI_SYSTEM_MD", instructions_file(&dir))
            .env("GEMINI_API_KEY", "invalid-key")
            .stdin(Stdio::null())
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&out.stdout).to_string();
        let stderr = String::from_utf8_lossy(&out.stderr).to_string();
        println!("exit {:?}\nstdout:\n{stdout}\nstderr (tail):\n{}", out.status.code(), stderr.lines().rev().take(6).collect::<Vec<_>>().join("\n"));
        assert!(!stderr.contains("Unknown argument") && !stderr.contains("Unknown arguments"), "every flag is accepted");
        // The tool server is only reached once the model answers, which an invalid key prevents,
        // so this is reported rather than required.
        match request.recv_timeout(Duration::from_secs(20)) {
            Ok(captured) => {
                println!("request to the app's MCP server:\n{}", captured.lines().take(6).collect::<Vec<_>>().join("\n"));
                assert!(captured.to_lowercase().contains("authorization: bearer token-for-gemini"));
            }
            Err(_) => println!("(the app's tool server wasn't reached; the key is invalid)"),
        }
        let failure = read_reply(&stdout, &stderr, "new").unwrap_err();
        assert!(failure.to_lowercase().contains("api key") || failure.to_lowercase().contains("auth"), "{failure}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn replies_and_errors_are_read_from_the_json_output() {
        let ok = r#"{"session_id": "abc", "response": "Saved Amit as your PhD student.", "stats": {}}"#;
        assert_eq!(read_reply(ok, "", "new").unwrap(), (Some("abc".into()), "Saved Amit as your PhD student.".into()));
        let failed = r#"{"error": {"type": "ApiError", "message": "API key not valid. Please pass a valid API key.", "code": 400}}"#;
        assert!(read_reply(failed, "", "new").unwrap_err().contains("API key not valid"));
        let crashed = read_reply("", "YOLO mode is enabled.\nError: quota exceeded\n    at foo (bar.js:1)", "new").unwrap_err();
        assert_eq!(crashed, "Gemini couldn't answer: Error: quota exceeded");
        // Real output when the key is wrong: nothing on stdout, nested JSON on stderr.
        let real = "Error when talking to Gemini API _ApiError: {\"error\":{\"message\":\"{\\n  \\\"error\\\": {\\n    \\\"message\\\": \\\"API key not valid. Please pass a valid API key.\\\"\\n  }\\n}\",\"code\":400}}\n    at throwErrorIfNotOK (file:///x.js:1:1)\n  }\n}";
        assert_eq!(read_reply("", real, "new").unwrap_err(), "Gemini couldn't answer: API key not valid. Please pass a valid API key.");
        assert!(read_reply(r#"{"session_id": "abc", "response": ""}"#, "", "new").unwrap_err().contains("nothing"));
    }
}
