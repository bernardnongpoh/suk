//! The tools Claude gets over MCP, the schedule proposals it makes, and the policy deciding which
//! outside tools (Google Calendar) it may use.
//!
//! Claude never touches the database directly: every tool validates its input and goes through
//! `Graph`. Calendar events are only created for items the user confirmed in the UI; that is
//! enforced here, not left to the prompt.

use std::collections::BTreeMap;
use std::sync::Mutex;

use chrono::NaiveDateTime;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::graph::LinkDetails;
use crate::relations::{allowed, details_text, lookup, sentence};
use crate::graph::{effective_tags, project_status, resolve_kind, Entity, Graph, GraphError, ENTITY_KINDS, PROJECT_STATUSES, RELATION_KINDS, ROLES};
use crate::graph::TaskQuery;
use crate::pages::{task_items, wikilinks};
use crate::watch;

/// The Google Calendar connector's tools, as Claude Code names them.
pub const CALENDAR_PREFIX: &str = "mcp__claude_ai_Google_Calendar__";
/// Name of this app's MCP server in the Claude Code config; tools become `mcp__suk__<tool>`.
pub const SERVER_NAME: &str = "suk";
/// The tool Claude Code asks before using any tool that isn't pre-approved.
pub const APPROVE_TOOL: &str = "approve";
const TIME_FORMAT: &str = "%Y-%m-%dT%H:%M";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ScheduleItem {
    pub title: String,
    /// Local time, "YYYY-MM-DDTHH:MM".
    pub start: String,
    pub end: String,
    /// The task this time is for, by name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum ItemState {
    Proposed,
    /// Confirmed; Claude may now add it to the calendar.
    Confirmed,
    /// Confirmed and added to the calendar.
    Added,
    /// Left out when the proposal was confirmed or dismissed.
    Skipped,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum ProposalStatus {
    Pending,
    Confirmed,
    Dismissed,
}

#[derive(Debug, Clone, Serialize)]
pub struct Proposal {
    pub id: String,
    pub summary: Option<String>,
    pub items: Vec<ScheduleItem>,
    pub states: Vec<ItemState>,
    pub status: ProposalStatus,
}

#[derive(Default)]
pub struct Proposals {
    inner: Mutex<Vec<Proposal>>,
}

impl Proposals {
    fn add(&self, summary: Option<String>, items: Vec<ScheduleItem>) -> Proposal {
        let mut list = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let proposal = Proposal {
            id: format!("p{}", list.len() + 1),
            summary,
            states: vec![ItemState::Proposed; items.len()],
            items,
            status: ProposalStatus::Pending,
        };
        list.push(proposal.clone());
        proposal
    }

    pub fn get(&self, id: &str) -> Option<Proposal> {
        self.lock().iter().find(|p| p.id == id).cloned()
    }

    /// Proposals made after the first `count`, i.e. during the current turn.
    pub fn since(&self, count: usize) -> Vec<Proposal> {
        self.lock().iter().skip(count).cloned().collect()
    }

    pub fn count(&self) -> usize {
        self.lock().len()
    }

    /// Confirms the chosen items of a pending proposal and returns it.
    pub fn confirm(&self, id: &str, chosen: &[usize]) -> Result<Proposal, String> {
        self.decide(id, |p| {
            if chosen.is_empty() || chosen.iter().any(|&i| i >= p.items.len()) {
                return Err("choose at least one item from the proposal".into());
            }
            p.status = ProposalStatus::Confirmed;
            for (i, state) in p.states.iter_mut().enumerate() {
                *state = if chosen.contains(&i) { ItemState::Confirmed } else { ItemState::Skipped };
            }
            Ok(())
        })
    }

    pub fn dismiss(&self, id: &str) -> Result<Proposal, String> {
        self.decide(id, |p| {
            p.status = ProposalStatus::Dismissed;
            p.states.fill(ItemState::Skipped);
            Ok(())
        })
    }

    fn decide(
        &self,
        id: &str,
        change: impl FnOnce(&mut Proposal) -> Result<(), String>,
    ) -> Result<Proposal, String> {
        let mut list = self.lock();
        let proposal = list
            .iter_mut()
            .find(|p| p.id == id)
            .ok_or_else(|| format!("no proposal {id}"))?;
        if proposal.status != ProposalStatus::Pending {
            return Err(format!("proposal {id} was already answered"));
        }
        change(proposal)?;
        Ok(proposal.clone())
    }

    /// The confirmed, not yet added item a calendar tool call is creating, as (proposal, index).
    pub fn confirmed_item_for(&self, input: &Value) -> Option<(String, usize)> {
        let text = input.to_string().to_lowercase();
        self.lock().iter().find_map(|p| {
            p.items.iter().enumerate().find_map(|(i, item)| {
                let matches = p.states[i] == ItemState::Confirmed
                    && text.contains(&json_text(&item.title).to_lowercase())
                    && text.contains(&item.start.to_lowercase());
                matches.then(|| (p.id.clone(), i))
            })
        })
    }

    pub fn mark_added(&self, id: &str, index: usize) {
        if let Some(p) = self.lock().iter_mut().find(|p| p.id == id) {
            if p.states.get(index) == Some(&ItemState::Confirmed) {
                p.states[index] = ItemState::Added;
            }
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Vec<Proposal>> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// What Claude's tool calls did during the current turn, for the app to act on afterwards.
#[derive(Default)]
pub struct Activity {
    inner: Mutex<(Vec<String>, Vec<(String, String)>)>,
    /// Links the user gave in the current turn; only these can be read.
    links: Mutex<Vec<String>>,
    /// Lets tests read pages served on this computer.
    #[cfg(test)]
    pub allow_local_links: std::sync::atomic::AtomicBool,
}

impl Activity {
    /// Starts a turn: clears what the last one did and allows reading `links`.
    pub fn begin(&self, links: Vec<String>) {
        self.take();
        *self.links.lock().unwrap_or_else(|e| e.into_inner()) = links;
    }

    /// Records that the turn saved or changed an entity.
    fn touch(&self, id: &str) {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if !inner.0.iter().any(|t| t == id) {
            inner.0.push(id.to_string());
        }
    }

    fn suggest(&self, tag: &str, title: &str) {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        inner.1.push((tag.to_string(), title.to_string()));
    }

    fn link_allowed(&self, url: &str) -> bool {
        let trim = |u: &str| u.trim_end_matches('/').to_string();
        self.links.lock().unwrap_or_else(|e| e.into_inner()).iter().any(|l| trim(l) == trim(url))
    }

    fn local_links(&self) -> bool {
        #[cfg(test)]
        return self.allow_local_links.load(std::sync::atomic::Ordering::Relaxed);
        #[cfg(not(test))]
        false
    }

    /// Takes the touched entity ids and suggested (tag, title) sections, clearing both.
    pub fn take(&self) -> (Vec<String>, Vec<(String, String)>) {
        std::mem::take(&mut *self.inner.lock().unwrap_or_else(|e| e.into_inner()))
    }
}

/// Everything a tool call can use.
pub struct Ctx<'a> {
    pub graph: &'a Graph,
    pub proposals: &'a Proposals,
    pub activity: &'a Activity,
    /// Reads the web for lookups such as a researcher's OpenAlex profile.
    pub fetch: &'a dyn watch::Fetch,
}

/// A string as it appears inside serialized JSON, so quotes in a title still match.
fn json_text(s: &str) -> String {
    let quoted = serde_json::to_string(s).unwrap_or_default();
    quoted[1..quoted.len() - 1].to_string()
}

/// Tool descriptions and input schemas, in MCP `tools/list` form.
pub fn definitions() -> Value {
    let kinds = ENTITY_KINDS;
    // LINKS_TO belongs to the notes editor.
    let relations: Vec<&str> = RELATION_KINDS.iter().copied().filter(|r| *r != "LINKS_TO").collect();
    json!([
        {
            "name": "find",
            "description": "Look up people, students, projects, courses, tasks and so on by name. Returns each match's type, details and relationships, including the collaborators on their projects. A partial description like \"the fuzzing project\" also works.",
            "inputSchema": {
                "type": "object",
                "properties": { "names": { "type": "array", "items": { "type": "string" }, "minItems": 1, "maxItems": 10 } },
                "required": ["names"]
            }
        },
        {
            "name": "list",
            "description": "List every entity of one type with its details and relationships. Use the tasks tool for tasks, and type Event for time already planned in this app.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "type": { "enum": kinds },
                    "status": { "enum": PROJECT_STATUSES, "description": "For projects: only those planned, in progress or completed." }
                },
                "required": ["type"]
            }
        },
        {
            "name": "tasks",
            "description": "Tasks, soonest due first then by priority, each with who it is assigned to (empty means the user's own), who it is for, who the user is waiting on, and its project or course.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "status": { "enum": ["open", "done", "all"], "description": "Default open." },
                    "due_by": { "type": "string", "description": "Only tasks due on or before this date, YYYY-MM-DD." },
                    "about": { "type": "string", "description": "Only tasks linked to this person, project or course, by name." },
                    "assigned": { "enum": ["mine", "others", "all"], "description": "The user's own tasks, tasks assigned to other people, or both. Default all." }
                }
            }
        },
        {
            "name": "people_at",
            "description": "Who the user knows at an organization, including its departments and labs: each person with their position or degree, dates, and roles. Accepts other names (IITG).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "organization": { "type": "string" },
                    "include_past": { "type": "boolean", "description": "Also people who used to be there or studied there. Default false." },
                    "role": { "enum": ROLES, "description": "Only people with this role." }
                },
                "required": ["organization"]
            }
        },
        {
            "name": "save",
            "description": "Create an entity, or update the details, roles and tags of an existing one with the same name. Everyone (students, collaborators, colleagues) is a Person; how they relate to the user goes in roles. Details merge with what is stored; set a detail to null to remove it. Roles and tags are added. Returns the stored entity.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "type": { "enum": kinds },
                    "name": { "type": "string", "minLength": 1, "maxLength": 120 },
                    "info": {
                        "type": "object",
                        "description": "Lowercase snake_case keys with short text values. For tasks: due (YYYY-MM-DD or YYYY-MM-DDTHH:MM), priority (high, medium, low), status (open, waiting, done), area.",
                        "additionalProperties": { "type": ["string", "null"] }
                    },
                    "roles": {
                        "type": "array",
                        "description": "For a Person: how they are connected to the user.",
                        "items": { "enum": ROLES }
                    },
                    "aliases": {
                        "type": "array",
                        "description": "Other names the page is known by (IITG and Indian Institute of Technology Guwahati for IIT Guwahati). Added to any already stored.",
                        "items": { "type": "string", "maxLength": 120 },
                        "maxItems": 10
                    },
                    "tags": {
                        "type": "array",
                        "description": "Short groupings such as phd, nba-accreditation or reading-group. The type is already a tag.",
                        "items": { "type": "string", "maxLength": 40 },
                        "maxItems": 10
                    }
                },
                "required": ["type", "name"]
            }
        },
        {
            "name": "link",
            "description": "Record a relationship between two entities, or update its details. Give from_type/to_type to create an endpoint that doesn't exist yet. Other names (aliases) find existing pages.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "from": { "type": "string" },
                    "relation": { "enum": relations },
                    "to": { "type": "string" },
                    "from_type": { "enum": kinds },
                    "to_type": { "enum": kinds },
                    "detail": { "type": "string", "maxLength": 120, "description": "Position for AFFILIATED_WITH (Associate Professor), degree for STUDIED_AT (PhD). Empty string clears it." },
                    "since": { "type": "string", "description": "YYYY, YYYY-MM or YYYY-MM-DD. Empty string clears it." },
                    "until": { "type": "string", "description": "When it ended: YYYY, YYYY-MM or YYYY-MM-DD. Empty string clears it." }
                },
                "required": ["from", "relation", "to"]
            }
        },
        {
            "name": "rename",
            "description": "Change a page's name, the title it is shown and linked under. It stays the same page: details, relationships, tasks, notes and conversations are kept, and its Obsidian file is renamed.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "The page's current name" },
                    "new_name": { "type": "string", "minLength": 1, "maxLength": 120 }
                },
                "required": ["name", "new_name"]
            }
        },
        {
            "name": "unlink",
            "description": "Remove a relationship that is no longer true.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "from": { "type": "string" },
                    "relation": { "enum": relations },
                    "to": { "type": "string" }
                },
                "required": ["from", "relation", "to"]
            }
        },
        {
            "name": "add_note",
            "description": "Append a line to an entity's notes page, for context that doesn't fit its details: what was discussed or decided, preferences, progress. The app adds today's date. Write [[Name]] to link other pages.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "name": { "type": "string" },
                    "text": { "type": "string", "minLength": 1, "maxLength": 2000 }
                },
                "required": ["name", "text"]
            }
        },
        {
            "name": "follow",
            "description": "Follow a person so the app checks their new papers (OpenAlex), homepage changes and feeds (blog, GitHub) in the background and notifies the user about what relates to their work. Also stores profile links the user pasted (homepage, X/Twitter, LinkedIn, Google Scholar, GitHub, DBLP, ORCID, OpenAlex) under the right details. X, LinkedIn and Google Scholar links are kept but can't be checked. Returns what will be checked and, when papers can't be checked yet, OpenAlex authors who might be this person.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "name": { "type": "string", "minLength": 1 },
                    "links": { "type": "array", "items": { "type": "string" }, "description": "Profile links from the user's message." },
                    "openalex": { "type": "string", "description": "The OpenAlex author id (A…) the user confirmed is this person." },
                    "follow": { "type": "boolean", "description": "false to stop following. Default true." }
                },
                "required": ["name"]
            }
        },
        {
            "name": "updates",
            "description": "Recent papers, posts and homepage changes from people the user follows, newest first, with how relevant each is to the user's work and why.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "Only this person's." },
                    "all": { "type": "boolean", "description": "Include items judged not relevant, and items found when following started. Default false." }
                }
            }
        },
        {
            "name": "fix_note",
            "description": "Correct or remove a wrong statement in an entity's notes page. old_text must appear exactly once in the notes; it is replaced by new_text, and a line left empty is removed. For details (info), use save instead.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "name": { "type": "string" },
                    "old_text": { "type": "string", "minLength": 3 },
                    "new_text": { "type": "string", "description": "Empty to remove it." }
                },
                "required": ["name", "old_text", "new_text"]
            }
        },
        {
            "name": "read_link",
            "description": "Read a web page the user pasted in their message, such as a student's or collaborator's profile, and get its title, email addresses and text. Only links from the user's message can be read.",
            "inputSchema": {
                "type": "object",
                "properties": { "url": { "type": "string" } },
                "required": ["url"]
            }
        },
        {
            "name": "suggest_section",
            "description": "Offer the user a sidebar section listing everything with a tag, for a grouping they will keep coming back to (a committee, a grant, a reading group). The app already offers sections for students, people, projects and courses by itself.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "title": { "type": "string", "minLength": 1, "maxLength": 40 },
                    "tag": { "type": "string", "minLength": 1, "maxLength": 40 }
                },
                "required": ["title", "tag"]
            }
        },
        {
            "name": "propose_schedule",
            "description": "Show the user time blocks to put on their calendar. The app displays them with checkboxes and a Confirm button. Nothing is added until they confirm; the app then tells you which items to add.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "summary": { "type": "string", "maxLength": 300 },
                    "items": {
                        "type": "array",
                        "minItems": 1,
                        "maxItems": 12,
                        "items": {
                            "type": "object",
                            "properties": {
                                "title": { "type": "string", "minLength": 1, "maxLength": 120 },
                                "start": { "type": "string", "description": "Local time, YYYY-MM-DDTHH:MM" },
                                "end": { "type": "string", "description": "Local time, YYYY-MM-DDTHH:MM" },
                                "task": { "type": "string", "description": "Name of the task this time is for, if any" },
                                "notes": { "type": "string", "maxLength": 300 }
                            },
                            "required": ["title", "start", "end"]
                        }
                    }
                },
                "required": ["items"]
            }
        },
        {
            "name": APPROVE_TOOL,
            "description": "Used by the app to answer permission checks. Never call this yourself.",
            "inputSchema": {
                "type": "object",
                "properties": { "tool_name": { "type": "string" }, "input": { "type": "object" } },
                "required": ["tool_name"]
            }
        }
    ])
}

