//! Local knowledge graph backed by an embedded LadybugDB database.
//!
//! Everything is stored as `Entity` nodes joined by `Link` relationships. Both carry a `kind`
//! validated against the lists below, so adding a type or relationship needs no schema migration.
//! Details such as a student's email or a task's due date live in each entity's `info` map; the
//! user's own writing lives in `notes`, and free-form groupings in `tags`.
//!
//! Chat messages are stored too, linked to the entities they were about, so every page can show
//! the conversations behind it. Sidebar sections are views over tags.

use std::collections::BTreeMap;
use std::fmt;
use std::path::Path;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use lbug::{Connection, Database, SystemConfig, Value};
use serde::Serialize;

pub const ENTITY_KINDS: &[&str] = &[
    // Anyone: students, collaborators, colleagues. How they relate to the user is a role.
    "Person",
    "Project",
    "Course",
    "Idea",
    "Task",
    "Document",
    "Event",
    "ResearchArea",
    // A free-standing page the user writes, like an Obsidian note.
    "Note",
    // A university, department, lab, company or funding agency.
    "Organization",
];

pub const RELATION_KINDS: &[&str] = &[
    "WORKS_ON",
    "SUPERVISES",
    "HAS_IDEA",
    "HAS_TASK",
    "RELATED_TO",
    "COLLABORATES_WITH",
    "AUTHORED",
    "MENTIONED_IN",
    "ATTACHED_TO",
    "TAKES",
    "SCHEDULED_FOR",
    // From a page to a page its notes link to with [[wikilinks]]; kept in sync with the notes.
    "LINKS_TO",
    // From a task to the person it is for ("Review Satya's survey").
    "FOR",
    // From a task to the person the user is waiting on ("Kavya's comments on the draft").
    "WAITING_ON",
    // From a task to each person who is to do it ("Rohan presents the state of the art").
    "ASSIGNED_TO",
    // From a person to where they work or study now or did before; the link's detail is the
    // position and since/until the dates.
    "AFFILIATED_WITH",
    // From a person to where they earned a degree; the detail is the degree.
    "STUDIED_AT",
    // From an organization to the one it belongs to (a department to its university).
    "PART_OF",
];

/// How a person is connected to the user, stored as tags; a person can have several.
pub const ROLES: &[&str] = &["student", "collaborator", "colleague", "faculty", "staff", "alumni"];

/// Kinds accepted as input that are stored as another kind with a role: a Student is a Person
/// with the student role. Also what `entities_of_kind` accepts.
const KIND_ALIASES: &[(&str, &str, &str)] = &[("Student", "Person", "student")];

/// Every kind name accepted as input: the stored kinds and their aliases.
pub const INPUT_KINDS: &[&str] = &[
    "Person", "Student", "Project", "Course", "Idea", "Task", "Document", "Event", "ResearchArea", "Note",
    "Organization",
];

/// Whether a name can be a new kind of page ("Grant", "Paper"): one or more capitalized words,
/// written together or spaced, and not a relationship name.
pub fn is_kind_name(kind: &str) -> bool {
    let kind = kind.trim();
    (3..=40).contains(&kind.chars().count())
        && kind.chars().next().is_some_and(|c| c.is_ascii_uppercase())
        && kind.chars().all(|c| c.is_ascii_alphabetic() || c == ' ')
        && kind.split_whitespace().all(|word| word.chars().next().is_some_and(|c| c.is_ascii_uppercase()))
}

/// Whether a name can be a new kind of relationship: WORKS_ON, REVIEWS.
pub fn is_relation_name(kind: &str) -> bool {
    (3..=40).contains(&kind.chars().count())
        && kind.chars().next().is_some_and(|c| c.is_ascii_uppercase())
        && kind.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
        && !RESERVED_RELATIONS.contains(&kind)
}

/// Relationships only the app itself makes.
pub const RESERVED_RELATIONS: &[&str] = &["LINKS_TO", "MENTIONED_IN"];

/// The stored kind for a kind name, and the role it implies. Kinds the app doesn't know are
/// kept as given, so new kinds of page (a Grant, a Paper) can appear as they're needed.
pub fn resolve_kind(kind: &str) -> Option<(String, Option<&'static str>)> {
    if let Some(k) = ENTITY_KINDS.iter().copied().find(|k| *k == kind) {
        return Some((k.to_string(), None));
    }
    if let Some((_, k, role)) = KIND_ALIASES.iter().find(|(alias, _, _)| *alias == kind) {
        return Some((k.to_string(), Some(*role)));
    }
    is_kind_name(kind).then(|| (kind.trim().to_string(), None))
}

/// A person's first role, e.g. "student".
pub fn role_of(entity: &Entity) -> Option<&'static str> {
    ROLES.iter().copied().find(|r| entity.tags.iter().any(|t| t == r))
}

/// Limits on `info`, so a runaway interpretation can't bloat an entity.
const MAX_INFO_KEYS: usize = 30;
const MAX_INFO_VALUE: usize = 2000;
const MAX_TAGS: usize = 20;
const MAX_NOTES: usize = 200_000;

const SCHEMA: &[&str] = &[
    "CREATE NODE TABLE IF NOT EXISTS Entity(id STRING, kind STRING, name STRING, created_at INT64, PRIMARY KEY(id))",
    "CREATE REL TABLE IF NOT EXISTS Link(FROM Entity TO Entity, kind STRING, created_at INT64)",
    // Added after the first release; existing databases gain the column on open.
    "ALTER TABLE Entity ADD IF NOT EXISTS info STRING DEFAULT '{}'",
    "ALTER TABLE Entity ADD IF NOT EXISTS tags STRING DEFAULT '[]'",
    "ALTER TABLE Entity ADD IF NOT EXISTS notes STRING DEFAULT ''",
    "ALTER TABLE Entity ADD IF NOT EXISTS updated_at INT64 DEFAULT 0",
    "CREATE NODE TABLE IF NOT EXISTS Message(id STRING, at INT64, role STRING, text STRING, focus STRING, PRIMARY KEY(id))",
    "CREATE REL TABLE IF NOT EXISTS Mentions(FROM Message TO Entity)",
    "CREATE NODE TABLE IF NOT EXISTS Section(tag STRING, title STRING, pinned BOOLEAN, position INT64, PRIMARY KEY(tag))",
    "ALTER TABLE Section ADD IF NOT EXISTS icon STRING",
    // Details the database filters and sorts on get real columns (see COLUMN_KEYS).
    "ALTER TABLE Entity ADD IF NOT EXISTS status STRING",
    "ALTER TABLE Entity ADD IF NOT EXISTS priority STRING",
    "ALTER TABLE Entity ADD IF NOT EXISTS area STRING",
    "ALTER TABLE Entity ADD IF NOT EXISTS due_date DATE",
    "ALTER TABLE Entity ADD IF NOT EXISTS due_time STRING",
    "ALTER TABLE Entity ADD IF NOT EXISTS done_at INT64",
    // Other names a page is found by, as a JSON list ("IITG" for IIT Guwahati).
    "ALTER TABLE Entity ADD IF NOT EXISTS aliases STRING DEFAULT '[]'",
    // Details on a relationship: a position or degree, and when it started and ended.
    "ALTER TABLE Link ADD IF NOT EXISTS detail STRING",
    "ALTER TABLE Link ADD IF NOT EXISTS since STRING",
    "ALTER TABLE Link ADD IF NOT EXISTS until STRING",
    // Things people the user follows did: new papers, homepage changes, feed posts.
    "CREATE NODE TABLE IF NOT EXISTS Activity(id STRING, person STRING, source STRING, kind STRING, title STRING, url STRING, summary STRING, published STRING, found_at INT64, baseline BOOLEAN, relevance STRING, reason STRING, related STRING, seen BOOLEAN, notified BOOLEAN, PRIMARY KEY(id))",
    // What was last seen at a watched source (a homepage's text), and when it was checked.
    "CREATE NODE TABLE IF NOT EXISTS SourceState(key STRING, value STRING, checked_at INT64, PRIMARY KEY(key))",
];

/// Something a followed person did, found at one of their sources.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Activity {
    pub id: String,
    /// The person's entity id.
    pub person: String,
    /// Where it was found: "openalex", "homepage", "feed".
    pub source: String,
    /// "paper", "page_change" or "post".
    pub kind: String,
    pub title: String,
    pub url: String,
    pub summary: String,
    /// When it was published, as the source gives it (YYYY-MM-DD when known).
    pub published: String,
    pub found_at: i64,
    /// Already there the first time the source was checked; never notified.
    pub baseline: bool,
    /// "high", "medium", "low" or "none" once judged; empty until then.
    pub relevance: String,
    pub reason: String,
    /// Names of the user's pages it relates to.
    pub related: Vec<String>,
    pub seen: bool,
    pub notified: bool,
}

/// Which activity to list.
#[derive(Debug, Default, Clone)]
pub struct ActivityQuery {
    pub person: Option<String>,
    pub unseen_only: bool,
    /// Only items judged high or medium relevance.
    pub relevant_only: bool,
    /// Only new items (not baseline) whose relevance hasn't been judged.
    pub unjudged_only: bool,
    pub limit: usize,
}

/// How deep "part of" chains are followed (lab, department, school, university).
const MAX_PART_DEPTH: usize = 5;
const MAX_ALIASES: usize = 10;

/// Details on a relationship. For each field, None leaves what is stored and Some("") clears it.
#[derive(Debug, Default, Clone)]
pub struct LinkDetails {
    pub detail: Option<String>,
    pub since: Option<String>,
    pub until: Option<String>,
}

/// A date on a relationship: YYYY, YYYY-MM or YYYY-MM-DD.
pub fn valid_link_date(value: &str) -> bool {
    let parts: Vec<&str> = value.split('-').collect();
    let lengths_ok = match parts.len() {
        1 => parts[0].len() == 4,
        2 => parts[0].len() == 4 && parts[1].len() == 2,
        3 => parts[0].len() == 4 && parts[1].len() == 2 && parts[2].len() == 2,
        _ => false,
    };
    lengths_ok && parts.iter().all(|p| p.chars().all(|c| c.is_ascii_digit()))
}

/// Someone's link to an organization, as found from that organization.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Affiliation {
    pub person: Entity,
    /// AFFILIATED_WITH or STUDIED_AT.
    pub kind: String,
    /// The organization linked to: the one asked about, or a part of it (a department).
    pub organization: Entity,
    pub detail: Option<String>,
    pub since: Option<String>,
    pub until: Option<String>,
}

impl Affiliation {
    /// Still there: no end date, or one that hasn't passed.
    pub fn current(&self) -> bool {
        is_current(self.until.as_deref())
    }
}

fn is_current(until: Option<&str>) -> bool {
    let today = chrono::Local::now().format("%Y-%m-%d").to_string();
    until.is_none_or(|u| u.is_empty() || today.as_str() <= u || today.starts_with(u))
}

/// Details stored in their own columns rather than the `info` JSON, so queries can filter and
/// sort on them. They still appear in `Entity::info`.
pub const COLUMN_KEYS: &[&str] = &["status", "priority", "area", "due"];
pub const PRIORITIES: &[&str] = &["high", "medium", "low"];
/// The detail holding a page's emoji icon.
pub const ICON_KEY: &str = "icon";
/// The tag of pages pinned to the sidebar's Favorites.
pub const FAVORITE: &str = "favorite";

/// An icon is one emoji (possibly several code points, like 🧑‍🎓 or 🏳️‍🌈), not text.
pub fn check_icon(value: &str) -> Result<String, GraphError> {
    let icon = value.trim();
    let emoji_like = !icon.is_empty()
        && icon.chars().count() <= 12
        && icon.chars().any(|c| !c.is_ascii())
        && !icon.chars().any(|c| c.is_alphabetic() && c.is_ascii() || c.is_whitespace());
    if emoji_like {
        Ok(icon.to_string())
    } else {
        Err(GraphError::InvalidInfo(format!("icon \"{icon}\" must be a single emoji")))
    }
}

/// Where a project stands, as stored in its status.
pub const PROJECT_STATUSES: &[&str] = &["planned", "in-progress", "completed"];

/// A project status in its stored form, from what the user or Obsidian may write
/// ("In progress", "active", "done").
pub fn project_status(value: &str) -> Option<&'static str> {
    let words = value.trim().to_lowercase().replace(['_', '-'], " ");
    match words.split_whitespace().collect::<Vec<_>>().join(" ").as_str() {
        "planned" | "plan" | "planning" | "proposed" | "upcoming" | "not started" | "idea" | "todo" | "to do" => Some("planned"),
        "in progress" | "inprogress" | "progress" | "active" | "ongoing" | "on going" | "started" | "running" | "current" | "wip" => {
            Some("in-progress")
        }
        "completed" | "complete" | "done" | "finished" | "closed" | "ended" => Some("completed"),
        _ => None,
    }
}

/// A detail value checked for the kind of page it's on: column-backed details as `column_value`,
/// and a project's status as one of `PROJECT_STATUSES`.
pub fn info_value(kind: &str, key: &str, value: &str) -> Result<String, GraphError> {
    if key == ICON_KEY {
        return check_icon(value);
    }
    if !COLUMN_KEYS.contains(&key) {
        return Ok(value.to_string());
    }
    if kind == "Project" && key == "status" {
        return project_status(value).map(String::from).ok_or_else(|| {
            GraphError::InvalidInfo(format!("project status \"{}\" must be planned, in progress or completed", value.trim()))
        });
    }
    column_value(key, value)
}

