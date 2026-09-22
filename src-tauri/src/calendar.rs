//! The time blocks the user confirmed, published as a calendar any app can subscribe to.
//!
//! Suk serves an iCalendar (RFC 5545) feed on localhost, at a fixed port and a secret path kept
//! for the life of the install, so a subscription made once keeps working. Apple Calendar,
//! Outlook and Thunderbird subscribe to the link and refresh themselves; Google Calendar only
//! reads links it can reach from the internet, so for Google the same feed is saved as a file to
//! import. Nothing is read back: the calendar shows what Suk publishes, and Suk never looks at
//! the rest of it.

use std::net::{Ipv4Addr, TcpListener};
use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;

use chrono::TimeZone;
use serde::Serialize;

use crate::graph::{Entity, Graph, GraphError};

/// The port asked for first, so the link stays the same between installs unless it's taken.
pub const PREFERRED_PORT: u16 = 8471;
/// What the calendar is called where it appears.
pub const NAME: &str = "Suk";
/// How often a subscribing app is asked to look again.
const REFRESH: &str = "PT15M";
/// The file a snapshot is saved to, for importing into Google Calendar.
pub const FILE: &str = "Suk.ics";

/// The info key on a task that says what became of it: the calendar event's id once it is on,
/// or "no" when the user said not to ask again.
pub const ON_CALENDAR: &str = "calendar";
pub const DECLINED: &str = "no";

/// A task with a date and a time that isn't on the calendar yet, and the block it would make.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Offer {
    pub task: Entity,
    /// Local time with this computer's offset, as Google wants it.
    pub start: String,
    pub end: String,
    /// How it reads in the question: "Thu 18 Sep, 11:00".
    pub when: String,
}

/// Tasks worth asking about: a date and a time, still to do, and no answer given yet.
pub fn offers(graph: &Graph) -> Result<Vec<Offer>, GraphError> {
    let mut offers = Vec::new();
    for task in graph.entities_of_kind("Task")? {
        let done = task.info.get("status").is_some_and(|s| s == "done");
        if done || task.info.contains_key(ON_CALENDAR) {
            continue;
        }
        let Some(due) = task.info.get("due").filter(|d| d.contains('T')) else { continue };
        let Some((start, end, when)) = window(due, crate::google::DEFAULT_MINUTES) else { continue };
        offers.push(Offer { task, start, end, when });
    }
    offers.sort_by(|a, b| a.start.cmp(&b.start));
    Ok(offers)
}

/// "2026-09-18T11:00" as the start and end of a block, in this computer's time zone.
pub fn window(due: &str, minutes: i64) -> Option<(String, String, String)> {
    let naive = chrono::NaiveDateTime::parse_from_str(due, "%Y-%m-%dT%H:%M").ok()?;
    let start = chrono::Local.from_local_datetime(&naive).single()?;
    let end = start + chrono::Duration::minutes(minutes);
    Some((start.to_rfc3339(), end.to_rfc3339(), start.format("%a %-d %b, %H:%M").to_string()))
}

/// One block on the calendar, from a confirmed Event page.
#[derive(Debug, Clone, PartialEq)]
pub struct Slot {
    /// The Event page's id, which keeps the entry the same one as it changes.
    pub id: String,
    pub title: String,
    /// Local time, "YYYY-MM-DDTHH:MM".
    pub start: String,
    pub end: String,
    pub notes: Option<String>,
    /// The task the block is for, shown in the entry's description.
    pub task: Option<String>,
    /// When the page last changed, in milliseconds.
    pub changed_at: i64,
}