/// Runs a tool. `Err` is a message for Claude, reported as a tool error.
pub fn call(ctx: &Ctx, name: &str, args: Value) -> Result<String, String> {
    let graph = ctx.graph;
    match name {
        "find" => find(graph, parse(args)?),
        "list" => list(graph, parse(args)?),
        "tasks" => tasks(graph, parse(args)?),
        "people_at" => people_at(graph, parse(args)?),
        "save" => save(ctx, parse(args)?),
        "link" => link(ctx, parse(args)?),
        "unlink" => unlink(ctx, parse(args)?),
        "rename" => rename(ctx, parse(args)?),
        "add_note" => add_note(ctx, parse(args)?),
        "fix_note" => fix_note(ctx, parse(args)?),
        "read_link" => {
            let args: LinkReadArgs = parse(args)?;
            if !ctx.activity.link_allowed(&args.url) {
                return Err("Only links the user pasted in this message can be read.".into());
            }
            let page = crate::links::read(&args.url, ctx.activity.local_links())?;
            Ok(serde_json::to_string(&page).map_err(|e| e.to_string())?)
        }
        "follow" => follow(ctx, parse(args)?),
        "updates" => updates(graph, parse(args)?),
        "suggest_section" => {
            let args: SectionArgs = parse(args)?;
            ctx.activity.suggest(&args.tag, &args.title);
            Ok("The app will offer the user this section below your reply. Don't mention it.".into())
        }
        "propose_schedule" => propose(ctx.proposals, parse(args)?),
        APPROVE_TOOL => Ok(approve(ctx.proposals, parse(args)?).to_string()),
        other => Err(format!("unknown tool: {other}")),
    }
}

