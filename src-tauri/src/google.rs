//! Google Calendar, signed in from inside the app.
//!
//! The user signs in once: Suk opens Google's consent page in their browser and catches the
//! answer on a loopback address, the way desktop apps are meant to (OAuth 2.0 for installed
//! apps, with PKCE). Suk asks only to manage events, never to read the rest of the calendar,
//! and everything it creates is marked as its own so it can be found, changed and taken back.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Write};
use std::net::{Ipv4Addr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::time::Duration;

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

/// Creating and changing the app's own events, and nothing else. Suk never asks to read the
/// calendar, so what else is on it stays private.
const SCOPE: &str = "https://www.googleapis.com/auth/calendar.events openid email";
const AUTH_URL: &str = "https://accounts.google.com/o/oauth2/v2/auth";
const TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
const EVENTS_URL: &str = "https://www.googleapis.com/calendar/v3/calendars/primary/events";
/// The mark every event carries, so Suk's own entries can be told apart from everything else.
pub const TAG: &str = "suk";
const FILE: &str = "google.json";
/// How long a block lasts when the task says only when it starts.
pub const DEFAULT_MINUTES: i64 = 60;

/// Suk's own OAuth client, set once when the app is built so nobody signing in has to think
/// about it. Google issues these for desktop apps knowing they ship inside the app: the secret
/// isn't one, and PKCE is what actually protects the exchange.
const BUILT_IN_ID: Option<&str> = option_env!("SUK_GOOGLE_CLIENT_ID");
const BUILT_IN_SECRET: Option<&str> = option_env!("SUK_GOOGLE_CLIENT_SECRET");

/// The app's OAuth client, from Google Cloud Console. Desktop clients have no real secret:
/// Google says as much, and PKCE is what actually protects the exchange.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct AppClient {
    pub id: String,
    #[serde(default)]
    pub secret: String,
}

impl AppClient {
    /// The one built into this copy of Suk, if it was built with one.
    pub fn built_in() -> AppClient {
        AppClient {
            id: BUILT_IN_ID.unwrap_or_default().to_string(),
            secret: BUILT_IN_SECRET.unwrap_or_default().to_string(),
        }
    }

    pub fn is_set(&self) -> bool {
        self.id.trim().ends_with(".apps.googleusercontent.com")
    }
}

/// What is kept between runs: how to get a new access token, and which account it belongs to.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct Session {
    pub refresh_token: String,
    pub account: String,
    #[serde(default)]
    access_token: String,
    /// When the access token stops working, in seconds since the epoch.
    #[serde(default)]
    expires_at: i64,
}

impl Session {
    pub fn load(dir: &Path) -> Option<Session> {
        let text = std::fs::read_to_string(dir.join(FILE)).ok()?;
        serde_json::from_str::<Session>(&text).ok().filter(|s| !s.refresh_token.is_empty())
    }

    pub fn save(&self, dir: &Path) -> Result<(), String> {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        let path = dir.join(FILE);
        std::fs::write(&path, serde_json::to_string_pretty(self).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
        // The refresh token is as good as a password, so only the user may read it.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
        }
        Ok(())
    }

    pub fn forget(dir: &Path) {
        let _ = std::fs::remove_file(dir.join(FILE));
    }
}

/// A verifier and the challenge Google is given: proof, later, that the same app is asking.
#[derive(Debug, Clone, PartialEq)]
pub struct Pkce {
    pub verifier: String,
    pub challenge: String,
}

impl Pkce {
    pub fn new() -> Pkce {
        let verifier = format!("{}{}", uuid::Uuid::new_v4().simple(), uuid::Uuid::new_v4().simple());
        Pkce::from_verifier(verifier)
    }

    pub fn from_verifier(verifier: impl Into<String>) -> Pkce {
        let verifier = verifier.into();
        let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
        Pkce { verifier, challenge }
    }
}

/// Where to send the user to say yes.
pub fn consent_url(client: &AppClient, redirect: &str, pkce: &Pkce) -> String {
    let query = [
        ("client_id", client.id.trim()),
        ("redirect_uri", redirect),
        ("response_type", "code"),
        ("scope", SCOPE),
        ("code_challenge", &pkce.challenge),
        ("code_challenge_method", "S256"),
        // Offline and a fresh consent, so Suk is given a refresh token it can keep using.
        ("access_type", "offline"),
        ("prompt", "consent"),
    ];
    let query: Vec<String> = query.iter().map(|(k, v)| format!("{k}={}", encode(v))).collect();
    format!("{AUTH_URL}?{}", query.join("&"))
}

