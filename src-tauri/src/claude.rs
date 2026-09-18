//! Chat handled by Claude through the Claude Code CLI.
//!
//! One `claude` process is kept running in streaming mode, so a message doesn't pay for startup
//! and connecting to MCP servers each time. If it exits, the next message starts a new one that
//! resumes the same session. Claude gets no built-in tools (no shell, no files): only this app's
//! tools over MCP, plus Google Calendar as allowed by `tools::approve`.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::Mutex;
use std::time::Duration;

use chrono::Local;
use serde_json::{json, Value};

use crate::mcp::Endpoint;
use crate::tools::{calendar_access, CalendarAccess, APPROVE_TOOL, CALENDAR_PREFIX, SERVER_NAME};

/// A turn that runs longer than this is abandoned and the process restarted.
const TURN_TIMEOUT: Duration = Duration::from_secs(300);
const MODEL: &str = "sonnet";
/// For background questions such as rating updates: smaller and cheaper.
const BACKGROUND_MODEL: &str = "haiku";
/// A one-shot question that runs longer than this is abandoned.
const ONE_SHOT_TIMEOUT: Duration = Duration::from_secs(180);
/// Connectors Claude has no use for here; hidden so their tools don't crowd the context.
const HIDDEN_SERVERS: &[&str] = &[
    "mcp__claude_ai_Gmail",
    "mcp__claude_ai_Google_Drive",
    "mcp__claude_ai_Claude_Docs",
];
/// The Google Calendar connector as listed in Claude Code's startup event.
const CALENDAR_SERVER: &str = "claude.ai Google Calendar";
const APP_TOOLS: &[&str] = &[
    "find", "list", "tasks", "people_at", "record", "rename", "unlink", "fix_note", "read_link", "follow", "updates", "suggest_section", "propose_schedule",
];
/// Where the session id is kept, so the conversation continues after the app restarts.
const SESSION_FILE: &str = "session-id";
/// The day the saved session started; a new day starts a new session.
const SESSION_DATE_FILE: &str = "session-date";
/// A session whose last call read more than this many tokens is replaced by a new one with a
/// short summary, so every message doesn't re-read a long conversation.
const MAX_CONTEXT: u64 = 16_000;
const STOPPED: &str = "Claude stopped unexpectedly (see the app log)";

pub(crate) const SYSTEM_PROMPT: &str = r#"You are the assistant inside Suk, a personal app for running one's work: people, projects, tasks, ideas, notes and plans. Many users are academics (students, research, teaching, admin); use those words when the user does.

Messages start with the local date and time in brackets. [App] lines come from the app. [Known] lists saved pages the message names, [Mentioned] pages picked while typing, [Focus] the page the user has open ("he", "it", "this" mean that page; stay on it). Each record starts with the page's name; use exactly that name, even if you also know a longer one; look something up with find only when you need more than they say.

Recording
- Save what the user tells you right away, in one record call per message: every page with the details stated, note lines, and the relationships. Don't ask first; confirm in one short sentence ("Saved Amit as your PhD student, working on compiler fuzzing."). Don't ask for missing details either; the app shows a form for new people.
- A task the user states is saved at once, in their words, even when you don't know who or what it's about ("Follow up on the email to my faculty-advisee students"). Never ask before saving it; say in your reply what could be added. Turn relative dates into real ones from the date at the top.
- Only connect what the user connected. Someone's details sent after your question aren't necessarily its answer: save the person, don't add them to earlier tasks, notes or links, and ask if they might belong ("Is Amit one of your advisees?").
- Names are unique across types and other names (aliases) find the same page; reuse stored names exactly. To change a page's name use rename; never make a second page.
- The user ("I", "me", "my") is never a page and needs no link.
- Never invent facts; answer only from what's stored or said, and say when you don't know.
- When told something saved is wrong, undo it completely (rename, unlink, fix_note, record with null details), restore what was meant, or ask if unclear; then say what changed.