fn parse<T: for<'de> Deserialize<'de>>(args: Value) -> Result<T, String> {
    serde_json::from_value(args).map_err(|e| format!("invalid arguments: {e}"))
}

fn db(e: GraphError) -> String {
    e.to_string()
}

#[derive(Deserialize)]
struct FindArgs {
    names: Vec<String>,
}

fn find(graph: &Graph, args: FindArgs) -> Result<String, String> {
    let mut found = Vec::new();
    for name in &args.names {
        match lookup(graph, name).map_err(db)? {
            Some(entity) => {
                let mut described = describe(graph, &entity, true)?;
                if graph.find_by_name(name).map_err(db)?.is_none() {
                    described["note"] = format!("closest match for \"{name}\", not an exact name").into();
                }
                found.push(described)
            }
            None => found.push(json!({ "query": name, "found": false })),
        }
    }
    Ok(Value::Array(found).to_string())
}

#[derive(Deserialize)]
struct ListArgs {
    #[serde(rename = "type")]
    kind: String,
    status: Option<String>,
}

fn list(graph: &Graph, args: ListArgs) -> Result<String, String> {
    let entities = graph.entities_of_kind(&args.kind).map_err(db)?;
    let status = match &args.status {
        Some(status) => Some(project_status(status).ok_or_else(|| format!("status must be one of {}", PROJECT_STATUSES.join(", ")))?),
        None => None,
    };
    let described = entities
        .iter()
        .filter(|e| status.is_none_or(|s| e.info.get("status").map(String::as_str) == Some(s)))
        .map(|e| describe(graph, e, false))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Value::Array(described).to_string())
}

/// An entity with its details and relationships as sentences; `deep` also follows its projects
/// and courses one step, so a student's collaborators show up.
/// Notes longer than this are cut when shown to Claude: in full for `find`, briefly for `list`.
const NOTES_IN_FIND: usize = 4000;
const NOTES_IN_LIST: usize = 300;

pub(crate) fn describe(graph: &Graph, entity: &Entity, deep: bool) -> Result<Value, String> {
    let links = graph.links(&entity.id).map_err(db)?;
    let with_details = |fact: String, l: &crate::graph::Link| match details_text(l.detail.as_deref(), l.since.as_deref(), l.until.as_deref()) {
        Some(details) => format!("{fact} ({details})"),
        None => fact,
    };
    let mut facts: Vec<String> = links
        .iter()
        .map(|l| {
            let fact = if l.outgoing {
                sentence(&entity.name, &l.kind, &l.other.name)
            } else {
                sentence(&l.other.name, &l.kind, &entity.name)
            };
            with_details(fact, l)
        })
        .collect();
    if deep {
        // One step further through projects, courses and organizations, so a student's
        // collaborators and a department's university show up.
        for l in links.iter().filter(|l| matches!(l.other.kind.as_str(), "Project" | "Course") || (l.kind == "PART_OF" && l.outgoing)) {
            for next in graph.links(&l.other.id).map_err(db)? {
                if next.other.id == entity.id {
                    continue;
                }
                let fact = with_details(
                    if next.outgoing {
                        sentence(&l.other.name, &next.kind, &next.other.name)
                    } else {
                        sentence(&next.other.name, &next.kind, &l.other.name)
                    },
                    &next,
                );
                if !facts.contains(&fact) {
                    facts.push(fact);
                }
            }
        }
    }
    let mut described = json!({
        "name": entity.name,
        "type": entity.kind,
        "tags": effective_tags(entity),
        "info": entity.info,
        "relationships": facts,
    });
    if !entity.aliases.is_empty() {
        described["also_called"] = json!(entity.aliases);
    }
    let notes = entity.notes.trim();
    if !notes.is_empty() {
        let limit = if deep { NOTES_IN_FIND } else { NOTES_IN_LIST };
        let mut shown: String = notes.chars().take(limit).collect();
        if shown.len() < notes.len() {
            shown.push_str(" …(cut)");
        }
        described["notes"] = shown.into();
    }
    Ok(described)
}

#[derive(Deserialize)]
struct PeopleAtArgs {
    organization: String,
    #[serde(default)]
    include_past: bool,
    #[serde(default)]
    role: Option<String>,
}

fn people_at(graph: &Graph, args: PeopleAtArgs) -> Result<String, String> {
    let org = graph
        .find_by_name(&args.organization)
        .map_err(db)?
        .filter(|e| e.kind == "Organization")
        .ok_or_else(|| format!("no organization called \"{}\"", args.organization))?;
    let people: Vec<Value> = graph
        .affiliations_at(&org.id, args.include_past)
        .map_err(db)?
        .into_iter()
        .filter(|a| args.role.as_ref().is_none_or(|r| a.person.tags.contains(r)))
        .map(|a| {
            json!({
                "name": a.person.name,
                "roles": a.person.tags.iter().filter(|t| ROLES.contains(&t.as_str())).collect::<Vec<_>>(),
                "relation": if a.kind == "STUDIED_AT" { "studied at" } else { "at" },
                "where": a.organization.name,
                "details": details_text(a.detail.as_deref(), a.since.as_deref(), a.until.as_deref()),
                "current": a.current(),
            })
        })
        .collect();
    Ok(json!({ "organization": org.name, "people": people }).to_string())
}

#[derive(Deserialize)]
struct TasksArgs {
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    due_by: Option<String>,
    #[serde(default)]
    about: Option<String>,
    #[serde(default)]
    assigned: Option<String>,
}

fn tasks(graph: &Graph, args: TasksArgs) -> Result<String, String> {
    let done = match args.status.as_deref().unwrap_or("open") {
        "open" => Some(false),
        "done" => Some(true),
        "all" => None,
        other => return Err(format!("status {other} must be open, done or all")),
    };
    let linked_to = match &args.about {
        Some(name) => Some(lookup(graph, name).map_err(db)?.ok_or_else(|| format!("nothing named \"{name}\""))?.id),
        None => None,
    };
    let assigned = match args.assigned.as_deref().unwrap_or("all") {
        "mine" => Some(false),
        "others" => Some(true),
        "all" => None,
        other => return Err(format!("assigned {other} must be mine, others or all")),
    };
    let query = TaskQuery { done, due_by: args.due_by, linked_to, assigned, ..Default::default() };
    let names = |v: &[Entity]| v.iter().map(|e| e.name.clone()).collect::<Vec<_>>();
    let items: Vec<Value> = task_items(graph, &query)
        .map_err(db)?
        .iter()
        .map(|item| {
            json!({
                "name": item.task.name,
                "info": item.task.info,
                "assigned_to": names(&item.assigned_to),
                "for": names(&item.for_people),
                "waiting_on": names(&item.waiting_on),
                "part_of": names(&item.part_of),
            })
        })
        .collect();
    Ok(Value::Array(items).to_string())
}

#[derive(Deserialize)]
struct SaveArgs {
    #[serde(rename = "type")]
    kind: String,
    name: String,
    #[serde(default)]
    info: BTreeMap<String, Option<String>>,
    #[serde(default)]
    roles: Vec<String>,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default)]
    aliases: Vec<String>,
}

/// The user and email addresses are not entities; the model once saved the user by email.
fn check_name(name: &str) -> Result<(), String> {
    if name.contains('@') {
        return Err(format!("\"{name}\" is not a name. The user is never saved, and emails go in info"));
    }
    Ok(())
}

