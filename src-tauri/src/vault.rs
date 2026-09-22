//! Two-way sync between the graph and an Obsidian vault: a folder of Markdown files, one per page.
//!
//! A page's file holds its type, tags and details as frontmatter (Obsidian "properties"), the
//! user's notes as the body, and a generated list of its relationships as [[wikilinks]] so
//! Obsidian's graph and backlinks work. Edits made in Obsidian are read back; the frontmatter `id`
//! keeps a page's identity when its file is renamed or moved.
//!
//! Files changed in the last few seconds are left alone in both directions, so the app never
//! rewrites a note while it is being typed in Obsidian. When both sides changed, the newer one
//! wins: a file is only read in if it changed after the page last did.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, SystemTime};

use serde::Serialize;

use crate::relations::{details_text, sentence};
use crate::graph::{
    info_value, effective_tags, kind_tag, normalize_tag, resolve_kind, section_title, Entity, Graph, Link,
    INPUT_KINDS,
};
use crate::pages::wikilinks;

/// Starts the generated part of a file; everything below it is rewritten by the app. An HTML
/// comment, so no Markdown editor or viewer shows it.
const MARKER: &str = "<!-- Suk keeps the list below up to date. Edits below this line are replaced. -->";
/// Markers written by earlier releases; files with them are read the same way and rewritten.
const OLD_MARKERS: &[&str] = &["%% Professor OS keeps the list below up to date. Edits below this line are replaced. %%"];

/// Where the generated part of a file starts, whichever marker it uses.
fn marker_position(text: &str) -> Option<usize> {
    std::iter::once(MARKER).chain(OLD_MARKERS.iter().copied()).filter_map(|m| text.find(m)).min()
}
/// How long a file must be unchanged before it is read or rewritten.
const SETTLE_MS: u64 = 3000;
/// Frontmatter keys Obsidian uses itself; kept out of a page's details.
const OBSIDIAN_KEYS: &[&str] = &["cssclasses", "publish", "cssclass"];

/// What a file says about its page.
#[derive(Debug, Default, PartialEq)]
pub struct Page {
    pub id: Option<String>,
    pub kind: Option<String>,
    pub tags: Vec<String>,
    /// Obsidian's `aliases`: other names the page is found by.
    pub aliases: Vec<String>,
    pub info: BTreeMap<String, String>,
    pub notes: String,
}

/// The folder a kind's pages go in, e.g. "Students".
pub fn folder(kind: &str) -> String {
    section_title(&kind_tag(kind))
}

/// The kind a folder holds, including folders of alias kinds from earlier releases ("Students").
fn kind_for_folder(name: &str) -> Option<&'static str> {
    INPUT_KINDS.iter().copied().find(|k| folder(k).eq_ignore_ascii_case(name))
}

/// A page name as a file name: characters that files or Obsidian links can't hold become "-".
pub fn file_stem(name: &str) -> String {
    let stem: String = name
        .chars()
        .map(|c| if "/\\:*?\"<>|#^[]".contains(c) || c.is_control() { '-' } else { c })
        .collect();
    let stem = stem.trim().trim_start_matches('.').to_string();
    if stem.is_empty() { "Untitled".into() } else { stem }
}

pub fn render(entity: &Entity, links: &[Link]) -> String {
    let mut out = String::from("---\n");
    out.push_str(&format!("id: {}\n", yaml(&entity.id)));
    out.push_str(&format!("type: {}\n", entity.kind));
    out.push_str("tags:\n");
    for tag in effective_tags(entity) {
        out.push_str(&format!("  - {tag}\n"));
    }
    if !entity.aliases.is_empty() {
        out.push_str("aliases:\n");
        for alias in &entity.aliases {
            out.push_str(&format!("  - {}\n", yaml(alias)));
        }
    }
    for (key, value) in &entity.info {
        out.push_str(&format!("{key}: {}\n", yaml(value)));
    }
    out.push_str("---\n");
    let notes = entity.notes.trim_end();
    if !notes.is_empty() {
        out.push_str(notes);
        out.push('\n');
    }
    let lines: Vec<String> = links
        .iter()
        .filter(|l| l.kind != "LINKS_TO")
        .map(|l| {
            let other = format!("[[{}]]", file_stem(&l.other.name));
            let line = if l.outgoing {
                capitalize(sentence("", &l.kind, &other).trim())
            } else {
                sentence(&other, &l.kind, "this")
            };
            match details_text(l.detail.as_deref(), l.since.as_deref(), l.until.as_deref()) {
                Some(details) => format!("{line} ({details})"),
                None => line,
            }
        })
        .collect();
    if !lines.is_empty() {
        out.push_str(&format!("\n{MARKER}\n## Connections\n"));
        for line in lines {
            out.push_str(&format!("- {line}\n"));
        }
    }
    out
}

fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    chars.next().map(|c| c.to_uppercase().chain(chars).collect()).unwrap_or_default()
}

/// A YAML scalar: plain when that reads back unchanged, otherwise double-quoted.
fn yaml(value: &str) -> String {
    let plain = !value.is_empty()
        && value.chars().next().is_some_and(|c| c.is_alphanumeric())
        && value.chars().all(|c| c.is_alphanumeric() || " .,@()/+_&'-".contains(c))
        && !value.ends_with(' ')
        && !["true", "false", "null", "yes", "no", "on", "off"].contains(&value.to_lowercase().as_str());
    if plain { value.to_string() } else { serde_json::to_string(value).unwrap_or_default() }
}