What goes where
- Person: everyone (students, collaborators, colleagues, staff). roles say how they're connected (student, collaborator, colleague, faculty, staff, alumni), only when the user says so. info: full_name, email, position, department, homepage, phone; students also program (PhD, MTech, MS, BTech), start, status, thesis, funding. An affiliation detail links them to the Organization.
- Organization: universities, departments, labs, companies, funders; save short and long names as aliases. Person AFFILIATED_WITH Organization (detail position or "PhD student", since/until) for where they are now, including current students; STUDIED_AT (detail degree) for finished degrees; department PART_OF university. When someone moves, set until on the old link. "Who do I know at X": people_at.
- Task: due (YYYY-MM-DD or YYYY-MM-DDTHH:MM), priority (high, medium, low, only when stated or implied), status (open, waiting, done; done when finished), area (research, teaching, students, admin). Task ASSIGNED_TO each person doing it (none means the user's own); FOR the person it's for; WAITING_ON whoever the user waits on (status waiting); project or course HAS_TASK it.
- Project: status planned, in-progress (anyone working on it) or completed (say "in progress"); funding. Person WORKS_ON Project; Project COLLABORATES_WITH an outside Person; HAS_IDEA Idea; RELATED_TO ResearchArea or Project.
- Course: code, semester, schedule, room; Person TAKES Course. Event: start, end; SCHEDULED_FOR Task. Person SUPERVISES Person only for another supervisor (co-advisor).
- New projects, courses, ideas, areas, organizations and notes get info.icon, one fitting emoji; never for people, tasks or events, and never replace one.
- Notes: one short factual line for context that fits no detail (decisions, progress, concerns); [[Name]] links pages.
- Tags: short lowercase groupings the user mentions (phd, reading-group). suggest_section only for a custom grouping they'll keep using.
- Any web address the user pastes about a person or their work, including a local one: read_link it, then save only what that page states (never guess roles): their position, department, email, the address as homepage, and a note line for what they work on. Keeping track of someone's work: follow; it explains what it needs. "Anything new from people I follow": updates.

Planning the day
Asked what to focus on, what their priorities are, or to plan a day or week: (1) get their open tasks with tasks (assigned mine, plus others' due soon) and read their Google Calendar for that period if you have it; (2) reply with a short ranked list, a reason each (deadlines, priority; something the user owes someone is urgent; waiting-on tasks can't be done); (3) always finish by calling propose_schedule with blocks in the free time, within 09:00-18:00 and leaving gaps. The app shows it with a Confirm button. Add calendar events only after an [App] message says the user confirmed, exactly those items (the app saved them already), and say they were added only if the calendar tool succeeded. Without calendar tools, say it isn't connected and still propose. Saving an Event never adds it to a calendar.

Style
Brief, plain text, "- " bullets at most; no headings, bold or tables. Don't mention tools, JSON or types. Refer to people by name or "they" unless the user used pronouns."#;

#[derive(Debug, Clone)]
pub struct Setup {
    pub binary: PathBuf,
    /// An empty directory to run in, so no project files or instructions are picked up.
    pub workdir: PathBuf,
    pub mcp: Endpoint,
}

/// What a finished turn produced besides its text.
#[derive(Debug, Default)]
pub struct Turn {
    pub text: String,
    /// Inputs of calendar events Claude created successfully.
    pub created_events: Vec<Value>,
    pub usage: Usage,
}

/// Tokens a turn used, as the assistant reports them.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct Usage {
    /// Model calls in the turn: one, plus one after each round of tool calls.
    pub calls: u64,
    /// Everything read, new or from the prompt cache.
    pub input: u64,
    /// The part of `input` read from the prompt cache.
    pub cached: u64,
    pub output: u64,
    /// What the last call read: the size of the conversation so far.
    pub last_context: u64,
}

impl Usage {
    /// From a `result` event's `usage` and `num_turns`.
    pub fn from_result(event: &Value) -> Usage {
        let n = |v: &Value| v.as_u64().unwrap_or(0);
        let u = &event["usage"];
        let cached = n(&u["cache_read_input_tokens"]);
        Usage {
            calls: n(&event["num_turns"]),
            input: n(&u["input_tokens"]) + n(&u["cache_creation_input_tokens"]) + cached,
            cached,
            output: n(&u["output_tokens"]),
            last_context: 0,
        }
    }
}

impl std::ops::AddAssign for Usage {
    fn add_assign(&mut self, other: Usage) {
        self.calls += other.calls;
        self.input += other.input;
        self.cached += other.cached;
        self.output += other.output;
        self.last_context = other.last_context;
    }
}