/// The confirmed blocks, oldest first. A block the user hasn't confirmed has no start and end,
/// so it never reaches here.
pub fn slots(graph: &Graph) -> Result<Vec<Slot>, GraphError> {
    let mut slots = Vec::new();
    for event in graph.entities_of_kind("Event")? {
        let (Some(start), Some(end)) = (event.info.get("start"), event.info.get("end")) else {
            continue;
        };
        let task = graph
            .links(&event.id)?
            .into_iter()
            .find(|l| l.kind == "SCHEDULED_FOR")
            .map(|l| l.other.name);
        slots.push(Slot {
            title: event.info.get("title").unwrap_or(&event.name).clone(),
            start: start.clone(),
            end: end.clone(),
            notes: event.info.get("notes").filter(|n| !n.trim().is_empty()).cloned(),
            task,
            changed_at: event.updated_at,
            id: event.id,
        });
    }
    slots.sort_by(|a, b| a.start.cmp(&b.start));
    Ok(slots)
}

/// The whole feed as iCalendar text.
pub fn ics(slots: &[Slot]) -> String {
    let mut lines = vec![
        "BEGIN:VCALENDAR".to_string(),
        "VERSION:2.0".to_string(),
        "PRODID:-//Suk//Suk//EN".to_string(),
        "CALSCALE:GREGORIAN".to_string(),
        "METHOD:PUBLISH".to_string(),
        format!("X-WR-CALNAME:{NAME}"),
        format!("NAME:{NAME}"),
        format!("REFRESH-INTERVAL;VALUE=DURATION:{REFRESH}"),
        format!("X-PUBLISHED-TTL:{REFRESH}"),
    ];
    for slot in slots {
        let stamp = stamp(slot.changed_at);
        let mut description = Vec::new();
        if let Some(notes) = &slot.notes {
            description.push(notes.clone());
        }
        if let Some(task) = &slot.task {
            description.push(format!("Task: {task}"));
        }
        lines.push("BEGIN:VEVENT".to_string());
        lines.push(format!("UID:{}", uid(&slot.id)));
        lines.push(format!("DTSTAMP:{stamp}"));
        lines.push(format!("LAST-MODIFIED:{stamp}"));
        lines.push(format!("DTSTART:{}", local_time(&slot.start)));
        lines.push(format!("DTEND:{}", local_time(&slot.end)));
        lines.push(format!("SUMMARY:{}", escape(&slot.title)));
        if !description.is_empty() {
            lines.push(format!("DESCRIPTION:{}", escape(&description.join("\n"))));
        }
        lines.push("END:VEVENT".to_string());
    }
    lines.push("END:VCALENDAR".to_string());
    lines.iter().map(|line| fold(line)).collect::<Vec<_>>().join("\r\n") + "\r\n"
}

/// An identifier the same entry keeps for good, from the page's id.
fn uid(id: &str) -> String {
    let safe: String = id.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
    format!("{safe}@suk.local")
}

/// "2026-09-25T14:00" as a floating local time, which every calendar shows in the reader's own
/// time zone: the hour the user planned, wherever they open it.
fn local_time(when: &str) -> String {
    let digits: String = when.chars().filter(|c| c.is_ascii_digit()).collect();
    let mut stamp = digits;
    stamp.truncate(14);
    while stamp.len() < 14 {
        stamp.push('0');
    }
    format!("{}T{}", &stamp[..8], &stamp[8..14])
}

/// Milliseconds since the epoch as a UTC timestamp.
fn stamp(millis: i64) -> String {
    let seconds = millis / 1000;
    let (days, rest) = (seconds.div_euclid(86_400), seconds.rem_euclid(86_400));
    let (y, m, d) = civil_from_days(days);
    format!("{y:04}{m:02}{d:02}T{:02}{:02}{:02}Z", rest / 3600, (rest % 3600) / 60, rest % 60)
}

/// Days since 1970-01-01 as a calendar date (Howard Hinnant's civil_from_days).
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// Commas, semicolons, backslashes and newlines carry meaning in iCalendar text.
fn escape(text: &str) -> String {
    text.replace('\\', "\\\\").replace(';', "\\;").replace(',', "\\,").replace('\n', "\\n").replace('\r', "")
}