fn unquote(value: &str) -> String {
    let value = value.trim();
    if value.len() >= 2 && value.starts_with('"') && value.ends_with('"') {
        if let Ok(s) = serde_json::from_str::<String>(value) {
            return s;
        }
    }
    if value.len() >= 2 && value.starts_with('\'') && value.ends_with('\'') {
        return value[1..value.len() - 1].replace("''", "'");
    }
    value.to_string()
}

/// "Full Name" -> "full_name"; None when no usable key remains.
fn info_key(key: &str) -> Option<String> {
    let mut out = String::new();
    for c in key.trim().to_lowercase().chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else if !out.ends_with('_') {
            out.push('_');
        }
    }
    let out: String = out.trim_matches('_').chars().take(32).collect();
    out.starts_with(|c: char| c.is_ascii_lowercase()).then_some(out)
}

pub fn parse(text: &str) -> Page {
    let text = text.replace("\r\n", "\n");
    let mut page = Page::default();
    let mut body = text.as_str();
    if let Some(rest) = text.strip_prefix("---\n") {
        let end = rest.find("\n---\n").map(|i| (i, i + 5)).or_else(|| {
            rest.strip_suffix("\n---").map(|r| (r.len(), rest.len())).or_else(|| rest.starts_with("---\n").then_some((0, 4)))
        });
        if let Some((end, after)) = end {
            read_frontmatter(&rest[..end], &mut page);
            body = &rest[after..];
        }
    }
    let body = match marker_position(body) {
        Some(i) => &body[..i],
        None => body,
    };
    page.notes = body.trim_start_matches('\n').trim_end().to_string();
    page
}

fn read_frontmatter(yaml: &str, page: &mut Page) {
    let mut entries: Vec<(String, Vec<String>)> = Vec::new();
    for line in yaml.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if let (Some(item), Some((_, values))) = (trimmed.strip_prefix("- "), entries.last_mut()) {
            if line.starts_with(char::is_whitespace) || line.starts_with("- ") {
                values.push(unquote(item));
                continue;
            }
        }
        let Some((key, value)) = line.split_once(':') else { continue };
        let value = value.trim();
        let values = if value.starts_with('[') && value.ends_with(']') {
            value[1..value.len() - 1].split(',').map(unquote).filter(|v| !v.is_empty()).collect()
        } else if value.is_empty() {
            Vec::new()
        } else {
            vec![unquote(value)]
        };
        entries.push((key.trim().to_string(), values));
    }
    for (key, values) in entries {
        match key.to_lowercase().as_str() {
            "id" => page.id = values.into_iter().next(),
            "type" => page.kind = values.into_iter().next(),
            "tags" | "tag" => {
                for value in values {
                    page.tags.extend(value.split([',', ' ']).filter_map(normalize_tag));
                }
            }
            "aliases" | "alias" => page.aliases.extend(values.into_iter().filter(|v| !v.trim().is_empty())),
            k if OBSIDIAN_KEYS.contains(&k) => {}
            _ => {
                if let (Some(key), false) = (info_key(&key), values.is_empty()) {
                    page.info.insert(key, values.join(", "));
                }
            }
        }
    }
}

/// The generated part of a file, for comparing two versions of it.
fn generated(text: &str) -> &str {
    marker_position(text).map_or("", |i| text[i..].trim_end())
}

/// Whether two versions of a file say the same thing, ignoring formatting.
fn same(a: &str, b: &str) -> bool {
    parse(a) == parse(b) && generated(a) == generated(b)
}

#[derive(Debug, Default, Serialize, PartialEq)]
pub struct SyncReport {
    /// Pages changed from files edited in Obsidian.
    pub imported: Vec<String>,
    pub written: usize,
    pub deleted: Vec<String>,
}

#[derive(Default)]
struct State {
    /// Content last written or read, by path.
    known: HashMap<PathBuf, String>,
    /// Modification times at the last read, by path.
    mtimes: HashMap<PathBuf, SystemTime>,
    /// Each page's file, by entity id.
    paths: HashMap<String, PathBuf>,
    /// The graph as of the last complete write-out.
    fingerprint: Option<(usize, i64, usize)>,
}

pub struct Vault {
    root: PathBuf,
    state: Mutex<State>,
    settle_ms: AtomicU64,
}

/// What reading a file did.
enum Imported {
    /// The file was applied to the page; true if anything changed.
    Applied(Entity, bool),
    /// The page changed after the file did, so the page's version will be written instead.
    Older(Entity),
}