#[derive(Default)]
pub struct Claude {
    process: Mutex<Option<Process>>,
    session_id: Mutex<Option<String>>,
    /// The Google Calendar connector's status when Claude last started, e.g. "needs-auth".
    calendar_status: Mutex<Option<String>>,
    /// How much the last call read, to start a new session before the conversation grows long.
    last_context: Mutex<u64>,
}

struct Process {
    child: Child,
    stdin: ChildStdin,
    events: Receiver<Value>,
}

impl Drop for Process {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Claude {
    /// The saved session to resume, when none is known in memory yet.
    fn saved_session(&self, setup: &Setup) -> Option<String> {
        let mut session = lock(&self.session_id);
        if session.is_none() {
            *session = std::fs::read_to_string(setup.workdir.join(SESSION_FILE))
                .ok()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty());
        }
        session.clone()
    }

    /// Starts a new session when the saved one is from another day or has grown past
    /// MAX_CONTEXT tokens. Returns whether the next message begins a new session, which then
    /// needs a short summary of what came before. Everything else is in the database.
    pub fn begins_fresh(&self, setup: &Setup) -> bool {
        let saved = self.saved_session(setup);
        let today = Local::now().format("%Y-%m-%d").to_string();
        let started = std::fs::read_to_string(setup.workdir.join(SESSION_DATE_FILE)).unwrap_or_default();
        let too_long = *lock(&self.last_context) > MAX_CONTEXT;
        if saved.is_some() && (started.trim() != today || too_long) {
            eprintln!("claude: starting a new session ({})", if too_long { "conversation grew long" } else { "new day" });
            *lock(&self.process) = None;
            *lock(&self.session_id) = None;
            *lock(&self.last_context) = 0;
            let _ = std::fs::remove_file(setup.workdir.join(SESSION_FILE));
            return true;
        }
        saved.is_none()
    }

    /// None until Claude has answered once.
    pub fn calendar_status(&self) -> Option<String> {
        lock(&self.calendar_status).clone()
    }

    /// False only when Claude reported the connector can't be used.
    pub fn calendar_usable(&self) -> bool {
        !matches!(self.calendar_status().as_deref(), Some("needs-auth" | "failed" | "disabled") | Some("missing"))
    }

    /// Sends a message and waits for Claude's reply, reporting progress through `on_status`.
    pub fn send(
        &self,
        setup: &Setup,
        message: &str,
        on_status: &mut dyn FnMut(&str),
    ) -> Result<Turn, String> {
        // Held for the whole turn: one conversation, one turn at a time.
        let mut process = lock(&self.process);
        let content = stamped(message);
        let line = json!({
            "type": "user",
            "message": { "role": "user", "content": content }
        })
        .to_string();

        // A process that exited since the last turn is replaced, resuming the session. If a fresh
        // process dies at once, the saved session couldn't be resumed: start a new one instead.
        let mut result = Err(String::new());
        for attempt in 0..2 {
            let exited = |p: &mut Process| !matches!(p.child.try_wait(), Ok(None));
            let fresh = process.as_mut().is_none_or(exited);
            if fresh {
                *process = Some(self.spawn(setup)?);
            }
            let running = process.as_mut().expect("running");
            result = writeln!(running.stdin, "{line}")
                .and_then(|_| running.stdin.flush())
                .map_err(|_| STOPPED.to_string())
                .and_then(|_| self.read_turn(setup, running, on_status));
            let Err(e) = &result else { break };
            // Don't reuse a process in an unknown state.
            *process = None;
            let resumed = lock(&self.session_id).is_some();
            if attempt == 0 && e == STOPPED && resumed {
                eprintln!("claude: couldn't continue the saved session; starting a new one");
                *lock(&self.session_id) = None;
                let _ = std::fs::remove_file(setup.workdir.join(SESSION_FILE));
                continue;
            }
            break;
        }
        result
    }