/// Checks and normalizes a value for a column-backed detail: due is YYYY-MM-DD or
/// YYYY-MM-DDTHH:MM, priority is high/medium/low, status and area are lowercase.
pub fn column_value(key: &str, value: &str) -> Result<String, GraphError> {
    let value = value.trim();
    match key {
        "due" => {
            let valid = chrono::NaiveDate::parse_from_str(value, "%Y-%m-%d").is_ok()
                || chrono::NaiveDateTime::parse_from_str(value, "%Y-%m-%dT%H:%M").is_ok();
            if valid {
                Ok(value.to_string())
            } else {
                Err(GraphError::InvalidInfo(format!("due \"{value}\" must be YYYY-MM-DD or YYYY-MM-DDTHH:MM")))
            }
        }
        "priority" => {
            let lower = value.to_lowercase();
            if PRIORITIES.contains(&lower.as_str()) {
                Ok(lower)
            } else {
                Err(GraphError::InvalidInfo(format!("priority \"{value}\" must be high, medium or low")))
            }
        }
        _ => Ok(value.to_lowercase()),
    }
}

/// Which tasks to list.
#[derive(Debug, Default, Clone)]
pub struct TaskQuery {
    /// Leave out tasks whose status is done.
    pub open_only: bool,
    /// Only tasks done (true) or not done (false); None for both. Overrides `open_only`.
    pub done: Option<bool>,
    /// Only tasks due on or before this date (YYYY-MM-DD).
    pub due_by: Option<String>,
    /// Only tasks linked to this entity: assigned to, for or waiting on a person, or part of a
    /// project/course.
    pub linked_to: Option<String>,
    /// Only tasks assigned to someone (true), or only the user's own (false).
    pub assigned: Option<bool>,
}


#[derive(Debug)]
pub enum GraphError {
    UnknownEntityKind(String),
    UnknownRelationKind(String),
    EmptyName,
    /// A pronoun or generic word ("he", "project") used where a specific name is needed.
    InvalidName(String),
    InvalidInfo(String),
    NotFound(String),
    Db(lbug::Error),
}

impl fmt::Display for GraphError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownEntityKind(kind) => write!(f, "unknown entity kind: {kind}"),
            Self::UnknownRelationKind(kind) => write!(f, "unknown relationship kind: {kind}"),
            Self::EmptyName => write!(f, "entity name is empty"),
            Self::InvalidName(name) => write!(f, "\"{name}\" is not a specific name"),
            Self::InvalidInfo(why) => write!(f, "invalid info: {why}"),
            Self::NotFound(what) => write!(f, "not found: {what}"),
            Self::Db(err) => write!(f, "database error: {err}"),
        }
    }
}

impl std::error::Error for GraphError {}

impl From<lbug::Error> for GraphError {
    fn from(err: lbug::Error) -> Self {
        Self::Db(err)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Entity {
    pub id: String,
    pub kind: String,
    pub name: String,
    /// Details by lowercase key, e.g. "email", "due", "priority".
    pub info: BTreeMap<String, String>,
    /// Lowercase tags the user or Claude added; the kind's own tag is implied, not stored.
    pub tags: Vec<String>,
    /// Markdown the user writes on the entity's page.
    pub notes: String,
    /// Milliseconds since the epoch of the last change to info, tags or notes; 0 if never.
    pub updated_at: i64,
    /// Other names the page is found by.
    pub aliases: Vec<String>,
}

/// A chat message, as stored.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ChatRecord {
    pub id: String,
    pub at: i64,
    /// "user" or "agent".
    pub role: String,
    pub text: String,
    /// The entity whose page the message was sent from, if any.
    pub focus: Option<String>,
}

/// A sidebar section listing everything with `tag`. Dismissed suggestions are kept, unpinned,
/// so they aren't suggested again.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Section {
    pub tag: String,
    pub title: String,
    pub pinned: bool,
    /// An emoji chosen for the section; empty for the default icon.
    pub icon: String,
}

/// A relationship as seen from one entity.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Link {
    pub kind: String,
    /// True when the entity being inspected is the source of the relationship.
    pub outgoing: bool,
    pub other: Entity,
    pub detail: Option<String>,
    pub since: Option<String>,
    pub until: Option<String>,
}

pub struct Graph {
    db: Database,
}

impl Graph {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, GraphError> {
        Self::init(Database::new(path, SystemConfig::default())?)
    }

    #[cfg(test)]
    pub fn in_memory() -> Result<Self, GraphError> {
        Self::init(Database::in_memory(SystemConfig::default())?)
    }

    fn init(db: Database) -> Result<Self, GraphError> {
        {
            let conn = Connection::new(&db)?;
            for ddl in SCHEMA {
                conn.query(ddl)?;
            }
        }
        let graph = Self { db };
        graph.migrate_aliased_kinds()?;
        graph.migrate_info_columns()?;
        graph.migrate_project_statuses()?;
        Ok(graph)
    }

    /// Moves details that became columns out of the `info` JSON of rows saved before.
    fn migrate_info_columns(&self) -> Result<(), GraphError> {
        let conn = Connection::new(&self.db)?;
        let rows: Vec<(String, String)> = conn
            .query("MATCH (e:Entity) RETURN e.id, e.info")?
            .map(|row| (string(&row[0]), string(&row[1])))
            .collect();
        for (id, info) in rows {
            let info: BTreeMap<String, String> = serde_json::from_str(&info).unwrap_or_default();
            if !COLUMN_KEYS.iter().any(|k| info.contains_key(*k)) {
                continue;
            }
            let mut entity = self.get(&id)?.ok_or_else(|| GraphError::NotFound(id.clone()))?;
            // A value that doesn't fit its column stays as an ordinary detail under another name.
            for key in COLUMN_KEYS {
                if let Some(value) = info.get(*key) {
                    if column_value(key, value).is_err() {
                        entity.info.remove(*key);
                        entity.info.insert(format!("{key}_text"), value.clone());
                    }
                }
            }
            self.write_info(&id, &entity.info, now_ms())?;
        }
        Ok(())
    }

    /// Gives projects saved before statuses were fixed one of `PROJECT_STATUSES` ("active" becomes
    /// in-progress); a status that doesn't match any is kept as `status_text`.
    fn migrate_project_statuses(&self) -> Result<(), GraphError> {
        for mut project in self.entities_of_kind("Project")? {
            let Some(status) = project.info.get("status").cloned() else { continue };
            match project_status(&status) {
                Some(stored) if stored == status => continue,
                Some(stored) => {
                    project.info.insert("status".into(), stored.into());
                }
                None => {
                    project.info.remove("status");
                    project.info.insert("status_text".into(), status);
                }
            }
            self.write_info(&project.id, &project.info, now_ms())?;
        }
        Ok(())
    }

    /// Stores entities saved under an alias kind by earlier releases (Student) as the real kind
    /// with the role. Ids are kept, so links, messages and vault files still find them.
    fn migrate_aliased_kinds(&self) -> Result<(), GraphError> {
        for (alias, kind, role) in KIND_ALIASES {
            let conn = Connection::new(&self.db)?;
            let mut stmt = conn.prepare("MATCH (e:Entity) WHERE e.kind = $alias RETURN e.id")?;
            let ids: Vec<String> =
                conn.execute(&mut stmt, vec![("alias", (*alias).into())])?.map(|row| string(&row[0])).collect();
            for id in ids {
                self.set_kind(&id, kind)?;
                self.add_tags(&id, &[role.to_string()])?;
            }
        }
        Ok(())
    }

    /// Returns the entity with this name, creating it with `kind` if needed. Names match case-
    /// and whitespace-insensitively across kinds, so each name is one entity; the first spelling
    /// is kept. An alias kind ("Student") adds its role, also to an existing person.
    pub fn upsert_entity(&self, kind: &str, name: &str) -> Result<Entity, GraphError> {
        let (kind, role) = resolve_kind(kind).ok_or_else(|| GraphError::UnknownEntityKind(kind.into()))?;
        let kind = kind.as_str();
        let name = normalize(name);
        if name.is_empty() {
            return Err(GraphError::EmptyName);
        }
        if let Some(existing) = self.find_by_name(&name)? {
            return match role {
                Some(role) if existing.kind == kind && !existing.tags.iter().any(|t| t == role) => {
                    self.add_tags(&existing.id, &[role.to_string()])
                }
                _ => Ok(existing),
            };
        }
        let created = self.create_entity(kind, &name)?;
        match role {
            Some(role) => self.add_tags(&created.id, &[role.to_string()]),
            None => Ok(created),
        }
    }

    fn create_entity(&self, kind: &str, name: &str) -> Result<Entity, GraphError> {
        let id = entity_id(kind, &name);

        let conn = Connection::new(&self.db)?;
        let mut stmt = conn.prepare(
            &format!(
            "MERGE (e:Entity {{id: $id}}) \
             ON CREATE SET e.kind = $kind, e.name = $name, e.created_at = $now \
             RETURN {}",
            fields("e")
        ),
        )?;
        let mut rows = conn.execute(
            &mut stmt,
            vec![
                ("id", id.clone().into()),
                ("kind", kind.into()),
                ("name", name.into()),
                ("now", now_ms().into()),
            ],
        )?;
        rows.next()
            .map(|row| entity_from(&row, 0))
            .ok_or(GraphError::NotFound(id))
    }

    /// Links two existing entities. Linking the same pair with the same kind again is a no-op.
    pub fn link(&self, from_id: &str, kind: &str, to_id: &str) -> Result<(), GraphError> {
        if !RELATION_KINDS.contains(&kind) && !is_relation_name(kind) {
            return Err(GraphError::UnknownRelationKind(kind.into()));
        }

        let conn = Connection::new(&self.db)?;
        let mut stmt = conn.prepare(
            "MATCH (a:Entity {id: $from}), (b:Entity {id: $to}) \
             MERGE (a)-[r:Link {kind: $kind}]->(b) \
             ON CREATE SET r.created_at = $now \
             RETURN count(r)",
        )?;
        let mut rows = conn.execute(
            &mut stmt,
            vec![
                ("from", from_id.into()),
                ("to", to_id.into()),
                ("kind", kind.into()),
                ("now", now_ms().into()),
            ],
        )?;
        match rows.next().as_deref() {
            Some([Value::Int64(n), ..]) if *n > 0 => Ok(()),
            _ => Err(GraphError::NotFound(format!("{from_id} or {to_id}"))),
        }
    }

    /// Entities of a kind, by name; an alias kind ("Student") lists entities with its role.
    pub fn entities_of_kind(&self, kind: &str) -> Result<Vec<Entity>, GraphError> {
        let (kind, role) = resolve_kind(kind).ok_or_else(|| GraphError::UnknownEntityKind(kind.into()))?;
        let kind = kind.as_str();
        if let Some(role) = role {
            return Ok(self
                .entities_of_kind(kind)?
                .into_iter()
                .filter(|e| e.tags.iter().any(|t| t == role))
                .collect());
        }

        let conn = Connection::new(&self.db)?;
        let mut stmt = conn.prepare(
            &format!("MATCH (e:Entity) WHERE e.kind = $kind RETURN {} ORDER BY lower(e.name)", fields("e")),
        )?;
        let rows = conn.execute(&mut stmt, vec![("kind", kind.into())])?;
        Ok(rows.map(|row| entity_from(&row, 0)).collect())
    }

    pub fn get(&self, id: &str) -> Result<Option<Entity>, GraphError> {
        let conn = Connection::new(&self.db)?;
        let mut stmt =
            conn.prepare(&format!("MATCH (e:Entity {{id: $id}}) RETURN {}", fields("e")))?;
        let mut rows = conn.execute(&mut stmt, vec![("id", id.into())])?;
        Ok(rows.next().map(|row| entity_from(&row, 0)))
    }

    /// Merges `changes` into an entity's info; a `None` value removes that key.
    pub fn update_info(
        &self,
        id: &str,
        changes: &BTreeMap<String, Option<String>>,
    ) -> Result<Entity, GraphError> {
        let mut entity = self.get(id)?.ok_or_else(|| GraphError::NotFound(id.into()))?;
        for (key, value) in changes {
            let valid_key = key.len() <= 32
                && key.starts_with(|c: char| c.is_ascii_lowercase())
                && key.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
            if !valid_key {
                return Err(GraphError::InvalidInfo(format!(
                    "key \"{key}\" must be lowercase letters, digits and _"
                )));
            }
            match value.as_deref().map(str::trim) {
                None | Some("") => {
                    entity.info.remove(key);
                }
                Some(v) if v.chars().count() > MAX_INFO_VALUE => {
                    return Err(GraphError::InvalidInfo(format!("\"{key}\" is too long")));
                }
                Some(v) if COLUMN_KEYS.contains(&key.as_str()) || key == ICON_KEY => {
                    entity.info.insert(key.clone(), info_value(&entity.kind, key, v)?);
                }
                Some(v) => {
                    entity.info.insert(key.clone(), v.to_string());
                }
            }
        }
        if entity.info.len() > MAX_INFO_KEYS {
            return Err(GraphError::InvalidInfo(format!("at most {MAX_INFO_KEYS} keys")));
        }
        let now = now_ms();
        self.write_info(id, &entity.info, now)?;
        entity.updated_at = now;
        Ok(entity)
    }