impl Vault {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into(), state: Mutex::default(), settle_ms: AtomicU64::new(SETTLE_MS) }
    }

    /// The file's modification time, or None while it was changed too recently to touch.
    fn settled_mtime(&self, path: &Path) -> Option<SystemTime> {
        let mtime = std::fs::metadata(path).and_then(|m| m.modified()).ok()?;
        let age = SystemTime::now().duration_since(mtime).unwrap_or_default();
        (age >= Duration::from_millis(self.settle_ms.load(Ordering::Relaxed))).then_some(mtime)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Where an entity's file is, or will be written.
    pub fn path_of(&self, entity: &Entity) -> PathBuf {
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        self.target(entity, state.paths.get(&entity.id))
    }

    /// Forgets a page's file, because the page is gone from the app. Without this the file is
    /// read back on the next sync and the page returns.
    pub fn remove(&self, entity: &Entity) -> Result<(), String> {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let path = self.target(entity, state.paths.get(&entity.id));
        state.paths.remove(&entity.id);
        state.known.remove(&path);
        state.mtimes.remove(&path);
        state.fingerprint = None;
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(format!("couldn't delete {}: {e}", path.display())),
        }
    }

    /// Reads files changed in Obsidian into the graph, then writes changed pages out.
    pub fn sync(&self, graph: &Graph) -> Result<SyncReport, String> {
        std::fs::create_dir_all(&self.root).map_err(|e| e.to_string())?;
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let mut report = SyncReport::default();
        self.import(graph, &mut state, &mut report)?;
        self.export(graph, &mut state, &mut report)?;
        Ok(report)
    }

    fn import(&self, graph: &Graph, state: &mut State, report: &mut SyncReport) -> Result<(), String> {
        let mut seen = Vec::new();
        for path in markdown_files(&self.root) {
            seen.push(path.clone());
            let Some(mtime) = self.settled_mtime(&path) else { continue };
            if state.mtimes.get(&path) == Some(&mtime) {
                continue;
            }
            let Ok(content) = std::fs::read_to_string(&path) else { continue };
            state.mtimes.insert(path.clone(), mtime);
            if state.known.get(&path) == Some(&content) {
                continue;
            }
            let mtime_ms = mtime.duration_since(SystemTime::UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64);
            match self.import_file(graph, &path, &content, mtime_ms) {
                Ok(imported) => {
                    let (entity, older) = match imported {
                        Imported::Applied(entity, changed) => {
                            if changed {
                                report.imported.push(entity.name.clone());
                            }
                            (entity, false)
                        }
                        Imported::Older(entity) => (entity, true),
                    };
                    if let Some(old) = state.paths.insert(entity.id.clone(), path.clone()) {
                        if old != path {
                            state.known.remove(&old);
                        }
                    }
                    if older {
                        // Not recorded as known, so the export below rewrites it.
                        state.fingerprint = None;
                        continue;
                    }
                }
                Err(e) => eprintln!("vault: couldn't read {}: {e}", path.display()),
            }
            state.known.insert(path, content);
        }

        // A deleted note file deletes the note; other pages hold relationships that a file can't
        // restore, so their files are written again instead.
        let missing: Vec<(String, PathBuf)> = state
            .paths
            .iter()
            .filter(|(_, p)| !seen.contains(p))
            .map(|(id, p)| (id.clone(), p.clone()))
            .collect();
        for (id, path) in missing {
            state.known.remove(&path);
            state.mtimes.remove(&path);
            state.paths.remove(&id);
            if let Some(entity) = graph.get(&id).map_err(|e| e.to_string())?.filter(|e| e.kind == "Note") {
                graph.delete(&id).map_err(|e| e.to_string())?;
                report.deleted.push(entity.name);
            }
            state.fingerprint = None;
        }
        Ok(())
    }

    /// Applies one file, last modified at `mtime_ms`, to the graph.
    fn import_file(&self, graph: &Graph, path: &Path, content: &str, mtime_ms: i64) -> Result<Imported, String> {
        let err = |e: crate::graph::GraphError| e.to_string();
        let page = parse(content);
        let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or_default().to_string();
        let valid_kind = page.kind.as_deref().filter(|k| resolve_kind(k).is_some());

        let mut changed = false;
        let by_id = match &page.id {
            Some(id) => graph.get(id).map_err(err)?,
            None => None,
        };
        let found = match by_id {
            Some(entity) => Some(entity),
            None => graph.find_by_name(&stem).map_err(err)?,
        };
        if let Some(entity) = found.as_ref().filter(|e| e.updated_at > mtime_ms) {
            return Ok(Imported::Older(entity.clone()));
        }
        let mut entity = match found {
            Some(entity) => entity,
            None => {
                let folder_kind = path
                    .parent()
                    .and_then(|p| p.file_name())
                    .and_then(|f| f.to_str())
                    .and_then(kind_for_folder);
                changed = true;
                graph.upsert_entity(valid_kind.or(folder_kind).unwrap_or("Note"), &stem).map_err(err)?
            }
        };

        if file_stem(&entity.name) != stem {
            match graph.rename(&entity.id, &stem) {
                Ok(renamed) => {
                    entity = renamed;
                    changed = true;
                }
                Err(e) => eprintln!("vault: not renaming {} to {stem}: {e}", entity.name),
            }
        }
        if let Some(kind) = valid_kind.filter(|k| resolve_kind(k).map(|(k, _)| k) != Some(entity.kind.clone())) {
            entity = graph.change_kind(&entity.id, kind).map_err(err)?;
            changed = true;
        }

        // A date, priority, project status or icon the app can't use ("due: next Friday") is kept
        // as text beside it.
        let mut page_info = BTreeMap::new();
        for (key, value) in &page.info {
            match info_value(&entity.kind, key, value) {
                Err(_) => page_info.insert(format!("{key}_text"), value.clone()),
                _ => page_info.insert(key.clone(), value.clone()),
            };
        }
        let page = Page { info: page_info, ..page };
        let mut info: BTreeMap<String, Option<String>> = BTreeMap::new();
        for (key, value) in &page.info {
            if entity.info.get(key) != Some(value) {
                info.insert(key.clone(), Some(value.clone()));
            }
        }
        for key in entity.info.keys().filter(|k| !page.info.contains_key(*k)) {
            info.insert(key.clone(), None);
        }
        if !info.is_empty() {
            entity = graph.update_info(&entity.id, &info).map_err(err)?;
            changed = true;
        }

        let own = kind_tag(&entity.kind);
        let mut tags: Vec<String> = Vec::new();
        for tag in &page.tags {
            if *tag != own && !tags.contains(tag) {
                tags.push(tag.clone());
            }
        }
        if tags != entity.tags {
            entity = graph.set_tags(&entity.id, &tags).map_err(err)?;
            changed = true;
        }

        if page.aliases != entity.aliases {
            match graph.set_aliases(&entity.id, &page.aliases) {
                Ok(updated) => {
                    changed |= updated.aliases != entity.aliases;
                    entity = updated;
                }
                // Another page already has that name; leave the aliases as they were.
                Err(e) => eprintln!("vault: aliases for {} not changed: {e}", entity.name),
            }
        }

        if page.notes != entity.notes.trim() {
            entity = graph.set_notes(&entity.id, &page.notes).map_err(err)?;
            graph.sync_wikilinks(&entity.id, &wikilinks(&page.notes)).map_err(err)?;
            changed = true;
        }
        Ok(Imported::Applied(entity, changed))
    }

    fn export(&self, graph: &Graph, state: &mut State, report: &mut SyncReport) -> Result<(), String> {
        let fingerprint = graph.fingerprint().map_err(|e| e.to_string())?;
        if state.fingerprint == Some(fingerprint) {
            return Ok(());
        }
        let mut complete = true;
        for entity in graph.all_entities().map_err(|e| e.to_string())? {
            let links = graph.links(&entity.id).map_err(|e| e.to_string())?;
            let rendered = render(&entity, &links);
            let current = state.paths.get(&entity.id).cloned();
            let mut target = self.target(&entity, current.as_ref());
            // Another page already has that file (two pages with one name, from before names
            // were unique): stay where this page's file is, or skip it.
            if target.exists() && owner_of(&target).is_some_and(|id| id != entity.id) {
                match current.as_ref().filter(|c| c.exists() && owner_of(c).as_deref() == Some(entity.id.as_str())) {
                    Some(current) => target = current.clone(),
                    None => continue,
                }
            }

            if let Some(current) = current.filter(|c| *c != target && c.exists()) {
                // Renamed or retyped in the app: move the file, unless it is being edited.
                if self.settled_mtime(&current).is_none() {
                    complete = false;
                    continue;
                }
                if !target.exists() {
                    if let Some(dir) = target.parent() {
                        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
                    }
                    std::fs::rename(&current, &target).map_err(|e| e.to_string())?;
                    if let Some(content) = state.known.remove(&current) {
                        state.known.insert(target.clone(), content);
                    }
                    state.mtimes.remove(&current);
                }
            }
            state.paths.insert(entity.id.clone(), target.clone());

            if state.known.get(&target) == Some(&rendered) {
                continue;
            }
            if target.exists() {
                if self.settled_mtime(&target).is_none() {
                    complete = false;
                    continue;
                }
                let content = std::fs::read_to_string(&target).map_err(|e| e.to_string())?;
                let owner = parse(&content).id;
                if owner.as_ref().is_some_and(|id| *id != entity.id) {
                    eprintln!("vault: {} belongs to {owner:?}; not writing {}", target.display(), entity.name);
                    continue;
                }
                if same(&content, &rendered) {
                    state.known.insert(target, content);
                    continue;
                }
            }
            if let Some(dir) = target.parent() {
                std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
            }
            std::fs::write(&target, &rendered).map_err(|e| e.to_string())?;
            if let Ok(mtime) = std::fs::metadata(&target).and_then(|m| m.modified()) {
                state.mtimes.insert(target.clone(), mtime);
            }
            state.known.insert(target, rendered);
            report.written += 1;
        }
        if complete {
            state.fingerprint = Some(fingerprint);
        }
        Ok(())
    }

    /// The file for `entity`: where it already is, unless it sits in another kind's folder (its
    /// kind changed), renamed to match the page name.
    fn target(&self, entity: &Entity, current: Option<&PathBuf>) -> PathBuf {
        let name = format!("{}.md", file_stem(&entity.name));
        let default = self.root.join(folder(&entity.kind)).join(&name);
        let Some(dir) = current.and_then(|c| c.parent()) else { return default };
        let own_folder = folder(&entity.kind);
        let in_other_kind_folder = dir.parent() == Some(self.root.as_path())
            && dir
                .file_name()
                .and_then(|f| f.to_str())
                .is_some_and(|f| kind_for_folder(f).is_some() && !f.eq_ignore_ascii_case(&own_folder));
        if in_other_kind_folder { default } else { dir.join(name) }
    }
}