    fn read_turn(
        &self,
        setup: &Setup,
        process: &mut Process,
        on_status: &mut dyn FnMut(&str),
    ) -> Result<Turn, String> {
        let mut turn = Turn::default();
        // Calendar creations waiting for their result, by tool use id.
        let mut creating: Vec<(String, Value)> = Vec::new();
        // Text from every step of the turn: a reply is often written before a final tool call,
        // and the result event only repeats the last step's text.
        let mut texts: Vec<String> = Vec::new();
        loop {
            let event = match process.events.recv_timeout(TURN_TIMEOUT) {
                Ok(event) => event,
                Err(RecvTimeoutError::Timeout) => return Err("Claude took too long to answer".into()),
                Err(RecvTimeoutError::Disconnected) => return Err(STOPPED.into()),
            };
            if let Some(id) = event["session_id"].as_str() {
                let mut session = lock(&self.session_id);
                if session.as_deref() != Some(id) {
                    *session = Some(id.to_string());
                    let _ = std::fs::write(setup.workdir.join(SESSION_FILE), id);
                    let _ = std::fs::write(setup.workdir.join(SESSION_DATE_FILE), Local::now().format("%Y-%m-%d").to_string());
                }
            }
            match event["type"].as_str().unwrap_or_default() {
                "system" if event["subtype"] == "init" => {
                    let status = event["mcp_servers"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .find(|s| s["name"] == CALENDAR_SERVER)
                        .map_or("missing", |s| s["status"].as_str().unwrap_or("unknown"));
                    *lock(&self.calendar_status) = Some(status.to_string());
                }
                "assistant" => {
                    let usage = &event["message"]["usage"];
                    let read = ["input_tokens", "cache_creation_input_tokens", "cache_read_input_tokens"]
                        .iter()
                        .map(|k| usage[*k].as_u64().unwrap_or(0))
                        .sum::<u64>();
                    if read > 0 {
                        turn.usage.last_context = read;
                        *lock(&self.last_context) = read;
                    }
                    for block in blocks(&event) {
                        if let Some(text) = block["text"].as_str().filter(|_| block["type"] == "text") {
                            texts.push(text.trim().to_string());
                        }
                        if block["type"] == "tool_use" {
                            let name = block["name"].as_str().unwrap_or_default();
                            on_status(status_for(name));
                            if is_calendar_create(name) {
                                let id = block["id"].as_str().unwrap_or_default().to_string();
                                creating.push((id, block["input"].clone()));
                            }
                        }
                    }
                }
                "user" => {
                    for block in blocks(&event) {
                        if block["type"] != "tool_result" || block["is_error"] == true {
                            continue;
                        }
                        let id = block["tool_use_id"].as_str().unwrap_or_default();
                        if let Some(i) = creating.iter().position(|(c, _)| c == id) {
                            turn.created_events.push(creating.swap_remove(i).1);
                        }
                    }
                }
                "result" => {
                    let text = event["result"].as_str().unwrap_or_default().trim().to_string();
                    if event["is_error"] == true || event["subtype"] != "success" {
                        let detail = if text.is_empty() {
                            event["errors"].to_string()
                        } else {
                            text
                        };
                        return Err(format!("Claude couldn't answer: {detail}"));
                    }
                    texts.retain(|t| !t.is_empty());
                    turn.text = if texts.is_empty() { text } else { texts.join("\n\n") };
                    turn.usage = Usage { last_context: turn.usage.last_context, ..Usage::from_result(&event) };
                    return Ok(turn);
                }
                _ => {}
            }
        }
    }

    fn spawn(&self, setup: &Setup) -> Result<Process, String> {
        std::fs::create_dir_all(&setup.workdir).map_err(|e| e.to_string())?;
        let session = self.saved_session(setup);
        let mut child = Command::new(&setup.binary)
            .args(arguments(setup, session.as_deref()))
            .current_dir(&setup.workdir)
            // Load every tool up front rather than through tool search, which is disabled here.
            .env("ENABLE_TOOL_SEARCH", "false")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|e| format!("couldn't start Claude Code: {e}"))?;
        let stdin = child.stdin.take().ok_or("no stdin")?;
        let stdout = child.stdout.take().ok_or("no stdout")?;
        let (sender, events) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                match serde_json::from_str::<Value>(&line) {
                    Ok(event) => {
                        if sender.send(event).is_err() {
                            break;
                        }
                    }
                    Err(_) => eprintln!("claude: {line}"),
                }
            }
        });
        eprintln!("claude: started (resuming {session:?})");
        Ok(Process { child, stdin, events })
    }
}

