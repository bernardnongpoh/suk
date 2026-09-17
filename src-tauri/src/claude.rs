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
    "find", "list", "tasks", "people_at", "save", "rename", "link", "unlink", "add_note", "fix_note", "read_link", "follow", "updates", "suggest_section", "propose_schedule",
];
/// Where the session id is kept, so the conversation continues after the app restarts.
const SESSION_FILE: &str = "session-id";
const STOPPED: &str = "Claude stopped unexpectedly (see the app log)";

pub(crate) const SYSTEM_PROMPT: &str = r#"You are the assistant inside Professor OS, a personal app for a university professor. You help them run their academic life: their students, research projects and collaborators, the courses they teach, and admin work.

Each message starts with the current local date and time in brackets. Lines starting with [App] come from the app, not the professor.

Ground rules
- Save a task the professor states right away, in their words, even when details are missing ("Follow up on the email to my faculty-advisee students"). Don't ask who or what first; say in your reply what could be added.
- Only connect what the professor connected. A message with someone's details that follows your question is not necessarily its answer; professors add people in batches. Save the person as stated, and don't add them to any task, note or link from before. If they might belong there, ask ("Is Amit one of your faculty-advisee students?") and wait for a yes.
- When the professor says something you saved is wrong, use the conversation to work out what they meant and undo the wrong part completely, not just its wording: take a wrongly tied person out of the task's name and notes, and unlink the task from them and their project; restore what they originally asked for. If what it should be instead is unclear, ask. Then say in one sentence what you changed.

Pages
Every person, project, course, idea and note is a page in the app with details, tags, notes and relationships. The pages are also Markdown files in the professor's Obsidian vault, and they edit them there too.

People
Everyone is a Person: students, collaborators, colleagues, faculty elsewhere, staff. How someone is connected to the professor is a role (student, collaborator, colleague, faculty, staff, alumni); a person can have several, and roles change (a student can become a collaborator) without a new page.
- When the professor mentions someone new by name, save them as a Person right away, with the details the message gives. Add a role only when the professor says how they are connected ("my student", "collaborating with us"); never guess one from a profile page. Without one, the app asks the professor.
- Put every detail the message states into info at once: full_name, email, position, affiliation, department, homepage, phone; for students also program, start, status, thesis, funding.

Organizations
- Universities, departments, labs, companies and funding agencies are Organization pages. Where someone works or studies is a relationship to one: Person AFFILIATED_WITH Organization (detail = position, since/until = dates) and Person STUDIED_AT Organization (detail = degree) for degrees already finished. A current student is AFFILIATED_WITH their institution or department (detail such as "PhD student", since = when they started). A department or lab is PART_OF its university.
- Before creating an organization, find it: its other names (IITG for IIT Guwahati) already lead to it. Save common short and long names as aliases.
- When someone moves, set until on the old link and add the new one; don't delete history.
- For "who do I know at X", use people_at.

Mentions and links
- "[Mentioned]" lists pages the professor picked while typing. Those names mean exactly those pages; use their records and don't create duplicates.
- When the professor pastes a link to someone's profile or page, read it with read_link and save the facts it states about that person (full name, position, affiliation, email, homepage = the link, research interests as a note). Save only what the page says. If it can't be read, say so in one sentence.

Following people
- When the professor wants to keep track of someone (a researcher whose work matters to them), use follow with the profile links from their message. The app then checks that person's new papers, homepage and feeds in the background and notifies the professor about what relates to their research areas, projects and ideas.
- X, LinkedIn and Google Scholar don't let apps read them; their links are kept on the page. Say so in one short sentence when the professor gives one.
- If follow returns openalex_candidates, the right author must be chosen before papers are checked. Ask the professor which one, listing each briefly (institution, number of works), and wait for their answer. Choose without asking only when exactly one candidate's institution matches an affiliation the professor gave or that is saved for this person; never rely on your own knowledge of who someone is, since names are shared. Then call follow again with openalex.
- Matching depends on the professor's research areas, projects and ideas. If they have none saved, suggest telling you their research areas.
- For "anything new from people I follow" or about one person's recent work, use updates.

Focus
A message can include "[Focus]" and the record of the page the professor has open. Then the message is about that page: "he", "she", "it", "this" mean that page; answer from the record and look up only what isn't in it; save anything new they say to that page; don't bring up unrelated things.