    /// Writes details: column-backed ones to their columns, the rest as JSON. Marks when a task
    /// became done.
    fn write_info(&self, id: &str, info: &BTreeMap<String, String>, now: i64) -> Result<(), GraphError> {
        let json_info: BTreeMap<&String, &String> =
            info.iter().filter(|(k, _)| !COLUMN_KEYS.contains(&k.as_str())).collect();
        let json = serde_json::to_string(&json_info).map_err(|e| GraphError::InvalidInfo(e.to_string()))?;
        let column = |key: &str| info.get(key).cloned().unwrap_or_default();
        let due = column("due");
        let (due_date, due_time) = match due.split_once('T') {
            Some((date, time)) => (date.to_string(), time.to_string()),
            None => (due, String::new()),
        };
        let conn = Connection::new(&self.db)?;
        let mut stmt = conn.prepare(
            "MATCH (e:Entity {id: $id}) SET \
               e.info = $info, e.updated_at = $now, \
               e.done_at = CASE WHEN $status <> 'done' THEN NULL WHEN e.status = 'done' THEN e.done_at ELSE $now END, \
               e.status = CASE WHEN $status = '' THEN NULL ELSE $status END, \
               e.priority = CASE WHEN $priority = '' THEN NULL ELSE $priority END, \
               e.area = CASE WHEN $area = '' THEN NULL ELSE $area END, \
               e.due_date = CASE WHEN $due_date = '' THEN NULL ELSE date($due_date) END, \
               e.due_time = CASE WHEN $due_time = '' THEN NULL ELSE $due_time END",
        )?;
        conn.execute(
            &mut stmt,
            vec![
                ("id", id.into()),
                ("info", json.into()),
                ("now", now.into()),
                ("status", column("status").into()),
                ("priority", column("priority").into()),
                ("area", column("area").into()),
                ("due_date", due_date.into()),
                ("due_time", due_time.into()),
            ],
        )?;
        Ok(())
    }

    /// Tasks, filtered and ordered by the database: soonest due first (undated last), then by
    /// priority and name.
    pub fn tasks(&self, query: &TaskQuery) -> Result<Vec<Entity>, GraphError> {
        let mut conditions = vec!["e.kind = 'Task'".to_string()];
        match query.done {
            Some(true) => conditions.push("e.status = 'done'".into()),
            Some(false) => conditions.push("(e.status IS NULL OR e.status <> 'done')".into()),
            None if query.open_only => conditions.push("(e.status IS NULL OR e.status <> 'done')".into()),
            None => {}
        }
        let mut params: Vec<(&str, Value)> = Vec::new();
        if let Some(due_by) = &query.due_by {
            column_value("due", due_by)?;
            conditions.push("e.due_date IS NOT NULL AND e.due_date <= date($due_by)".into());
            params.push(("due_by", due_by.clone().into()));
        }
        let assigned = "EXISTS { MATCH (e)-[x:Link]->(:Entity) WHERE x.kind = 'ASSIGNED_TO' }";
        match query.assigned {
            Some(true) => conditions.push(assigned.into()),
            Some(false) => conditions.push(format!("NOT {assigned}")),
            None => {}
        }
        let pattern = match &query.linked_to {
            Some(id) => {
                params.push(("linked", id.clone().into()));
                conditions.push("l.kind IN ['FOR', 'WAITING_ON', 'HAS_TASK', 'ASSIGNED_TO']".into());
                "MATCH (e:Entity)-[l:Link]-(x:Entity {id: $linked})"
            }
            None => "MATCH (e:Entity)",
        };
        // Sorted on plain text and number keys: ordering on NULLs and booleans came out
        // inconsistently between runs.
        let conn = Connection::new(&self.db)?;
        let mut stmt = conn.prepare(&format!(
            "{pattern} WHERE {} \
             WITH DISTINCT e \
             RETURN {}, \
               CASE WHEN e.due_date IS NULL THEN '9999-12-31' ELSE string(e.due_date) END AS due_key, \
               CASE WHEN e.due_time IS NULL THEN '99:99' ELSE e.due_time END AS time_key, \
               CASE e.priority WHEN 'high' THEN 0 WHEN 'medium' THEN 1 WHEN 'low' THEN 2 ELSE 1 END AS rank \
             ORDER BY due_key, time_key, rank, lower(e.name)",
            conditions.join(" AND "),
            fields("e")
        ))?;
        let rows = conn.execute(&mut stmt, params)?;
        Ok(rows.map(|row| entity_from(&row, 0)).collect())
    }

    /// Marks a task done or open again.
    pub fn set_done(&self, id: &str, done: bool) -> Result<Entity, GraphError> {
        let status = if done { "done" } else { "open" };
        self.update_info(id, &BTreeMap::from([("status".to_string(), Some(status.to_string()))]))
    }

    /// Adds tags (normalized; "#PhD Students" becomes "phd-students") and returns the entity.
    pub fn add_tags(&self, id: &str, tags: &[String]) -> Result<Entity, GraphError> {
        let entity = self.get(id)?.ok_or_else(|| GraphError::NotFound(id.into()))?;
        let mut all = entity.tags.clone();
        all.extend(tags.iter().cloned());
        self.set_tags(id, &all)
    }

    /// Replaces an entity's tags. The kind's own tag is dropped, since it is always implied.
    pub fn set_tags(&self, id: &str, tags: &[String]) -> Result<Entity, GraphError> {
        let mut entity = self.get(id)?.ok_or_else(|| GraphError::NotFound(id.into()))?;
        let own = kind_tag(&entity.kind);
        let mut clean: Vec<String> = Vec::new();
        for tag in tags.iter().filter_map(|t| normalize_tag(t)) {
            if tag != own && !clean.contains(&tag) {
                clean.push(tag);
            }
        }
        if clean.len() > MAX_TAGS {
            return Err(GraphError::InvalidInfo(format!("at most {MAX_TAGS} tags")));
        }
        let json = serde_json::to_string(&clean).map_err(|e| GraphError::InvalidInfo(e.to_string()))?;
        let now = now_ms();
        let conn = Connection::new(&self.db)?;
        let mut stmt =
            conn.prepare("MATCH (e:Entity {id: $id}) SET e.tags = $tags, e.updated_at = $now")?;
        conn.execute(
            &mut stmt,
            vec![("id", id.into()), ("tags", json.into()), ("now", now.into())],
        )?;
        entity.tags = clean;
        entity.updated_at = now;
        Ok(entity)
    }

    pub fn set_notes(&self, id: &str, notes: &str) -> Result<Entity, GraphError> {
        let mut entity = self.get(id)?.ok_or_else(|| GraphError::NotFound(id.into()))?;
        if notes.chars().count() > MAX_NOTES {
            return Err(GraphError::InvalidInfo("notes are too long".into()));
        }
        let now = now_ms();
        let conn = Connection::new(&self.db)?;
        let mut stmt =
            conn.prepare("MATCH (e:Entity {id: $id}) SET e.notes = $notes, e.updated_at = $now")?;
        conn.execute(
            &mut stmt,
            vec![("id", id.into()), ("notes", notes.into()), ("now", now.into())],
        )?;
        entity.notes = notes.to_string();
        entity.updated_at = now;
        Ok(entity)
    }

    /// Makes the page's LINKS_TO relationships match the [[wikilinks]] in its notes. Links to
    /// names that don't exist yet are skipped; they appear once that page is created.
    pub fn sync_wikilinks(&self, id: &str, names: &[String]) -> Result<(), GraphError> {
        let conn = Connection::new(&self.db)?;
        let mut stmt = conn.prepare(
            "MATCH (a:Entity {id: $id})-[r:Link {kind: 'LINKS_TO'}]->(:Entity) DELETE r",
        )?;
        conn.execute(&mut stmt, vec![("id", id.into())])?;
        for name in names {
            if let Some(target) = self.find_by_name(name)?.filter(|t| t.id != id) {
                self.link(id, "LINKS_TO", &target.id)?;
            }
        }
        Ok(())
    }

    /// Everything tagged `tag`, including entities whose kind implies it ("student").
    pub fn entities_with_tag(&self, tag: &str) -> Result<Vec<Entity>, GraphError> {
        let Some(tag) = normalize_tag(tag) else {
            return Ok(Vec::new());
        };
        Ok(self
            .all_entities()?
            .into_iter()
            .filter(|e| effective_tags(e).contains(&tag))
            .collect())
    }

    /// Every entity, by name.
    pub fn all_entities(&self) -> Result<Vec<Entity>, GraphError> {
        let conn = Connection::new(&self.db)?;
        let rows = conn.query(&format!(
            "MATCH (e:Entity) RETURN {} ORDER BY lower(e.name)",
            fields("e")
        ))?;
        Ok(rows.map(|row| entity_from(&row, 0)).collect())
    }

    /// Entities created at or after `ms` (milliseconds since the epoch), oldest first.
    pub fn created_since(&self, ms: i64) -> Result<Vec<Entity>, GraphError> {
        let conn = Connection::new(&self.db)?;
        let mut stmt = conn.prepare(&format!(
            "MATCH (e:Entity) WHERE e.created_at >= $since RETURN {} ORDER BY e.created_at",
            fields("e")
        ))?;
        let rows = conn.execute(&mut stmt, vec![("since", ms.into())])?;
        Ok(rows.map(|row| entity_from(&row, 0)).collect())
    }

    /// Stores a chat message and links it to the entities it was about.
    pub fn add_message(
        &self,
        role: &str,
        text: &str,
        focus: Option<&str>,
        mentions: &[String],
    ) -> Result<ChatRecord, GraphError> {
        let record = ChatRecord {
            id: uuid::Uuid::new_v4().to_string(),
            at: message_time(),
            role: role.into(),
            text: text.into(),
            focus: focus.map(String::from),
        };
        let conn = Connection::new(&self.db)?;
        let mut stmt = conn.prepare(
            "CREATE (:Message {id: $id, at: $at, role: $role, text: $text, focus: $focus})",
        )?;
        conn.execute(
            &mut stmt,
            vec![
                ("id", record.id.clone().into()),
                ("at", record.at.into()),
                ("role", role.into()),
                ("text", text.into()),
                ("focus", focus.unwrap_or_default().into()),
            ],
        )?;
        let mut mention = conn.prepare(
            "MATCH (m:Message {id: $message}), (e:Entity {id: $entity}) MERGE (m)-[:Mentions]->(e)",
        )?;
        for id in mentions {
            conn.execute(
                &mut mention,
                vec![("message", record.id.clone().into()), ("entity", id.clone().into())],
            )?;
        }
        Ok(record)
    }

    /// The latest messages of the main chat (not sent from a page), oldest first.
    pub fn recent_messages(&self, limit: usize) -> Result<Vec<ChatRecord>, GraphError> {
        let conn = Connection::new(&self.db)?;
        let rows = conn.query(&format!(
            "MATCH (m:Message) WHERE m.focus = '' \
             RETURN m.id, m.at, m.role, m.text, m.focus ORDER BY m.at DESC LIMIT {limit}"
        ))?;
        let mut records: Vec<_> = rows.map(|row| record_from(&row)).collect();
        records.reverse();
        Ok(records)
    }

    /// The latest messages about an entity, from any chat, oldest first.
    pub fn messages_about(&self, id: &str, limit: usize) -> Result<Vec<ChatRecord>, GraphError> {
        let conn = Connection::new(&self.db)?;
        let mut stmt = conn.prepare(&format!(
            "MATCH (m:Message)-[:Mentions]->(e:Entity {{id: $id}}) \
             RETURN m.id, m.at, m.role, m.text, m.focus ORDER BY m.at DESC LIMIT {limit}"
        ))?;
        let rows = conn.execute(&mut stmt, vec![("id", id.into())])?;
        let mut records: Vec<_> = rows.map(|row| record_from(&row)).collect();
        records.reverse();
        Ok(records)
    }

    /// Stores an activity unless one with its id exists; returns whether it was new.
    pub fn add_activity(&self, activity: &Activity) -> Result<bool, GraphError> {
        let conn = Connection::new(&self.db)?;
        let mut stmt = conn.prepare("MATCH (a:Activity {id: $id}) RETURN count(a)")?;
        let exists = matches!(conn.execute(&mut stmt, vec![("id", activity.id.clone().into())])?.next().as_deref(), Some([Value::Int64(n), ..]) if *n > 0);
        if exists {
            return Ok(false);
        }
        let related = serde_json::to_string(&activity.related).unwrap_or_else(|_| "[]".into());
        let mut stmt = conn.prepare(
            "CREATE (:Activity {id: $id, person: $person, source: $source, kind: $kind, title: $title, url: $url, \
             summary: $summary, published: $published, found_at: $found_at, baseline: $baseline, relevance: $relevance, \
             reason: $reason, related: $related, seen: $seen, notified: $notified})",
        )?;
        conn.execute(
            &mut stmt,
            vec![
                ("id", activity.id.clone().into()),
                ("person", activity.person.clone().into()),
                ("source", activity.source.clone().into()),
                ("kind", activity.kind.clone().into()),
                ("title", activity.title.clone().into()),
                ("url", activity.url.clone().into()),
                ("summary", activity.summary.clone().into()),
                ("published", activity.published.clone().into()),
                ("found_at", activity.found_at.into()),
                ("baseline", Value::Bool(activity.baseline)),
                ("relevance", activity.relevance.clone().into()),
                ("reason", activity.reason.clone().into()),
                ("related", related.into()),
                ("seen", Value::Bool(activity.seen || activity.baseline)),
                ("notified", Value::Bool(activity.notified || activity.baseline)),
            ],
        )?;
        Ok(true)
    }

