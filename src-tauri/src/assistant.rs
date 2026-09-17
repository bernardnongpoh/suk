//! Which AI assistant runs the app (Claude Code or Codex), whether it's installed and signed in,
//! and installing it and signing in from the app. The app can't do anything without one.

use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Claude,
    Codex,
}

impl Kind {
    pub fn label(self) -> &'static str {
        match self {
            Kind::Claude => "Claude Code",
            Kind::Codex => "Codex",
        }
    }

    fn program(self) -> &'static str {
        match self {
            Kind::Claude => "claude",
            Kind::Codex => "codex",
        }
    }

    /// The official installer, run with `sh -c`. Both install into ~/.local/bin without sudo.
    pub fn install_command(self) -> &'static str {
        match self {
            Kind::Claude => "curl -fsSL https://claude.ai/install.sh | bash",
            Kind::Codex => "curl -fsSL https://chatgpt.com/codex/install.sh | sh",
        }
    }

    /// What someone needs to be able to sign in.
    pub fn requirement(self) -> &'static str {
        match self {
            Kind::Claude => "a Claude Pro or Max plan, or an Anthropic Console account",
            Kind::Codex => "a ChatGPT Plus, Pro, Business or Enterprise plan",
        }
    }
}

/// The app's saved preferences.
#[derive(Debug, Default, Clone, Serialize, Deserialize, PartialEq)]
pub struct Settings {
    #[serde(default)]
    pub assistant: Option<Kind>,
}

const SETTINGS_FILE: &str = "settings.json";

impl Settings {
    pub fn load(dir: &Path) -> Settings {
        std::fs::read_to_string(dir.join(SETTINGS_FILE))
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, dir: &Path) -> Result<(), String> {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        let text = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        std::fs::write(dir.join(SETTINGS_FILE), text).map_err(|e| e.to_string())
    }
}

/// Where an assistant stands on this computer.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Status {
    pub kind: Kind,
    pub label: &'static str,
    pub installed: bool,
    pub version: Option<String>,
    pub signed_in: bool,
    /// The account signed in, when the tool says.
    pub account: Option<String>,
    pub install_command: &'static str,
    pub requirement: &'static str,
}

impl Status {
    pub fn ready(&self) -> bool {
        self.installed && self.signed_in
    }
}

/// Finds a program. Apps opened from the Dock or a launcher don't get the shell's PATH, so the
/// usual install locations are checked too.
pub fn find_program(name: &str) -> Option<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let path_dirs = std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect::<Vec<_>>()).unwrap_or_default();
    let known = home
        .iter()
        .flat_map(|h| [h.join(".local/bin"), h.join(".claude/local"), h.join(".npm-global/bin"), h.join(".volta/bin"), h.join("bin")]);
    path_dirs
        .into_iter()
        .chain(known)
        .chain(["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin"].map(PathBuf::from))
        .map(|dir| dir.join(name))
        .find(|candidate| candidate.is_file())
}

pub fn find(kind: Kind) -> Option<PathBuf> {
    find_program(kind.program())
}

/// Checks whether an assistant is installed and signed in. Runs its CLI, so it takes a moment.
pub fn status(kind: Kind) -> Status {
    let binary = find(kind);
    let version = binary.as_deref().and_then(version);
    let (signed_in, account) = match (&binary, kind) {
        (Some(binary), Kind::Claude) => output(binary, &["auth", "status", "--json"])
            .map(|(_, out)| claude_auth(&out))
            .unwrap_or((false, None)),
        (Some(binary), Kind::Codex) => output(binary, &["login", "status"])
            .map(|(ok, out)| codex_auth(ok, &out))
            .unwrap_or((false, None)),
        (None, _) => (false, None),
    };
    Status {
        kind,
        label: kind.label(),
        installed: binary.is_some(),
        version,
        signed_in,
        account,
        install_command: kind.install_command(),
        requirement: kind.requirement(),
    }
}

fn version(binary: &Path) -> Option<String> {
    let (ok, out) = output(binary, &["--version"])?;
    let line = out.lines().next()?.trim().to_string();
    (ok && !line.is_empty()).then_some(line)
}

/// Runs a command, returning whether it succeeded and its stdout and stderr together.
fn output(binary: &Path, args: &[&str]) -> Option<(bool, String)> {
    let out = Command::new(binary).args(args).stdin(Stdio::null()).output().ok()?;
    let mut text = String::from_utf8_lossy(&out.stdout).to_string();
    text.push_str(&String::from_utf8_lossy(&out.stderr));
    Some((out.status.success(), text))
}

