//! Following people: reading what they publish and change at the sources that can be read (papers
//! on OpenAlex, homepages, RSS/Atom feeds including GitHub), and storing it as activity.
//!
//! X/Twitter, LinkedIn and Google Scholar forbid or block reading by apps, so their links are
//! kept for the professor to open but never fetched. The first check of a source stores what is
//! already there as baseline, so only later additions are news.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use serde::Serialize;
use serde_json::Value;

use crate::graph::{Activity, ActivityQuery, Entity, Graph, GraphError};
use crate::links::{decode, extract};

/// A followed person's sources are read at most this often, unless a check is asked for.
pub const CHECK_EVERY_MS: i64 = 6 * 60 * 60 * 1000;
/// The tag marking people the professor follows.
pub const FOLLOWING: &str = "following";
const OPENALEX: &str = "https://api.openalex.org";
const MAX_PAGE_TEXT: usize = 50_000;

/// Downloads a URL's body; the app uses the network, tests use canned responses.
pub trait Fetch {
    fn get(&self, url: &str) -> Result<String, String>;
}

pub struct Web;

impl Fetch for Web {
    fn get(&self, url: &str) -> Result<String, String> {
        crate::links::fetch(url, false)
    }
}

/// The detail key a profile link belongs under, and the value to store: a pasted X link is
/// `twitter`, an OpenAlex author link `openalex` with just its id.
pub fn classify_link(url: &str) -> Option<(&'static str, String)> {
    let url = url.trim();
    let parsed = reqwest::Url::parse(url).ok()?;
    let host = parsed.host_str()?.trim_start_matches("www.").to_lowercase();
    let path = parsed.path().trim_end_matches('/');
    let key = match host.as_str() {
        "x.com" | "twitter.com" | "mobile.twitter.com" => "twitter",
        h if h.ends_with("linkedin.com") => "linkedin",
        h if h.starts_with("scholar.google.") => "scholar",
        "dblp.org" | "dblp.uni-trier.de" => "dblp",
        "orcid.org" => "orcid",
        "semanticscholar.org" => "semantic_scholar",
        "openalex.org" | "api.openalex.org" => {
            let id = path.rsplit('/').next().filter(|id| id.starts_with('A'))?;
            return Some(("openalex", id.to_string()));
        }
        "github.com" if path.matches('/').count() == 1 => "github",
        _ if path.ends_with(".rss") || path.ends_with(".atom") || path.ends_with(".xml") || path.ends_with("/feed") || path.ends_with("/rss") => "feed",
        _ => "homepage",
    };
    Some((key, url.to_string()))
}

#[derive(Debug, Clone, PartialEq)]
pub enum Source {
    /// Papers by an OpenAlex author id ("A5051672229").
    OpenAlex(String),
    /// A page whose changes are reported.
    Page(String),
    /// An RSS or Atom feed.
    Feed(String),
}

impl Source {
    pub fn name(&self) -> &'static str {
        match self {
            Source::OpenAlex(_) => "openalex",
            Source::Page(_) => "homepage",
            Source::Feed(_) => "feed",
        }
    }

    fn key(&self, person: &str) -> String {
        match self {
            Source::OpenAlex(id) => format!("openalex:{person}:{id}"),
            Source::Page(url) => format!("homepage:{person}:{url}"),
            Source::Feed(url) => format!("feed:{person}:{url}"),
        }
    }
}

/// What can be checked for a person, and the links that can't be ("X", "LinkedIn").
pub fn sources(person: &Entity) -> (Vec<Source>, Vec<&'static str>) {
    let info = &person.info;
    let mut sources = Vec::new();
    if let Some(id) = info.get("openalex") {
        sources.push(Source::OpenAlex(id.clone()));
    }
    if let Some(url) = info.get("homepage").filter(|u| u.starts_with("http")) {
        sources.push(Source::Page(url.clone()));
    }
    if let Some(url) = info.get("feed").filter(|u| u.starts_with("http")) {
        sources.push(Source::Feed(url.clone()));
    }
    if let Some(github) = info.get("github") {
        let user = github.trim_end_matches('/').rsplit('/').next().unwrap_or_default();
        if !user.is_empty() {
            sources.push(Source::Feed(format!("https://github.com/{user}.atom")));
        }
    }
    let unsupported = [("twitter", "X"), ("linkedin", "LinkedIn"), ("scholar", "Google Scholar")]
        .into_iter()
        .filter(|(key, _)| info.contains_key(*key))
        .map(|(_, label)| label)
        .collect();
    (sources, unsupported)
}

/// A source as shown on a person's page.
#[derive(Debug, Clone, Serialize)]
pub struct SourceInfo {
    pub label: String,
    pub url: String,
    /// When it was last read, if ever.
    pub checked_at: Option<i64>,
}

/// Whether a person is followed, what can be checked, and what can't.
#[derive(Debug, Clone, Serialize)]
pub struct WatchInfo {
    pub following: bool,
    pub sources: Vec<SourceInfo>,
    /// Links kept but never read: "X", "LinkedIn", "Google Scholar".
    pub unsupported: Vec<&'static str>,
    /// Whether an OpenAlex author has been chosen, so papers can be checked.
    pub has_papers: bool,
}