/// Lines wrap at 75 octets, continuing with a leading space.
fn fold(line: &str) -> String {
    if line.len() <= 75 {
        return line.to_string();
    }
    let mut out = String::new();
    let mut width = 0;
    for c in line.chars() {
        let size = c.len_utf8();
        // The first line holds 75 octets, the rest 74 after the space that continues them.
        let limit = if out.contains("\r\n") { 74 } else { 75 };
        if width + size > limit {
            out.push_str("\r\n ");
            width = 1;
        }
        out.push(c);
        width += size;
    }
    out
}

/// Where the feed can be read, and the secret that makes it the user's alone.
#[derive(Debug, Clone, PartialEq)]
pub struct Feed {
    pub port: u16,
    pub key: String,
}

impl Feed {
    /// For Apple Calendar, Outlook and Thunderbird: subscribing to this keeps them up to date.
    pub fn webcal(&self) -> String {
        format!("webcal://127.0.0.1:{}/{}/{}", self.port, self.key, FILE)
    }

    /// The same feed over plain HTTP, for apps that ask for a URL rather than a subscription.
    pub fn url(&self) -> String {
        format!("http://127.0.0.1:{}/{}/{}", self.port, self.key, FILE)
    }
}

struct Server {
    key: String,
    body: Box<dyn Fn() -> Option<String> + Send + Sync>,
}

/// Serves the feed, asking for `port` but taking any free one if it is in use. `body` returns the
/// feed as it stands, or None while the user has publishing turned off.
pub fn start(
    port: u16,
    key: String,
    body: impl Fn() -> Option<String> + Send + Sync + 'static,
) -> std::io::Result<Feed> {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, port))
        .or_else(|_| TcpListener::bind((Ipv4Addr::LOCALHOST, 0)))?;
    listener.set_nonblocking(true)?;
    let feed = Feed { port: listener.local_addr()?.port(), key: key.clone() };
    let server = Arc::new(Server { key, body: Box::new(body) });
    let app = Router::new().route("/{key}/{file}", get(serve)).with_state(server);
    tauri::async_runtime::spawn(async move {
        let result = match tokio::net::TcpListener::from_std(listener) {
            Ok(listener) => axum::serve(listener, app).await,
            Err(e) => Err(e),
        };
        if let Err(e) = result {
            eprintln!("calendar feed stopped: {e}");
        }
    });
    Ok(feed)
}