/// Percent-encoding for everything that isn't safe in a query value.
fn encode(value: &str) -> String {
    value
        .bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            b' ' => "%20".to_string(),
            other => format!("%{other:02X}"),
        })
        .collect()
}

/// Waits on a loopback address for Google to send the user back, and gives up after a while.
pub struct Catcher {
    listener: TcpListener,
    pub redirect: String,
}

impl Catcher {
    pub fn new() -> std::io::Result<Catcher> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
        let redirect = format!("http://127.0.0.1:{}", listener.local_addr()?.port());
        Ok(Catcher { listener, redirect })
    }

    /// The code Google sends back, or what went wrong.
    pub fn wait(self, timeout: Duration) -> Result<String, String> {
        self.listener.set_nonblocking(false).map_err(|e| e.to_string())?;
        let deadline = std::time::Instant::now() + timeout;
        loop {
            if std::time::Instant::now() > deadline {
                return Err("Google didn't answer in time. Try signing in again.".into());
            }
            let (stream, _) = self.listener.accept().map_err(|e| e.to_string())?;
            match answer(stream) {
                Some(result) => return result,
                // A browser asking for the favicon, say; keep waiting for the real one.
                None => continue,
            }
        }
    }
}

/// Reads one request, replies with a page the user can close, and reports what it carried.
fn answer(mut stream: TcpStream) -> Option<Result<String, String>> {
    let mut line = String::new();
    BufReader::new(stream.try_clone().ok()?).read_line(&mut line).ok()?;
    let target = line.split_whitespace().nth(1)?;
    let (_, query) = target.split_once('?')?;
    let params = query_pairs(query);
    let result = match (params.get("code"), params.get("error")) {
        (Some(code), _) => Ok(code.clone()),
        (None, Some(error)) if error == "access_denied" => Err("You said no to Google's request.".into()),
        (None, Some(error)) => Err(format!("Google refused: {error}")),
        _ => return None,
    };
    let body = match &result {
        Ok(_) => "<h2>Suk is connected.</h2><p>You can close this tab and go back to the app.</p>",
        Err(message) => {
            let _ = message;
            "<h2>Not connected.</h2><p>Go back to the app and try again.</p>"
        }
    };
    let page = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(page.as_bytes());
    let _ = stream.flush();
    Some(result)
}

fn query_pairs(query: &str) -> BTreeMap<String, String> {
    query
        .split('&')
        .filter_map(|pair| pair.split_once('='))
        .map(|(k, v)| (k.to_string(), decode(v)))
        .collect()
}