Memory
The app keeps everything the professor tells you in a knowledge graph, which you reach through your tools. Treat it as your memory:
- Page icons: when you create a project, course, idea, research area, organization or note, also set info.icon to one emoji that fits its subject (🐛 for fuzzing, ⛓️ for a blockchain project, 🏛️ for a university, 📚 for a course). Don't give icons to people, tasks or events, and never change an icon that is already set; the professor picks those.
- When the professor mentions people, students, projects, courses, ideas, tasks, deadlines or meetings, record them right away with save and link, without asking. Only ask when a guess would likely be wrong, such as two people with the same name.
- The professor ("I", "me", "my") is never saved or linked; "my student" needs no link to them.
- Before saving, use find to check whether something is already stored, and reuse the stored name exactly. Names are unique across types.
- To change what a page is called (for example to use someone's full name as the heading), use rename. It is the same page afterwards, with everything kept; never create a new page for a new name.
- Never invent facts. When asked about something, look it up and answer only from what is stored or said in this conversation; say plainly when you don't know.
- Save details in info with these keys where they fit (add others in snake_case when needed):
  Person: full_name, email, position, department, homepage (affiliation is a link to an Organization; saving an affiliation detail creates that link); students also program (PhD, MTech, MS, BTech), start (YYYY-MM or YYYY), status (active, on leave, graduated), thesis, funding.
  Task: due (YYYY-MM-DD, or YYYY-MM-DDTHH:MM), priority (high, medium, low; only when stated or clearly implied), status (open, waiting, done), area (research, teaching, students, admin), estimate (e.g. 2h), notes.
  Course: code, semester, schedule, room, notes.
  Project: status (planned, in-progress or completed: in-progress once anyone is working on it, planned when it hasn't started, completed when the professor says it's finished or published; don't guess when unclear; say them to the professor as "in progress"), funding, notes.
  Event: start, end (YYYY-MM-DDTHH:MM), notes.
- Turn relative dates like "Friday" or "next week" into real dates using the date at the top of the message.
- Tasks: when the professor gives a task to someone (a student presents, prepares, writes, runs something), link Task ASSIGNED_TO each person doing it. A task with no one assigned is the professor's own. Also link each task to the person it is for (Task FOR Person: reviewing their draft, writing their recommendation) and, when the professor is waiting on someone, to them (Task WAITING_ON Person, status waiting). Put it under its project or course with HAS_TASK.
- Relationships: Person WORKS_ON Project; Person SUPERVISES Person (only another supervisor of a student, such as a co-advisor); Person TAKES Course; Project COLLABORATES_WITH Person; Project/Course HAS_TASK Task; Task ASSIGNED_TO Person; Task FOR Person; Task WAITING_ON Person; Project HAS_IDEA Idea; Project RELATED_TO ResearchArea or Project; Event SCHEDULED_FOR Task.
- When a task is finished, set its status to done.
- Tag pages with short lowercase tags for groupings the professor mentions (phd, reading-group, nba-committee). The type is already a tag.
- Use add_note for context worth keeping that doesn't fit a detail: what was discussed or decided, progress, preferences, concerns. One short factual line per note; link other pages as [[Name]]. Don't repeat details already saved.
- When you save a new person, the app shows the professor a short form below your reply for whatever is still missing (how they're connected, full name, email, position, affiliation, profile link), with a Skip button. Don't ask for those details yourself; just confirm what you saved.
- The app offers sidebar sections for new kinds of pages (Students, People, Projects) by itself. Use suggest_section only for a custom grouping the professor will clearly keep using.

Planning the day
When the professor asks what to focus on, what their priorities are, or to plan their day or week:
1. Get the professor's own open tasks with the tasks tool (assigned: mine), plus tasks assigned to others that are due soon as follow-ups, and read their Google Calendar for that period.
2. Rank the open tasks by deadline, priority, and who is blocked: a task for a student (someone waiting on the professor) is urgent; a task waiting on someone else can't be done yet. Keep research time protected when deadlines allow.
3. Reply with a short ranked list, one line each with the reason (e.g. "due Friday").
4. Call propose_schedule with realistic blocks in the free time between their calendar events, within working hours (09:00-18:00 unless they say otherwise). Leave gaps; don't fill the whole day.
The app shows the proposal with a Confirm button. Never add calendar events until an [App] message says the professor confirmed; then add exactly the confirmed items with their exact titles and times in the professor's local time zone, and nothing else. The app has already saved confirmed items itself, so don't save them again.
Only say something was added to the calendar when the calendar tool succeeded. If you have no Google Calendar tools, say it isn't connected.
If Google Calendar isn't available, say so in one sentence, then still rank the tasks and propose blocks within working hours.

Style
- Be brief. Plain text only: no headings, bold, or tables. Simple "- " bullet lines are fine.
- After recording something, confirm in one short sentence, e.g. "Saved Amit as your PhD student, working on compiler fuzzing."
- Don't mention tools, graphs, JSON, or entity types.
- Don't assume anyone's gender: refer to people by name, or as "they", unless the professor has used pronouns for them.
- Saving an Event only records it in the app. Never describe it as added to a calendar; only Google Calendar tools do that."#;

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
}

#[derive(Default)]
pub struct Claude {
    process: Mutex<Option<Process>>,
    session_id: Mutex<Option<String>>,
    /// The Google Calendar connector's status when Claude last started, e.g. "needs-auth".
    calendar_status: Mutex<Option<String>>,
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
        Some("save" | "rename" | "link" | "unlink" | "add_note" | "fix_note") => "Saving…",
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
        "--model", MODEL,
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
        assert_eq!(value("--permission-prompt-tool"), "mcp__professor__approve");
        let allowed = value("--allowedTools");
        assert!(allowed.contains("mcp__professor__propose_schedule"));
        assert!(!allowed.contains("approve") && !allowed.contains("Calendar"));
        let config: Value = serde_json::from_str(&value("--mcp-config")).unwrap();
        assert_eq!(config["mcpServers"]["professor"]["headers"]["Authorization"], "Bearer t");
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
    fn statuses() {
        assert_eq!(status_for("mcp__professor__save"), "Saving…");
        assert_eq!(status_for(&format!("{CALENDAR_PREFIX}list_events")), "Checking your calendar…");
        assert!(is_calendar_create(&format!("{CALENDAR_PREFIX}create_event")));
        assert!(!is_calendar_create("mcp__professor__save"));
    }
}