fn save(ctx: &Ctx, args: SaveArgs) -> Result<String, String> {
    let graph = ctx.graph;
    if resolve_kind(&args.kind).is_none() {
        return Err(format!("unknown type {}", args.kind));
    }
    check_name(&args.name)?;
    let mut entity = graph.upsert_entity(&args.kind, &args.name).map_err(db)?;
    ctx.activity.touch(&entity.id);
    // A person's affiliation becomes a link to the organization's page, not text.
    let mut info = args.info.clone();
    let affiliation = match entity.kind.as_str() {
        "Person" => info.remove("affiliation").flatten().filter(|a| !a.trim().is_empty()),
        _ => None,
    };
    if !info.is_empty() {
        entity = graph.update_info(&entity.id, &info).map_err(db)?;
    }
    let mut linked_to = None;
    if let Some(organization) = &affiliation {
        let details = LinkDetails { detail: entity.info.get("position").cloned(), ..Default::default() };
        let org = graph.affiliate(&entity.id, "AFFILIATED_WITH", organization, &details).map_err(db)?;
        ctx.activity.touch(&org.id);
        linked_to = Some(org.name);
        entity = graph.get(&entity.id).map_err(db)?.ok_or("page disappeared")?;
    }
    if !args.aliases.is_empty() {
        let mut aliases = entity.aliases.clone();
        aliases.extend(args.aliases.iter().cloned());
        entity = graph.set_aliases(&entity.id, &aliases).map_err(db)?;
    }
    if let Some(role) = args.roles.iter().find(|r| !ROLES.contains(&r.as_str())) {
        return Err(format!("unknown role {role}; use one of {}", ROLES.join(", ")));
    }
    let tags: Vec<String> = args.roles.iter().chain(&args.tags).cloned().collect();
    if !tags.is_empty() {
        entity = graph.add_tags(&entity.id, &tags).map_err(db)?;
    }
    let mut result = json!({
        "saved": { "name": entity.name, "type": entity.kind, "tags": effective_tags(&entity), "info": entity.info }
    });
    if let Some(org) = linked_to {
        result["affiliation"] = format!("linked to the organization page {org}").into();
    }
    if !entity.aliases.is_empty() {
        result["saved"]["also_called"] = json!(entity.aliases);
    }
    if resolve_kind(&args.kind).map(|(k, _)| k) != Some(entity.kind.as_str()) {
        result["note"] = format!(
            "\"{}\" already exists as a {}; names are unique across types, so that entity was updated.",
            entity.name, entity.kind
        )
        .into();
    }
    Ok(result.to_string())
}

#[derive(Deserialize)]
struct LinkArgs {
    from: String,
    relation: String,
    to: String,
    #[serde(default)]
    from_type: Option<String>,
    #[serde(default)]
    to_type: Option<String>,
    #[serde(default)]
    detail: Option<String>,
    #[serde(default)]
    since: Option<String>,
    #[serde(default)]
    until: Option<String>,
}

fn endpoint(graph: &Graph, name: &str, kind: Option<&str>) -> Result<Entity, String> {
    check_name(name)?;
    if let Some(entity) = graph.find_by_name(name).map_err(db)? {
        return Ok(entity);
    }
    match kind {
        Some(kind) => graph.upsert_entity(kind, name).map_err(db),
        None => Err(format!("no entity named \"{name}\"; save it first or give its type")),
    }
}

fn link(ctx: &Ctx, args: LinkArgs) -> Result<String, String> {
    let graph = ctx.graph;
    if !RELATION_KINDS.contains(&args.relation.as_str()) {
        return Err(format!("unknown relation {}", args.relation));
    }
    let from = endpoint(graph, &args.from, args.from_type.as_deref())?;
    let to = endpoint(graph, &args.to, args.to_type.as_deref())?;
    ctx.activity.touch(&from.id);
    ctx.activity.touch(&to.id);
    if from.id == to.id || !allowed(&args.relation, &from.kind, &to.kind) {
        return Err(format!(
            "{} can't go from a {} to a {}",
            args.relation, from.kind, to.kind
        ));
    }
    let details = LinkDetails { detail: args.detail.clone(), since: args.since.clone(), until: args.until.clone() };
    graph.link_with(&from.id, &args.relation, &to.id, &details).map_err(db)?;
    let fact = sentence(&from.name, &args.relation, &to.name);
    let stored = graph.links(&from.id).map_err(db)?.into_iter().find(|l| l.outgoing && l.kind == args.relation && l.other.id == to.id);
    Ok(match stored.and_then(|l| details_text(l.detail.as_deref(), l.since.as_deref(), l.until.as_deref())) {
        Some(details) => format!("Linked: {fact} ({details})."),
        None => format!("Linked: {fact}."),
    })
}

#[derive(Deserialize)]
struct RenameArgs {
    name: String,
    new_name: String,
}

fn rename(ctx: &Ctx, args: RenameArgs) -> Result<String, String> {
    check_name(&args.new_name)?;
    let entity = ctx
        .graph
        .find_by_name(&args.name)
        .map_err(db)?
        .ok_or_else(|| format!("no page named \"{}\"", args.name))?;
    let renamed = ctx.graph.rename(&entity.id, &args.new_name).map_err(db)?;
    ctx.activity.touch(&renamed.id);
    Ok(format!("Renamed \"{}\" to \"{}\"; everything on the page is kept.", entity.name, renamed.name))
}

fn unlink(ctx: &Ctx, args: LinkArgs) -> Result<String, String> {
    let graph = ctx.graph;
    let (Some(from), Some(to)) = (
        graph.find_by_name(&args.from).map_err(db)?,
        graph.find_by_name(&args.to).map_err(db)?,
    ) else {
        return Ok("No such relationship.".into());
    };
    ctx.activity.touch(&from.id);
    ctx.activity.touch(&to.id);
    Ok(if graph.unlink(&from.id, &args.relation, &to.id).map_err(db)? {
        format!("Removed: {}.", sentence(&from.name, &args.relation, &to.name))
    } else {
        "No such relationship.".into()
    })
}

#[derive(Deserialize)]
struct NoteArgs {
    name: String,
    text: String,
}

fn add_note(ctx: &Ctx, args: NoteArgs) -> Result<String, String> {
    let graph = ctx.graph;
    check_name(&args.name)?;
    let entity = graph
        .find_by_name(&args.name)
        .map_err(db)?
        .ok_or_else(|| format!("no entity named \"{}\"; save it first", args.name))?;
    let text = args.text.split_whitespace().collect::<Vec<_>>().join(" ");
    // The date is added here; Claude sometimes writes one too.
    let text = strip_leading_date(&text);
    let line = format!("- {}: {text}", chrono::Local::now().format("%Y-%m-%d"));
    let existing = entity.notes.trim_end();
    let notes = if existing.is_empty() { line } else { format!("{existing}\n{line}") };
    let entity = graph.set_notes(&entity.id, &notes).map_err(db)?;
    graph.sync_wikilinks(&entity.id, &wikilinks(&entity.notes)).map_err(db)?;
    ctx.activity.touch(&entity.id);
    Ok(format!("Added to {}'s notes.", entity.name))
}

#[derive(Deserialize)]
struct FixNoteArgs {
    name: String,
    old_text: String,
    new_text: String,
}