/// `claude auth status --json`: {"loggedIn": true, "email": …}.
fn claude_auth(output: &str) -> (bool, Option<String>) {
    let start = output.find('{').unwrap_or(0);
    match serde_json::from_str::<Value>(&output[start..]) {
        Ok(v) => (v["loggedIn"] == true, v["email"].as_str().map(String::from)),
        Err(_) => (false, None),
    }
}

/// `codex login status` succeeds when signed in and says how ("Logged in using ChatGPT").
fn codex_auth(ok: bool, output: &str) -> (bool, Option<String>) {
    let signed_in = ok && !output.to_lowercase().contains("not logged in");
    let how = strip_ansi(output).lines().map(str::trim).find(|l| l.to_lowercase().starts_with("logged in")).map(String::from);
    (signed_in, if signed_in { how } else { None })
}

/// Runs an assistant's official installer, passing each line of output to `on_line`.
pub fn install(kind: Kind, on_line: &mut dyn FnMut(&str)) -> Result<(), String> {
    let mut child = Command::new("sh")
        .args(["-c", &format!("({}) 2>&1", kind.install_command())])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .spawn()
        .map_err(|e| format!("couldn't run the installer: {e}"))?;
    let stdout = child.stdout.take().ok_or("no installer output")?;
    for line in BufReader::new(stdout).lines().map_while(Result::ok) {
        let line = strip_ansi(&line);
        if !line.trim().is_empty() {
            on_line(line.trim_end());
        }
    }
    let status = child.wait().map_err(|e| e.to_string())?;
    if !status.success() {
        return Err(format!("The {} installer stopped with an error; see its output above.", kind.label()));
    }
    if find(kind).is_none() {
        return Err(format!("{} was installed but the app can't find it; restart the app.", kind.label()));
    }
    Ok(())
}

/// What to show while signing in: the page to open, and for Codex the code to enter there.
#[derive(Debug, Clone, Default, Serialize, PartialEq)]
pub struct SignInPrompt {
    pub url: Option<String>,
    /// Codex: the one-time code to type on the page.
    pub code: Option<String>,
    /// Claude: the page shows a code to paste back into the app.
    pub needs_code: bool,
}

/// Reads sign-in instructions from a login command's output so far.
pub fn sign_in_prompt(kind: Kind, output: &str) -> SignInPrompt {
    let text = strip_ansi(output);
    // Output arrives in pieces: a link counts once the line it's on has ended.
    let url = text
        .split_inclusive('\n')
        .filter(|line| line.ends_with('\n'))
        .flat_map(str::split_whitespace)
        .filter_map(|word| word.find("https://").map(|i| &word[i..]))
        .map(|url| url.trim_end_matches(['.', ',', ')', ']']).to_string())
        .next();
    match kind {
        Kind::Claude => SignInPrompt { needs_code: url.is_some(), url, code: None },
        Kind::Codex => {
            // "Enter this one-time code … \n   IB3U-Y26HE"
            let code = text
                .split_whitespace()
                .find(|w| w.len() >= 7 && w.contains('-') && w.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '-'))
                .map(String::from);
            SignInPrompt { url, code, needs_code: false }
        }
    }
}

/// Removes terminal color codes and hyperlink markup from command output.
pub fn strip_ansi(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\u{1b}' => match chars.peek() {
                // CSI: ESC [ … letter
                Some('[') => {
                    chars.next();
                    for c in chars.by_ref() {
                        if c.is_ascii_alphabetic() {
                            break;
                        }
                    }
                }
                // OSC (hyperlinks): ESC ] … BEL or ESC \
                Some(']') => {
                    chars.next();
                    while let Some(c) = chars.next() {
                        if c == '\u{7}' {
                            break;
                        }
                        if c == '\u{1b}' && chars.peek() == Some(&'\\') {
                            chars.next();
                            break;
                        }
                    }
                }
                _ => {}
            },
            _ => out.push(c),
        }
    }
    out
}

/// A sign-in in progress. Claude Code waits for the code its page shows; Codex finishes by itself
/// once the code is entered in the browser.
#[derive(Default)]
pub struct SignIn {
    running: Mutex<Option<(Kind, Child, Option<ChildStdin>)>>,
}