pub fn watch_info(graph: &Graph, person: &Entity) -> Result<WatchInfo, GraphError> {
    let (found, unsupported) = sources(person);
    let mut infos = Vec::new();
    for source in &found {
        let (label, url) = match source {
            Source::OpenAlex(id) => ("Papers".to_string(), format!("https://openalex.org/{id}")),
            Source::Page(url) => ("Homepage".to_string(), url.clone()),
            Source::Feed(url) if url.starts_with("https://github.com/") => ("GitHub".to_string(), url.trim_end_matches(".atom").to_string()),
            Source::Feed(url) => ("Feed".to_string(), url.clone()),
        };
        let checked_at = graph.source_state(&source.key(&person.id))?.map(|(_, at)| at);
        infos.push(SourceInfo { label, url, checked_at });
    }
    Ok(WatchInfo {
        following: person.tags.iter().any(|t| t == FOLLOWING),
        has_papers: found.iter().any(|s| matches!(s, Source::OpenAlex(_))),
        sources: infos,
        unsupported,
    })
}

/// Stores profile links under the keys they belong to (twitter, homepage, openalex…).
pub fn add_links(graph: &Graph, id: &str, urls: &[String]) -> Result<Entity, String> {
    let mut info = std::collections::BTreeMap::new();
    for url in urls {
        let (key, value) = classify_link(url).ok_or_else(|| format!("{url} isn't a web link"))?;
        info.insert(key.to_string(), Some(value));
    }
    graph.update_info(id, &info).map_err(|e| e.to_string())
}

/// Starts or stops following a person.
pub fn set_following(graph: &Graph, person: &Entity, follow: bool) -> Result<Entity, GraphError> {
    let mut tags: Vec<String> = person.tags.iter().filter(|t| *t != FOLLOWING).cloned().collect();
    if follow {
        tags.push(FOLLOWING.into());
    }
    graph.set_tags(&person.id, &tags)
}

/// Whether text is an OpenAlex author id such as "A5051672229".
pub fn is_author_id(id: &str) -> bool {
    id.len() > 1 && id.starts_with('A') && id[1..].bytes().all(|b| b.is_ascii_digit())
}

/// An OpenAlex author who might be the person.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AuthorCandidate {
    pub id: String,
    pub name: String,
    pub works_count: i64,
    pub institutions: Vec<String>,
    pub orcid: Option<String>,
}

/// OpenAlex authors matching a name, most published first.
pub fn author_candidates(fetch: &dyn Fetch, name: &str) -> Result<Vec<AuthorCandidate>, String> {
    let url = format!(
        "{OPENALEX}/authors?search={}&per-page=6&select=id,display_name,works_count,last_known_institutions,orcid",
        encode(name)
    );
    parse_authors(&fetch.get(&url)?)
}

pub fn parse_authors(json: &str) -> Result<Vec<AuthorCandidate>, String> {
    let value: Value = serde_json::from_str(json).map_err(|e| format!("OpenAlex answered oddly: {e}"))?;
    Ok(value["results"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|a| AuthorCandidate {
            id: a["id"].as_str().unwrap_or_default().rsplit('/').next().unwrap_or_default().to_string(),
            name: a["display_name"].as_str().unwrap_or_default().to_string(),
            works_count: a["works_count"].as_i64().unwrap_or_default(),
            institutions: a["last_known_institutions"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|i| i["display_name"].as_str().map(String::from))
                .collect(),
            orcid: a["orcid"].as_str().map(String::from),
        })
        .filter(|a| !a.id.is_empty())
        .collect())
}

/// Something found at a source, before it is stored as activity.
#[derive(Debug, Clone, PartialEq)]
pub struct Found {
    /// Stable within the source: a work id, a post link.
    pub key: String,
    pub kind: &'static str,
    pub title: String,
    pub url: String,
    pub summary: String,
    pub published: String,
}

/// An author's latest works on OpenAlex.
pub fn parse_works(json: &str) -> Result<Vec<Found>, String> {
    let value: Value = serde_json::from_str(json).map_err(|e| format!("OpenAlex answered oddly: {e}"))?;
    Ok(value["results"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|w| {
            let key = w["id"].as_str()?.to_string();
            let location = &w["primary_location"];
            let venue = location["source"]["display_name"].as_str().unwrap_or_default();
            let abstract_text = rebuild_abstract(&w["abstract_inverted_index"]);
            let summary = [venue, abstract_text.as_str()].iter().filter(|s| !s.is_empty()).cloned().collect::<Vec<_>>().join(" — ");
            Some(Found {
                url: w["doi"].as_str().or(location["landing_page_url"].as_str()).unwrap_or(&key).to_string(),
                key,
                kind: "paper",
                title: w["title"].as_str().unwrap_or("Untitled").to_string(),
                summary: truncate(&summary, 700),
                published: w["publication_date"].as_str().unwrap_or_default().to_string(),
            })
        })
        .collect())
}

/// OpenAlex stores abstracts as word -> positions.
fn rebuild_abstract(index: &Value) -> String {
    let Some(map) = index.as_object() else { return String::new() };
    let mut words: Vec<(u64, &str)> = map
        .iter()
        .flat_map(|(word, positions)| positions.as_array().into_iter().flatten().filter_map(|p| p.as_u64()).map(move |p| (p, word.as_str())))
        .collect();
    words.sort();
    words.into_iter().map(|(_, w)| w).collect::<Vec<_>>().join(" ")
}

/// Posts in an RSS or Atom feed, newest first as the feed lists them.
pub fn parse_feed(xml: &str) -> Vec<Found> {
    let (open, close) = if xml.contains("<entry") { ("<entry", "</entry>") } else { ("<item", "</item>") };
    let mut found = Vec::new();
    let mut rest = xml;
    while let Some(start) = rest.find(open) {
        let Some(end) = rest[start..].find(close) else { break };
        let block = &rest[start..start + end];
        rest = &rest[start + end + close.len()..];
        let title = tag_text(block, "title").unwrap_or_default();
        let url = tag_text(block, "link")
            .filter(|l| l.starts_with("http"))
            .or_else(|| attribute(block, "<link", "href"))
            .unwrap_or_default();
        let published = tag_text(block, "published")
            .or_else(|| tag_text(block, "updated"))
            .or_else(|| tag_text(block, "pubDate").map(|d| rfc2822_date(&d)))
            .unwrap_or_default();
        let summary = tag_text(block, "summary")
            .or_else(|| tag_text(block, "description"))
            .or_else(|| tag_text(block, "content"))
            .map(|s| extract(&s).text)
            .unwrap_or_default();
        let key = tag_text(block, "id").or_else(|| tag_text(block, "guid")).unwrap_or_else(|| url.clone());
        if title.is_empty() && url.is_empty() {
            continue;
        }
        found.push(Found { key, kind: "post", title, url, summary: truncate(&summary, 500), published: published.chars().take(10).collect() });
    }
    found
}