    /// Activity, newest first.
    pub fn activities(&self, query: &ActivityQuery) -> Result<Vec<Activity>, GraphError> {
        let mut conditions = vec!["true".to_string()];
        let mut params: Vec<(&str, Value)> = Vec::new();
        if let Some(person) = &query.person {
            conditions.push("a.person = $person".into());
            params.push(("person", person.clone().into()));
        }
        if query.unseen_only {
            conditions.push("a.seen = false".into());
        }
        if query.relevant_only {
            conditions.push("a.relevance IN ['high', 'medium']".into());
        }
        if query.unjudged_only {
            conditions.push("a.baseline = false AND a.relevance = ''".into());
        }
        let limit = if query.limit == 0 { 200 } else { query.limit };
        let conn = Connection::new(&self.db)?;
        let mut stmt = conn.prepare(&format!(
            "MATCH (a:Activity) WHERE {} RETURN a.id, a.person, a.source, a.kind, a.title, a.url, a.summary, a.published, \
             a.found_at, a.baseline, a.relevance, a.reason, a.related, a.seen, a.notified \
             ORDER BY a.published DESC, a.found_at DESC LIMIT {limit}",
            conditions.join(" AND ")
        ))?;
        let rows = conn.execute(&mut stmt, params)?;
        let flag = |v: &Value| matches!(v, Value::Bool(true));
        Ok(rows
            .map(|row| Activity {
                id: string(&row[0]),
                person: string(&row[1]),
                source: string(&row[2]),
                kind: string(&row[3]),
                title: string(&row[4]),
                url: string(&row[5]),
                summary: string(&row[6]),
                published: string(&row[7]),
                found_at: match row[8] {
                    Value::Int64(n) => n,
                    _ => 0,
                },
                baseline: flag(&row[9]),
                relevance: string(&row[10]),
                reason: string(&row[11]),
                related: serde_json::from_str(&string(&row[12])).unwrap_or_default(),
                seen: flag(&row[13]),
                notified: flag(&row[14]),
            })
            .collect())
    }

    /// Records how relevant an activity is to the user's work, and why.
    /// Stores how relevant an activity is; false if there is no such activity.
    pub fn judge_activity(&self, id: &str, relevance: &str, reason: &str, related: &[String]) -> Result<bool, GraphError> {
        if !["high", "medium", "low", "none"].contains(&relevance) {
            return Err(GraphError::InvalidInfo(format!("relevance {relevance} must be high, medium, low or none")));
        }
        let conn = Connection::new(&self.db)?;
        let mut stmt = conn.prepare("MATCH (a:Activity {id: $id}) SET a.relevance = $relevance, a.reason = $reason, a.related = $related RETURN a.id")?;
        let mut rows = conn.execute(
            &mut stmt,
            vec![
                ("id", id.into()),
                ("relevance", relevance.into()),
                ("reason", reason.into()),
                ("related", serde_json::to_string(related).unwrap_or_else(|_| "[]".into()).into()),
            ],
        )?;
        Ok(rows.next().is_some())
    }

    /// Marks activities seen: these ids, or all of them.
    pub fn mark_activities_seen(&self, ids: Option<&[String]>) -> Result<(), GraphError> {
        let conn = Connection::new(&self.db)?;
        match ids {
            Some(ids) => {
                let mut stmt = conn.prepare("MATCH (a:Activity {id: $id}) SET a.seen = true")?;
                for id in ids {
                    conn.execute(&mut stmt, vec![("id", id.clone().into())])?;
                }
            }
            None => {
                conn.query("MATCH (a:Activity) WHERE a.seen = false SET a.seen = true")?;
            }
        }
        Ok(())
    }

    pub fn mark_activity_notified(&self, id: &str) -> Result<(), GraphError> {
        let conn = Connection::new(&self.db)?;
        let mut stmt = conn.prepare("MATCH (a:Activity {id: $id}) SET a.notified = true")?;
        conn.execute(&mut stmt, vec![("id", id.into())])?;
        Ok(())
    }

    /// What a watched source held when last checked, and when; None if never checked.
    pub fn source_state(&self, key: &str) -> Result<Option<(String, i64)>, GraphError> {
        let conn = Connection::new(&self.db)?;
        let mut stmt = conn.prepare("MATCH (s:SourceState {key: $key}) RETURN s.value, s.checked_at")?;
        let mut rows = conn.execute(&mut stmt, vec![("key", key.into())])?;
        Ok(rows.next().map(|row| (string(&row[0]), match row[1] { Value::Int64(n) => n, _ => 0 })))
    }

    /// Remembers what a source held when it was checked at `checked_at`.
    pub fn set_source_state(&self, key: &str, value: &str, checked_at: i64) -> Result<(), GraphError> {
        let conn = Connection::new(&self.db)?;
        let mut stmt = conn.prepare("MERGE (s:SourceState {key: $key}) SET s.value = $value, s.checked_at = $now")?;
        conn.execute(&mut stmt, vec![("key", key.into()), ("value", value.into()), ("now", checked_at.into())])?;
        Ok(())
    }

    /// All sections, pinned or dismissed, in sidebar order.
    pub fn sections(&self) -> Result<Vec<Section>, GraphError> {
        let conn = Connection::new(&self.db)?;
        let rows = conn.query("MATCH (s:Section) RETURN s.tag, s.title, s.pinned, s.icon ORDER BY s.position")?;
        Ok(rows
            .map(|row| Section {
                tag: string(&row[0]),
                title: string(&row[1]),
                pinned: matches!(row[2], Value::Bool(true)),
                icon: string(&row[3]),
            })
            .collect())
    }

    /// Sets or clears (None) a section's icon.
    pub fn set_section_icon(&self, tag: &str, icon: Option<&str>) -> Result<(), GraphError> {
        let tag = normalize_tag(tag).ok_or(GraphError::EmptyName)?;
        let icon = icon.map(check_icon).transpose()?.unwrap_or_default();
        let conn = Connection::new(&self.db)?;
        let mut stmt = conn.prepare("MATCH (s:Section {tag: $tag}) SET s.icon = $icon RETURN s.tag")?;
        let mut rows = conn.execute(&mut stmt, vec![("tag", tag.clone().into()), ("icon", icon.into())])?;
        rows.next().map(|_| ()).ok_or(GraphError::NotFound(tag))
    }

    /// Adds a section to the sidebar, at the end, or pins a dismissed one again.
    pub fn pin_section(&self, tag: &str, title: &str) -> Result<Section, GraphError> {
        self.put_section(tag, title, true)
    }

    /// Hides a section, or records a declined suggestion so it isn't made again.
    pub fn dismiss_section(&self, tag: &str) -> Result<Section, GraphError> {
        let title = self
            .sections()?
            .into_iter()
            .find(|s| Some(&s.tag) == normalize_tag(tag).as_ref())
            .map_or_else(|| section_title(tag), |s| s.title);
        self.put_section(tag, &title, false)
    }

    fn put_section(&self, tag: &str, title: &str, pinned: bool) -> Result<Section, GraphError> {
        let tag = normalize_tag(tag).ok_or(GraphError::EmptyName)?;
        let title = normalize(title);
        if title.is_empty() {
            return Err(GraphError::EmptyName);
        }
        let conn = Connection::new(&self.db)?;
        let mut stmt = conn.prepare(
            "MERGE (s:Section {tag: $tag}) \
             ON CREATE SET s.position = $position \
             SET s.title = $title, s.pinned = $pinned",
        )?;
        conn.execute(
            &mut stmt,
            vec![
                ("tag", tag.clone().into()),
                ("title", title.clone().into()),
                ("pinned", Value::Bool(pinned)),
                ("position", now_ms().into()),
            ],
        )?;
        let icon = self.sections()?.into_iter().find(|s| s.tag == tag).map(|s| s.icon).unwrap_or_default();
        Ok(Section { tag, title, pinned, icon })
    }

    /// The entity with exactly this name (ignoring case and spacing), if any.
    pub fn find_by_name(&self, name: &str) -> Result<Option<Entity>, GraphError> {
        let conn = Connection::new(&self.db)?;
        let mut stmt = conn.prepare(
            &format!(
                "MATCH (e:Entity) WHERE lower(e.name) = $name OR lower(e.aliases) CONTAINS $quoted \
                 RETURN {}, lower(e.name) = $name AS exact ORDER BY exact DESC, e.created_at LIMIT 1",
                fields("e")
            ),
        )?;
        let name = normalize(name).to_lowercase();
        let quoted = serde_json::to_string(&name).unwrap_or_default();
        let mut rows = conn.execute(&mut stmt, vec![("name", name.into()), ("quoted", quoted.into())])?;
        Ok(rows.next().map(|row| entity_from(&row, 0)))
    }

    /// Replaces a page's other names. An alias already used as another page's name or alias is
    /// refused, so a name always finds one page.
    pub fn set_aliases(&self, id: &str, aliases: &[String]) -> Result<Entity, GraphError> {
        let mut entity = self.get(id)?.ok_or_else(|| GraphError::NotFound(id.into()))?;
        let mut clean: Vec<String> = Vec::new();
        for alias in aliases.iter().map(|a| normalize(a)).filter(|a| !a.is_empty()) {
            if alias.eq_ignore_ascii_case(&entity.name) || clean.iter().any(|c| c.eq_ignore_ascii_case(&alias)) {
                continue;
            }
            if let Some(other) = self.find_by_name(&alias)?.filter(|e| e.id != id) {
                return Err(GraphError::InvalidName(format!("{alias} already means {}", other.name)));
            }
            clean.push(alias);
        }
        if clean.len() > MAX_ALIASES {
            return Err(GraphError::InvalidInfo(format!("at most {MAX_ALIASES} other names")));
        }
        let json = serde_json::to_string(&clean).map_err(|e| GraphError::InvalidInfo(e.to_string()))?;
        let now = now_ms();
        let conn = Connection::new(&self.db)?;
        let mut stmt = conn.prepare("MATCH (e:Entity {id: $id}) SET e.aliases = $aliases, e.updated_at = $now")?;
        conn.execute(&mut stmt, vec![("id", id.into()), ("aliases", json.into()), ("now", now.into())])?;
        entity.aliases = clean;
        entity.updated_at = now;
        Ok(entity)
    }

    /// Links two entities and sets the relationship's details.
    pub fn link_with(&self, from_id: &str, kind: &str, to_id: &str, details: &LinkDetails) -> Result<(), GraphError> {
        for date in [&details.since, &details.until].into_iter().flatten() {
            if !date.is_empty() && !valid_link_date(date) {
                return Err(GraphError::InvalidInfo(format!("date \"{date}\" must be YYYY, YYYY-MM or YYYY-MM-DD")));
            }
        }
        self.link(from_id, kind, to_id)?;
        let conn = Connection::new(&self.db)?;
        let mut stmt = conn.prepare(
            "MATCH (a:Entity {id: $from})-[r:Link {kind: $kind}]->(b:Entity {id: $to}) SET \
               r.detail = CASE WHEN $set_detail THEN (CASE WHEN $detail = '' THEN NULL ELSE $detail END) ELSE r.detail END, \
               r.since = CASE WHEN $set_since THEN (CASE WHEN $since = '' THEN NULL ELSE $since END) ELSE r.since END, \
               r.until = CASE WHEN $set_until THEN (CASE WHEN $until = '' THEN NULL ELSE $until END) ELSE r.until END",
        )?;
        let field = |v: &Option<String>| (Value::Bool(v.is_some()), v.as_deref().map(str::trim).unwrap_or_default().to_string());
        let (set_detail, detail) = field(&details.detail);
        let (set_since, since) = field(&details.since);
        let (set_until, until) = field(&details.until);
        conn.execute(
            &mut stmt,
            vec![
                ("from", from_id.into()),
                ("kind", kind.into()),
                ("to", to_id.into()),
                ("set_detail", set_detail),
                ("detail", detail.into()),
                ("set_since", set_since),
                ("since", since.into()),
                ("set_until", set_until),
                ("until", until.into()),
            ],
        )?;
        if kind == "AFFILIATED_WITH" {
            self.sync_affiliation(from_id)?;
        }
        Ok(())
    }

    /// Links a person to an organization by name or other name, creating the organization if
    /// there is none. Refuses a name that belongs to another kind of page.
    pub fn affiliate(&self, person_id: &str, kind: &str, organization: &str, details: &LinkDetails) -> Result<Entity, GraphError> {
        let organization = match self.find_by_name(organization)? {
            Some(org) if org.kind == "Organization" => org,
            Some(other) => {
                return Err(GraphError::InvalidName(format!("{} is a {}, not an organization", other.name, other.kind)))
            }
            None => self.upsert_entity("Organization", organization)?,
        };
        self.link_with(person_id, kind, &organization.id, details)?;
        Ok(organization)
    }

    /// Everyone linked to an organization or any part of it (its departments and their labs),
    /// found with one recursive query. Past links are left out unless `include_past`.
    pub fn affiliations_at(&self, organization_id: &str, include_past: bool) -> Result<Vec<Affiliation>, GraphError> {
        let conn = Connection::new(&self.db)?;
        let mut stmt = conn.prepare(&format!(
            "MATCH (p:Entity)-[a:Link]->(o:Entity)-[:Link*0..{MAX_PART_DEPTH} (r, n | WHERE r.kind = 'PART_OF')]->(top:Entity {{id: $id}}) \
             WHERE a.kind IN ['AFFILIATED_WITH', 'STUDIED_AT'] AND p.kind = 'Person' \
             RETURN DISTINCT a.kind, a.detail, a.since, a.until, {}, {} ORDER BY lower(p.name)",
            fields("p"),
            fields("o")
        ))?;
        let width = fields("p").split(", ").count();
        let rows = conn.execute(&mut stmt, vec![("id", organization_id.into())])?;
        let affiliations: Vec<Affiliation> = rows
            .map(|row| Affiliation {
                kind: string(&row[0]),
                detail: optional(&row[1]),
                since: optional(&row[2]),
                until: optional(&row[3]),
                person: entity_from(&row, 4),
                organization: entity_from(&row, 4 + width),
            })
            .collect();
        Ok(affiliations.into_iter().filter(|a| include_past || a.current()).collect())
    }