impl SignIn {
    /// Starts signing in and waits until it finishes, passing instructions to `on_prompt` as soon
    /// as the login command prints them.
    pub fn run(&self, kind: Kind, on_prompt: &mut dyn FnMut(&SignInPrompt)) -> Result<(), String> {
        let binary = find(kind).ok_or_else(|| format!("{} isn't installed", kind.label()))?;
        let args: &[&str] = match kind {
            Kind::Claude => &["auth", "login"],
            Kind::Codex => &["login", "--device-auth"],
        };
        let mut child = Command::new(binary)
            .args(args)
            // Nothing to open automatically: the app opens the page itself.
            .env("BROWSER", "true")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("couldn't start signing in: {e}"))?;
        let stdin = child.stdin.take();
        let mut stdout = child.stdout.take().ok_or("no output")?;
        let mut stderr = child.stderr.take().ok_or("no output")?;
        self.cancel();
        *lock(&self.running) = Some((kind, child, stdin));

        // The prompt text isn't newline-terminated ("Paste code here >"), so read what arrives.
        let (sender, receiver) = std::sync::mpsc::channel::<String>();
        let err_sender = sender.clone();
        std::thread::spawn(move || pump(&mut stdout, sender));
        std::thread::spawn(move || pump(&mut stderr, err_sender));
        let mut seen = String::new();
        let mut shown = SignInPrompt::default();
        loop {
            match receiver.recv_timeout(std::time::Duration::from_millis(200)) {
                Ok(chunk) => {
                    seen.push_str(&chunk);
                    let prompt = sign_in_prompt(kind, &seen);
                    if prompt != shown && prompt.url.is_some() {
                        on_prompt(&prompt);
                        shown = prompt;
                    }
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                // Output closed; the process is exiting.
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => std::thread::sleep(std::time::Duration::from_millis(100)),
            }
            let mut running = lock(&self.running);
            let Some((_, child, _)) = running.as_mut() else {
                return Err("Sign-in was cancelled.".into());
            };
            if let Some(exit) = child.try_wait().map_err(|e| e.to_string())? {
                *running = None;
                drop(running);
                if exit.success() && status(kind).signed_in {
                    return Ok(());
                }
                let detail = strip_ansi(&seen).lines().map(str::trim).filter(|l| !l.is_empty()).last().unwrap_or_default().to_string();
                return Err(format!("Signing in to {} didn't finish. {detail}", kind.label()));
            }
        }
    }

    /// Gives Claude Code the code shown after signing in.
    pub fn submit_code(&self, code: &str) -> Result<(), String> {
        let mut running = lock(&self.running);
        match running.as_mut() {
            Some((Kind::Claude, _, Some(stdin))) => {
                writeln!(stdin, "{}", code.trim()).and_then(|_| stdin.flush()).map_err(|e| format!("couldn't pass the code on: {e}"))
            }
            Some(_) => Err("This sign-in doesn't need a code.".into()),
            None => Err("No sign-in is in progress.".into()),
        }
    }