/// The page id a file declares, if it can be read.
fn owner_of(path: &Path) -> Option<String> {
    std::fs::read_to_string(path).ok().and_then(|text| parse(&text).id)
}

/// Every .md file under `root`, skipping hidden folders such as .obsidian and .trash.
fn markdown_files(root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let mut dirs = vec![root.to_path_buf()];
    while let Some(dir) = dirs.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            if entry.file_name().to_string_lossy().starts_with('.') {
                continue;
            }
            if path.is_dir() {
                dirs.push(path);
            } else if path.extension().is_some_and(|e| e == "md") {
                files.push(path);
            }
        }
    }
    files.sort();
    files
}


/// Whether Obsidian has this folder registered as a vault (read from Obsidian's own settings).
pub fn registered_in_obsidian(root: &Path) -> bool {
    let Some(home) = std::env::var_os("HOME") else { return false };
    let config = PathBuf::from(home).join("Library/Application Support/obsidian/obsidian.json");
    let Ok(text) = std::fs::read_to_string(config) else { return false };
    let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) else { return false };
    json["vaults"]
        .as_object()
        .is_some_and(|vaults| vaults.values().any(|v| v["path"].as_str().map(Path::new) == Some(root)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entity(kind: &str, name: &str) -> Entity {
        Entity {
            id: format!("{}:{}", kind.to_lowercase(), name.to_lowercase()),
            kind: kind.into(),
            name: name.into(),
            info: BTreeMap::new(),
            tags: Vec::new(),
            notes: String::new(),
            updated_at: 0,
            aliases: Vec::new(),
        }
    }

    #[test]
    fn render_and_parse_round_trip() {
        let mut satya = entity("Person", "Satya");
        satya.info.insert("email".into(), "satya@example.edu".into());
        satya.info.insert("full_name".into(), "Satya: \"S\" Das".into());
        satya.info.insert("start".into(), "2026-08".into());
        satya.tags = vec!["student".into(), "phd".into()];
        satya.notes = "Met on [[2026-09-17]].\n\n- Likes Rust\n".into();
        let links = vec![
            Link { kind: "WORKS_ON".into(), outgoing: true, other: entity("Project", "Fuzzing"), detail: None, since: None, until: None },
            Link { kind: "SUPERVISES".into(), outgoing: false, other: entity("Person", "Prof Sharma"), detail: None, since: None, until: None },
            Link { kind: "LINKS_TO".into(), outgoing: false, other: entity("Note", "Meeting"), detail: None, since: None, until: None },
            Link {
                kind: "AFFILIATED_WITH".into(),
                outgoing: true,
                other: entity("Organization", "IIT Guwahati"),
                detail: Some("PhD scholar".into()),
                since: Some("2024".into()),
                until: None,
            },
        ];
        let text = render(&satya, &links);
        assert_eq!(
            text,
            "---\nid: \"person:satya\"\ntype: Person\ntags:\n  - person\n  - student\n  - phd\nemail: satya@example.edu\n\
             full_name: \"Satya: \\\"S\\\" Das\"\nstart: 2026-08\n---\nMet on [[2026-09-17]].\n\n- Likes Rust\n\n\
             <!-- Suk keeps the list below up to date. Edits below this line are replaced. -->\n\
             ## Connections\n- Works on [[Fuzzing]]\n- [[Prof Sharma]] supervises this\n\
             - Is at [[IIT Guwahati]] (PhD scholar, since 2024)\n"
        );
        let page = parse(&text);
        assert_eq!(page.id.as_deref(), Some("person:satya"));
        assert_eq!(page.kind.as_deref(), Some("Person"));
        assert_eq!(page.tags, vec!["person", "student", "phd"]);
        assert_eq!(page.info, satya.info);
        assert_eq!(page.notes, "Met on [[2026-09-17]].\n\n- Likes Rust");
    }

    #[test]
    fn files_from_earlier_releases_are_read_and_their_marker_replaced() {
        let old = "---\ntype: Project\n---\nGrammar fuzzing.\n\n%% Professor OS keeps the list below up to date. Edits below this line are replaced. %%\n## Connections\n- Old list\n";
        let page = parse(old);
        assert_eq!(page.notes, "Grammar fuzzing.");
        assert_eq!(generated(old), "%% Professor OS keeps the list below up to date. Edits below this line are replaced. %%\n## Connections\n- Old list");
        let link = Link { kind: "WORKS_ON".into(), outgoing: false, other: entity("Person", "Satya"), detail: None, since: None, until: None };
        let rendered = render(&entity("Project", "Fuzzing"), &[link]);
        assert!(!same(old, &rendered), "rewritten with the new marker");
        assert!(rendered.contains("<!-- Suk keeps the list"));
    }

    #[test]
    fn parses_what_obsidian_and_people_write() {
        let page = parse(
            "---\r\ntags: [PhD, fuzzing group]\r\nFull Name: 'O''Brien'\r\naliases:\r\n  - OB\r\nprogram:\r\n---\r\n\r\nBody",
        );
        assert_eq!(page.tags, vec!["phd", "fuzzing", "group"]);
        assert_eq!(page.info, BTreeMap::from([("full_name".to_string(), "O'Brien".to_string())]));
        assert_eq!(page.notes, "Body");
        assert_eq!(parse("No frontmatter\n\n# Heading").notes, "No frontmatter\n\n# Heading");
        assert_eq!(parse("---\ntags: a, #b\n---\n").tags, vec!["a", "b"]);
        assert_eq!(parse("---\nunterminated: yes").notes, "---\nunterminated: yes");
        assert_eq!(file_stem("A/B: c?"), "A-B- c-");
    }

    /// A vault in a temp dir that syncs files as soon as they change.
    struct TestVault {
        vault: Vault,
        graph: Graph,
    }

    impl TestVault {
        fn new(name: &str) -> Self {
            let root = std::env::temp_dir().join(format!("suk-vault-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
            let vault = Vault::new(root);
            vault.settle_ms.store(0, Ordering::Relaxed);
            Self { vault, graph: Graph::in_memory().unwrap() }
        }

        fn sync(&self) -> SyncReport {
            self.vault.sync(&self.graph).unwrap()
        }

        fn write(&self, rel: &str, content: &str) {
            // Files written in the same instant as a page change would tie.
            std::thread::sleep(Duration::from_millis(5));
            let path = self.vault.root().join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, content).unwrap();
        }

        fn read(&self, rel: &str) -> String {
            std::fs::read_to_string(self.vault.root().join(rel)).unwrap_or_default()
        }
    }

    impl Drop for TestVault {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(self.vault.root());
        }
    }

    #[test]
    fn a_page_deleted_in_the_app_does_not_come_back_from_its_file() {
        let t = TestVault::new("deleted");
        let g = &t.graph;
        let amit = g.upsert_entity("Person", "Amit").unwrap();
        t.sync();
        assert!(t.vault.root().join("People/Amit.md").exists());

        g.delete(&amit.id).unwrap();
        t.vault.remove(&amit).unwrap();
        assert!(!t.vault.root().join("People/Amit.md").exists(), "the file goes with the page");

        t.sync();
        assert!(g.find_by_name("Amit").unwrap().is_none(), "and the page stays gone");
        // Deleting a page whose file was already removed is not an error.
        t.vault.remove(&amit).unwrap();
    }

    #[test]
    fn app_changes_are_written_and_obsidian_edits_read_back() {
        let t = TestVault::new("sync");
        let g = &t.graph;
        let satya = g.upsert_entity("Student", "Satya").unwrap();
        let fuzzing = g.upsert_entity("Project", "Fuzzing").unwrap();
        g.link(&satya.id, "WORKS_ON", &fuzzing.id).unwrap();

        assert_eq!(t.sync(), SyncReport { written: 2, ..Default::default() });
        assert!(t.read("People/Satya.md").contains("- Works on [[Fuzzing]]"));
        assert!(t.read("Projects/Fuzzing.md").contains("- [[Satya]] works on this"));
        // Nothing changed: nothing written.
        assert_eq!(t.sync(), SyncReport::default());

        // Edited in Obsidian: notes, a property, a tag, and a link.
        let edited = t.read("People/Satya.md").replace(
            "---\n- Works",
            "---\n",
        );
        let edited = edited.replace("  - student\n", "  - student\n  - phd\nemail: satya@example.edu\n");
        let edited = edited.replace(&format!("{MARKER}"), &format!("Wants to try [[Fuzzing]] on LLVM.\n\n{MARKER}"));
        t.write("People/Satya.md", &edited);
        let report = t.sync();
        assert_eq!(report.imported, vec!["Satya"]);
        let satya = g.get(&satya.id).unwrap().unwrap();
        assert_eq!(satya.info["email"], "satya@example.edu");
        assert_eq!(satya.tags, vec!["student", "phd"]);
        assert_eq!(satya.notes, "Wants to try [[Fuzzing]] on LLVM.");
        assert!(g.links(&satya.id).unwrap().iter().any(|l| l.kind == "LINKS_TO" && l.other.name == "Fuzzing"));
        // The file is rewritten only if the app's version says something different; here the new
        // LINKS_TO is not listed, so the file is left as the user wrote it.
        assert_eq!(report.written, 0);
        assert_eq!(t.read("People/Satya.md"), edited);
    }

    #[test]
    fn new_renamed_moved_and_deleted_files() {
        let t = TestVault::new("files");
        let g = &t.graph;
        g.upsert_entity("Student", "Satya").unwrap();
        t.sync();

        // A note created in Obsidian, and a page in a kind folder without frontmatter.
        t.write("Reading list.md", "Read the [[Satya]] draft");
        t.write("People/Dr Chen.md", "Met at PLDI");
        let report = t.sync();
        assert_eq!(report.imported, vec!["Dr Chen", "Reading list"]);
        assert_eq!(g.find_by_name("Dr Chen").unwrap().unwrap().kind, "Person");
        let note = g.find_by_name("Reading list").unwrap().unwrap();
        assert_eq!(note.kind, "Note");
        // Frontmatter is added, and the note stays where the user put it.
        assert!(t.read("Reading list.md").starts_with("---\nid: \"note:reading list\"\ntype: Note\n"));
        assert_eq!(parse(&t.read("Reading list.md")).notes, "Read the [[Satya]] draft");

        // Renamed and moved in Obsidian: same page, new name, file stays put.
        let content = t.read("People/Satya.md");
        std::fs::create_dir_all(t.vault.root().join("PhD")).unwrap();
        std::fs::remove_file(t.vault.root().join("People/Satya.md")).unwrap();
        t.write("PhD/Satya Das.md", &content);
        let report = t.sync();
        assert_eq!(report.imported, vec!["Satya Das"]);
        assert!(report.deleted.is_empty());
        let satya = g.get("person:satya").unwrap().unwrap();
        assert_eq!(satya.name, "Satya Das");
        assert!(!t.vault.root().join("People/Satya.md").exists());
        assert_eq!(t.vault.path_of(&satya), t.vault.root().join("PhD/Satya Das.md"));

        // Retyped in the app: a file in a kind folder moves to the new kind's folder.
        let idea = g.upsert_entity("Idea", "Type inference").unwrap();
        t.sync();
        g.change_kind(&idea.id, "Project").unwrap();
        g.set_notes(&idea.id, "Now a funded project.").unwrap();
        t.sync();
        assert!(!t.vault.root().join("Ideas/Type inference.md").exists());
        assert!(t.read("Projects/Type inference.md").contains("Now a funded project."));

        // A student's file from an earlier release: read as a person with the student role, and
        // moved from Students/ to People/.
        t.write("Students/Ravi.md", "---\nid: \"student:ravi\"\ntype: Student\ntags:\n  - student\n---\nOld notes");
        t.sync();
        t.sync();
        let ravi = g.find_by_name("Ravi").unwrap().unwrap();
        assert_eq!((ravi.kind.as_str(), ravi.tags.clone()), ("Person", vec!["student".to_string()]));
        assert!(!t.vault.root().join("Students/Ravi.md").exists());
        assert!(t.read("People/Ravi.md").contains("Old notes"));

        // Deleting a note file deletes the note; deleting a student's file brings it back.
        std::fs::remove_file(t.vault.root().join("Reading list.md")).unwrap();
        std::fs::remove_file(t.vault.root().join("PhD/Satya Das.md")).unwrap();
        let report = t.sync();
        assert_eq!(report.deleted, vec!["Reading list"]);
        assert!(g.find_by_name("Reading list").unwrap().is_none());
        assert!(t.read("People/Satya Das.md").contains("id: \"person:satya\""));
    }

    #[test]
    fn task_details_from_obsidian_use_the_columns_or_are_kept_as_text() {
        let t = TestVault::new("tasks");
        t.write("Tasks/NBA report.md", "---\ntype: Task\ndue: 2026-09-25\npriority: High\nstatus: open\n---\nDraft section 3");
        t.write("Tasks/Grant.md", "---\ntype: Task\ndue: next Friday\npriority: urgent\n---\n");
        let report = t.sync();
        assert_eq!(report.imported, vec!["Grant", "NBA report"]);
        let g = &t.graph;
        let nba = g.find_by_name("NBA report").unwrap().unwrap();
        assert_eq!((nba.info["due"].as_str(), nba.info["priority"].as_str()), ("2026-09-25", "high"));
        assert_eq!(g.tasks(&crate::graph::TaskQuery { due_by: Some("2026-09-30".into()), ..Default::default() }).unwrap().len(), 1);
        let grant = g.find_by_name("Grant").unwrap().unwrap();
        assert_eq!(grant.info.get("due_text").map(String::as_str), Some("next Friday"));
        assert_eq!(grant.info.get("priority_text").map(String::as_str), Some("urgent"));
        assert!(!grant.info.contains_key("due"));
    }

    #[test]
    fn icons_round_trip_with_obsidian() {
        let t = TestVault::new("icons");
        let g = &t.graph;
        let p = g.upsert_entity("Project", "Fuzzing").unwrap();
        g.update_info(&p.id, &BTreeMap::from([("icon".to_string(), Some("🐛".to_string()))])).unwrap();
        t.sync();
        let file = t.read("Projects/Fuzzing.md");
        assert!(file.contains("icon: 🐛") || file.contains("icon: \"🐛\""), "{file}");
        std::thread::sleep(std::time::Duration::from_millis(1100));
        t.write("Projects/Fuzzing.md", &file.replace("🐛", "🧪"));
        t.write("Projects/Grant.md", "---\ntype: Project\nicon: LiFolder\n---\n");
        t.sync();
        assert_eq!(g.get(&p.id).unwrap().unwrap().info["icon"], "🧪");
        let grant = g.find_by_name("Grant").unwrap().unwrap();
        assert_eq!((grant.info.get("icon"), grant.info.get("icon_text").map(String::as_str)), (None, Some("LiFolder")));
    }

    #[test]
    fn project_statuses_from_obsidian() {
        let t = TestVault::new("project-status");
        t.write("Projects/Fuzzing.md", "---\ntype: Project\nstatus: In Progress\n---\n");
        t.write("Projects/Grant.md", "---\ntype: Project\nstatus: someday\n---\n");
        t.sync();
        let g = &t.graph;
        assert_eq!(g.find_by_name("Fuzzing").unwrap().unwrap().info["status"], "in-progress");
        let grant = g.find_by_name("Grant").unwrap().unwrap();
        assert_eq!((grant.info.get("status"), grant.info.get("status_text").map(String::as_str)), (None, Some("someday")));
        t.sync();
        assert!(t.read("Projects/Fuzzing.md").contains("status: in-progress"), "{}", t.read("Projects/Fuzzing.md"));
    }

    #[test]
    fn aliases_are_obsidian_aliases_both_ways() {
        let t = TestVault::new("aliases");
        let g = &t.graph;
        let iitg = g.upsert_entity("Organization", "IIT Guwahati").unwrap();
        g.set_aliases(&iitg.id, &["IITG".into()]).unwrap();
        t.sync();
        let file = t.read("Organizations/IIT Guwahati.md");
        assert!(file.contains("aliases:\n  - IITG\n"), "{file}");

        // Added in Obsidian, in its inline list form.
        t.write("Organizations/IIT Guwahati.md", &file.replace("aliases:\n  - IITG\n", "aliases: [IITG, Indian Institute of Technology Guwahati]\n"));
        t.write("Organizations/IISc.md", "---\ntype: Organization\naliases:\n  - Indian Institute of Science\n  - IITG\n---\n");
        t.sync();
        assert_eq!(g.get(&iitg.id).unwrap().unwrap().aliases, vec!["IITG", "Indian Institute of Technology Guwahati"]);
        assert_eq!(g.find_by_name("indian institute of technology guwahati").unwrap().unwrap().id, iitg.id);
        // IISc can't take IITG's other name; its page is still created.
        let iisc = g.find_by_name("IISc").unwrap().unwrap();
        assert_eq!(iisc.kind, "Organization");
        assert!(iisc.aliases.is_empty());
    }

    #[test]
    fn two_pages_with_one_name_keep_their_own_files() {
        let t = TestVault::new("same-name");
        // As earlier releases could store them: a person and a student both named Amit.
        t.write("People/Amit.md", "---\nid: \"person:amit\"\ntype: Person\n---\nThe colleague");
        t.write("Students/Amit.md", "---\nid: \"student:amit\"\ntype: Student\n---\nThe student");
        let g = &t.graph;
        g.upsert_entity("Person", "Amit").unwrap();
        t.sync();
        let report = t.sync();
        assert!(report.imported.is_empty() && report.deleted.is_empty());
        assert!(t.read("People/Amit.md").contains("The colleague"));
        // The second Amit's file isn't overwritten into the first's.
        assert!(t.read("Students/Amit.md").contains("The student") || !t.vault.root().join("Students/Amit.md").exists());
        assert!(t.read("People/Amit.md").contains("person:amit"));
    }

    #[test]
    fn recently_changed_files_are_left_alone() {
        let t = TestVault::new("settle");
        let satya = t.graph.upsert_entity("Student", "Satya").unwrap();
        t.sync();
        t.vault.settle_ms.store(SETTLE_MS, Ordering::Relaxed);
        // Being typed in Obsidian right now (after an app change): not read, and not overwritten.
        t.graph.set_notes(&satya.id, "From the app").unwrap();
        t.write("People/Satya.md", "---\nid: person:satya\ntype: Person\n---\nhalf a sent");
        let report = t.vault.sync(&t.graph).unwrap();
        assert_eq!(report, SyncReport::default());
        assert_eq!(t.read("People/Satya.md"), "---\nid: person:satya\ntype: Person\n---\nhalf a sent");
        // Once it settles, Obsidian's newer version wins.
        t.vault.settle_ms.store(0, Ordering::Relaxed);
        let report = t.sync();
        assert_eq!(report.imported, vec!["Satya"]);
        assert_eq!(t.graph.get(&satya.id).unwrap().unwrap().notes, "half a sent");
    }

    #[test]
    fn after_a_restart_the_newer_side_wins() {
        let t = TestVault::new("restart");
        let satya = t.graph.upsert_entity("Student", "Satya").unwrap();
        let fuzzing = t.graph.upsert_entity("Project", "Fuzzing").unwrap();
        t.graph.set_notes(&satya.id, "\nStarted in August").unwrap();
        t.sync();
        // The app quit before writing its latest change to Satya; Fuzzing was edited in Obsidian
        // while the app was closed.
        std::thread::sleep(Duration::from_millis(5));
        t.graph.set_notes(&satya.id, "Started in August. Wants to work on rustc.").unwrap();
        let fuzzing_file = t.read("Projects/Fuzzing.md").replace("tags:\n  - project\n---\n", "tags:\n  - project\n---\nTarget: rustc\n");
        t.write("Projects/Fuzzing.md", &fuzzing_file);

        let restarted = Vault::new(t.vault.root());
        restarted.settle_ms.store(0, Ordering::Relaxed);
        let report = restarted.sync(&t.graph).unwrap();
        assert_eq!(report.imported, vec!["Fuzzing"]);
        assert_eq!(t.graph.get(&fuzzing.id).unwrap().unwrap().notes, "Target: rustc");
        assert_eq!(t.graph.get(&satya.id).unwrap().unwrap().notes, "Started in August. Wants to work on rustc.");
        assert!(t.read("People/Satya.md").contains("Wants to work on rustc."));
        // A leading blank line in the app's notes isn't a difference worth importing.
        assert!(restarted.sync(&t.graph).unwrap().imported.is_empty());
    }
}