fn tag_text(block: &str, tag: &str) -> Option<String> {
    let start = block.find(&format!("<{tag}"))?;
    let after = &block[start..];
    let open_end = after.find('>')?;
    if after[..open_end].ends_with('/') {
        return None;
    }
    let content = &after[open_end + 1..];
    let end = content.find(&format!("</{tag}>"))?;
    let raw = content[..end].trim();
    let raw = raw.strip_prefix("<![CDATA[").and_then(|r| r.strip_suffix("]]>")).unwrap_or(raw);
    Some(decode(raw).trim().to_string()).filter(|t| !t.is_empty())
}

fn attribute(block: &str, element: &str, name: &str) -> Option<String> {
    let start = block.find(element)?;
    let tag = &block[start..start + block[start..].find('>')?];
    let at = tag.find(&format!("{name}=\""))? + name.len() + 2;
    Some(decode(&tag[at..at + tag[at..].find('"')?]))
}

/// "Tue, 15 Sep 2026 10:00:00 GMT" -> "2026-09-15".
fn rfc2822_date(date: &str) -> String {
    chrono::DateTime::parse_from_rfc2822(date.trim()).map(|d| d.format("%Y-%m-%d").to_string()).unwrap_or_else(|_| date.to_string())
}

/// Lines of a page's text that weren't there before.
pub fn added_lines(old: &str, new: &str) -> Vec<String> {
    let before: std::collections::HashSet<&str> = old.lines().map(str::trim).collect();
    new.lines()
        .map(str::trim)
        .filter(|l| l.len() > 3 && !before.contains(l))
        .map(String::from)
        .collect()
}

/// What checking a person found.
#[derive(Debug, Default, Serialize)]
pub struct CheckReport {
    /// Newly found activity that isn't baseline.
    pub new: Vec<Activity>,
    /// Sources that failed, with why.
    pub errors: Vec<String>,
    pub checked: Vec<String>,
}

/// Reads a person's sources that are due (or all, when `force`) and stores what's new.
pub fn check_person(graph: &Graph, fetch: &dyn Fetch, person: &Entity, force: bool, now: i64) -> Result<CheckReport, GraphError> {
    let mut report = CheckReport::default();
    for source in sources(person).0 {
        let key = source.key(&person.id);
        let state = graph.source_state(&key)?;
        if !force && state.as_ref().is_some_and(|(_, at)| now - at < CHECK_EVERY_MS) {
            continue;
        }
        let baseline = state.is_none();
        let found = match &source {
            Source::OpenAlex(id) => {
                let url = format!(
                    "{OPENALEX}/works?filter=author.id:{id}&sort=publication_date:desc&per-page=25&select=id,title,publication_date,primary_location,abstract_inverted_index,doi"
                );
                fetch.get(&url).and_then(|json| parse_works(&json))
            }
            Source::Feed(url) => fetch.get(url).map(|xml| parse_feed(&xml)),
            Source::Page(url) => fetch.get(url).map(|html| {
                let text = truncate(&extract(&html).text, MAX_PAGE_TEXT);
                let previous = state.as_ref().map(|(value, _)| value.clone()).unwrap_or_default();
                let added = if baseline || previous.is_empty() { Vec::new() } else { added_lines(&previous, &text) };
                // The page's text is what gets compared next time.
                let _ = graph.set_source_state(&key, &text, now);
                if added.is_empty() {
                    return Vec::new();
                }
                let host = reqwest::Url::parse(url).ok().and_then(|u| u.host_str().map(String::from)).unwrap_or_default();
                vec![Found {
                    key: format!("{url}#{}", hash(&text)),
                    kind: "page_change",
                    title: format!("{} updated {host}", person.name),
                    url: url.clone(),
                    summary: truncate(&added.into_iter().take(12).collect::<Vec<_>>().join("\n"), 800),
                    published: chrono::Local::now().format("%Y-%m-%d").to_string(),
                }]
            }),
        };
        match found {
            Ok(items) => {
                for item in items {
                    let activity = Activity {
                        id: format!("{}-{}", source.name(), hash(&(&person.id, &item.key))),
                        person: person.id.clone(),
                        source: source.name().to_string(),
                        kind: item.kind.to_string(),
                        title: item.title,
                        url: item.url,
                        summary: item.summary,
                        published: item.published,
                        found_at: now,
                        baseline,
                        relevance: String::new(),
                        reason: String::new(),
                        related: Vec::new(),
                        seen: false,
                        notified: false,
                    };
                    if graph.add_activity(&activity)? && !baseline {
                        report.new.push(activity);
                    }
                }
                if !matches!(source, Source::Page(_)) {
                    graph.set_source_state(&key, "", now)?;
                }
                report.checked.push(source.name().to_string());
            }
            Err(e) => report.errors.push(format!("{}: {e}", source.name())),
        }
    }
    Ok(report)
}