    pub fn cancel(&self) {
        if let Some((_, mut child, _)) = lock(&self.running).take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn pump(reader: &mut impl Read, sender: std::sync::mpsc::Sender<String>) {
    let mut buffer = [0u8; 4096];
    while let Ok(n) = reader.read(&mut buffer) {
        if n == 0 || sender.send(String::from_utf8_lossy(&buffer[..n]).to_string()).is_err() {
            break;
        }
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// [App] notes to pass to the assistant with the user's next message, whichever it is.
#[derive(Default)]
pub struct Notes(Mutex<Vec<String>>);

impl Notes {
    pub fn add(&self, note: String) {
        lock(&self.0).push(note);
    }

    /// The message with pending notes before it; the notes are then cleared.
    pub fn with_message(&self, message: &str) -> String {
        let notes = std::mem::take(&mut *lock(&self.0));
        let mut content = String::new();
        for note in notes {
            content.push_str(&note);
            content.push('\n');
        }
        content.push_str(message);
        content
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_round_trip_and_ignore_old_keys() {
        let dir = std::env::temp_dir().join(format!("suk-settings-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(SETTINGS_FILE), r#"{ "model": null }"#).unwrap();
        assert_eq!(Settings::load(&dir), Settings::default());
        Settings { assistant: Some(Kind::Codex) }.save(&dir).unwrap();
        assert_eq!(Settings::load(&dir).assistant, Some(Kind::Codex));
        std::fs::write(dir.join(SETTINGS_FILE), "not json").unwrap();
        assert_eq!(Settings::load(&dir).assistant, None);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn signed_in_state_is_read_from_each_cli() {
        let logged_in = r#"{ "loggedIn": true, "authMethod": "claude.ai", "email": "prof@example.edu" }"#;
        assert_eq!(claude_auth(logged_in), (true, Some("prof@example.edu".into())));
        assert_eq!(claude_auth(r#"{ "loggedIn": false, "authMethod": "none" }"#), (false, None));
        assert_eq!(claude_auth("error: unknown command"), (false, None));

        assert_eq!(codex_auth(true, "Logged in using ChatGPT\n"), (true, Some("Logged in using ChatGPT".into())));
        assert_eq!(codex_auth(false, "Not logged in\n"), (false, None));
    }

    #[test]
    fn sign_in_instructions_are_read_from_login_output() {
        // Real output of `claude auth login` without a terminal, hyperlink markup included.
        let claude = "Opening browser to sign in…\nIf the browser didn't open, visit: \u{1b}]8;;https://claude.com/cai/oauth/authorize?code=true&client_id=9d1c\u{7}https://claude.com/cai/oauth/authorize?code=true&client_id=9d1c\u{1b}]8;;\u{7}\nPaste code here if prompted > ";
        let prompt = sign_in_prompt(Kind::Claude, claude);
        assert_eq!(prompt.url.as_deref(), Some("https://claude.com/cai/oauth/authorize?code=true&client_id=9d1c"));
        assert!(prompt.needs_code && prompt.code.is_none());
        assert_eq!(sign_in_prompt(Kind::Claude, "Opening browser to sign in…"), SignInPrompt::default());
        // A link cut off mid-way isn't used yet.
        assert_eq!(sign_in_prompt(Kind::Claude, "visit: https://claude.com/cai/oauth/auth"), SignInPrompt::default());

        // Real output of `codex login --device-auth`.
        let codex = "Welcome to Codex [v\u{1b}[90m0.154.0\u{1b}[0m]\n\nFollow these steps to sign in with ChatGPT using device code authorization:\n\n1. Open this link in your browser and sign in to your account\n   \u{1b}[94mhttps://auth.openai.com/codex/device\u{1b}[0m\n\n2. Enter this one-time code \u{1b}[90m(expires in 15 minutes)\u{1b}[0m\n   \u{1b}[94mIB3U-Y26HE\u{1b}[0m\n";
        let prompt = sign_in_prompt(Kind::Codex, codex);
        assert_eq!(prompt.url.as_deref(), Some("https://auth.openai.com/codex/device"));
        assert_eq!(prompt.code.as_deref(), Some("IB3U-Y26HE"));
        assert!(!prompt.needs_code);
    }

    #[test]
    fn notes_go_before_the_next_message_once() {
        let notes = Notes::default();
        notes.add("[App] The user ticked off a task.".into());
        assert_eq!(notes.with_message("Hi"), "[App] The user ticked off a task.\nHi");
        assert_eq!(notes.with_message("Again"), "Again");
    }

    /// Runs the real `claude auth login` against an empty config directory (never the real login),
    /// checks the app gets the sign-in page and a code prompt, then cancels. Needs Claude Code:
    /// `cargo test --lib real_claude_sign_in -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn real_claude_sign_in_prompt_and_cancel() {
        let config = std::env::temp_dir().join(format!("suk-claude-login-{}", std::process::id()));
        std::fs::create_dir_all(&config).unwrap();
        // Only this test runs in the process when invoked as documented.
        std::env::set_var("CLAUDE_CONFIG_DIR", &config);
        assert!(!status(Kind::Claude).signed_in, "an empty config is signed out");
        let sign_in = std::sync::Arc::new(SignIn::default());
        let canceller = sign_in.clone();
        let mut prompts = Vec::new();
        let result = sign_in.run(Kind::Claude, &mut |prompt| {
            prompts.push(prompt.clone());
            let canceller = canceller.clone();
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_millis(500));
                canceller.cancel();
            });
        });
        println!("prompts {prompts:?} result {result:?}");
        assert!(prompts[0].url.as_deref().unwrap().starts_with("https://claude.com/cai/oauth/authorize"));
        assert!(prompts[0].needs_code);
        assert_eq!(result.unwrap_err(), "Sign-in was cancelled.");
        std::fs::remove_dir_all(&config).unwrap();
    }

    #[test]
    fn a_missing_program_is_not_installed() {
        assert!(find_program("suk-no-such-tool").is_none());
        assert!(find_program("sh").is_some());
    }
}