fn decode(value: &str) -> String {
    let bytes = value.replace('+', " ").into_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(byte) = u8::from_str_radix(&String::from_utf8_lossy(&bytes[i + 1..i + 3]), 16) {
                out.push(byte);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The account an id_token belongs to. It comes straight from Google over TLS, so its payload is
/// read for the address to show, nothing more.
pub fn account_of(id_token: &str) -> Option<String> {
    let payload = id_token.split('.').nth(1)?;
    let bytes = URL_SAFE_NO_PAD.decode(payload).ok()?;
    let value: Value = serde_json::from_slice(&bytes).ok()?;
    value.get("email")?.as_str().map(str::to_string)
}

/// A block to put on the calendar.
#[derive(Debug, Clone, PartialEq)]
pub struct Block {
    /// The page this came from, kept on the event so the two stay tied together.
    pub page_id: String,
    pub title: String,
    /// Local time with the computer's offset, "2026-09-25T14:00:00+05:30".
    pub start: String,
    pub end: String,
    pub notes: Option<String>,
}

/// The event as Google is asked to create it, marked as Suk's own.
pub fn event_body(block: &Block) -> Value {
    let mut description = block.notes.clone().unwrap_or_default();
    if !description.is_empty() {
        description.push_str("\n\n");
    }
    description.push_str("Added from Suk");
    json!({
        "summary": block.title,
        "description": description,
        "start": { "dateTime": block.start },
        "end": { "dateTime": block.end },
        "source": { "title": "Suk", "url": "https://github.com/bernardnongpoh/suk" },
        // The mark that makes an event Suk's: it survives edits and is invisible to the user.
        "extendedProperties": { "private": { TAG: block.page_id } },
    })
}

/// Talks to Google. Blocks; call it off the async runtime.
pub struct Calendar {
    client: AppClient,
    dir: PathBuf,
}

impl Calendar {
    pub fn new(client: AppClient, dir: PathBuf) -> Calendar {
        Calendar { client, dir }
    }

    /// Signs in: opens Google in the browser, waits for the answer, and keeps the session.
    /// `open` is given the URL so tests can answer without a browser.
    pub fn sign_in(&self, open: impl FnOnce(&str)) -> Result<Session, String> {
        if !self.client.is_set() {
            return Err("Suk needs a Google client ID first.".into());
        }
        let pkce = Pkce::new();
        let catcher = Catcher::new().map_err(|e| e.to_string())?;
        let redirect = catcher.redirect.clone();
        open(&consent_url(&self.client, &redirect, &pkce));
        let code = catcher.wait(Duration::from_secs(300))?;
        let answer = self.token_call(&[
            ("grant_type", "authorization_code"),
            ("code", &code),
            ("code_verifier", &pkce.verifier),
            ("redirect_uri", &redirect),
        ])?;
        let refresh_token = answer["refresh_token"]
            .as_str()
            .ok_or("Google didn't send a refresh token. Sign in again and allow access.")?
            .to_string();
        let session = Session {
            refresh_token,
            account: answer["id_token"].as_str().and_then(account_of).unwrap_or_default(),
            access_token: answer["access_token"].as_str().unwrap_or_default().to_string(),
            expires_at: now() + answer["expires_in"].as_i64().unwrap_or(3600) - 60,
        };
        session.save(&self.dir)?;
        Ok(session)
    }

    /// A working access token, asking Google for a new one when the old has run out.
    fn access_token(&self) -> Result<String, String> {
        let mut session = Session::load(&self.dir).ok_or("Google Calendar isn't connected.")?;
        if !session.access_token.is_empty() && session.expires_at > now() {
            return Ok(session.access_token);
        }
        let answer = self
            .token_call(&[("grant_type", "refresh_token"), ("refresh_token", &session.refresh_token)])
            .map_err(|e| {
                if e.contains("invalid_grant") {
                    "Google signed Suk out. Connect it again in Settings.".to_string()
                } else {
                    e
                }
            })?;
        session.access_token = answer["access_token"].as_str().unwrap_or_default().to_string();
        session.expires_at = now() + answer["expires_in"].as_i64().unwrap_or(3600) - 60;
        session.save(&self.dir)?;
        Ok(session.access_token)
    }

    /// Puts a block on the calendar and returns the event's id and link.
    pub fn add(&self, block: &Block) -> Result<(String, String), String> {
        let token = self.access_token()?;
        let answer = call(|client| {
            client.post(self.events_url()).bearer_auth(&token).json(&event_body(block))
        })?;
        let id = answer["id"].as_str().unwrap_or_default().to_string();
        let link = answer["htmlLink"].as_str().unwrap_or_default().to_string();
        Ok((id, link))
    }

    /// Takes one of Suk's events back off the calendar.
    pub fn remove(&self, event_id: &str) -> Result<(), String> {
        let token = self.access_token()?;
        let url = format!("{}/{event_id}", self.events_url());
        match call(|client| client.delete(&url).bearer_auth(&token)) {
            // Already gone is the state we wanted.
            Err(e) if e.contains("404") || e.contains("410") => Ok(()),
            Err(e) => Err(e),
            Ok(_) => Ok(()),
        }
    }

    fn events_url(&self) -> String {
        std::env::var("SUK_GOOGLE_EVENTS_URL").unwrap_or_else(|_| EVENTS_URL.to_string())
    }

    fn token_call(&self, extra: &[(&str, &str)]) -> Result<Value, String> {
        let url = std::env::var("SUK_GOOGLE_TOKEN_URL").unwrap_or_else(|_| TOKEN_URL.to_string());
        let mut form: Vec<(String, String)> = vec![
            ("client_id".into(), self.client.id.trim().to_string()),
            ("client_secret".into(), self.client.secret.trim().to_string()),
        ];
        form.extend(extra.iter().map(|(k, v)| (k.to_string(), v.to_string())));
        call(|client| client.post(&url).form(&form))
    }
}

/// Sends a request and reads Google's answer, turning its error shape into a sentence.
fn call(build: impl FnOnce(&reqwest::Client) -> reqwest::RequestBuilder) -> Result<Value, String> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|e| e.to_string())?;
    let request = build(&client);
    tauri::async_runtime::block_on(async move {
        let response = request.send().await.map_err(|e| format!("couldn't reach Google: {e}"))?;
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        let value: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
        if status.is_success() {
            return Ok(value);
        }
        let message = value["error"]["message"]
            .as_str()
            .or_else(|| value["error_description"].as_str())
            .or_else(|| value["error"].as_str())
            .unwrap_or(text.trim());
        Err(format!("Google answered {}: {message}", status.as_u16()))
    })
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_consent_page_asks_only_for_events_and_proves_who_is_asking() {
        let client = AppClient { id: "123.apps.googleusercontent.com".into(), secret: "shh".into() };
        // The example from RFC 7636, so the challenge is known to be right.
        let pkce = Pkce::from_verifier("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk");
        assert_eq!(pkce.challenge, "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM");

        let url = consent_url(&client, "http://127.0.0.1:53211", &pkce);
        assert!(url.starts_with("https://accounts.google.com/o/oauth2/v2/auth?"));
        assert!(url.contains("client_id=123.apps.googleusercontent.com"));
        assert!(url.contains("redirect_uri=http%3A%2F%2F127.0.0.1%3A53211"));
        assert!(url.contains("code_challenge=E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"));
        assert!(url.contains("code_challenge_method=S256"));
        assert!(url.contains("access_type=offline"));
        assert!(url.contains("calendar.events"), "events only");
        assert!(!url.contains("auth/calendar&") && !url.contains("calendar.readonly"), "never the whole calendar");

        assert!(!AppClient::default().is_set());
        // Built without one, signing in says so rather than sending people to Google half-ready.
        assert_eq!(AppClient::built_in().is_set(), BUILT_IN_ID.is_some_and(|id| id.ends_with(".apps.googleusercontent.com")));
        assert!(!AppClient { id: "nonsense".into(), secret: String::new() }.is_set());
        assert!(client.is_set());
    }

    #[test]
    fn an_event_is_marked_as_suks_own() {
        let block = Block {
            page_id: "task:review amit's literature survey".into(),
            title: "Review Amit's literature survey".into(),
            start: "2026-09-25T14:00:00+05:30".into(),
            end: "2026-09-25T15:00:00+05:30".into(),
            notes: Some("Chapter 2 first".into()),
        };
        let body = event_body(&block);
        assert_eq!(body["summary"], "Review Amit's literature survey");
        assert_eq!(body["start"]["dateTime"], "2026-09-25T14:00:00+05:30");
        assert_eq!(body["end"]["dateTime"], "2026-09-25T15:00:00+05:30");
        assert_eq!(body["description"], "Chapter 2 first\n\nAdded from Suk");
        assert_eq!(body["source"]["title"], "Suk");
        assert_eq!(body["extendedProperties"]["private"]["suk"], "task:review amit's literature survey");
    }

    #[test]
    fn the_session_is_kept_for_next_time_and_can_be_forgotten() {
        let dir = std::env::temp_dir().join(format!("suk-google-{}", uuid::Uuid::new_v4().simple()));
        assert!(Session::load(&dir).is_none());
        let session = Session { refresh_token: "1//refresh".into(), account: "prof@example.edu".into(), ..Session::default() };
        session.save(&dir).unwrap();
        assert_eq!(Session::load(&dir).unwrap().account, "prof@example.edu");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(dir.join(FILE)).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600, "the refresh token is the user's alone");
        }
        Session::forget(&dir);
        assert!(Session::load(&dir).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn google_sends_the_user_back_with_a_code_or_a_refusal() {
        let id_token = format!(
            "header.{}.signature",
            URL_SAFE_NO_PAD.encode(br#"{"email":"prof@example.edu","sub":"1"}"#)
        );
        assert_eq!(account_of(&id_token).as_deref(), Some("prof@example.edu"));
        assert_eq!(account_of("not a token"), None);

        for (query, expected) in [
            ("code=4%2F0Ab_xyz&scope=email", Ok("4/0Ab_xyz".to_string())),
            ("error=access_denied", Err("You said no to Google's request.".to_string())),
        ] {
            let catcher = Catcher::new().unwrap();
            let redirect = catcher.redirect.clone();
            let waiting = std::thread::spawn(move || catcher.wait(Duration::from_secs(5)));
            let port = redirect.rsplit(':').next().unwrap().to_string();
            let mut stream = TcpStream::connect(format!("127.0.0.1:{port}")).unwrap();
            stream.write_all(format!("GET /?{query} HTTP/1.1\r\nHost: localhost\r\n\r\n").as_bytes()).unwrap();
            let mut reply = String::new();
            let _ = BufReader::new(&stream).read_line(&mut reply);
            assert!(reply.starts_with("HTTP/1.1 200 OK"), "the browser is always answered: {reply}");
            assert_eq!(waiting.join().unwrap(), expected);
        }
    }
}