/// How Claude is told to judge updates.
pub const JUDGE_SYSTEM: &str = "You help a university professor keep up with researchers they follow. \
You get the professor's research areas, projects and ideas, and new papers, posts and page changes from people they follow. \
Judge each item only against the professor's work as listed; don't guess at interests that aren't listed. \
Keep reasons under 20 words, and name the specific project, idea or area.";

/// How many items are judged in one question.
const JUDGE_BATCH: usize = 20;

/// The professor's research areas, projects and ideas: what updates are judged against.
fn interests(graph: &Graph) -> Result<Vec<(&'static str, Entity)>, GraphError> {
    let mut found = Vec::new();
    for kind in ["ResearchArea", "Project", "Idea"] {
        found.extend(graph.entities_of_kind(kind)?.into_iter().map(|e| (kind, e)));
    }
    Ok(found)
}

/// Judges new activity that hasn't been judged, asking `ask(prompt, schema)` for the answer.
/// Nothing is judged while the professor has no research areas, projects or ideas to match.
/// Returns how many items were judged.
pub fn judge_pending(graph: &Graph, ask: &dyn Fn(&str, &Value) -> Result<Value, String>) -> Result<usize, String> {
    if interests(graph).map_err(|e| e.to_string())?.is_empty() {
        return Ok(0);
    }
    let pending = graph
        .activities(&ActivityQuery { unjudged_only: true, limit: JUDGE_BATCH, ..Default::default() })
        .map_err(|e| e.to_string())?;
    if pending.is_empty() {
        return Ok(0);
    }
    let prompt = judge_prompt(graph, &pending).map_err(|e| e.to_string())?;
    let answer = ask(&prompt, &judge_schema())?;
    apply_judgements(graph, &answer).map_err(|e| e.to_string())
}

/// Relevant new activity the professor hasn't been notified about, marked notified.
pub fn take_notifications(graph: &Graph) -> Result<Vec<Activity>, GraphError> {
    let items: Vec<Activity> = graph
        .activities(&ActivityQuery { relevant_only: true, unseen_only: true, ..Default::default() })?
        .into_iter()
        .filter(|a| !a.notified && !a.baseline)
        .collect();
    for item in &items {
        graph.mark_activity_notified(&item.id)?;
    }
    Ok(items)
}

/// Runs checks one at a time, and backs off judging after Claude fails.
#[derive(Default)]
pub struct Watcher {
    running: std::sync::Mutex<()>,
    judge_after: std::sync::Mutex<i64>,
}

/// What a round of checking found.
#[derive(Debug, Default)]
pub struct Round {
    pub new: usize,
    pub judged: usize,
    /// Relevant new items to notify the professor about, with the person's name.
    pub notify: Vec<(Activity, String)>,
    pub errors: Vec<String>,
}

/// How long judging waits after Claude couldn't answer.
const JUDGE_RETRY_MS: i64 = 30 * 60 * 1000;
/// At most this many batches are judged in one round.
const JUDGE_ROUNDS: usize = 5;

impl Watcher {
    /// Checks the sources that are due for everyone followed, or all sources of one person
    /// (`only`), judges what's new with `ask` when Claude is available, and picks what to notify.
    pub fn round(
        &self,
        graph: &Graph,
        fetch: &dyn Fetch,
        ask: Option<&dyn Fn(&str, &Value) -> Result<Value, String>>,
        only: Option<&str>,
        now: i64,
    ) -> Result<Round, String> {
        let _running = self.running.lock().unwrap_or_else(|e| e.into_inner());
        let db = |e: GraphError| e.to_string();
        let people = match only {
            Some(id) => graph.get(id).map_err(db)?.into_iter().collect(),
            None => graph.entities_with_tag(FOLLOWING).map_err(db)?,
        };
        let mut round = Round::default();
        for person in people.iter().filter(|p| p.kind == "Person") {
            let report = check_person(graph, fetch, person, only.is_some(), now).map_err(db)?;
            round.new += report.new.len();
            round.errors.extend(report.errors.into_iter().map(|e| format!("{}: {e}", person.name)));
        }
        if let Some(ask) = ask {
            let mut judge_after = self.judge_after.lock().unwrap_or_else(|e| e.into_inner());
            if only.is_some() || now >= *judge_after {
                for _ in 0..JUDGE_ROUNDS {
                    match judge_pending(graph, ask) {
                        Ok(0) => break,
                        Ok(n) => round.judged += n,
                        Err(e) => {
                            *judge_after = now + JUDGE_RETRY_MS;
                            round.errors.push(format!("judging: {e}"));
                            break;
                        }
                    }
                }
            }
        }
        for item in take_notifications(graph).map_err(db)? {
            let name = graph.get(&item.person).map_err(db)?.map(|p| p.name).unwrap_or_default();
            round.notify.push((item, name));
        }
        Ok(round)
    }
}

/// A notification's title and body for relevant new items.
pub fn notification(items: &[(Activity, String)]) -> Option<(String, String)> {
    match items {
        [] => None,
        [(item, name)] => {
            let what = match item.kind.as_str() {
                "paper" => format!("New paper by {name}"),
                "page_change" => format!("{name} updated their page"),
                _ => format!("New from {name}"),
            };
            let body = if item.kind == "page_change" { item.reason.clone() } else { format!("{}\n{}", item.title, item.reason) };
            Some((what, body))
        }
        many => Some((
            format!("{} updates from people you follow", many.len()),
            many.iter().take(3).map(|(item, name)| format!("{name}: {}", item.title)).collect::<Vec<_>>().join("\n"),
        )),
    }
}