fn arguments(setup: &Setup, session: Option<&str>) -> Vec<String> {
    let allowed: Vec<_> = APP_TOOLS
        .iter()
        .map(|t| format!("mcp__{SERVER_NAME}__{t}"))
        .collect();
    let mut args: Vec<String> = [
        "--print",
        "--input-format", "stream-json",
        "--output-format", "stream-json",
        "--verbose",
        "--model", MODEL,
        // Ignore the user's Claude Code settings, hooks and instructions; this is not a coding session.
        "--setting-sources", "",
        "--tools", "",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    args.extend([
        "--system-prompt".into(),
        SYSTEM_PROMPT.into(),
        "--mcp-config".into(),
        setup.mcp.claude_config(SERVER_NAME),
        "--allowedTools".into(),
        allowed.join(","),
        "--disallowedTools".into(),
        HIDDEN_SERVERS.join(","),
        "--permission-prompt-tool".into(),
        format!("mcp__{SERVER_NAME}__{APPROVE_TOOL}"),
    ]);
    if let Some(session) = session {
        args.extend(["--resume".into(), session.into()]);
    }
    args
}

fn blocks(event: &Value) -> &[Value] {
    event["message"]["content"].as_array().map(Vec::as_slice).unwrap_or_default()
}

fn is_calendar_create(tool: &str) -> bool {
    tool.strip_prefix(CALENDAR_PREFIX)
        .is_some_and(|action| calendar_access(action) == CalendarAccess::Create)
}

/// The message with the current local date and time before it, as the instructions describe.
pub(crate) fn stamped(message: &str) -> String {
    let now = Local::now().format("%A %Y-%m-%d %H:%M, UTC%:z");
    format!("[{now}]\n{message}")
}

pub(crate) fn status_for(tool: &str) -> &'static str {
    if let Some(action) = tool.strip_prefix(CALENDAR_PREFIX) {
        return match calendar_access(action) {
            CalendarAccess::Create => "Adding to your calendar…",
            _ => "Checking your calendar…",
        };
    }
    match tool.strip_prefix(&format!("mcp__{SERVER_NAME}__")) {
        Some("find" | "list") => "Looking through your notes…",
        Some("tasks") => "Checking your tasks…",
        Some("people_at") => "Looking up who you know there…",
        Some("record" | "save" | "rename" | "link" | "unlink" | "add_note" | "fix_note") => "Saving…",
        Some("read_link") => "Reading the link…",
        Some("follow") => "Looking up their work…",
        Some("updates") => "Checking updates…",
        Some("propose_schedule") => "Drafting a schedule…",
        _ => "Working…",
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Finds the `claude` executable. Apps opened from the Finder don't get the shell's PATH, so the
/// usual install locations are checked too.
pub fn find_binary() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let path_dirs = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect::<Vec<_>>())
        .unwrap_or_default();
    let known = home.iter().flat_map(|h| [h.join(".local/bin"), h.join(".claude/local")]);
    path_dirs
        .into_iter()
        .chain(known)
        .chain(["/opt/homebrew/bin", "/usr/local/bin"].map(PathBuf::from))
        .map(|dir| dir.join(if cfg!(windows) { "claude.exe" } else { "claude" }))
        .find(|candidate| candidate.is_file())
}

/// Asks Claude a single question whose answer must match `schema`, without tools, MCP servers,
/// the user's settings, or a saved session. Used for background work such as judging updates.
pub fn ask_json(binary: &Path, workdir: &Path, system: &str, prompt: &str, schema: &Value) -> Result<Value, String> {
    std::fs::create_dir_all(workdir).map_err(|e| e.to_string())?;
    let mut child = Command::new(binary)
        .args(one_shot_arguments(system, schema))
        .current_dir(workdir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("couldn't start Claude Code: {e}"))?;
    // The prompt goes on stdin so its length doesn't matter.
    let mut stdin = child.stdin.take().ok_or("no stdin")?;
    stdin.write_all(prompt.as_bytes()).map_err(|e| e.to_string())?;
    drop(stdin);
    let mut stdout = child.stdout.take().ok_or("no stdout")?;
    let reader = std::thread::spawn(move || {
        let mut out = String::new();
        std::io::Read::read_to_string(&mut stdout, &mut out).map(|_| out)
    });
    let deadline = std::time::Instant::now() + ONE_SHOT_TIMEOUT;
    loop {
        match child.try_wait().map_err(|e| e.to_string())? {
            Some(_) => break,
            None if std::time::Instant::now() > deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("Claude took too long to answer".into());
            }
            None => std::thread::sleep(Duration::from_millis(200)),
        }
    }
    let output = reader.join().map_err(|_| "couldn't read Claude's answer")?.map_err(|e| e.to_string())?;
    structured_output(&output)
}