fn fix_note(ctx: &Ctx, args: FixNoteArgs) -> Result<String, String> {
    let graph = ctx.graph;
    let entity = graph
        .find_by_name(&args.name)
        .map_err(db)?
        .ok_or_else(|| format!("no entity named \"{}\"", args.name))?;
    match entity.notes.matches(args.old_text.as_str()).count() {
        0 => return Err(format!("\"{}\" isn't in {}'s notes; quote it exactly", args.old_text, entity.name)),
        1 => {}
        n => return Err(format!("\"{}\" appears {n} times in {}'s notes; quote more of it", args.old_text, entity.name)),
    }
    let new_text = args.new_text.trim();
    let notes = if args.old_text.contains('\n') {
        entity.notes.replacen(&args.old_text, new_text, 1)
    } else {
        // The line it was on goes when nothing but its bullet and date is left.
        entity
            .notes
            .lines()
            .filter_map(|line| match line.contains(args.old_text.as_str()) {
                false => Some(line.to_string()),
                true => Some(line.replacen(&args.old_text, new_text, 1)).filter(|fixed| !is_empty_note_line(fixed)),
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    let entity = graph.set_notes(&entity.id, &notes).map_err(db)?;
    graph.sync_wikilinks(&entity.id, &wikilinks(&entity.notes)).map_err(db)?;
    ctx.activity.touch(&entity.id);
    Ok(format!("Corrected {}'s notes.", entity.name))
}

/// Whether text starts with a YYYY-MM-DD date.
fn starts_with_date(text: &str) -> bool {
    let bytes = text.as_bytes();
    bytes.len() >= 10 && bytes[..10].iter().enumerate().all(|(i, b)| if i == 4 || i == 7 { *b == b'-' } else { b.is_ascii_digit() })
}

/// A notes line with nothing left but its bullet and date ("- 2026-09-17:").
fn is_empty_note_line(line: &str) -> bool {
    let rest = line.trim().trim_start_matches(['-', '*']).trim();
    let rest = if starts_with_date(rest) { &rest[10..] } else { rest };
    rest.trim_matches([':', ' ', '-', '—']).is_empty()
}

/// "2026-09-17: Met" -> "Met".
fn strip_leading_date(text: &str) -> String {
    if !starts_with_date(text) {
        return text.to_string();
    }
    let rest = text[10..].trim_start_matches([':', ' ', '-', '—']);
    if rest.is_empty() { text.to_string() } else { rest.to_string() }
}

#[derive(Deserialize)]
struct FollowArgs {
    name: String,
    #[serde(default)]
    links: Vec<String>,
    openalex: Option<String>,
    follow: Option<bool>,
}

fn follow(ctx: &Ctx, args: FollowArgs) -> Result<String, String> {
    let graph = ctx.graph;
    check_name(&args.name)?;
    let follow = args.follow.unwrap_or(true);
    for url in &args.links {
        if !ctx.activity.link_allowed(url) {
            return Err(format!("{url} isn't in the user's message; only save links they gave"));
        }
        if !ctx.activity.local_links() {
            crate::links::check_public(url)?;
        }
    }
    if let Some(id) = args.openalex.as_deref().filter(|id| !watch::is_author_id(id)) {
        return Err(format!("{id} isn't an OpenAlex author id; they look like A5051672229"));
    }
    let mut person = match graph.find_by_name(&args.name).map_err(db)? {
        Some(e) if e.kind != "Person" => return Err(format!("{} is a {}, not a person", e.name, e.kind)),
        Some(e) => e,
        None => graph.upsert_entity("Person", &args.name).map_err(db)?,
    };
    ctx.activity.touch(&person.id);
    let mut links = args.links.clone();
    links.extend(args.openalex.iter().map(|id| format!("https://openalex.org/{id}")));
    if !links.is_empty() {
        person = watch::add_links(graph, &person.id, &links)?;
    }
    person = watch::set_following(graph, &person, follow).map_err(db)?;
    let info = watch::watch_info(graph, &person).map_err(db)?;
    let mut result = json!({
        "person": person.name,
        "following": info.following,
        "checked": info.sources.iter().map(|s| format!("{} ({})", s.label, s.url)).collect::<Vec<_>>(),
        "not_checkable": info.unsupported,
    });
    if follow && !info.has_papers {
        let name = person.info.get("full_name").cloned().unwrap_or_else(|| person.name.clone());
        match watch::author_candidates(ctx.fetch, &name) {
            Ok(candidates) if !candidates.is_empty() => {
                result["openalex_candidates"] = json!(candidates);
                result["next"] = "Papers can't be checked until the right OpenAlex author is chosen. Ask the user which one it is and wait for the answer, unless exactly one candidate's institution matches an affiliation the user gave or that is saved for this person (not your own knowledge). Then call follow again with openalex.".into();
            }
            Ok(_) => result["papers"] = "No OpenAlex author found under this name, so papers won't be checked.".into(),
            Err(e) => result["papers"] = format!("Couldn't look up their papers: {e}").into(),
        }
    }
    Ok(result.to_string())
}

#[derive(Deserialize)]
struct UpdatesArgs {
    name: Option<String>,
    #[serde(default)]
    all: bool,
}

fn updates(graph: &Graph, args: UpdatesArgs) -> Result<String, String> {
    let person = match &args.name {
        Some(name) => Some(graph.find_by_name(name).map_err(db)?.ok_or_else(|| format!("no page named {name}"))?),
        None => None,
    };
    let query = crate::graph::ActivityQuery {
        person: person.as_ref().map(|p| p.id.clone()),
        relevant_only: !args.all,
        limit: 30,
        ..Default::default()
    };
    let mut names = BTreeMap::new();
    let mut items = Vec::new();
    for a in graph.activities(&query).map_err(db)? {
        if !args.all && a.baseline {
            continue;
        }
        if !names.contains_key(&a.person) {
            let name = graph.get(&a.person).map_err(db)?.map(|p| p.name).unwrap_or_default();
            names.insert(a.person.clone(), name);
        }
        let mut item = json!({
            "person": names[&a.person],
            "kind": a.kind,
            "title": a.title,
            "published": a.published,
            "url": a.url,
            "summary": a.summary.chars().take(300).collect::<String>(),
            "relevance": if a.relevance.is_empty() { "not judged yet" } else { a.relevance.as_str() },
        });
        if !a.reason.is_empty() {
            item["reason"] = a.reason.clone().into();
        }
        if !a.related.is_empty() {
            item["related"] = json!(a.related);
        }
        items.push(item);
    }
    let following: Vec<String> = graph.entities_with_tag(watch::FOLLOWING).map_err(db)?.into_iter().map(|p| p.name).collect();
    Ok(json!({ "following": following, "updates": items }).to_string())
}

#[derive(Deserialize)]
struct LinkReadArgs {
    url: String,
}

#[derive(Deserialize)]
struct SectionArgs {
    title: String,
    tag: String,
}

#[derive(Deserialize)]
struct ProposeArgs {
    #[serde(default)]
    summary: Option<String>,
    items: Vec<ScheduleItem>,
}

fn propose(proposals: &Proposals, args: ProposeArgs) -> Result<String, String> {
    if args.items.is_empty() {
        return Err("propose at least one item".into());
    }
    for item in &args.items {
        let parse = |t: &str| {
            NaiveDateTime::parse_from_str(t, TIME_FORMAT)
                .map_err(|_| format!("\"{t}\" must be local time as YYYY-MM-DDTHH:MM"))
        };
        if item.title.trim().is_empty() {
            return Err("every item needs a title".into());
        }
        if parse(&item.end)? <= parse(&item.start)? {
            return Err(format!("\"{}\" ends before it starts", item.title));
        }
    }
    let proposal = proposals.add(args.summary, args.items);
    Ok(format!(
        "The app shows these blocks as proposal {} below your reply, with a Confirm button. Don't add anything to the calendar now. In your reply, give the ranked priorities as asked, then one short sentence asking them to confirm the blocks; don't list the blocks again.",
        proposal.id
    ))
}

#[derive(Deserialize)]
struct ApproveArgs {
    tool_name: String,
    #[serde(default)]
    input: Value,
}

/// Answers Claude Code's permission check for a tool that isn't pre-approved.
fn approve(proposals: &Proposals, args: ApproveArgs) -> Value {
    let deny = |message: &str| json!({ "behavior": "deny", "message": message });
    let allow = || json!({ "behavior": "allow", "updatedInput": args.input });
    let Some(action) = args.tool_name.strip_prefix(CALENDAR_PREFIX) else {
        eprintln!("claude: denied {}", args.tool_name);
        return deny("Suk only allows its own tools and reading Google Calendar.");
    };
    match calendar_access(action) {
        CalendarAccess::Read => allow(),
        CalendarAccess::Create if proposals.confirmed_item_for(&args.input).is_some() => allow(),
        CalendarAccess::Create => deny(
            "Only items the user confirmed can be added, using their exact title and start time. Use propose_schedule first.",
        ),
        CalendarAccess::Other => {
            eprintln!("claude: denied {}", args.tool_name);
            deny("Suk doesn't allow changing or deleting calendar events.")
        }
    }
}

#[derive(Debug, PartialEq)]
pub enum CalendarAccess {
    Read,
    Create,
    Other,
}

/// Classifies a calendar tool by its name, e.g. "list_events" or "create_event".
pub fn calendar_access(action: &str) -> CalendarAccess {
    let verb = action.split('_').next().unwrap_or_default();
    match verb {
        "list" | "get" | "search" | "find" | "query" | "suggest" => CalendarAccess::Read,
        "create" | "insert" | "add" | "quick" => CalendarAccess::Create,
        _ if action.contains("free") || action.contains("busy") => CalendarAccess::Read,
        _ => CalendarAccess::Other,
    }
}

/// Stores confirmed items as Events linked to their tasks, so Today shows them without Claude.
pub fn record_confirmed(graph: &Graph, proposal: &Proposal) -> Result<(), GraphError> {
    for (item, state) in proposal.items.iter().zip(&proposal.states) {
        if *state != ItemState::Confirmed {
            continue;
        }
        let when = item.start.replace('T', " ");
        let event = graph.upsert_entity("Event", &format!("{} ({when})", item.title))?;
        let mut info = BTreeMap::new();
        info.insert("start".to_string(), Some(item.start.clone()));
        info.insert("end".to_string(), Some(item.end.clone()));
        info.insert("title".to_string(), Some(item.title.clone()));
        info.insert("notes".to_string(), item.notes.clone());
        graph.update_info(&event.id, &info)?;
        if let Some(task) = item.task.as_deref() {
            if let Some(task) = graph.find_by_name(task)?.filter(|t| t.kind == "Task") {
                graph.link(&event.id, "SCHEDULED_FOR", &task.id)?;
            }
        }
    }
    Ok(())
}

/// Marks a stored Event as added to the calendar.
pub fn record_added(graph: &Graph, item: &ScheduleItem) -> Result<(), GraphError> {
    let when = item.start.replace('T', " ");
    if let Some(event) = graph.find_by_name(&format!("{} ({when})", item.title))? {
        let info = BTreeMap::from([("calendar".to_string(), Some("added".to_string()))]);
        graph.update_info(&event.id, &info)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup() -> (Graph, Proposals) {
        (Graph::in_memory().unwrap(), Proposals::default())
    }

    fn run(g: &Graph, p: &Proposals, name: &str, args: Value) -> Result<Value, String> {
        run_with(g, p, &Activity::default(), name, args)
    }

    fn run_with(g: &Graph, p: &Proposals, a: &Activity, name: &str, args: Value) -> Result<Value, String> {
        run_fetching(g, p, a, &Offline, name, args)
    }

    fn run_fetching(g: &Graph, p: &Proposals, a: &Activity, fetch: &dyn watch::Fetch, name: &str, args: Value) -> Result<Value, String> {
        let ctx = Ctx { graph: g, proposals: p, activity: a, fetch };
        call(&ctx, name, args).map(|text| serde_json::from_str(&text).unwrap_or(Value::String(text)))
    }

    struct Offline;

    impl watch::Fetch for Offline {
        fn get(&self, _: &str) -> Result<String, String> {
            Err("offline".into())
        }
    }

    /// Answers OpenAlex author searches from a saved response.
    struct Authors;

    impl watch::Fetch for Authors {
        fn get(&self, url: &str) -> Result<String, String> {
            assert!(url.starts_with("https://api.openalex.org/authors?search=Andreas%20Zeller"), "{url}");
            Ok(include_str!("fixtures/openalex_authors.json").into())
        }
    }

    #[test]
    fn wrong_notes_are_corrected_or_removed() {
        let (g, p) = setup();
        let a = Activity::default();
        run(&g, &p, "save", json!({"type": "Task", "name": "Follow up on email"})).unwrap();
        run(&g, &p, "add_note", json!({"name": "Follow up on email", "text": "Sent as her Faculty Advisor"})).unwrap();
        run(&g, &p, "add_note", json!({"name": "Follow up on email", "text": "Reply by Friday to [[Kavya Rao]]"})).unwrap();
        let fix = |old: &str, new: &str| run_with(&g, &p, &a, "fix_note", json!({"name": "Follow up on email", "old_text": old, "new_text": new}));
        assert!(fix("Faculty Adviser", "").unwrap_err().contains("isn't in"));
        assert!(fix("Sent", "x").is_ok());
        assert!(fix("x as her", "").is_ok());
        let notes = g.find_by_name("Follow up on email").unwrap().unwrap().notes;
        assert!(notes.contains("Faculty Advisor") && !notes.contains("Sent") && notes.contains("Reply by Friday"), "{notes}");
        fix("Faculty Advisor", "").unwrap();
        let notes = g.find_by_name("Follow up on email").unwrap().unwrap().notes;
        assert_eq!(notes.lines().count(), 1, "{notes}");
        assert!(notes.contains("Reply by Friday to [[Kavya Rao]]"));
        fix("Friday", "Monday").unwrap();
        assert!(g.find_by_name("Follow up on email").unwrap().unwrap().notes.contains("Reply by Monday"));
        assert!(a.take().0.contains(&"task:follow up on email".to_string()));
    }

    #[test]
    fn projects_are_listed_by_status() {
        let (g, p) = setup();
        run(&g, &p, "save", json!({"type": "Project", "name": "Solidity Compiler Fuzzing", "info": {"status": "in progress"}})).unwrap();
        run(&g, &p, "save", json!({"type": "Project", "name": "LLM Static Analysis", "info": {"status": "planned"}})).unwrap();
        run(&g, &p, "save", json!({"type": "Project", "name": "Grammar Inference", "info": {"status": "completed"}})).unwrap();
        let err = run(&g, &p, "save", json!({"type": "Project", "name": "Grammar Inference", "info": {"status": "paused"}})).unwrap_err();
        assert!(err.contains("planned, in progress or completed"), "{err}");
        let names = |status: Value| -> Vec<String> {
            let mut args = json!({"type": "Project"});
            if !status.is_null() { args["status"] = status; }
            run(&g, &p, "list", args).unwrap().as_array().unwrap().iter().map(|e| e["name"].as_str().unwrap().to_string()).collect()
        };
        assert_eq!(names(json!("in-progress")), vec!["Solidity Compiler Fuzzing"]);
        assert_eq!(names(json!("completed")), vec!["Grammar Inference"]);
        assert_eq!(names(Value::Null).len(), 3);
    }

    #[test]
    fn following_people_and_listing_their_updates() {
        let (g, p) = setup();
        let a = Activity::default();
        let x = "https://x.com/AndreasZeller";
        let home = "https://andreas-zeller.info/";
        let err = run_with(&g, &p, &a, "follow", json!({"name": "Andreas Zeller", "links": [x]})).unwrap_err();
        assert!(err.contains("only save links they gave"), "{err}");

        a.begin(vec![x.into(), home.into()]);
        let result = run_fetching(&g, &p, &a, &Authors, "follow", json!({"name": "Andreas Zeller", "links": [x, home]})).unwrap();
        assert_eq!(result["following"], true);
        assert_eq!(result["checked"], json!(["Homepage (https://andreas-zeller.info/)"]));
        assert_eq!(result["not_checkable"], json!(["X"]));
        assert_eq!(result["openalex_candidates"][0]["id"], "A5051672229");
        assert_eq!(result["openalex_candidates"][1]["institutions"][0], "Roche (Switzerland)");
        assert!(result["next"].as_str().unwrap().contains("Ask the user which one"));
        let person = g.find_by_name("Andreas Zeller").unwrap().unwrap();
        assert_eq!((person.info["twitter"].as_str(), person.info["homepage"].as_str()), (x, home));
        assert!(a.take().0.contains(&person.id));

        assert!(run_with(&g, &p, &a, "follow", json!({"name": "Andreas Zeller", "openalex": "W123"})).is_err());
        let result = run_with(&g, &p, &a, "follow", json!({"name": "Andreas Zeller", "openalex": "A5051672229"})).unwrap();
        assert_eq!(result["checked"][0], "Papers (https://openalex.org/A5051672229)");
        assert!(result.get("openalex_candidates").is_none() && result.get("papers").is_none());

        // Offline lookups are reported, not failures.
        run_with(&g, &p, &a, "save", json!({"type": "Person", "name": "Kavya Rao"})).unwrap();
        let result = run_with(&g, &p, &a, "follow", json!({"name": "Kavya Rao"})).unwrap();
        assert_eq!(result["papers"], "Couldn't look up their papers: offline");
        run_with(&g, &p, &a, "save", json!({"type": "Project", "name": "Fuzzing"})).unwrap();
        assert!(run_with(&g, &p, &a, "follow", json!({"name": "Fuzzing"})).unwrap_err().contains("not a person"));

        let base = crate::graph::Activity {
            id: "w1".into(),
            person: person.id.clone(),
            source: "openalex".into(),
            kind: "paper".into(),
            title: "Old paper".into(),
            url: "https://doi.org/1".into(),
            summary: String::new(),
            published: "2025-01-01".into(),
            found_at: 1,
            baseline: true,
            relevance: String::new(),
            reason: String::new(),
            related: vec![],
            seen: false,
            notified: false,
        };
        g.add_activity(&base).unwrap();
        let new = crate::graph::Activity { id: "w2".into(), title: "Fuzzing solc".into(), published: "2026-09-15".into(), baseline: false, ..base.clone() };
        g.add_activity(&new).unwrap();
        g.judge_activity("w2", "high", "Same topic as your Fuzzing project", &["Fuzzing".into()]).unwrap();
        let result = run_with(&g, &p, &a, "updates", json!({})).unwrap();
        assert_eq!(result["following"], json!(["Andreas Zeller", "Kavya Rao"]));
        assert_eq!(result["updates"].as_array().unwrap().len(), 1);
        assert_eq!(result["updates"][0]["reason"], "Same topic as your Fuzzing project");
        assert_eq!(result["updates"][0]["person"], "Andreas Zeller");
        let all = run_with(&g, &p, &a, "updates", json!({"name": "Andreas Zeller", "all": true})).unwrap();
        assert_eq!(all["updates"][1]["relevance"], "not judged yet");

        let result = run_with(&g, &p, &a, "follow", json!({"name": "Kavya Rao", "follow": false})).unwrap();
        assert_eq!(result["following"], false);
    }

    #[test]
    fn tags_notes_sections_and_activity() {
        let (g, p) = setup();
        let a = Activity::default();
        let saved = run_with(&g, &p, &a, "save", json!({"type": "Student", "name": "Satya", "tags": ["PhD", "student"]})).unwrap();
        assert_eq!(saved["saved"]["tags"], json!(["person", "student", "phd"]));
        run_with(&g, &p, &a, "link", json!({"from": "Satya", "relation": "WORKS_ON", "to": "Fuzzing", "to_type": "Project"})).unwrap();
        run_with(&g, &p, &a, "add_note", json!({"name": "satya", "text": "Wants to target  [[Fuzzing]] on LLVM"})).unwrap();
        run_with(&g, &p, &a, "add_note", json!({"name": "Satya", "text": "2026-01-02: Prefers Tuesday meetings"})).unwrap();
        assert!(run_with(&g, &p, &a, "add_note", json!({"name": "Nobody", "text": "x"})).is_err());
        run_with(&g, &p, &a, "suggest_section", json!({"title": "PhD students", "tag": "phd"})).unwrap();
        assert!(run(&g, &p, "link", json!({"from": "Satya", "relation": "LINKS_TO", "to": "Fuzzing"})).is_err());

        let satya = g.find_by_name("Satya").unwrap().unwrap();
        let today = chrono::Local::now().format("%Y-%m-%d");
        assert_eq!(satya.notes, format!("- {today}: Wants to target [[Fuzzing]] on LLVM\n- {today}: Prefers Tuesday meetings"));
        assert!(g.links(&satya.id).unwrap().iter().any(|l| l.kind == "LINKS_TO"));
        let (touched, suggested) = a.take();
        assert_eq!(touched, vec!["person:satya", "project:fuzzing"]);
        assert_eq!(suggested, vec![("phd".to_string(), "PhD students".to_string())]);
        assert_eq!(a.take(), (vec![], vec![]));

        let found = run(&g, &p, "find", json!({"names": ["Satya"]})).unwrap();
        assert!(found[0]["notes"].as_str().unwrap().contains("Tuesday"));
        let listed = run(&g, &p, "list", json!({"type": "Student"})).unwrap();
        assert_eq!(listed[0]["tags"], json!(["person", "student", "phd"]));
        let names: Vec<_> = definitions().as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap().to_string()).collect();
        assert!(names.contains(&"add_note".to_string()) && names.contains(&"suggest_section".to_string()));
        assert!(!definitions().to_string().contains("LINKS_TO"));

        // Links: only those the user gave in this turn.
        let err = run_with(&g, &p, &a, "read_link", json!({"url": "https://example.org/profile"})).unwrap_err();
        assert!(err.contains("Only links the user pasted"));
        a.begin(vec!["http://127.0.0.1:9/profile/".into()]);
        let err = run_with(&g, &p, &a, "read_link", json!({"url": "http://127.0.0.1:9/profile"})).unwrap_err();
        assert!(err.contains("this computer"), "{err}");
    }

    #[test]
    fn save_link_find_and_list() {
        let (g, p) = setup();
        run(&g, &p, "save", json!({"type": "Student", "name": "Amit", "info": {"email": "amit@example.edu", "program": "PhD"}})).unwrap();
        run(&g, &p, "save", json!({"type": "Student", "name": "amit", "info": {"program": null, "year": "2"}})).unwrap();
        run(&g, &p, "link", json!({"from": "Amit", "relation": "WORKS_ON", "to": "Compiler Fuzzing", "to_type": "Project"})).unwrap();
        run(&g, &p, "link", json!({"from": "Compiler Fuzzing", "relation": "COLLABORATES_WITH", "to": "Prof Sharma", "to_type": "Person"})).unwrap();

        let found = run(&g, &p, "find", json!({"names": ["amit", "Nobody"]})).unwrap();
        assert_eq!(
            found,
            json!([
                {"name": "Amit", "type": "Person", "tags": ["person", "student"], "info": {"email": "amit@example.edu", "year": "2"},
                 "relationships": ["Amit works on Compiler Fuzzing", "Compiler Fuzzing collaborates with Prof Sharma"]},
                {"query": "Nobody", "found": false}
            ])
        );
        let partial = run(&g, &p, "find", json!({"names": ["the fuzzing project"]})).unwrap();
        assert_eq!(partial[0]["name"], "Compiler Fuzzing");
        assert!(partial[0]["note"].as_str().unwrap().contains("closest match"));
        let people = run(&g, &p, "list", json!({"type": "Person"})).unwrap();
        let sharma = people.as_array().unwrap().iter().find(|p| p["name"] == "Prof Sharma").unwrap();
        assert_eq!(sharma["relationships"], json!(["Compiler Fuzzing collaborates with Prof Sharma"]));
    }

    #[test]
    fn tasks_tool_filters_in_the_database_and_shows_people() {
        let (g, p) = setup();
        run(&g, &p, "save", json!({"type": "Person", "name": "Satya", "roles": ["student"]})).unwrap();
        run(&g, &p, "save", json!({"type": "Task", "name": "Review Satya's survey", "info": {"due": "2026-09-18", "priority": "high"}})).unwrap();
        run(&g, &p, "link", json!({"from": "Review Satya's survey", "relation": "FOR", "to": "Satya"})).unwrap();
        run(&g, &p, "save", json!({"type": "Task", "name": "NBA report", "info": {"due": "2026-09-25", "status": "open"}})).unwrap();
        run(&g, &p, "save", json!({"type": "Task", "name": "Old", "info": {"status": "done"}})).unwrap();
        let bad = run(&g, &p, "save", json!({"type": "Task", "name": "Grant", "info": {"due": "Friday"}}));
        assert!(bad.unwrap_err().contains("YYYY-MM-DD"));
        assert!(run(&g, &p, "link", json!({"from": "Satya", "relation": "FOR", "to": "NBA report"})).is_err(), "FOR goes from a task");

        let open = run(&g, &p, "tasks", json!({})).unwrap();
        assert_eq!(open.as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect::<Vec<_>>(), vec!["Review Satya's survey", "NBA report", "Grant"]);
        assert_eq!(open[0]["for"], json!(["Satya"]));
        let soon = run(&g, &p, "tasks", json!({"due_by": "2026-09-20"})).unwrap();
        assert_eq!(soon.as_array().unwrap().len(), 1);
        assert_eq!(run(&g, &p, "tasks", json!({"about": "satya"})).unwrap().as_array().unwrap().len(), 1);
        assert_eq!(run(&g, &p, "tasks", json!({"status": "done"})).unwrap()[0]["name"], "Old");
        assert!(run(&g, &p, "tasks", json!({"about": "Nobody"})).is_err());

        run(&g, &p, "save", json!({"type": "Person", "name": "Rohan", "roles": ["student"]})).unwrap();
        run(&g, &p, "link", json!({"from": "NBA report", "relation": "ASSIGNED_TO", "to": "Rohan"})).unwrap();
        let others = run(&g, &p, "tasks", json!({"assigned": "others"})).unwrap();
        assert_eq!(others.as_array().unwrap().len(), 1);
        assert_eq!(others[0]["assigned_to"], json!(["Rohan"]));
        let mine = run(&g, &p, "tasks", json!({"assigned": "mine"})).unwrap();
        assert!(mine.as_array().unwrap().iter().all(|t| t["name"] != "NBA report"));
        assert!(run(&g, &p, "tasks", json!({"assigned": "theirs"})).is_err());
    }

    #[test]
    fn organizations_through_tools() {
        let (g, p) = setup();
        let a = Activity::default();
        run(&g, &p, "save", json!({"type": "Organization", "name": "IIT Guwahati", "aliases": ["IITG"], "info": {"type": "university", "city": "Guwahati"}})).unwrap();
        run(&g, &p, "save", json!({"type": "Organization", "name": "CSE, IIT Guwahati", "aliases": ["IITG CSE"]})).unwrap();
        run(&g, &p, "link", json!({"from": "IITG CSE", "relation": "PART_OF", "to": "IITG"})).unwrap();

        // An affiliation given as a detail becomes a link, to the page its other name finds.
        let saved = run_with(&g, &p, &a, "save", json!({"type": "Person", "name": "Satya", "roles": ["student"], "info": {"position": "PhD scholar", "affiliation": "IITG CSE"}})).unwrap();
        assert_eq!(saved["affiliation"], "linked to the organization page CSE, IIT Guwahati");
        assert_eq!(saved["saved"]["info"]["affiliation"], "CSE, IIT Guwahati");
        let (touched, _) = a.take();
        assert!(touched.contains(&"organization:cse, iit guwahati".to_string()));

        // A new organization is created once.
        run(&g, &p, "save", json!({"type": "Person", "name": "Kavya Rao", "info": {"affiliation": "IISc"}})).unwrap();
        assert_eq!(g.find_by_name("IISc").unwrap().unwrap().kind, "Organization");
        let linked = run(&g, &p, "link", json!({"from": "Kavya Rao", "relation": "STUDIED_AT", "to": "IITG", "detail": "MTech", "until": "2015"})).unwrap();
        assert_eq!(linked, json!("Linked: Kavya Rao studied at IIT Guwahati (MTech, until 2015)."));
        assert!(run(&g, &p, "link", json!({"from": "Kavya Rao", "relation": "STUDIED_AT", "to": "IITG", "since": "long ago"})).is_err());

        let at = run(&g, &p, "people_at", json!({"organization": "iitg"})).unwrap();
        assert_eq!(at["organization"], "IIT Guwahati");
        assert_eq!(at["people"].as_array().unwrap().iter().map(|x| x["name"].as_str().unwrap()).collect::<Vec<_>>(), vec!["Satya"]);
        assert_eq!(at["people"][0]["where"], "CSE, IIT Guwahati");
        assert_eq!(at["people"][0]["details"], "PhD scholar");
        let all = run(&g, &p, "people_at", json!({"organization": "IITG", "include_past": true})).unwrap();
        assert_eq!(all["people"].as_array().unwrap().len(), 2);
        let students = run(&g, &p, "people_at", json!({"organization": "IITG", "include_past": true, "role": "student"})).unwrap();
        assert_eq!(students["people"].as_array().unwrap().len(), 1);
        assert!(run(&g, &p, "people_at", json!({"organization": "Satya"})).is_err());

        let found = run(&g, &p, "find", json!({"names": ["IITG CSE"]})).unwrap();
        assert_eq!(found[0]["also_called"], json!(["IITG CSE"]));
        assert!(found[0]["relationships"].as_array().unwrap().iter().any(|r| r == "Satya is at CSE, IIT Guwahati (PhD scholar)"));
    }

    #[test]
    fn rename_keeps_the_page() {
        let (g, p) = setup();
        let a = Activity::default();
        run_with(&g, &p, &a, "save", json!({"type": "Person", "name": "Tenzin", "roles": ["student"], "info": {"full_name": "Tenzin Norbu Bhutia"}})).unwrap();
        run_with(&g, &p, &a, "link", json!({"from": "Tenzin", "relation": "WORKS_ON", "to": "Fuzzing", "to_type": "Project"})).unwrap();
        let before = g.find_by_name("Tenzin").unwrap().unwrap();

        let reply = run_with(&g, &p, &a, "rename", json!({"name": "tenzin", "new_name": "Tenzin Norbu Bhutia"})).unwrap();
        assert!(reply.as_str().unwrap().contains("everything on the page is kept"));
        let after = g.find_by_name("Tenzin Norbu Bhutia").unwrap().unwrap();
        assert_eq!(after.id, before.id);
        assert_eq!(after.info, before.info);
        assert_eq!(after.tags, vec!["student"]);
        assert_eq!(g.links(&after.id).unwrap().len(), 1);
        assert!(g.find_by_name("Tenzin").unwrap().is_none());
        assert_eq!(g.entities_of_kind("Person").unwrap().len(), 1, "no second page");

        assert!(run(&g, &p, "rename", json!({"name": "Nobody", "new_name": "X"})).is_err());
        assert!(run(&g, &p, "rename", json!({"name": "Tenzin Norbu Bhutia", "new_name": "Fuzzing"})).unwrap_err().contains("already exists"));
    }

    #[test]
    fn invalid_calls_are_errors_for_claude() {
        let (g, p) = setup();
        assert!(run(&g, &p, "save", json!({"type": "Robot", "name": "R2"})).is_err());
        assert!(run(&g, &p, "save", json!({"type": "Student", "name": "A", "info": {"E-mail": "x"}})).is_err());
        let missing = run(&g, &p, "link", json!({"from": "Amit", "relation": "WORKS_ON", "to": "Fuzzing"}));
        assert!(missing.unwrap_err().contains("save it first"));
        run(&g, &p, "save", json!({"type": "Task", "name": "Grade exams"})).unwrap();
        let wrong = run(&g, &p, "link", json!({"from": "Grade exams", "relation": "SUPERVISES", "to": "Bo", "to_type": "Student"}));
        assert!(wrong.unwrap_err().contains("can't go from a Task"));
        assert!(run(&g, &p, "drop_database", json!({})).is_err());
        let the_user = run(&g, &p, "link", json!({"from": "prof@example.edu", "from_type": "Person", "relation": "SUPERVISES", "to": "Bo", "to_type": "Student"}));
        assert!(the_user.unwrap_err().contains("never saved"));
        assert!(g.find_by_name("prof@example.edu").unwrap().is_none());
    }

    #[test]
    fn save_reports_an_existing_name_of_another_type() {
        let (g, p) = setup();
        run(&g, &p, "save", json!({"type": "Project", "name": "LLVM"})).unwrap();
        let saved = run(&g, &p, "save", json!({"type": "ResearchArea", "name": "llvm"})).unwrap();
        assert_eq!(saved["saved"]["type"], "Project");
        assert!(saved["note"].as_str().unwrap().contains("already exists as a Project"));
    }

    fn item(title: &str, start: &str, end: &str) -> Value {
        json!({"title": title, "start": start, "end": end})
    }

    #[test]
    fn proposals_validate_times() {
        let (g, p) = setup();
        let bad = run(&g, &p, "propose_schedule", json!({"items": [item("Write", "tomorrow 10am", "2026-09-17T11:00")]}));
        assert!(bad.unwrap_err().contains("YYYY-MM-DDTHH:MM"));
        let backwards = run(&g, &p, "propose_schedule", json!({"items": [item("Write", "2026-09-17T11:00", "2026-09-17T10:00")]}));
        assert!(backwards.unwrap_err().contains("ends before"));
        assert_eq!(p.count(), 0);
        run(&g, &p, "propose_schedule", json!({"items": [item("Write", "2026-09-17T10:00", "2026-09-17T11:00")]})).unwrap();
        assert_eq!(p.since(0)[0].id, "p1");
    }

    fn approve_call(p: &Proposals, tool: &str, input: Value) -> String {
        let reply = approve(p, parse(json!({"tool_name": tool, "input": input})).unwrap());
        reply["behavior"].as_str().unwrap().to_string()
    }

    #[test]
    fn calendar_writes_need_a_confirmed_matching_item() {
        let (g, p) = setup();
        let create = format!("{CALENDAR_PREFIX}create_event");
        let event = json!({"summary": "Write \"grant\" report", "start": {"dateTime": "2026-09-17T10:00:00+05:30"}});

        assert_eq!(approve_call(&p, &format!("{CALENDAR_PREFIX}list_events"), json!({})), "allow");
        assert_eq!(approve_call(&p, &create, event.clone()), "deny");
        assert_eq!(approve_call(&p, &format!("{CALENDAR_PREFIX}delete_event"), json!({})), "deny");
        assert_eq!(approve_call(&p, "mcp__claude_ai_Gmail__search_threads", json!({})), "deny");
        assert_eq!(approve_call(&p, "Bash", json!({"command": "ls"})), "deny");

        run(&g, &p, "propose_schedule", json!({"items": [
            item("Write \"grant\" report", "2026-09-17T10:00", "2026-09-17T11:00"),
            item("Review thesis", "2026-09-17T14:00", "2026-09-17T15:00"),
        ]})).unwrap();
        // Proposed but not confirmed.
        assert_eq!(approve_call(&p, &create, event.clone()), "deny");
        assert!(p.confirm("p1", &[]).is_err());
        assert!(p.confirm("p1", &[5]).is_err());
        let confirmed = p.confirm("p1", &[0]).unwrap();
        assert_eq!(confirmed.states, vec![ItemState::Confirmed, ItemState::Skipped]);
        assert!(p.dismiss("p1").is_err(), "already answered");

        assert_eq!(approve_call(&p, &create, event.clone()), "allow");
        // Same title at a different time, and the item left out, stay denied.
        let moved = json!({"summary": "Write \"grant\" report", "start": {"dateTime": "2026-09-17T16:00:00+05:30"}});
        assert_eq!(approve_call(&p, &create, moved), "deny");
        let skipped = json!({"summary": "Review thesis", "start": {"dateTime": "2026-09-17T14:00:00+05:30"}});
        assert_eq!(approve_call(&p, &create, skipped), "deny");
        // Once added, a second copy is denied.
        p.mark_added("p1", 0);
        assert_eq!(approve_call(&p, &create, event), "deny");
    }

    #[test]
    fn confirmed_items_become_events_linked_to_tasks() {
        let (g, p) = setup();
        run(&g, &p, "save", json!({"type": "Task", "name": "Finish grant report"})).unwrap();
        let mut with_task = item("Grant report", "2026-09-17T10:00", "2026-09-17T11:30");
        with_task["task"] = "finish grant report".into();
        run(&g, &p, "propose_schedule", json!({"items": [with_task, item("Office hours", "2026-09-17T15:00", "2026-09-17T16:00")]})).unwrap();
        let proposal = p.confirm("p1", &[0]).unwrap();
        record_confirmed(&g, &proposal).unwrap();

        let events = g.entities_of_kind("Event").unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].name, "Grant report (2026-09-17 10:00)");
        assert_eq!(events[0].info["end"], "2026-09-17T11:30");
        let links = g.links(&events[0].id).unwrap();
        assert_eq!((links[0].kind.as_str(), links[0].other.name.as_str()), ("SCHEDULED_FOR", "Finish grant report"));

        record_added(&g, &proposal.items[0]).unwrap();
        assert_eq!(g.entities_of_kind("Event").unwrap()[0].info["calendar"], "added");
    }

    #[test]
    fn calendar_tool_names_are_classified() {
        for (name, access) in [
            ("list_events", CalendarAccess::Read),
            ("get_event", CalendarAccess::Read),
            ("find_free_time", CalendarAccess::Read),
            ("list_calendars", CalendarAccess::Read),
            ("create_event", CalendarAccess::Create),
            ("update_event", CalendarAccess::Other),
            ("delete_event", CalendarAccess::Other),
            ("respond_to_event", CalendarAccess::Other),
        ] {
            assert_eq!(calendar_access(name), access, "{name}");
        }
    }
}