async fn serve(State(server): State<Arc<Server>>, Path((key, file)): Path<(String, String)>) -> Response {
    if key != server.key || !file.eq_ignore_ascii_case(FILE) {
        return StatusCode::NOT_FOUND.into_response();
    }
    match (server.body)() {
        Some(text) => (
            [
                (header::CONTENT_TYPE, "text/calendar; charset=utf-8"),
                (header::CACHE_CONTROL, "no-cache"),
            ],
            text,
        )
            .into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    fn slot() -> Slot {
        Slot {
            id: "event:write icse intro (2026-09-25 14:00)".into(),
            title: "Write ICSE intro".into(),
            start: "2026-09-25T14:00".into(),
            end: "2026-09-25T15:30".into(),
            notes: Some("Bring Satya's draft; section 1, 2".into()),
            task: Some("Write the ICSE paper".into()),
            changed_at: 1_789_000_000_000,
        }
    }

    #[test]
    fn a_confirmed_block_reads_as_an_entry_any_calendar_understands() {
        let text = ics(&[slot()]);
        assert!(text.starts_with("BEGIN:VCALENDAR\r\n"));
        assert!(text.ends_with("END:VCALENDAR\r\n"));
        assert!(text.contains("X-WR-CALNAME:Suk"));
        assert!(text.contains("UID:event-write-icse-intro--2026-09-25-14-00-@suk.local"));
        assert!(text.contains("DTSTART:20260925T140000"), "the hour as planned, in local time");
        assert!(text.contains("DTEND:20260925T153000"));
        assert!(text.contains("SUMMARY:Write ICSE intro"));
        assert!(text.contains("DTSTAMP:20260910T002640Z"), "{text}");
        // Commas and semicolons are escaped, and the task comes with the block.
        assert!(text.contains("Bring Satya's draft\\; section 1\\, 2\\nTask: Write the ICSE paper"));
        // Every line fits in 75 octets, folded with a leading space where it doesn't.
        for line in text.lines() {
            assert!(line.len() <= 75, "too long: {line}");
        }
        assert!(ics(&[]).contains("END:VCALENDAR"), "an empty calendar is still a calendar");
    }

    #[test]
    fn the_link_stays_the_same_and_only_the_right_one_is_answered() {
        let feed = Feed { port: 8471, key: "abc123".into() };
        assert_eq!(feed.webcal(), "webcal://127.0.0.1:8471/abc123/Suk.ics");
        assert_eq!(feed.url(), "http://127.0.0.1:8471/abc123/Suk.ics");
    }

    #[test]
    fn a_task_with_a_date_and_a_time_is_worth_asking_about() {
        let g = Graph::in_memory().unwrap();
        let set = |name: &str, pairs: &[(&str, &str)]| {
            let task = g.upsert_entity("Task", name).unwrap();
            let info = pairs.iter().map(|(k, v)| (k.to_string(), Some(v.to_string()))).collect();
            g.update_info(&task.id, &info).unwrap();
            task
        };
        set("Review Amit's survey", &[("due", "2026-09-18T11:00")]);
        set("Later that day", &[("due", "2026-09-18T16:00")]);
        set("No time, just a day", &[("due", "2026-09-18")]);
        set("No date at all", &[("priority", "high")]);
        set("Finished", &[("due", "2026-09-18T09:00"), ("status", "done")]);
        let asked = set("Already answered", &[("due", "2026-09-18T08:00")]);
        g.update_info(&asked.id, &BTreeMap::from([(ON_CALENDAR.to_string(), Some(DECLINED.to_string()))])).unwrap();

        let offers = offers(&g).unwrap();
        assert_eq!(
            offers.iter().map(|o| o.task.name.as_str()).collect::<Vec<_>>(),
            vec!["Review Amit's survey", "Later that day"],
            "only tasks with a time, still to do, and not yet answered"
        );
        assert!(offers[0].start.starts_with("2026-09-18T11:00:00"), "{}", offers[0].start);
        assert!(offers[0].end.starts_with("2026-09-18T12:00:00"), "an hour by default");
        assert!(offers[0].start.len() > 19, "carries this computer's offset");
        assert_eq!(offers[0].when, "Fri 18 Sep, 11:00");
        assert!(window("2026-09-18", 60).is_none(), "a day with no time is not a block");
    }

    #[test]
    fn only_blocks_with_a_start_and_an_end_are_published() {
        let g = Graph::in_memory().unwrap();
        let task = g.upsert_entity("Task", "Write the ICSE paper").unwrap();
        let event = g.upsert_entity("Event", "Write ICSE intro (2026-09-25 14:00)").unwrap();
        let info = |pairs: [(&str, &str); 3]| {
            pairs.into_iter().map(|(k, v)| (k.to_string(), Some(v.to_string()))).collect()
        };
        g.update_info(&event.id, &info([("start", "2026-09-25T14:00"), ("end", "2026-09-25T15:30"), ("title", "Write ICSE intro")])).unwrap();
        g.link(&event.id, "SCHEDULED_FOR", &task.id).unwrap();
        // Proposed but not confirmed: no times, so nothing to publish.
        g.upsert_entity("Event", "Maybe read papers").unwrap();

        let slots = slots(&g).unwrap();
        assert_eq!(slots.len(), 1);
        assert_eq!(slots[0].title, "Write ICSE intro");
        assert_eq!(slots[0].task.as_deref(), Some("Write the ICSE paper"));
    }
}