/// The professor's interests and the new activity, for Claude to judge.
pub fn judge_prompt(graph: &Graph, items: &[Activity]) -> Result<String, GraphError> {
    let mut prompt = String::from("The professor's work:\n");
    for (kind, e) in interests(graph)? {
        let notes = truncate(&e.notes.replace('\n', " "), 240);
        let tags = e.tags.join(", ");
        prompt.push_str(&format!(
            "- {kind}: {}{}{}\n",
            e.name,
            if tags.is_empty() { String::new() } else { format!(" [{tags}]") },
            if notes.is_empty() { String::new() } else { format!(" — {notes}") }
        ));
    }
    prompt.push_str("\nNew activity from people the professor follows:\n");
    for item in items {
        let who = graph.get(&item.person)?.map(|p| p.name).unwrap_or_default();
        prompt.push_str(&format!(
            "- id {}: {who}, {} ({}): {} — {}\n",
            item.id,
            item.kind,
            item.published,
            item.title,
            truncate(&item.summary.replace('\n', " "), 500)
        ));
    }
    prompt.push_str(
        "\nFor each item, judge how relevant it is to the professor's work: high (directly on one of their projects, ideas or areas), medium (useful nearby work), low (same broad field), none. Give a short reason naming what it relates to, and the exact names of the related pages from the list above.",
    );
    Ok(prompt)
}

pub fn judge_schema() -> Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "items": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "id": { "type": "string" },
                        "relevance": { "enum": ["high", "medium", "low", "none"] },
                        "reason": { "type": "string" },
                        "related": { "type": "array", "items": { "type": "string" } }
                    },
                    "required": ["id", "relevance", "reason", "related"]
                }
            }
        },
        "required": ["items"]
    })
}

/// Stores Claude's judgements; ids it didn't mention stay unjudged. Returns how many were stored.
pub fn apply_judgements(graph: &Graph, judged: &Value) -> Result<usize, GraphError> {
    let mut count = 0;
    for item in judged["items"].as_array().into_iter().flatten() {
        let (Some(id), Some(relevance)) = (item["id"].as_str(), item["relevance"].as_str()) else { continue };
        let related: Vec<String> = item["related"].as_array().into_iter().flatten().filter_map(|r| r.as_str().map(String::from)).collect();
        if graph.judge_activity(id, relevance, item["reason"].as_str().unwrap_or_default(), &related).unwrap_or(false) {
            count += 1;
        }
    }
    Ok(count)
}

fn hash(value: &impl Hash) -> String {
    let mut hasher = DefaultHasher::new();
    value.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut out: String = text.chars().take(max).collect();
    out.push('…');
    out
}