fn one_shot_arguments(system: &str, schema: &Value) -> Vec<String> {
    [
        "--print",
        "--output-format", "json",
        "--model", BACKGROUND_MODEL,
        "--setting-sources", "",
        "--tools", "",
        "--strict-mcp-config",
        "--no-session-persistence",
        "--system-prompt", system,
        "--json-schema", &schema.to_string(),
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

/// The answer in `claude -p --output-format json` output.
fn structured_output(output: &str) -> Result<Value, String> {
    let result: Value = serde_json::from_str(output.trim()).map_err(|_| {
        let first = output.lines().next().unwrap_or("no output");
        format!("Claude couldn't answer: {first}")
    })?;
    if result["is_error"] == true {
        return Err(format!("Claude couldn't answer: {}", result["result"].as_str().unwrap_or("unknown error")));
    }
    match &result["structured_output"] {
        Value::Null => Err("Claude didn't give a structured answer".into()),
        answer => Ok(answer.clone()),
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arguments_lock_claude_down_and_resume() {
        let setup = Setup {
            binary: "claude".into(),
            workdir: "/tmp".into(),
            mcp: Endpoint { url: "http://127.0.0.1:1/mcp".into(), token: "t".into() },
        };
        let args = arguments(&setup, Some("abc"));
        let value = |flag: &str| {
            let i = args.iter().position(|a| a == flag).unwrap_or_else(|| panic!("{flag}"));
            args[i + 1].clone()
        };
        assert_eq!(value("--tools"), "");
        assert_eq!(value("--setting-sources"), "");
        assert_eq!(value("--resume"), "abc");
        assert_eq!(value("--permission-prompt-tool"), "mcp__suk__approve");
        let allowed = value("--allowedTools");
        assert!(allowed.contains("mcp__suk__propose_schedule"));
        assert!(!allowed.contains("approve") && !allowed.contains("Calendar"));
        let config: Value = serde_json::from_str(&value("--mcp-config")).unwrap();
        assert_eq!(config["mcpServers"]["suk"]["headers"]["Authorization"], "Bearer t");
        assert!(!arguments(&setup, None).contains(&"--resume".to_string()));
    }

    #[test]
    fn one_shot_questions_are_locked_down_and_answers_read() {
        let args = one_shot_arguments("Judge.", &json!({"type": "object"}));
        for flag in ["--strict-mcp-config", "--no-session-persistence"] {
            assert!(args.contains(&flag.to_string()), "{flag}");
        }
        let at = |flag: &str| args[args.iter().position(|a| a == flag).unwrap() + 1].clone();
        assert_eq!((at("--tools"), at("--setting-sources"), at("--json-schema")), ("".into(), "".into(), r#"{"type":"object"}"#.into()));

        let ok = r#"{"type":"result","is_error":false,"result":"{}","structured_output":{"items":[{"id":"a1","relevance":"high"}]}}"#;
        assert_eq!(structured_output(ok).unwrap()["items"][0]["id"], "a1");
        let failed = r#"{"type":"result","is_error":true,"result":"Not logged in"}"#;
        assert_eq!(structured_output(failed).unwrap_err(), "Claude couldn't answer: Not logged in");
        assert!(structured_output("zsh: command not found").is_err());
    }

    #[test]
    fn usage_is_read_from_the_result_event() {
        let event = json!({"type": "result", "num_turns": 3, "usage": {"input_tokens": 12, "cache_creation_input_tokens": 1000, "cache_read_input_tokens": 20000, "output_tokens": 300}});
        assert_eq!(Usage::from_result(&event), Usage { calls: 3, input: 21012, cached: 20000, output: 300, last_context: 0 });
    }

    #[test]
    fn statuses() {
        assert_eq!(status_for("mcp__suk__save"), "Saving…");
        assert_eq!(status_for(&format!("{CALENDAR_PREFIX}list_events")), "Checking your calendar…");
        assert!(is_calendar_create(&format!("{CALENDAR_PREFIX}create_event")));
        assert!(!is_calendar_create("mcp__suk__save"));
    }
}