    /// Keeps a person's `affiliation` detail, shown under their name, in step with where they
    /// currently work: their current AFFILIATED_WITH organizations, or nothing.
    pub fn sync_affiliation(&self, person_id: &str) -> Result<(), GraphError> {
        let names: Vec<String> = self
            .links(person_id)?
            .into_iter()
            .filter(|l| l.outgoing && l.kind == "AFFILIATED_WITH" && is_current(l.until.as_deref()))
            .map(|l| l.other.name)
            .collect();
        let value = (!names.is_empty()).then(|| names.join(", "));
        let person = self.get(person_id)?.ok_or_else(|| GraphError::NotFound(person_id.into()))?;
        if person.info.get("affiliation") != value.as_ref() {
            self.update_info(person_id, &BTreeMap::from([("affiliation".to_string(), value)]))?;
        }
        Ok(())
    }

    /// People whose affiliation is only text, with the organization that text already names.
    pub fn unlinked_affiliations(&self) -> Result<Vec<(Entity, String, Option<Entity>)>, GraphError> {
        let mut found = Vec::new();
        for person in self.entities_of_kind("Person")? {
            let Some(text) = person.info.get("affiliation").cloned() else { continue };
            let linked = self.links(&person.id)?.iter().any(|l| l.outgoing && l.kind == "AFFILIATED_WITH");
            if linked {
                continue;
            }
            let organization = self.find_by_name(&text)?.filter(|e| e.kind == "Organization");
            found.push((person, text, organization));
        }
        Ok(found)
    }

    /// Most recently created entities first.
    pub fn recent_entities(&self, limit: usize) -> Result<Vec<Entity>, GraphError> {
        let conn = Connection::new(&self.db)?;
        let rows = conn.query(&format!(
            "MATCH (e:Entity) RETURN {} ORDER BY e.created_at DESC LIMIT {limit}",
            fields("e")
        ))?;
        Ok(rows.map(|row| entity_from(&row, 0)).collect())
    }

    /// Renames an entity, keeping its id. Fails if another entity already has the name.
    pub fn rename(&self, id: &str, name: &str) -> Result<Entity, GraphError> {
        let name = normalize(name);
        if name.is_empty() {
            return Err(GraphError::EmptyName);
        }
        if let Some(other) = self.find_by_name(&name)?.filter(|e| e.id != id) {
            return Err(GraphError::InvalidName(format!("{} already exists", other.name)));
        }
        let conn = Connection::new(&self.db)?;
        let mut stmt =
            conn.prepare("MATCH (e:Entity {id: $id}) SET e.name = $name, e.updated_at = $now")?;
        conn.execute(
            &mut stmt,
            vec![("id", id.into()), ("name", name.into()), ("now", now_ms().into())],
        )?;
        self.resync_affiliated(id)?;
        self.get(id)?.ok_or_else(|| GraphError::NotFound(id.into()))
    }

    /// Updates the affiliation shown for everyone linked to an organization that changed.
    fn resync_affiliated(&self, organization_id: &str) -> Result<(), GraphError> {
        for link in self.links(organization_id)? {
            if !link.outgoing && link.kind == "AFFILIATED_WITH" {
                self.sync_affiliation(&link.other.id)?;
            }
        }
        Ok(())
    }

    /// Changes an entity's kind, keeping its id, details and relationships.
    pub fn change_kind(&self, id: &str, kind: &str) -> Result<Entity, GraphError> {
        let (kind, role) = resolve_kind(kind).ok_or_else(|| GraphError::UnknownEntityKind(kind.into()))?;
        let kind = kind.as_str();
        self.set_kind(id, kind)?;
        if let Some(role) = role {
            return self.add_tags(id, &[role.to_string()]);
        }
        self.get(id)?.ok_or_else(|| GraphError::NotFound(id.into()))
    }

    /// Deletes an entity with its relationships. Messages that mentioned it are kept.
    pub fn delete(&self, id: &str) -> Result<(), GraphError> {
        let affiliated: Vec<String> = self
            .links(id)?
            .into_iter()
            .filter(|l| !l.outgoing && l.kind == "AFFILIATED_WITH")
            .map(|l| l.other.id)
            .collect();
        let conn = Connection::new(&self.db)?;
        let mut stmt = conn.prepare("MATCH (e:Entity {id: $id}) DETACH DELETE e")?;
        conn.execute(&mut stmt, vec![("id", id.into())])?;
        for person in affiliated {
            self.sync_affiliation(&person)?;
        }
        Ok(())
    }

    fn set_kind(&self, id: &str, kind: &str) -> Result<(), GraphError> {
        let conn = Connection::new(&self.db)?;
        let mut stmt =
            conn.prepare("MATCH (e:Entity {id: $id}) SET e.kind = $kind, e.updated_at = $now")?;
        conn.execute(
            &mut stmt,
            vec![("id", id.into()), ("kind", kind.into()), ("now", now_ms().into())],
        )?;
        Ok(())
    }

    /// Changes whenever an entity or relationship is added, changed or removed:
    /// (entities + relationships, latest change time, a sum over relationship times).
    pub fn fingerprint(&self) -> Result<(usize, i64, usize), GraphError> {
        let conn = Connection::new(&self.db)?;
        let int = |v: &Value| match v {
            Value::Int64(n) => *n,
            _ => 0,
        };
        let mut entities = conn.query(
            "MATCH (e:Entity) RETURN count(e), max(e.updated_at), max(e.created_at)",
        )?;
        let row = entities.next().unwrap_or_default();
        let (count, updated, created) = (int(&row[0]), int(&row[1]), int(&row[2]));
        let mut links = conn.query("MATCH ()-[r:Link]->() RETURN count(r), sum(r.created_at % 1000003)")?;
        let row = links.next().unwrap_or_default();
        Ok(((count + int(&row[0])) as usize, updated.max(created), int(&row[1]) as usize))
    }

    /// Removes a relationship; returns whether one existed.
    pub fn unlink(&self, from_id: &str, kind: &str, to_id: &str) -> Result<bool, GraphError> {
        let conn = Connection::new(&self.db)?;
        let mut stmt = conn.prepare(
            "MATCH (a:Entity {id: $from})-[r:Link {kind: $kind}]->(b:Entity {id: $to}) \
             DELETE r RETURN count(*)",
        )?;
        let mut rows = conn.execute(
            &mut stmt,
            vec![("from", from_id.into()), ("kind", kind.into()), ("to", to_id.into())],
        )?;
        let removed = matches!(rows.next().as_deref(), Some([Value::Int64(n), ..]) if *n > 0);
        if removed && kind == "AFFILIATED_WITH" {
            self.sync_affiliation(from_id)?;
        }
        Ok(removed)
    }

    /// All relationships touching an entity, outgoing first, each group oldest first.
    pub fn links(&self, id: &str) -> Result<Vec<Link>, GraphError> {
        let conn = Connection::new(&self.db)?;
        let mut links = Vec::new();
        for (outgoing, pattern) in [(true, "-[r:Link]->"), (false, "<-[r:Link]-")] {
            let mut stmt = conn.prepare(&format!(
                "MATCH (a:Entity {{id: $id}}){pattern}(b:Entity) \
                 RETURN r.kind, r.detail, r.since, r.until, {} ORDER BY r.created_at",
                fields("b")
            ))?;
            for row in conn.execute(&mut stmt, vec![("id", id.into())])? {
                links.push(Link {
                    kind: string(&row[0]),
                    outgoing,
                    other: entity_from(&row, 4),
                    detail: optional(&row[1]),
                    since: optional(&row[2]),
                    until: optional(&row[3]),
                });
            }
        }
        Ok(links)
    }
}

/// Collapses runs of whitespace and trims, so "  Compiler   Fuzzing " == "Compiler Fuzzing".
pub fn normalize(name: &str) -> String {
    name.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn entity_id(kind: &str, name: &str) -> String {
    format!("{}:{}", kind.to_lowercase(), name.to_lowercase())
}

/// The entity columns every query returns, in `entity_from` order.
fn fields(var: &str) -> String {
    ["id", "kind", "name", "info", "tags", "notes", "updated_at", "status", "priority", "area", "due_date", "due_time", "aliases"]
        .map(|f| format!("{var}.{f}"))
        .join(", ")
}

fn entity_from(row: &[Value], at: usize) -> Entity {
    // Columns added in later releases read as NULL on old rows, which parse to empty values.
    let mut info: BTreeMap<String, String> = serde_json::from_str(&string(&row[at + 3])).unwrap_or_default();
    for (key, index) in [("status", 7), ("priority", 8), ("area", 9)] {
        let value = string(&row[at + index]);
        if !value.is_empty() {
            info.insert(key.into(), value);
        }
    }
    if let Value::Date(date) = &row[at + 10] {
        let time = string(&row[at + 11]);
        let due = if time.is_empty() { date.to_string() } else { format!("{date}T{time}") };
        info.insert("due".into(), due);
    }
    Entity {
        id: string(&row[at]),
        kind: string(&row[at + 1]),
        name: string(&row[at + 2]),
        info,
        tags: serde_json::from_str(&string(&row[at + 4])).unwrap_or_default(),
        notes: string(&row[at + 5]),
        updated_at: match row[at + 6] {
            Value::Int64(n) => n,
            _ => 0,
        },
        aliases: serde_json::from_str(&string(&row[at + 12])).unwrap_or_default(),
    }
}

/// A text value, or None for NULL or empty.
fn optional(value: &Value) -> Option<String> {
    Some(string(value)).filter(|s| !s.is_empty())
}

fn record_from(row: &[Value]) -> ChatRecord {
    let focus = string(&row[4]);
    ChatRecord {
        id: string(&row[0]),
        at: match row[1] {
            Value::Int64(n) => n,
            _ => 0,
        },
        role: string(&row[2]),
        text: string(&row[3]),
        focus: (!focus.is_empty()).then_some(focus),
    }
}

/// "#PhD Students" -> "phd-students"; None when nothing usable is left.
pub fn normalize_tag(tag: &str) -> Option<String> {
    let words: Vec<String> = tag
        .trim()
        .trim_start_matches('#')
        .to_lowercase()
        .split(|c: char| c.is_whitespace() || c == '_' || c == '-')
        .map(|w| w.chars().filter(|c| c.is_alphanumeric()).collect::<String>())
        .filter(|w| !w.is_empty())
        .collect();
    let tag: String = words.join("-").chars().take(40).collect();
    (!tag.is_empty()).then_some(tag)
}

/// The tag every entity of a kind has: "Student" -> "student", "ResearchArea" -> "research-area".
pub fn kind_tag(kind: &str) -> String {
    let mut tag = String::new();
    for (i, c) in kind.chars().enumerate() {
        if c.is_uppercase() && i > 0 {
            tag.push('-');
        }
        tag.extend(c.to_lowercase());
    }
    tag
}

/// The kind's tag followed by the entity's own tags.
pub fn effective_tags(entity: &Entity) -> Vec<String> {
    let mut tags = vec![kind_tag(&entity.kind)];
    tags.extend(entity.tags.iter().cloned());
    tags
}

/// A sidebar title for a tag: "student" -> "Students", "person" -> "People",
/// "nba-accreditation" -> "Nba accreditation".
pub fn section_title(tag: &str) -> String {
    let tag = normalize_tag(tag).unwrap_or_default();
    let known = match tag.as_str() {
        "person" => Some("People"),
        "research-area" => Some("Research areas"),
        "student" => Some("Students"),
        "collaborator" => Some("Collaborators"),
        "colleague" => Some("Colleagues"),
        "faculty" => Some("Faculty"),
        "staff" => Some("Staff"),
        "alumni" => Some("Alumni"),
        "project" => Some("Projects"),
        "course" => Some("Courses"),
        "idea" => Some("Ideas"),
        "task" => Some("Tasks"),
        "event" => Some("Events"),
        "document" => Some("Documents"),
        "note" => Some("Notes"),
        "organization" => Some("Organizations"),
        _ => None,
    };
    if let Some(title) = known {
        return title.into();
    }
    let text = tag.replace('-', " ");
    let mut chars = text.chars();
    chars.next().map(|c| c.to_uppercase().chain(chars).collect()).unwrap_or_default()
}

fn string(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        _ => String::new(),
    }
}

#[cfg(test)]
pub fn tests_now() -> i64 {
    now_ms()
}