fn encode(text: &str) -> String {
    text.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::{BTreeMap, HashMap};

    /// Canned responses by the longest matching URL prefix; records what was asked for.
    struct Canned {
        responses: RefCell<HashMap<String, String>>,
        asked: RefCell<Vec<String>>,
    }

    impl Canned {
        fn new(pairs: &[(&str, &str)]) -> Self {
            Canned {
                responses: RefCell::new(pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()),
                asked: RefCell::new(Vec::new()),
            }
        }
        fn set(&self, prefix: &str, body: &str) {
            self.responses.borrow_mut().insert(prefix.into(), body.into());
        }
    }

    impl Fetch for Canned {
        fn get(&self, url: &str) -> Result<String, String> {
            self.asked.borrow_mut().push(url.to_string());
            self.responses
                .borrow()
                .iter()
                .filter(|(prefix, _)| url.starts_with(prefix.as_str()))
                .max_by_key(|(prefix, _)| prefix.len())
                .map(|(_, body)| body.clone())
                .filter(|body| body != FAIL)
                .ok_or_else(|| format!("no response for {url}"))
        }
    }

    /// A canned response that fails.
    const FAIL: &str = "<fail>";
    const WORKS: &str = include_str!("fixtures/openalex_works.json");
    const AUTHORS: &str = include_str!("fixtures/openalex_authors.json");

    #[test]
    fn links_are_recognized() {
        let cases = [
            ("https://x.com/AndreasZeller", Some(("twitter", "https://x.com/AndreasZeller"))),
            ("https://twitter.com/foo/", Some(("twitter", "https://twitter.com/foo/"))),
            ("https://www.linkedin.com/in/kavya-rao/", Some(("linkedin", "https://www.linkedin.com/in/kavya-rao/"))),
            ("https://scholar.google.com/citations?user=abc", Some(("scholar", "https://scholar.google.com/citations?user=abc"))),
            ("https://dblp.org/pid/z/AndreasZeller.html", Some(("dblp", "https://dblp.org/pid/z/AndreasZeller.html"))),
            ("https://openalex.org/A5051672229", Some(("openalex", "A5051672229"))),
            ("https://github.com/zeller", Some(("github", "https://github.com/zeller"))),
            ("https://github.com/zeller/fuzzingbook", Some(("homepage", "https://github.com/zeller/fuzzingbook"))),
            ("https://andreas-zeller.info/feed", Some(("feed", "https://andreas-zeller.info/feed"))),
            ("https://andreas-zeller.info/", Some(("homepage", "https://andreas-zeller.info/"))),
        ];
        for (url, expected) in cases {
            assert_eq!(classify_link(url).map(|(k, v)| (k, v)), expected.map(|(k, v)| (k, v.to_string())), "{url}");
        }
        assert_eq!(classify_link("not a link"), None);
    }

    fn person(info: &[(&str, &str)]) -> Entity {
        Entity {
            id: "person:andreas zeller".into(),
            kind: "Person".into(),
            name: "Andreas Zeller".into(),
            info: info.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
            tags: vec![FOLLOWING.into()],
            notes: String::new(),
            updated_at: 0,
            aliases: Vec::new(),
        }
    }

    #[test]
    fn sources_come_from_links_and_social_links_are_only_kept() {
        let p = person(&[
            ("openalex", "A5051672229"),
            ("homepage", "https://andreas-zeller.info/"),
            ("github", "https://github.com/andreas-zeller"),
            ("twitter", "https://x.com/AndreasZeller"),
            ("linkedin", "https://linkedin.com/in/zeller"),
        ]);
        let (found, unsupported) = sources(&p);
        assert_eq!(
            found,
            vec![
                Source::OpenAlex("A5051672229".into()),
                Source::Page("https://andreas-zeller.info/".into()),
                Source::Feed("https://github.com/andreas-zeller.atom".into()),
            ]
        );
        assert_eq!(unsupported, vec!["X", "LinkedIn"]);
    }

    #[test]
    fn following_and_links_show_on_the_page() {
        let g = Graph::in_memory().unwrap();
        let p = g.upsert_entity("Person", "Andreas Zeller").unwrap();
        let p = g.add_tags(&p.id, &["collaborator".into()]).unwrap();
        let p = add_links(&g, &p.id, &["https://x.com/AndreasZeller".into(), "https://andreas-zeller.info/".into(), "https://github.com/zeller".into()]).unwrap();
        assert!(add_links(&g, &p.id, &["andreas-zeller.info".into()]).is_err());
        let info = watch_info(&g, &p).unwrap();
        assert!(!info.following && !info.has_papers);
        assert_eq!(info.sources.iter().map(|s| (s.label.as_str(), s.url.as_str())).collect::<Vec<_>>(), vec![("Homepage", "https://andreas-zeller.info/"), ("GitHub", "https://github.com/zeller")]);
        assert_eq!(info.unsupported, vec!["X"]);

        let p = set_following(&g, &p, true).unwrap();
        let p = set_following(&g, &p, true).unwrap();
        assert_eq!(p.tags.iter().filter(|t| *t == FOLLOWING).count(), 1);
        assert!(p.tags.contains(&"collaborator".to_string()));
        assert_eq!(g.entities_with_tag(FOLLOWING).unwrap().len(), 1);
        let p = add_links(&g, &p.id, &["https://openalex.org/A5051672229".into()]).unwrap();
        let info = watch_info(&g, &p).unwrap();
        assert!(info.following && info.has_papers);
        assert_eq!(info.sources[0].checked_at, None);
        assert!(!set_following(&g, &p, false).unwrap().tags.contains(&FOLLOWING.to_string()));
        assert!(is_author_id("A5051672229") && !is_author_id("A") && !is_author_id("W123"));
    }

    #[test]
    fn openalex_authors_and_works_are_read() {
        let authors = parse_authors(AUTHORS).unwrap();
        assert_eq!(authors[0].id, "A5051672229");
        assert_eq!(authors[0].institutions[0], "Helmholtz Center for Information Security");
        assert!(authors[0].works_count > 400);
        assert_eq!(authors[1].institutions[0], "Roche (Switzerland)", "a different Andreas Zeller");

        let works = parse_works(WORKS).unwrap();
        assert_eq!(works.len(), 2);
        let first = &works[0];
        assert!(first.title.starts_with("Neural Program Modeling"));
        assert_eq!(first.published, "2026-09-04");
        assert_eq!(first.url, "https://doi.org/10.1145/3845620");
        assert!(first.summary.starts_with("ACM Transactions on Software Engineering and Methodology — Understanding the semantics of program code is"), "{}", first.summary);
    }

    #[test]
    fn feeds_are_read() {
        let atom = r#"<?xml version="1.0"?><feed xmlns="http://www.w3.org/2005/Atom"><title>GitHub</title>
          <entry><id>tag:github.com,2008:PushEvent/1</id><published>2026-09-16T10:00:00Z</published>
            <link type="text/html" rel="alternate" href="https://github.com/zeller/fuzzingbook/compare/a...b"/>
            <title type="html">zeller pushed to main in zeller/fuzzingbook</title>
            <content type="html">&lt;p&gt;Add &lt;b&gt;grammar fuzzing&lt;/b&gt; chapter&lt;/p&gt;</content></entry>
          <entry><id>tag:2</id><updated>2026-09-10T08:00:00Z</updated><link href="https://example.org/2"/><title>Second</title></entry></feed>"#;
        let items = parse_feed(atom);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].title, "zeller pushed to main in zeller/fuzzingbook");
        assert_eq!(items[0].url, "https://github.com/zeller/fuzzingbook/compare/a...b");
        assert_eq!(items[0].published, "2026-09-16");
        assert_eq!(items[0].summary, "Add grammar fuzzing chapter");
        assert_eq!(items[1].published, "2026-09-10");

        let rss = r#"<rss><channel><title>Blog</title><item><title><![CDATA[Fuzzing Solidity & friends]]></title>
          <link>https://blog.example.org/solidity</link><guid>https://blog.example.org/?p=7</guid>
          <pubDate>Tue, 15 Sep 2026 10:00:00 GMT</pubDate><description>We fuzzed solc.</description></item></channel></rss>"#;
        let items = parse_feed(rss);
        assert_eq!((items[0].title.as_str(), items[0].url.as_str(), items[0].published.as_str(), items[0].key.as_str()), ("Fuzzing Solidity & friends", "https://blog.example.org/solidity", "2026-09-15", "https://blog.example.org/?p=7"));
    }

    #[test]
    fn first_check_is_baseline_and_later_additions_are_news() {
        let g = Graph::in_memory().unwrap();
        let p = g.upsert_entity("Person", "Andreas Zeller").unwrap();
        let info: BTreeMap<String, Option<String>> = [
            ("openalex", "A5051672229"),
            ("homepage", "https://andreas-zeller.info/"),
            ("feed", "https://andreas-zeller.info/feed"),
        ]
        .iter()
        .map(|(k, v)| (k.to_string(), Some(v.to_string())))
        .collect();
        let p = g.update_info(&p.id, &info).unwrap();
        let page = "<html><body><h1>Andreas Zeller</h1><p>Faculty at CISPA</p></body></html>";
        let feed = "<rss><channel><item><title>Old post</title><link>https://andreas-zeller.info/old</link></item></channel></rss>";
        let fetch = Canned::new(&[
            ("https://api.openalex.org/works?filter=author.id:A5051672229", WORKS),
            ("https://andreas-zeller.info/feed", feed),
            ("https://andreas-zeller.info/", page),
        ]);
        let now = 1_000_000_000_000;

        let first = check_person(&g, &fetch, &p, false, now).unwrap();
        assert!(first.new.is_empty() && first.errors.is_empty(), "{first:?}");
        assert_eq!(first.checked, vec!["openalex", "homepage", "feed"]);
        assert_eq!(g.activities(&ActivityQuery::default()).unwrap().len(), 3, "two papers and a post as baseline");
        assert!(g.activities(&ActivityQuery { unseen_only: true, ..Default::default() }).unwrap().is_empty());

        // Too soon to check again, unless forced.
        let asked = fetch.asked.borrow().len();
        check_person(&g, &fetch, &p, false, now + 60_000).unwrap();
        assert_eq!(fetch.asked.borrow().len(), asked);

        // A new post, a new paper, and a homepage change.
        fetch.set("https://andreas-zeller.info/feed", &feed.replace("<item>", "<item><title>Fuzzing Solidity compilers</title><link>https://andreas-zeller.info/solidity</link></item><item>"));
        let works: Value = serde_json::from_str(WORKS).unwrap();
        let mut more = works.clone();
        let mut newer = works["results"][0].clone();
        newer["id"] = "https://openalex.org/W1".into();
        newer["title"] = "Grammar-Based Fuzzing of Smart Contract Compilers".into();
        newer["publication_date"] = "2026-09-15".into();
        more["results"].as_array_mut().unwrap().insert(0, newer);
        fetch.set("https://api.openalex.org/works?filter=author.id:A5051672229", &more.to_string());
        fetch.set("https://andreas-zeller.info/", &page.replace("<p>Faculty at CISPA</p>", "<p>Faculty at CISPA</p><p>New: PhD positions in compiler testing</p>"));

        let later = check_person(&g, &fetch, &p, false, now + CHECK_EVERY_MS + 1).unwrap();
        let mut titles: Vec<String> = later.new.iter().map(|a| a.title.clone()).collect();
        titles.sort();
        assert_eq!(titles, vec!["Andreas Zeller updated andreas-zeller.info", "Fuzzing Solidity compilers", "Grammar-Based Fuzzing of Smart Contract Compilers"]);
        let change = later.new.iter().find(|a| a.kind == "page_change").unwrap();
        assert_eq!(change.summary, "New: PhD positions in compiler testing");
        assert_eq!(g.activities(&ActivityQuery { unjudged_only: true, ..Default::default() }).unwrap().len(), 3);

        // Nothing new: nothing reported, including the unchanged homepage.
        let again = check_person(&g, &fetch, &p, true, now + 2 * CHECK_EVERY_MS).unwrap();
        assert!(again.new.is_empty(), "{:?}", again.new);

        // A failing source is reported without stopping the others.
        fetch.set("https://andreas-zeller.info/feed", FAIL);
        let failing = check_person(&g, &fetch, &p, true, now + 3 * CHECK_EVERY_MS).unwrap();
        assert_eq!(failing.errors.len(), 1);
        assert_eq!(failing.checked.len(), 2);
    }

    #[test]
    fn a_round_checks_followed_people_judges_and_notifies_once() {
        let g = Graph::in_memory().unwrap();
        g.upsert_entity("Project", "Solidity Compiler Fuzzing").unwrap();
        let followed = g.upsert_entity("Person", "Andreas Zeller").unwrap();
        let info = [("feed", "https://zeller.example/feed")].iter().map(|(k, v)| (k.to_string(), Some(v.to_string()))).collect();
        let followed = g.update_info(&followed.id, &info).unwrap();
        let followed = set_following(&g, &followed, true).unwrap();
        let other = g.upsert_entity("Person", "Not Followed").unwrap();
        let info = [("feed", "https://other.example/feed")].iter().map(|(k, v)| (k.to_string(), Some(v.to_string()))).collect();
        g.update_info(&other.id, &info).unwrap();

        let old = "<rss><item><title>Old</title><link>https://zeller.example/old</link></item></rss>";
        let fetch = Canned::new(&[("https://zeller.example/feed", old), ("https://other.example/feed", old)]);
        let watcher = Watcher::default();
        let judge = |_: &str, _: &Value| -> Result<Value, String> {
            Ok(serde_json::json!({ "items": [{ "id": "?", "relevance": "high", "reason": "", "related": [] }] }))
        };
        let now = 1_000_000_000_000;
        let round = watcher.round(&g, &fetch, Some(&judge), None, now).unwrap();
        assert_eq!((round.new, round.judged, round.notify.len()), (0, 0, 0));
        assert_eq!(fetch.asked.borrow().as_slice(), ["https://zeller.example/feed"], "only followed people");

        let new = "<rss><item><title>Fuzzing solc</title><link>https://zeller.example/solc</link></item><item><title>Cooking</title><link>https://zeller.example/food</link></item></rss>";
        fetch.set("https://zeller.example/feed", new);
        // Claude fails: nothing is notified, and judging waits before trying again.
        let failing = |_: &str, _: &Value| -> Result<Value, String> { Err("not logged in".into()) };
        let later = now + CHECK_EVERY_MS;
        let round = watcher.round(&g, &fetch, Some(&failing), None, later).unwrap();
        assert_eq!((round.new, round.judged, round.notify.len()), (2, 0, 0));
        assert_eq!(round.errors, vec!["judging: not logged in"]);

        let judged = RefCell::new(0);
        let judge = |prompt: &str, _: &Value| -> Result<Value, String> {
            *judged.borrow_mut() += 1;
            let id = |title: &str| {
                let line = prompt.lines().find(|l| l.contains(title)).unwrap();
                line["- id ".len()..line.find(':').unwrap()].to_string()
            };
            Ok(serde_json::json!({ "items": [
                { "id": id("Fuzzing solc"), "relevance": "high", "reason": "Fuzzes the Solidity compiler, like your project", "related": ["Solidity Compiler Fuzzing"] },
                { "id": id("Cooking"), "relevance": "none", "reason": "Unrelated", "related": [] }
            ]}))
        };
        let round = watcher.round(&g, &fetch, Some(&judge), None, later + 60_000).unwrap();
        assert_eq!((round.judged, *judged.borrow()), (0, 0), "waits after a failure");

        // Checking one person now doesn't wait.
        let round = watcher.round(&g, &fetch, Some(&judge), Some(&followed.id), later + 60_000).unwrap();
        assert_eq!(round.judged, 2);
        assert_eq!(round.notify.len(), 1);
        let (title, body) = notification(&round.notify).unwrap();
        assert_eq!(title, "New from Andreas Zeller");
        assert_eq!(body, "Fuzzing solc\nFuzzes the Solidity compiler, like your project");

        let round = watcher.round(&g, &fetch, Some(&judge), None, later + JUDGE_RETRY_MS + 1).unwrap();
        assert!(round.notify.is_empty() && round.errors.is_empty(), "{round:?}");
        assert_eq!(*judged.borrow(), 1);
    }

    #[test]
    fn nothing_is_judged_without_the_professors_work_to_match() {
        let g = Graph::in_memory().unwrap();
        let p = g.upsert_entity("Person", "Andreas Zeller").unwrap();
        let mut item = activity(&p.id, "openalex-1");
        g.add_activity(&item).unwrap();
        let asked = RefCell::new(0);
        let ask = |_: &str, _: &Value| -> Result<Value, String> {
            *asked.borrow_mut() += 1;
            Ok(serde_json::json!({ "items": [{ "id": "openalex-1", "relevance": "medium", "reason": "Nearby", "related": [] }] }))
        };
        assert_eq!(judge_pending(&g, &ask).unwrap(), 0);
        assert_eq!(*asked.borrow(), 0);

        g.upsert_entity("ResearchArea", "Fuzzing").unwrap();
        assert_eq!(judge_pending(&g, &ask).unwrap(), 1);
        assert_eq!(judge_pending(&g, &ask).unwrap(), 0, "already judged");
        assert_eq!(*asked.borrow(), 1);

        // A failed question leaves items to judge next time.
        item.id = "openalex-2".into();
        g.add_activity(&item).unwrap();
        assert!(judge_pending(&g, &|_: &str, _: &Value| Err("offline".to_string())).is_err());
        assert_eq!(g.activities(&ActivityQuery { unjudged_only: true, ..Default::default() }).unwrap().len(), 1);
    }

    fn activity(person: &str, id: &str) -> Activity {
        Activity {
            id: id.into(),
            person: person.into(),
            source: "openalex".into(),
            kind: "paper".into(),
            title: "Grammar-Based Fuzzing of Smart Contract Compilers".into(),
            url: "https://doi.org/x".into(),
            summary: "ICSE — We fuzz Solidity compilers.".into(),
            published: "2026-09-15".into(),
            found_at: 1,
            baseline: false,
            relevance: String::new(),
            reason: String::new(),
            related: vec![],
            seen: false,
            notified: false,
        }
    }

    #[test]
    fn judgements_are_asked_for_with_the_professors_work_and_stored() {
        let g = Graph::in_memory().unwrap();
        let project = g.upsert_entity("Project", "Solidity Compiler Fuzzing").unwrap();
        g.set_notes(&project.id, "BTP project: fuzz solc with grammar-based inputs").unwrap();
        g.upsert_entity("ResearchArea", "Program Repair").unwrap();
        let p = g.upsert_entity("Person", "Andreas Zeller").unwrap();
        let item = activity(&p.id, "openalex-1");
        g.add_activity(&item).unwrap();
        let prompt = judge_prompt(&g, &[item]).unwrap();
        assert!(prompt.contains("- Project: Solidity Compiler Fuzzing — BTP project: fuzz solc with grammar-based inputs"));
        assert!(prompt.contains("- ResearchArea: Program Repair"));
        assert!(prompt.contains("- id openalex-1: Andreas Zeller, paper (2026-09-15): Grammar-Based Fuzzing of Smart Contract Compilers — ICSE — We fuzz Solidity compilers."));

        let judged = serde_json::json!({ "items": [
            { "id": "openalex-1", "relevance": "high", "reason": "Fuzzes Solidity compilers, as your project does", "related": ["Solidity Compiler Fuzzing"] },
            { "id": "ghost", "relevance": "high", "reason": "", "related": [] },
            { "id": "openalex-1", "relevance": "extreme", "reason": "", "related": [] }
        ]});
        assert_eq!(apply_judgements(&g, &judged).unwrap(), 1);
        let notify = take_notifications(&g).unwrap();
        assert_eq!(notify.len(), 1);
        assert!(take_notifications(&g).unwrap().is_empty(), "notified once");
        let stored = g.activities(&ActivityQuery { relevant_only: true, ..Default::default() }).unwrap();
        assert_eq!(stored[0].related, vec!["Solidity Compiler Fuzzing"]);
    }
}