/// Strictly increasing message times, so messages stored in the same millisecond keep their order.
fn message_time() -> i64 {
    static LAST: AtomicI64 = AtomicI64::new(0);
    let now = now_ms();
    LAST.fetch_update(Ordering::SeqCst, Ordering::SeqCst, |last| Some(now.max(last + 1)))
        .map_or(now, |last| now.max(last + 1))
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upsert_is_idempotent_and_keeps_first_spelling() {
        let g = Graph::in_memory().unwrap();
        let a = g.upsert_entity("Project", "Compiler Fuzzing").unwrap();
        let b = g.upsert_entity("Project", "  compiler   FUZZING ").unwrap();
        assert_eq!(a, b);
        assert_eq!(a.name, "Compiler Fuzzing");
        assert_eq!(g.entities_of_kind("Project").unwrap().len(), 1);
    }

    #[test]
    fn one_entity_per_name_across_kinds() {
        let g = Graph::in_memory().unwrap();
        let person = g.upsert_entity("Person", "Amit").unwrap();
        // Saying Amit is a student adds the role to the same person.
        let student = g.upsert_entity("Student", "amit").unwrap();
        assert_eq!(student.id, person.id);
        assert_eq!((student.kind.as_str(), student.tags.clone()), ("Person", vec!["student".to_string()]));
        assert_eq!(g.entities_of_kind("Student").unwrap(), vec![student.clone()]);
        assert_eq!(g.entities_of_kind("Person").unwrap(), vec![student]);

        let project = g.upsert_entity("Project", "Compiler Fuzzing").unwrap();
        assert_eq!(g.upsert_entity("Idea", "compiler fuzzing").unwrap(), project);
        assert_eq!(g.find_by_name(" COMPILER  fuzzing").unwrap(), Some(project.clone()));
        assert_eq!(g.find_by_name("nobody").unwrap(), None);
        assert_eq!(g.recent_entities(1).unwrap(), vec![project]);
    }

    #[test]
    fn new_kind_and_relationship_names_are_recognized() {
        for name in ["Grant", "Paper", "Reading Group", "ResearchArea"] {
            assert!(is_kind_name(name), "{name}");
        }
        for name in ["grant", "G", "Grant!", "WORKS_ON", "Grant 2"] {
            assert!(!is_kind_name(name), "{name}");
        }
        for name in ["REVIEWS", "IS_EXAMINER_FOR", "FUNDS"] {
            assert!(is_relation_name(name), "{name}");
        }
        for name in ["reviews", "Reviews", "LINKS_TO", "MENTIONED_IN", "IS-FOR", "AB"] {
            assert!(!is_relation_name(name), "{name}");
        }
    }

    #[test]
    fn rejects_invalid_input() {
        let g = Graph::in_memory().unwrap();
        // A new kind of page is allowed; something that isn't a kind name isn't.
        assert_eq!(g.upsert_entity("Grant", "SERB CRG").unwrap().kind, "Grant");
        assert!(matches!(g.upsert_entity("robot!", "R2"), Err(GraphError::UnknownEntityKind(_))));
        assert!(matches!(g.upsert_entity("a", "R2"), Err(GraphError::UnknownEntityKind(_))));
        assert!(matches!(g.upsert_entity("Idea", "   "), Err(GraphError::EmptyName)));
        let a = g.upsert_entity("Student", "Rahul").unwrap();
        // A new relationship is allowed; a name that isn't one isn't.
        let b = g.upsert_entity("Note", "Other").unwrap();
        assert!(g.link(&a.id, "REVIEWS", &b.id).is_ok());
        assert!(matches!(g.link(&a.id, "likes", &b.id), Err(GraphError::UnknownRelationKind(_))));
        assert!(matches!(g.link(&a.id, "LINKS_TO ", &b.id), Err(GraphError::UnknownRelationKind(_))));
        assert!(matches!(g.link(&a.id, "WORKS_ON", "project:missing"), Err(GraphError::NotFound(_))));
    }

    #[test]
    fn readme_example_graph() {
        let g = Graph::in_memory().unwrap();
        let rahul = g.upsert_entity("Student", "Rahul").unwrap();
        let fuzzing = g.upsert_entity("Project", "Compiler Fuzzing").unwrap();
        let llvm = g.upsert_entity("ResearchArea", "LLVM").unwrap();
        let sharma = g.upsert_entity("Person", "Prof Sharma").unwrap();

        g.link(&rahul.id, "WORKS_ON", &fuzzing.id).unwrap();
        g.link(&rahul.id, "WORKS_ON", &fuzzing.id).unwrap(); // duplicate is merged
        g.link(&fuzzing.id, "RELATED_TO", &llvm.id).unwrap();
        g.link(&fuzzing.id, "COLLABORATES_WITH", &sharma.id).unwrap();

        let links = g.links(&fuzzing.id).unwrap();
        let summary: Vec<_> = links
            .iter()
            .map(|l| (l.kind.as_str(), l.outgoing, l.other.name.as_str()))
            .collect();
        assert_eq!(
            summary,
            vec![
                ("RELATED_TO", true, "LLVM"),
                ("COLLABORATES_WITH", true, "Prof Sharma"),
                ("WORKS_ON", false, "Rahul"),
            ]
        );
        assert_eq!(g.links(&rahul.id).unwrap().len(), 1);
    }

    #[test]
    fn entities_of_kind_filters_and_sorts() {
        let g = Graph::in_memory().unwrap();
        g.upsert_entity("Idea", "zero-shot repair").unwrap();
        g.upsert_entity("Idea", "LLM triage").unwrap();
        g.upsert_entity("Student", "Rahul").unwrap();
        let names: Vec<_> = g
            .entities_of_kind("Idea")
            .unwrap()
            .into_iter()
            .map(|e| e.name)
            .collect();
        assert_eq!(names, vec!["LLM triage", "zero-shot repair"]);
    }

    #[test]
    fn info_merges_removes_and_validates() {
        let g = Graph::in_memory().unwrap();
        let amit = g.upsert_entity("Student", "Amit").unwrap();
        assert!(amit.info.is_empty());
        let set = |pairs: &[(&str, Option<&str>)]| -> BTreeMap<String, Option<String>> {
            pairs.iter().map(|(k, v)| (k.to_string(), v.map(String::from))).collect()
        };
        g.update_info(&amit.id, &set(&[("email", Some("amit@example.edu")), ("program", Some("PhD"))]))
            .unwrap();
        let updated = g
            .update_info(&amit.id, &set(&[("program", None), ("year", Some(" 2 "))]))
            .unwrap();
        let expected: BTreeMap<_, _> =
            [("email".to_string(), "amit@example.edu".to_string()), ("year".into(), "2".into())].into();
        assert_eq!(updated.info, expected);
        // Stored, and returned by every query.
        assert_eq!(g.find_by_name("amit").unwrap().unwrap().info, expected);
        assert_eq!(g.entities_of_kind("Student").unwrap()[0].info, expected);

        for bad in ["Email", "e-mail", "", "1st"] {
            assert!(matches!(
                g.update_info(&amit.id, &set(&[(bad, Some("x"))])),
                Err(GraphError::InvalidInfo(_))
            ));
        }
        assert!(matches!(
            g.update_info("student:ghost", &set(&[("email", Some("x"))])),
            Err(GraphError::NotFound(_))
        ));
    }

    #[test]
    fn unlink_removes_only_that_relationship() {
        let g = Graph::in_memory().unwrap();
        let rahul = g.upsert_entity("Student", "Rahul").unwrap();
        let p = g.upsert_entity("Project", "Fuzzing").unwrap();
        g.link(&rahul.id, "WORKS_ON", &p.id).unwrap();
        g.link(&p.id, "COLLABORATES_WITH", &rahul.id).unwrap();
        assert!(g.unlink(&rahul.id, "WORKS_ON", &p.id).unwrap());
        assert!(!g.unlink(&rahul.id, "WORKS_ON", &p.id).unwrap());
        let links = g.links(&rahul.id).unwrap();
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].kind, "COLLABORATES_WITH");
    }

    fn set(g: &Graph, id: &str, pairs: &[(&str, &str)]) -> Result<Entity, GraphError> {
        let map: BTreeMap<String, Option<String>> =
            pairs.iter().map(|(k, v)| (k.to_string(), Some(v.to_string()))).collect();
        g.update_info(id, &map)
    }

    fn details(detail: Option<&str>, since: Option<&str>, until: Option<&str>) -> LinkDetails {
        LinkDetails { detail: detail.map(String::from), since: since.map(String::from), until: until.map(String::from) }
    }

    fn activity(id: &str, person: &str, published: &str, baseline: bool) -> Activity {
        Activity {
            id: id.into(),
            person: person.into(),
            source: "openalex".into(),
            kind: "paper".into(),
            title: format!("Paper {id}"),
            url: format!("https://doi.org/{id}"),
            summary: String::new(),
            published: published.into(),
            found_at: now_ms(),
            baseline,
            relevance: String::new(),
            reason: String::new(),
            related: Vec::new(),
            seen: false,
            notified: false,
        }
    }

    #[test]
    fn pages_and_sections_have_emoji_icons() {
        let g = Graph::in_memory().unwrap();
        let p = g.upsert_entity("Project", "Fuzzing").unwrap();
        let set = |icon: &str| g.update_info(&p.id, &BTreeMap::from([(ICON_KEY.to_string(), Some(icon.to_string()))]));
        for icon in ["🐛", "🧑‍🎓", "⛓️", "🏳️‍🌈", "1️⃣"] {
            assert_eq!(set(icon).unwrap().info[ICON_KEY], icon);
        }
        for bad in ["bug", "🐛 fuzz", "LiFolder", "a very long text"] {
            assert!(set(bad).is_err(), "{bad}");
        }
        assert!(!g.update_info(&p.id, &BTreeMap::from([(ICON_KEY.to_string(), None)])).unwrap().info.contains_key(ICON_KEY));

        g.pin_section("project", "Projects").unwrap();
        assert!(g.set_section_icon("unknown", Some("🧪")).is_err());
        g.set_section_icon("project", Some("🚀")).unwrap();
        assert!(g.set_section_icon("project", Some("rocket")).is_err());
        // Renaming or unpinning keeps the icon.
        assert_eq!(g.pin_section("project", "Research projects").unwrap().icon, "🚀");
        g.dismiss_section("project").unwrap();
        assert_eq!(g.sections().unwrap()[0].icon, "🚀");
        g.set_section_icon("project", None).unwrap();
        assert_eq!(g.sections().unwrap()[0].icon, "");
    }

    #[test]
    fn projects_are_planned_in_progress_or_completed() {
        let g = Graph::in_memory().unwrap();
        for (given, stored) in [("Planned", "planned"), ("In progress", "in-progress"), ("in_progress", "in-progress"), ("active", "in-progress"), ("Done", "completed"), ("completed", "completed")] {
            assert_eq!(project_status(given), Some(stored), "{given}");
        }
        assert_eq!(project_status("someday"), None);

        let p = g.upsert_entity("Project", "Fuzzing").unwrap();
        let set = |value: &str| g.update_info(&p.id, &BTreeMap::from([("status".to_string(), Some(value.to_string()))]));
        assert_eq!(set("In Progress").unwrap().info["status"], "in-progress");
        let err = set("someday").unwrap_err().to_string();
        assert!(err.contains("planned, in progress or completed"), "{err}");
        assert_eq!(g.get(&p.id).unwrap().unwrap().info["status"], "in-progress", "unchanged after a bad value");
        // Other pages keep their own statuses.
        let t = g.upsert_entity("Task", "Review").unwrap();
        assert_eq!(g.update_info(&t.id, &BTreeMap::from([("status".to_string(), Some("waiting".to_string()))])).unwrap().info["status"], "waiting");

        // Saved before statuses were fixed.
        let old = g.upsert_entity("Project", "Old grant").unwrap();
        let odd = g.upsert_entity("Project", "Side thing").unwrap();
        g.write_info(&old.id, &BTreeMap::from([("status".to_string(), "active".to_string())]), 1).unwrap();
        g.write_info(&odd.id, &BTreeMap::from([("status".to_string(), "someday maybe".to_string())]), 1).unwrap();
        g.migrate_project_statuses().unwrap();
        assert_eq!(g.get(&old.id).unwrap().unwrap().info["status"], "in-progress");
        let odd = g.get(&odd.id).unwrap().unwrap();
        assert_eq!((odd.info.get("status"), odd.info["status_text"].as_str()), (None, "someday maybe"));
    }

    #[test]
    fn activity_is_stored_once_judged_and_marked() {
        let g = Graph::in_memory().unwrap();
        assert!(g.add_activity(&activity("w1", "person:kavya", "2026-09-01", true)).unwrap());
        assert!(g.add_activity(&activity("w2", "person:kavya", "2026-09-10", false)).unwrap());
        assert!(g.add_activity(&activity("w3", "person:arun", "2026-09-12", false)).unwrap());
        assert!(!g.add_activity(&activity("w2", "person:kavya", "2026-09-10", false)).unwrap(), "same id is not stored twice");

        let ids = |q: ActivityQuery| -> Vec<String> { g.activities(&q).unwrap().into_iter().map(|a| a.id).collect() };
        assert_eq!(ids(ActivityQuery::default()), vec!["w3", "w2", "w1"]);
        assert_eq!(ids(ActivityQuery { person: Some("person:kavya".into()), ..Default::default() }), vec!["w2", "w1"]);
        // Baseline items are seen already and never judged.
        assert_eq!(ids(ActivityQuery { unseen_only: true, ..Default::default() }), vec!["w3", "w2"]);
        assert_eq!(ids(ActivityQuery { unjudged_only: true, ..Default::default() }), vec!["w3", "w2"]);

        g.judge_activity("w2", "high", "Fuzzes Solidity compilers, like your project", &["Solidity Compiler Fuzzing".into()]).unwrap();
        g.judge_activity("w3", "none", "Unrelated", &[]).unwrap();
        assert!(g.judge_activity("w3", "very", "", &[]).is_err());
        assert!(!g.judge_activity("gone", "high", "", &[]).unwrap());
        assert_eq!(ids(ActivityQuery { relevant_only: true, ..Default::default() }), vec!["w2"]);
        assert!(ids(ActivityQuery { unjudged_only: true, ..Default::default() }).is_empty());
        let w2 = g.activities(&ActivityQuery { relevant_only: true, ..Default::default() }).unwrap().remove(0);
        assert_eq!((w2.related, w2.notified), (vec!["Solidity Compiler Fuzzing".to_string()], false));

        g.mark_activity_notified("w2").unwrap();
        g.mark_activities_seen(Some(&["w2".into()])).unwrap();
        assert_eq!(ids(ActivityQuery { unseen_only: true, ..Default::default() }), vec!["w3"]);
        g.mark_activities_seen(None).unwrap();
        assert!(ids(ActivityQuery { unseen_only: true, ..Default::default() }).is_empty());

        assert_eq!(g.source_state("homepage:person:kavya").unwrap(), None);
        g.set_source_state("homepage:person:kavya", "Welcome", 42).unwrap();
        let (value, checked_at) = g.source_state("homepage:person:kavya").unwrap().unwrap();
        assert_eq!(value, "Welcome");
        assert_eq!(checked_at, 42);
    }

    #[test]
    fn aliases_find_one_page_by_any_name() {
        let g = Graph::in_memory().unwrap();
        let iitg = g.upsert_entity("Organization", "IIT Guwahati").unwrap();
        let with = g.set_aliases(&iitg.id, &["IITG".into(), " Indian Institute of Technology  Guwahati".into(), "iitg".into(), "IIT Guwahati".into()]).unwrap();
        assert_eq!(with.aliases, vec!["IITG", "Indian Institute of Technology Guwahati"]);
        assert_eq!(g.find_by_name("iitg").unwrap().unwrap().id, iitg.id);
        assert_eq!(g.find_by_name("indian institute of technology guwahati").unwrap().unwrap().id, iitg.id);
        // An alias is a whole name: part of one doesn't match.
        assert!(g.find_by_name("IIT").unwrap().is_none());
        // Saying "IITG" again finds the same page rather than making one.
        assert_eq!(g.upsert_entity("Organization", "IITG").unwrap().id, iitg.id);
        g.upsert_entity("Organization", "IISc").unwrap();
        assert!(matches!(g.set_aliases("organization:iisc", &["IITG".into()]), Err(GraphError::InvalidName(_))));
        assert!(matches!(g.rename("organization:iisc", "iitg"), Err(GraphError::InvalidName(_))));
        // An exact name wins over someone else's alias.
        let person = g.upsert_entity("Person", "Kavya").unwrap();
        assert_eq!(g.find_by_name("Kavya").unwrap().unwrap().id, person.id);
    }

    #[test]
    fn affiliations_carry_details_and_roll_up_through_departments() {
        let g = Graph::in_memory().unwrap();
        let iitg = g.upsert_entity("Organization", "IIT Guwahati").unwrap();
        g.set_aliases(&iitg.id, &["IITG".into()]).unwrap();
        let cse = g.upsert_entity("Organization", "CSE, IIT Guwahati").unwrap();
        let lab = g.upsert_entity("Organization", "Software Analysis Lab").unwrap();
        g.link(&cse.id, "PART_OF", &iitg.id).unwrap();
        g.link(&lab.id, "PART_OF", &cse.id).unwrap();

        let satya = g.upsert_entity("Student", "Satya").unwrap();
        let rao = g.upsert_entity("Person", "Kavya Rao").unwrap();
        let old = g.upsert_entity("Person", "Arun").unwrap();
        let visitor = g.upsert_entity("Person", "Meera").unwrap();

        // By the text people write: "IITG" is the existing page.
        let org = g.affiliate(&satya.id, "AFFILIATED_WITH", "CSE, IIT Guwahati", &details(Some("PhD scholar"), Some("2024"), None)).unwrap();
        assert_eq!(org.id, cse.id);
        g.affiliate(&old.id, "AFFILIATED_WITH", "IITG", &details(Some("Postdoc"), Some("2020"), Some("2022-06"))).unwrap();
        g.affiliate(&old.id, "AFFILIATED_WITH", "Google", &details(Some("Engineer"), Some("2022-07"), None)).unwrap();
        g.affiliate(&rao.id, "STUDIED_AT", "Software Analysis Lab", &details(Some("MTech"), None, Some("2015"))).unwrap();
        g.affiliate(&rao.id, "AFFILIATED_WITH", "IISc", &details(Some("Associate Professor"), Some("2019"), None)).unwrap();
        g.link(&visitor.id, "COLLABORATES_WITH", &lab.id).ok();

        // New organizations were created once, as organizations.
        assert_eq!(g.find_by_name("Google").unwrap().unwrap().kind, "Organization");
        assert!(g.affiliate(&satya.id, "AFFILIATED_WITH", "Satya", &LinkDetails::default()).is_err(), "a person isn't an organization");

        let summary = |include_past: bool| -> Vec<(String, String, String, Option<String>)> {
            g.affiliations_at(&iitg.id, include_past)
                .unwrap()
                .into_iter()
                .map(|a| (a.person.name, a.kind, a.organization.name, a.detail))
                .collect()
        };
        assert_eq!(
            summary(false),
            vec![("Satya".into(), "AFFILIATED_WITH".into(), "CSE, IIT Guwahati".into(), Some("PhD scholar".into()))]
        );
        assert_eq!(
            summary(true),
            vec![
                ("Arun".into(), "AFFILIATED_WITH".into(), "IIT Guwahati".into(), Some("Postdoc".into())),
                ("Kavya Rao".into(), "STUDIED_AT".into(), "Software Analysis Lab".into(), Some("MTech".into())),
                ("Satya".into(), "AFFILIATED_WITH".into(), "CSE, IIT Guwahati".into(), Some("PhD scholar".into())),
            ]
        );

        // Details show on the person's links, and the shown affiliation is where they are now.
        let arun_links = g.links(&old.id).unwrap();
        let google = arun_links.iter().find(|l| l.other.name == "Google").unwrap();
        assert_eq!((google.detail.as_deref(), google.since.as_deref(), google.until.as_deref()), (Some("Engineer"), Some("2022-07"), None));
        assert_eq!(g.get(&old.id).unwrap().unwrap().info["affiliation"], "Google");
        assert_eq!(g.get(&satya.id).unwrap().unwrap().info["affiliation"], "CSE, IIT Guwahati");

        // Ending a link, renaming or deleting the organization keep the shown affiliation right.
        g.link_with(&satya.id, "AFFILIATED_WITH", &cse.id, &details(None, None, Some("2020"))).unwrap();
        assert!(!g.get(&satya.id).unwrap().unwrap().info.contains_key("affiliation"));
        let satya_link = g.links(&satya.id).unwrap().into_iter().find(|l| l.kind == "AFFILIATED_WITH").unwrap();
        assert_eq!(satya_link.detail.as_deref(), Some("PhD scholar"), "unchanged details are kept");
        g.link_with(&satya.id, "AFFILIATED_WITH", &cse.id, &details(None, None, Some(""))).unwrap();
        g.rename(&cse.id, "Department of CSE").unwrap();
        assert_eq!(g.get(&satya.id).unwrap().unwrap().info["affiliation"], "Department of CSE");
        g.delete(&cse.id).unwrap();
        assert!(!g.get(&satya.id).unwrap().unwrap().info.contains_key("affiliation"));

        assert!(matches!(g.link_with(&satya.id, "AFFILIATED_WITH", &iitg.id, &details(None, Some("last year"), None)), Err(GraphError::InvalidInfo(_))));
    }

    #[test]
    fn affiliations_written_as_text_are_found_with_their_match() {
        let g = Graph::in_memory().unwrap();
        let iitg = g.upsert_entity("Organization", "IIT Guwahati").unwrap();
        g.set_aliases(&iitg.id, &["IITG".into()]).unwrap();
        let a = g.upsert_entity("Person", "Tenzin").unwrap();
        set(&g, &a.id, &[("affiliation", "IITG")]).unwrap();
        let b = g.upsert_entity("Person", "Kavya").unwrap();
        set(&g, &b.id, &[("affiliation", "IISc Bangalore")]).unwrap();
        let c = g.upsert_entity("Person", "Linked").unwrap();
        g.affiliate(&c.id, "AFFILIATED_WITH", "IIT Guwahati", &LinkDetails::default()).unwrap();

        let found: Vec<_> = g
            .unlinked_affiliations()
            .unwrap()
            .into_iter()
            .map(|(p, text, org)| (p.name, text, org.map(|o| o.name)))
            .collect();
        assert_eq!(
            found,
            vec![
                ("Kavya".to_string(), "IISc Bangalore".to_string(), None),
                ("Tenzin".to_string(), "IITG".to_string(), Some("IIT Guwahati".to_string())),
            ]
        );
    }

    #[test]
    fn task_details_are_columns_the_database_filters_and_sorts() {
        let g = Graph::in_memory().unwrap();
        let task = |name: &str, pairs: &[(&str, &str)]| {
            let t = g.upsert_entity("Task", name).unwrap();
            set(&g, &t.id, pairs).unwrap()
        };
        let slides = task("Prepare slides", &[("due", "2026-09-18T11:00"), ("priority", "High"), ("status", "open"), ("area", "Teaching")]);
        task("Review survey", &[("due", "2026-09-18"), ("priority", "high"), ("status", "open")]);
        task("NBA report", &[("due", "2026-09-25"), ("priority", "medium")]);
        task("Read paper", &[("priority", "low")]);
        task("Old task", &[("due", "2026-09-01"), ("status", "done")]);
        task("Grade quiz", &[("due", "2026-09-18"), ("priority", "low"), ("notes_link", "x")]);

        // Stored in columns, still read back as details, normalized.
        assert_eq!(slides.info["priority"], "high");
        assert_eq!(slides.info["area"], "teaching");
        let stored = g.get(&slides.id).unwrap().unwrap();
        assert_eq!(stored.info.get("due").map(String::as_str), Some("2026-09-18T11:00"));
        let conn = Connection::new(&g.db).unwrap();
        let mut raw = conn.query("MATCH (e:Entity {id: 'task:prepare slides'}) RETURN e.info, e.due_date, e.priority").unwrap();
        let row = raw.next().unwrap();
        assert_eq!(string(&row[0]), "{}", "not duplicated in the JSON");
        assert!(matches!(row[1], Value::Date(_)));

        let names = |q: TaskQuery| -> Vec<String> { g.tasks(&q).unwrap().into_iter().map(|t| t.name).collect() };
        // Soonest first; on one day a task with a time comes before the rest of that day, which
        // go by priority; undated tasks last.
        assert_eq!(
            names(TaskQuery { open_only: true, ..Default::default() }),
            vec!["Prepare slides", "Review survey", "Grade quiz", "NBA report", "Read paper"]
        );
        assert_eq!(
            names(TaskQuery { open_only: true, due_by: Some("2026-09-18".into()), ..Default::default() }),
            vec!["Prepare slides", "Review survey", "Grade quiz"]
        );
        assert_eq!(names(TaskQuery { done: Some(true), ..Default::default() }), vec!["Old task"]);
        assert_eq!(names(TaskQuery::default()).len(), 6);

        for (key, bad) in [("due", "Friday"), ("due", "2026-13-01"), ("priority", "urgent")] {
            assert!(matches!(set(&g, &slides.id, &[(key, bad)]), Err(GraphError::InvalidInfo(_))), "{key}={bad}");
        }
        // Removing a detail clears its column.
        let cleared = g.update_info(&slides.id, &BTreeMap::from([("due".to_string(), None)])).unwrap();
        assert!(!cleared.info.contains_key("due"));
        assert!(!g.get(&slides.id).unwrap().unwrap().info.contains_key("due"));
        assert!(matches!(g.tasks(&TaskQuery { due_by: Some("soon".into()), ..Default::default() }), Err(GraphError::InvalidInfo(_))));
    }

    #[test]
    fn tasks_link_to_people_projects_and_can_be_ticked_off() {
        let g = Graph::in_memory().unwrap();
        let satya = g.upsert_entity("Student", "Satya").unwrap();
        let kavya = g.upsert_entity("Person", "Kavya").unwrap();
        let fuzzing = g.upsert_entity("Project", "Fuzzing").unwrap();
        let review = g.upsert_entity("Task", "Review Satya's survey").unwrap();
        let comments = g.upsert_entity("Task", "Kavya's comments on the draft").unwrap();
        let unrelated = g.upsert_entity("Task", "Book travel").unwrap();
        g.link(&review.id, "FOR", &satya.id).unwrap();
        g.link(&fuzzing.id, "HAS_TASK", &review.id).unwrap();
        g.link(&comments.id, "WAITING_ON", &kavya.id).unwrap();
        g.link(&fuzzing.id, "HAS_TASK", &comments.id).unwrap();
        set(&g, &review.id, &[("due", "2026-09-18")]).unwrap();
        let _ = unrelated;

        let linked = |id: &str| -> Vec<String> {
            g.tasks(&TaskQuery { linked_to: Some(id.into()), open_only: true, ..Default::default() })
                .unwrap()
                .into_iter()
                .map(|t| t.name)
                .collect()
        };
        assert_eq!(linked(&satya.id), vec!["Review Satya's survey"]);
        assert_eq!(linked(&kavya.id), vec!["Kavya's comments on the draft"]);
        assert_eq!(linked(&fuzzing.id), vec!["Review Satya's survey", "Kavya's comments on the draft"]);

        let done = g.set_done(&review.id, true).unwrap();
        assert_eq!(done.info["status"], "done");
        assert!(linked(&satya.id).is_empty());
        let conn = Connection::new(&g.db).unwrap();
        let done_at = |g: &Graph| {
            let mut stmt = conn.prepare("MATCH (e:Entity {id: $id}) RETURN e.done_at").unwrap();
            let row = conn.execute(&mut stmt, vec![("id", review.id.clone().into())]).unwrap().next().unwrap();
            let _ = g;
            match row[0] {
                Value::Int64(n) => n,
                _ => 0,
            }
        };
        let first = done_at(&g);
        assert!(first > 0);
        // Saving again keeps when it was done; reopening clears it.
        set(&g, &review.id, &[("priority", "low")]).unwrap();
        assert_eq!(done_at(&g), first);
        g.set_done(&review.id, false).unwrap();
        assert_eq!(done_at(&g), 0);
        assert_eq!(linked(&satya.id), vec!["Review Satya's survey"]);
    }

    #[test]
    fn tasks_assigned_to_people_are_told_apart_from_the_users_own() {
        let g = Graph::in_memory().unwrap();
        let rohan = g.upsert_entity("Student", "Rohan Das").unwrap();
        let kabir = g.upsert_entity("Student", "Kabir Mehta").unwrap();
        let sota = g.upsert_entity("Task", "Present state of the art on Solidity Compiler Fuzzing").unwrap();
        let review = g.upsert_entity("Task", "Review Rohan's slides").unwrap();
        g.link(&sota.id, "ASSIGNED_TO", &rohan.id).unwrap();
        g.link(&sota.id, "ASSIGNED_TO", &kabir.id).unwrap();
        g.link(&review.id, "FOR", &rohan.id).unwrap();

        let names = |q: TaskQuery| -> Vec<String> { g.tasks(&q).unwrap().into_iter().map(|t| t.name).collect() };
        assert_eq!(names(TaskQuery { assigned: Some(true), ..Default::default() }), vec!["Present state of the art on Solidity Compiler Fuzzing"]);
        assert_eq!(names(TaskQuery { assigned: Some(false), ..Default::default() }), vec!["Review Rohan's slides"]);
        // On Rohan's page: what he's assigned and what the user owes him.
        assert_eq!(names(TaskQuery { linked_to: Some(rohan.id.clone()), ..Default::default() }).len(), 2);
        assert_eq!(names(TaskQuery { linked_to: Some(kabir.id.clone()), ..Default::default() }), vec!["Present state of the art on Solidity Compiler Fuzzing"]);
    }

    #[test]
    fn task_details_saved_as_json_by_earlier_releases_move_to_columns() {
        let dir = std::env::temp_dir().join(format!("suk-columns-{}", now_ms()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("suk.lbdb");
        {
            let g = Graph::open(&path).unwrap();
            let conn = Connection::new(&g.db).unwrap();
            conn.query("CREATE (:Entity {id: 'task:nba', kind: 'Task', name: 'NBA report', created_at: 1, info: '{\"due\":\"2026-09-25\",\"priority\":\"high\",\"estimate\":\"2h\"}'})").unwrap();
            conn.query("CREATE (:Entity {id: 'task:odd', kind: 'Task', name: 'Odd', created_at: 2, info: '{\"due\":\"next week\"}'})").unwrap();
        }
        let g = Graph::open(&path).unwrap();
        let nba = g.get("task:nba").unwrap().unwrap();
        assert_eq!(nba.info.get("due").map(String::as_str), Some("2026-09-25"));
        assert_eq!(nba.info["estimate"], "2h");
        assert_eq!(g.tasks(&TaskQuery { due_by: Some("2026-09-30".into()), ..Default::default() }).unwrap().len(), 1);
        let odd = g.get("task:odd").unwrap().unwrap();
        assert_eq!((odd.info.get("due"), odd.info.get("due_text").map(String::as_str)), (None, Some("next week")));
        drop(g);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn students_from_earlier_releases_become_people_with_the_role() {
        let dir = std::env::temp_dir().join(format!("suk-migrate-{}", now_ms()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("suk.lbdb");
        {
            let g = Graph::open(&path).unwrap();
            let conn = Connection::new(&g.db).unwrap();
            // As the previous release stored a student.
            conn.query("CREATE (:Entity {id: 'student:rahul', kind: 'Student', name: 'Rahul', created_at: 1, info: '{\"email\":\"r@x.in\"}'})").unwrap();
            conn.query("CREATE (:Entity {id: 'project:fuzzing', kind: 'Project', name: 'Fuzzing', created_at: 2})").unwrap();
            g.link("student:rahul", "WORKS_ON", "project:fuzzing").unwrap();
        }
        let g = Graph::open(&path).unwrap();
        let rahul = g.get("student:rahul").unwrap().unwrap();
        assert_eq!((rahul.kind.as_str(), rahul.tags.clone()), ("Person", vec!["student".to_string()]));
        assert_eq!(rahul.info["email"], "r@x.in");
        assert_eq!(g.links(&rahul.id).unwrap().len(), 1);
        assert_eq!(g.entities_of_kind("Student").unwrap().len(), 1);
        drop(g);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn rename_change_kind_and_delete() {
        let g = Graph::in_memory().unwrap();
        let satya = g.upsert_entity("Person", "Satya").unwrap();
        g.upsert_entity("Project", "Fuzzing").unwrap();
        g.link(&satya.id, "WORKS_ON", "project:fuzzing").unwrap();
        let msg = g.add_message("user", "Satya works on fuzzing", None, &[satya.id.clone()]).unwrap();

        let renamed = g.rename(&satya.id, " Satya  Das ").unwrap();
        assert_eq!((renamed.id.as_str(), renamed.name.as_str()), ("person:satya", "Satya Das"));
        assert!(matches!(g.rename(&satya.id, "fuzzing"), Err(GraphError::InvalidName(_))));
        let student = g.change_kind(&satya.id, "Student").unwrap();
        assert_eq!((student.kind.as_str(), student.tags.clone()), ("Person", vec!["student".to_string()]));
        assert!(g.change_kind(&satya.id, "robot").is_err());
        assert_eq!(g.change_kind(&satya.id, "Grant Application").unwrap().kind, "Grant Application");

        g.delete(&satya.id).unwrap();
        assert!(g.get(&satya.id).unwrap().is_none());
        assert!(g.links("project:fuzzing").unwrap().is_empty());
        assert_eq!(g.recent_messages(5).unwrap()[0].id, msg.id);
    }

    #[test]
    fn tags_are_normalized_and_kind_tags_implied() {
        let g = Graph::in_memory().unwrap();
        let satya = g.upsert_entity("Student", "Satya").unwrap();
        let tagged = g
            .set_tags(&satya.id, &["#PhD Students".into(), "phd_students".into(), "Student".into(), "person".into(), " ".into()])
            .unwrap();
        // The kind's own tag (person) is implied; a role is a real tag.
        assert_eq!(tagged.tags, vec!["phd-students", "student"]);
        let tagged = g.add_tags(&satya.id, &["Fuzzing Group".into()]).unwrap();
        assert_eq!(tagged.tags, vec!["phd-students", "student", "fuzzing-group"]);
        assert_eq!(effective_tags(&tagged), vec!["person", "phd-students", "student", "fuzzing-group"]);
        assert_eq!(role_of(&tagged), Some("student"));

        g.upsert_entity("Project", "Fuzzing").unwrap();
        let names = |tag: &str| -> Vec<String> {
            g.entities_with_tag(tag).unwrap().into_iter().map(|e| e.name).collect()
        };
        assert_eq!(names("student"), vec!["Satya"]);
        assert_eq!(names("#Fuzzing group"), vec!["Satya"]);
        assert_eq!(names("project"), vec!["Fuzzing"]);
        assert!(names("nothing").is_empty());

        assert_eq!(kind_tag("ResearchArea"), "research-area");
        assert_eq!(section_title("person"), "People");
        assert_eq!(section_title("nba-accreditation"), "Nba accreditation");
    }

    #[test]
    fn notes_and_wikilinks_stay_in_sync() {
        let g = Graph::in_memory().unwrap();
        let page = g.upsert_entity("Note", "Reading group").unwrap();
        let satya = g.upsert_entity("Student", "Satya").unwrap();
        g.upsert_entity("Project", "Fuzzing").unwrap();
        g.link(&satya.id, "WORKS_ON", "project:fuzzing").unwrap();

        let saved = g.set_notes(&page.id, "Talked with [[Satya]] about [[Fuzzing]] and [[Later]]").unwrap();
        assert!(saved.updated_at > 0);
        g.sync_wikilinks(&page.id, &["Satya".into(), "fuzzing".into(), "Later".into(), "Reading group".into()])
            .unwrap();
        let targets: Vec<_> = g.links(&page.id).unwrap().into_iter().map(|l| l.other.name).collect();
        assert_eq!(targets, vec!["Satya", "Fuzzing"]);

        // Editing the notes replaces the links but leaves other relationships alone.
        g.sync_wikilinks(&page.id, &["Satya".into()]).unwrap();
        assert_eq!(g.links(&page.id).unwrap().len(), 1);
        assert_eq!(g.links(&satya.id).unwrap().len(), 2);
        assert_eq!(g.get(&page.id).unwrap().unwrap().notes, saved.notes);
    }

    #[test]
    fn messages_are_stored_with_what_they_mention() {
        let g = Graph::in_memory().unwrap();
        let satya = g.upsert_entity("Student", "Satya").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(2));
        let start = now_ms();
        g.add_message("user", "I have a student Satya", None, &[satya.id.clone()]).unwrap();
        g.add_message("agent", "Saved Satya.", None, &[satya.id.clone()]).unwrap();
        g.add_message("user", "What's his email?", Some(&satya.id), &[satya.id.clone()]).unwrap();
        g.add_message("user", "Unrelated", None, &[]).unwrap();

        let main: Vec<_> = g.recent_messages(10).unwrap().into_iter().map(|m| m.text).collect();
        assert_eq!(main, vec!["I have a student Satya", "Saved Satya.", "Unrelated"]);
        assert_eq!(g.recent_messages(1).unwrap()[0].text, "Unrelated");
        let about = g.messages_about(&satya.id, 10).unwrap();
        assert_eq!(about.len(), 3);
        assert_eq!(about[2].focus.as_deref(), Some(satya.id.as_str()));
        assert!(about[0].at >= start);

        assert_eq!(g.created_since(start).unwrap().len(), 0);
        g.upsert_entity("Project", "Fuzzing").unwrap();
        assert_eq!(g.created_since(start).unwrap()[0].name, "Fuzzing");
    }

    #[test]
    fn sections_pin_dismiss_and_rename() {
        let g = Graph::in_memory().unwrap();
        g.pin_section("student", "Students").unwrap();
        g.dismiss_section("Project").unwrap();
        g.pin_section("#NBA Accreditation", "NBA").unwrap();
        let summary = |g: &Graph| -> Vec<(String, String, bool)> {
            g.sections().unwrap().into_iter().map(|s| (s.tag, s.title, s.pinned)).collect()
        };
        assert_eq!(
            summary(&g),
            vec![
                ("student".into(), "Students".into(), true),
                ("project".into(), "Projects".into(), false),
                ("nba-accreditation".into(), "NBA".into(), true),
            ]
        );
        // Renaming keeps the position; removing keeps the title.
        g.pin_section("student", "My students").unwrap();
        g.dismiss_section("student").unwrap();
        assert_eq!(summary(&g)[0], ("student".into(), "My students".into(), false));
        assert!(g.pin_section("  ", "x").is_err());
    }

    /// Prints every entity and outgoing link in a database. Use a copy, not the live file:
    /// `SUK_DB=/path/copy/suk.lbdb cargo test --lib dump_graph -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn dump_graph() {
        let g = Graph::open(std::env::var("SUK_DB").expect("set SUK_DB")).unwrap();
        for kind in ENTITY_KINDS {
            for e in g.entities_of_kind(kind).unwrap() {
                println!("{kind:<12} {} {:?} {:?} notes:{}", e.name, e.info, e.tags, e.notes.len());
                for l in g.links(&e.id).unwrap().iter().filter(|l| l.outgoing) {
                    println!("{:<12}   {} -> {} ({})", "", l.kind, l.other.name, l.other.kind);
                }
            }
        }
    }

    /// Prints the messages about a page (all chats), or without PAGE the main chat's latest, from a
    /// copy of a database:
    /// `SUK_DB=/path/copy/suk.lbdb PAGE=person:x cargo test --lib dump_messages -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn dump_messages() {
        let g = Graph::open(std::env::var("SUK_DB").expect("set SUK_DB")).unwrap();
        let messages = match std::env::var("PAGE") {
            Ok(page) => g.messages_about(&page, 50).unwrap(),
            Err(_) => g.recent_messages(40).unwrap(),
        };
        for m in messages {
            println!("[{}] {} (focus {:?}): {}", m.at, m.role, m.focus, m.text);
        }
    }

    #[test]
    fn data_persists_across_reopen() {
        let dir = std::env::temp_dir().join(format!("suk-test-{}", now_ms()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("suk.lbdb");
        {
            let g = Graph::open(&path).unwrap();
            let rahul = g.upsert_entity("Student", "Rahul").unwrap();
            let project = g.upsert_entity("Project", "LLVM Security").unwrap();
            g.link(&rahul.id, "WORKS_ON", &project.id).unwrap();
        }
        {
            // Reopening re-runs the schema DDL, which must tolerate existing tables.
            let g = Graph::open(&path).unwrap();
            assert_eq!(g.entities_of_kind("Student").unwrap().len(), 1);
            assert_eq!(g.links("person:rahul").unwrap().len(), 1);
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
