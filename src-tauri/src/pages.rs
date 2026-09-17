//! Pages: finding them (global search), the [[wikilinks]] in their notes, and what the app offers
//! after a chat turn (sidebar sections for new kinds of things, a form for missing details).

use serde::Serialize;

use std::collections::BTreeMap;

use crate::graph::{
    effective_tags, normalize_tag, role_of, section_title, Entity, Graph, GraphError, TaskQuery, ROLES,
};

/// Names linked with [[Name]] or [[Name|shown text]], in order, without duplicates.
pub fn wikilinks(text: &str) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find("[[") {
        rest = &rest[start + 2..];
        let Some(end) = rest.find("]]") else { break };
        let inner = &rest[..end];
        rest = &rest[end + 2..];
        let name = inner.split('|').next().unwrap_or_default().trim();
        if !name.is_empty() && !inner.contains('\n') && !names.iter().any(|n| n.eq_ignore_ascii_case(name)) {
            names.push(name.to_string());
        }
    }
    names
}

#[derive(Debug, Serialize)]
pub struct Hit {
    pub entity: Entity,
    /// Where the query matched: "name", an info key such as "email", "tag" or "notes".
    pub field: String,
    /// The matching text, when it isn't the name.
    pub snippet: Option<String>,
}

/// Entities matching `query` in their name, details, tags or notes; best matches first.
pub fn search(graph: &Graph, query: &str, limit: usize) -> Result<Vec<Hit>, GraphError> {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return Ok(Vec::new());
    }
    let mut scored: Vec<(u32, Hit)> = graph
        .all_entities()?
        .into_iter()
        .filter_map(|entity| score(&entity, &q).map(|(score, field, snippet)| (score, Hit { entity, field, snippet })))
        .collect();
    scored.sort_by(|(a, x), (b, y)| {
        b.cmp(a)
            .then(y.entity.updated_at.cmp(&x.entity.updated_at))
            .then(x.entity.name.to_lowercase().cmp(&y.entity.name.to_lowercase()))
    });
    Ok(scored.into_iter().take(limit).map(|(_, hit)| hit).collect())
}

fn score(entity: &Entity, q: &str) -> Option<(u32, String, Option<String>)> {
    let name = entity.name.to_lowercase();
    let name_score = if name == q {
        100
    } else if name.starts_with(q) {
        90
    } else if name.split(|c: char| !c.is_alphanumeric()).any(|w| w.starts_with(q)) {
        80
    } else if name.contains(q) {
        70
    } else {
        0
    };
    if name_score > 0 {
        return Some((name_score, "name".into(), None));
    }
    // Another name (IITG) finds the page almost as well as its own.
    if let Some(alias) = entity.aliases.iter().find(|a| {
        let a = a.to_lowercase();
        a == q || a.starts_with(q) || a.split(|c: char| !c.is_alphanumeric()).any(|w| w.starts_with(q))
    }) {
        let exact = alias.to_lowercase() == q;
        return Some((if exact { 95 } else { 78 }, "alias".into(), Some(alias.clone())));
    }
    // A full name counts almost like the name itself.
    for (key, value) in &entity.info {
        if value.to_lowercase().contains(q) {
            let score = if key == "full_name" { 75 } else { 50 };
            return Some((score, key.clone(), Some(value.clone())));
        }
    }
    if let Some(tag) = effective_tags(entity).into_iter().find(|t| t.contains(&q.replace(' ', "-"))) {
        return Some((45, "tag".into(), Some(format!("#{tag}"))));
    }
    let notes = entity.notes.to_lowercase();
    notes.find(q).map(|at| (30, "notes".into(), Some(snippet(&entity.notes, at, q.len()))))
}

/// About 80 characters of `text` around the match at byte `at`.
fn snippet(text: &str, at: usize, len: usize) -> String {
    // Lowercasing can shift byte offsets for some scripts; clamp to character boundaries.
    let floor = |mut i: usize| {
        i = i.min(text.len());
        while !text.is_char_boundary(i) {
            i -= 1;
        }
        i
    };
    let mut start = floor(at.saturating_sub(40));
    // Begin at a word, not partway through one.
    if start > 0 {
        if let Some(space) = text[start..floor(at)].find(char::is_whitespace) {
            start += space + 1;
        }
    }
    let end = floor(at + len + 40);
    let mut out = text[start..end].split_whitespace().collect::<Vec<_>>().join(" ");
    if start > 0 {
        out.insert(0, '…');
    }
    if end < text.len() {
        out.push('…');
    }
    out
}

/// Kinds that already have their own place (Today, Notes) and don't need a section.
const NO_SECTION: &[&str] = &["task", "event", "note", "document", crate::graph::FAVORITE];

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct SectionIdea {
    pub tag: String,
    pub title: String,
    /// A few of the names the section would list.
    pub members: Vec<String>,
    pub count: usize,
}

/// Sections to offer after a turn: one per tag of the touched entities that has no section yet
/// (pinned or declined), plus those Claude suggested as (tag, title).
pub fn section_ideas(
    graph: &Graph,
    touched: &[Entity],
    suggested: &[(String, String)],
) -> Result<Vec<SectionIdea>, GraphError> {
    let known: Vec<String> = graph.sections()?.into_iter().map(|s| s.tag).collect();
    let mut wanted: Vec<(String, String)> = Vec::new();
    for entity in touched {
        for tag in effective_tags(entity) {
            // A person with a role belongs in that role's section ("Students") rather than People.
            let generic_person = tag == "person" && role_of(entity).is_some();
            if !NO_SECTION.contains(&tag.as_str()) && !generic_person {
                wanted.push((tag.clone(), section_title(&tag)));
            }
        }
    }
    for (tag, title) in suggested {
        if let Some(tag) = normalize_tag(tag) {
            wanted.push((tag, title.trim().to_string()));
        }
    }
    let mut ideas: Vec<SectionIdea> = Vec::new();
    for (tag, title) in wanted {
        if known.contains(&tag) || ideas.iter().any(|i| i.tag == tag) {
            continue;
        }
        let members: Vec<String> = graph.entities_with_tag(&tag)?.into_iter().map(|e| e.name).collect();
        if members.is_empty() {
            continue;
        }
        ideas.push(SectionIdea {
            title: if title.is_empty() { section_title(&tag) } else { title },
            count: members.len(),
            members: members.into_iter().take(5).collect(),
            tag,
        });
    }
    Ok(ideas)
}

/// Section suggestions for the sidebar: tags in use that aren't pinned, most used first. Unlike
/// chat offers, these include declined and removed sections, so they can be added back.
pub fn unsectioned_tags(graph: &Graph) -> Result<Vec<SectionIdea>, GraphError> {
    let known: Vec<String> = graph.sections()?.into_iter().filter(|s| s.pinned).map(|s| s.tag).collect();
    let mut counts: Vec<(String, Vec<String>)> = Vec::new();
    for entity in graph.all_entities()? {
        for tag in effective_tags(&entity) {
            if NO_SECTION.contains(&tag.as_str()) || known.contains(&tag) {
                continue;
            }
            match counts.iter_mut().find(|(t, _)| *t == tag) {
                Some((_, names)) => names.push(entity.name.clone()),
                None => counts.push((tag, vec![entity.name.clone()])),
            }
        }
    }
    counts.sort_by(|a, b| b.1.len().cmp(&a.1.len()).then(a.0.cmp(&b.0)));
    Ok(counts
        .into_iter()
        .map(|(tag, names)| SectionIdea {
            title: section_title(&tag),
            count: names.len(),
            members: names.into_iter().take(5).collect(),
            tag,
        })
        .collect())
}

/// The message as sent to Claude: with the records of pages picked while typing, and of the
/// page it was sent from, so Claude knows exactly who and what is meant.
pub fn claude_message(
    graph: &Graph,
    focus: Option<&Entity>,
    mentioned: &[Entity],
    message: &str,
) -> Result<String, String> {
    let mut content = String::new();
    let mentioned: Vec<&Entity> = mentioned.iter().filter(|m| Some(&m.id) != focus.map(|f| &f.id)).collect();
    if !mentioned.is_empty() {
        content.push_str("[Mentioned] Pages the user picked while typing:\n");
        for page in mentioned {
            content.push_str(&format!("{}\n", crate::tools::describe(graph, page, false)?));
        }
        content.push('\n');
    }
    if let Some(page) = focus {
        content.push_str(&format!(
            "[Focus] The user has the page for {} open. Its record:\n{}\n\n",
            page.name,
            crate::tools::describe(graph, page, true)?
        ));
    }
    content.push_str(message);
    Ok(content)
}

/// What the app offers after a turn.
#[derive(Debug, Default)]
pub struct TurnOutcome {
    pub sections: Vec<SectionIdea>,
    pub details: Vec<DetailsRequest>,
}

/// Stores a finished exchange, linked to the pages it was about (created since `started`,
/// `touched` by tools, and the `focus` page), and works out what to offer for them.
pub fn record_turn(
    graph: &Graph,
    started: i64,
    focus: Option<&Entity>,
    mentioned: &[String],
    touched: &[String],
    suggested: &[(String, String)],
    message: &str,
    reply: &str,
) -> Result<TurnOutcome, GraphError> {
    let created = graph.created_since(started)?;
    let mut ids: Vec<String> = Vec::new();
    for id in created.iter().map(|e| &e.id).chain(touched) {
        if !ids.contains(id) {
            ids.push(id.clone());
        }
    }
    let mut pages = Vec::new();
    for id in &ids {
        pages.extend(graph.get(id)?);
    }
    // The conversation also belongs on the page it was sent from and the pages it mentioned,
    // which don't need offers of their own.
    let mut about = ids.clone();
    for id in focus.iter().map(|f| &f.id).chain(mentioned) {
        if !about.contains(id) {
            about.push(id.clone());
        }
    }
    let focus_id = focus.map(|f| f.id.as_str());
    graph.add_message("user", message, focus_id, &about)?;
    graph.add_message("agent", reply, focus_id, &about)?;

    let new_pages: Vec<Entity> = pages.iter().filter(|p| created.iter().any(|c| c.id == p.id)).cloned().collect();
    Ok(TurnOutcome {
        sections: section_ideas(graph, &pages, suggested)?,
        details: details_requests(&new_pages),
    })
}

/// A task with who it's for, who the user is waiting on, and the project or course it
/// belongs to.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct TaskItem {
    pub task: Entity,
    /// Who is to do it; empty for the user's own tasks.
    pub assigned_to: Vec<Entity>,
    #[serde(rename = "for")]
    pub for_people: Vec<Entity>,
    pub waiting_on: Vec<Entity>,
    pub part_of: Vec<Entity>,
}

/// Tasks as the database lists them, with their people and projects.
pub fn task_items(graph: &Graph, query: &TaskQuery) -> Result<Vec<TaskItem>, GraphError> {
    let mut items = Vec::new();
    for task in graph.tasks(query)? {
        let mut item = TaskItem { assigned_to: vec![], for_people: vec![], waiting_on: vec![], part_of: vec![], task };
        for link in graph.links(&item.task.id)? {
            match (link.kind.as_str(), link.outgoing) {
                ("ASSIGNED_TO", true) => item.assigned_to.push(link.other),
                ("FOR", true) => item.for_people.push(link.other),
                ("WAITING_ON", true) => item.waiting_on.push(link.other),
                ("HAS_TASK", false) => item.part_of.push(link.other),
                _ => {}
            }
        }
        items.push(item);
    }
    Ok(items)
}

/// Marks an entity whose details form the user skipped.
pub const DETAILS_SKIPPED: &str = "details_skipped";

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Field {
    pub key: &'static str,
    pub label: &'static str,
    /// Suggestions offered while typing; any text is accepted.
    pub options: &'static [&'static str],
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct RoleOption {
    pub tag: &'static str,
    pub label: &'static str,
}

/// How someone can be connected to the user, as offered in the form.
pub const ROLE_OPTIONS: &[RoleOption] = &[
    RoleOption { tag: "student", label: "My student" },
    RoleOption { tag: "collaborator", label: "Collaborator" },
    RoleOption { tag: "colleague", label: "Colleague" },
    RoleOption { tag: "faculty", label: "Faculty elsewhere" },
    RoleOption { tag: "staff", label: "Staff" },
    RoleOption { tag: "alumni", label: "Former student" },
];

/// Who someone is, whatever their role.
const IDENTITY: &[Field] = &[
    Field { key: "full_name", label: "Full name", options: &[] },
    Field { key: "email", label: "Email", options: &[] },
    Field { key: "position", label: "Position", options: &["PhD scholar", "Assistant Professor", "Associate Professor", "Professor", "Postdoc", "Engineer"] },
    Field { key: "affiliation", label: "Affiliation", options: &[] },
    Field { key: "homepage", label: "Profile link", options: &[] },
];

/// Extra details a role asks for.
fn role_fields(role: &str) -> &'static [Field] {
    const STUDENT: &[Field] = &[
        Field { key: "program", label: "Programme", options: &["PhD", "MTech", "MS", "BTech", "Intern"] },
        Field { key: "start", label: "Started", options: &[] },
        Field { key: "thesis", label: "Thesis topic", options: &[] },
    ];
    match role {
        "student" => STUDENT,
        _ => &[],
    }
}

/// What to ask about a person: their connection to the user if unknown, and missing details.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct DetailsRequest {
    pub entity: Entity,
    /// Roles to choose from; empty when the person already has one.
    pub roles: Vec<RoleOption>,
    /// Missing identity details.
    pub fields: Vec<Field>,
    /// Missing details for each role, shown once that role is chosen.
    pub role_fields: BTreeMap<&'static str, Vec<Field>>,
}

/// The form for a person, or None for other kinds and people with nothing left to ask.
pub fn details_request(entity: &Entity) -> Option<DetailsRequest> {
    if entity.kind != "Person" {
        return None;
    }
    // A page already named with a full name ("Satya Prakash Das") doesn't need one asked for.
    let named_in_full = entity.name.split_whitespace().count() > 1;
    let missing = |fields: &[Field]| -> Vec<Field> {
        fields
            .iter()
            .filter(|f| !entity.info.contains_key(f.key) && !(f.key == "full_name" && named_in_full))
            .cloned()
            .collect()
    };
    let has_role = role_of(entity).is_some();
    let request = DetailsRequest {
        entity: entity.clone(),
        roles: if has_role { Vec::new() } else { ROLE_OPTIONS.to_vec() },
        fields: missing(IDENTITY),
        role_fields: ROLE_OPTIONS
            .iter()
            .map(|r| (r.tag, missing(role_fields(r.tag))))
            .filter(|(_, fields)| !fields.is_empty())
            .collect(),
    };
    let role_missing = ROLES
        .iter()
        .filter(|r| entity.tags.iter().any(|t| t == *r))
        .any(|r| request.role_fields.contains_key(r));
    (!has_role || !request.fields.is_empty() || role_missing).then_some(request)
}

/// A form for each newly created person that has something left to ask.
pub fn details_requests(created: &[Entity]) -> Vec<DetailsRequest> {
    created
        .iter()
        .filter(|e| !e.info.contains_key(DETAILS_SKIPPED))
        .filter_map(details_request)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn wikilinks_are_parsed() {
        assert_eq!(
            wikilinks("See [[Satya]] and [[Compiler Fuzzing|the project]], [[satya]] again, [[ ]] [[broken\nlink]] [[open"),
            vec!["Satya", "Compiler Fuzzing"]
        );
        assert!(wikilinks("no links [x]").is_empty());
    }

    fn info(g: &Graph, id: &str, pairs: &[(&str, &str)]) {
        let map: BTreeMap<String, Option<String>> =
            pairs.iter().map(|(k, v)| (k.to_string(), Some(v.to_string()))).collect();
        g.update_info(id, &map).unwrap();
    }

    #[test]
    fn search_ranks_names_then_details_then_notes() {
        let g = Graph::in_memory().unwrap();
        let satya = g.upsert_entity("Student", "Satya").unwrap();
        info(&g, &satya.id, &[("full_name", "Satyajit Das"), ("email", "satya.d@example.edu")]);
        let meeting = g.upsert_entity("Note", "Group meeting").unwrap();
        g.set_notes(&meeting.id, "Long preamble about the lab and its many projects. Then Satya presented fuzzing results.").unwrap();
        let das = g.upsert_entity("Person", "Prof Das").unwrap();
        g.set_tags(&das.id, &["satya-committee".into()]).unwrap();
        g.upsert_entity("Project", "Fuzzing").unwrap();

        let hits = search(&g, "satya", 10).unwrap();
        let summary: Vec<_> = hits.iter().map(|h| (h.entity.name.as_str(), h.field.as_str())).collect();
        assert_eq!(summary, vec![("Satya", "name"), ("Prof Das", "tag"), ("Group meeting", "notes")]);
        assert_eq!(hits[2].snippet.as_deref(), Some("…the lab and its many projects. Then Satya presented fuzzing results."));

        assert_eq!(search(&g, "das", 10).unwrap()[0].entity.name, "Prof Das");
        assert_eq!(search(&g, "das", 10).unwrap()[1].field, "full_name");
        assert_eq!(search(&g, "example.edu", 10).unwrap()[0].field, "email");
        assert!(search(&g, "  ", 10).unwrap().is_empty());
        assert_eq!(search(&g, "a", 2).unwrap().len(), 2);
    }

    #[test]
    fn search_finds_organizations_by_other_names() {
        let g = Graph::in_memory().unwrap();
        let iitg = g.upsert_entity("Organization", "IIT Guwahati").unwrap();
        g.set_aliases(&iitg.id, &["IITG".into(), "Indian Institute of Technology Guwahati".into()]).unwrap();
        let satya = g.upsert_entity("Person", "Satya").unwrap();
        g.affiliate(&satya.id, "AFFILIATED_WITH", "IITG", &crate::graph::LinkDetails::default()).unwrap();
        let hits = search(&g, "iitg", 10).unwrap();
        assert_eq!((hits[0].entity.name.as_str(), hits[0].field.as_str(), hits[0].snippet.as_deref()), ("IIT Guwahati", "alias", Some("IITG")));
        assert_eq!(search(&g, "technology", 10).unwrap()[0].entity.name, "IIT Guwahati");
        // People show up by where they are, after the organization itself.
        let guwahati = search(&g, "guwahati", 10).unwrap();
        assert_eq!(guwahati.iter().map(|h| h.entity.name.as_str()).collect::<Vec<_>>(), vec!["IIT Guwahati", "Satya"]);
    }

    #[test]
    fn section_ideas_skip_known_and_unsuitable_tags() {
        let g = Graph::in_memory().unwrap();
        let satya = g.upsert_entity("Student", "Satya").unwrap();
        let fuzzing = g.upsert_entity("Project", "Fuzzing").unwrap();
        let task = g.upsert_entity("Task", "Review Satya's draft").unwrap();
        g.dismiss_section("project").unwrap();

        let ideas = section_ideas(&g, &[satya.clone(), fuzzing, task], &[]).unwrap();
        assert_eq!(
            ideas,
            vec![SectionIdea { tag: "student".into(), title: "Students".into(), members: vec!["Satya".into()], count: 1 }]
        );

        g.set_tags(&satya.id, &["nba-committee".into()]).unwrap();
        let suggested = [("NBA Committee".to_string(), "NBA accreditation".to_string()), ("ghost".into(), "Ghost".into())];
        let ideas = section_ideas(&g, &[], &suggested).unwrap();
        assert_eq!(ideas.len(), 1, "a tag nothing has is not offered");
        assert_eq!((ideas[0].tag.as_str(), ideas[0].title.as_str()), ("nba-committee", "NBA accreditation"));

        g.pin_section("student", "Students").unwrap();
        let sidebar: Vec<_> = unsectioned_tags(&g).unwrap().into_iter().map(|i| (i.tag, i.count)).collect();
        assert_eq!(sidebar, vec![("nba-committee".to_string(), 1), ("person".to_string(), 1), ("project".to_string(), 1)]);
    }

    #[test]
    fn a_turn_is_stored_with_its_pages_and_offers() {
        let g = Graph::in_memory().unwrap();
        let fuzzing = g.upsert_entity("Project", "Fuzzing").unwrap();
        g.pin_section("project", "Projects").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(2));
        let started = crate::graph::tests_now();
        // The turn created Satya and linked him to the existing project.
        let satya = g.upsert_entity("Student", "Satya").unwrap();
        g.link(&satya.id, "WORKS_ON", &fuzzing.id).unwrap();

        let outcome = record_turn(
            &g,
            started,
            None,
            &[],
            &[satya.id.clone(), fuzzing.id.clone()],
            &[],
            "I have a student Satya working on Fuzzing",
            "Saved Satya as your student, working on Fuzzing.",
        )
        .unwrap();
        assert_eq!(outcome.sections.iter().map(|s| s.title.as_str()).collect::<Vec<_>>(), vec!["Students"]);
        assert_eq!(outcome.details.len(), 1);
        assert_eq!(outcome.details[0].entity.name, "Satya");
        assert_eq!(g.messages_about(&fuzzing.id, 10).unwrap().len(), 2);
        assert_eq!(g.recent_messages(10).unwrap()[0].text, "I have a student Satya working on Fuzzing");

        // Asked from Satya's page: stored against Satya, nothing new offered.
        let outcome = record_turn(&g, crate::graph::tests_now(), Some(&satya), &[], &[], &[], "His email?", "Not saved yet.").unwrap();
        assert!(outcome.sections.iter().all(|s| s.tag != "project") && outcome.details.is_empty());
        let about = g.messages_about(&satya.id, 10).unwrap();
        assert_eq!(about.len(), 4);
        assert_eq!(about[3].focus.as_deref(), Some("person:satya"));
        assert_eq!(g.recent_messages(10).unwrap().len(), 2, "page chats stay out of the main chat");

        let focused = claude_message(&g, Some(&satya), &[], "What's his email?").unwrap();
        assert!(focused.starts_with("[Focus] The user has the page for Satya open. Its record:\n{"));
        assert!(focused.contains("Satya works on Fuzzing") && focused.ends_with("\n\nWhat's his email?"));

        // Picked while typing: records come first, and the exchange is stored on those pages.
        let meera = g.upsert_entity("Student", "Meera").unwrap();
        let sent = claude_message(&g, None, &[meera.clone(), fuzzing.clone()], "Meera joins Fuzzing").unwrap();
        assert!(sent.starts_with("[Mentioned] Pages the user picked while typing:\n{\"name\":\"Meera\""));
        assert!(sent.contains("\"name\":\"Fuzzing\"") && sent.ends_with("\n\nMeera joins Fuzzing"));
        let outcome = record_turn(&g, crate::graph::tests_now(), None, &[meera.id.clone()], &[], &[], "Meera joins Fuzzing", "Done.").unwrap();
        assert!(outcome.details.is_empty() && outcome.sections.is_empty(), "nothing new, nothing offered");
        assert_eq!(g.messages_about(&meera.id, 10).unwrap().len(), 2);
    }

    #[test]
    fn task_items_carry_their_people_and_projects() {
        let g = Graph::in_memory().unwrap();
        let satya = g.upsert_entity("Student", "Satya").unwrap();
        let kavya = g.upsert_entity("Person", "Kavya").unwrap();
        let fuzzing = g.upsert_entity("Project", "Fuzzing").unwrap();
        let review = g.upsert_entity("Task", "Review Satya's survey").unwrap();
        g.link(&review.id, "FOR", &satya.id).unwrap();
        g.link(&review.id, "WAITING_ON", &kavya.id).unwrap();
        g.link(&fuzzing.id, "HAS_TASK", &review.id).unwrap();
        g.upsert_entity("Task", "Book travel").unwrap();

        let items = task_items(&g, &TaskQuery { linked_to: Some(satya.id.clone()), ..Default::default() }).unwrap();
        assert_eq!(items.len(), 1);
        let names = |v: &[Entity]| v.iter().map(|e| e.name.clone()).collect::<Vec<_>>();
        assert_eq!((names(&items[0].for_people), names(&items[0].waiting_on), names(&items[0].part_of)), (vec!["Satya".to_string()], vec!["Kavya".to_string()], vec!["Fuzzing".to_string()]));
        let json = serde_json::to_value(&items[0]).unwrap();
        assert_eq!(json["for"][0]["name"], "Satya");
        assert_eq!(task_items(&g, &TaskQuery::default()).unwrap().len(), 2);
    }

    #[test]
    fn details_are_requested_only_for_missing_fields() {
        let g = Graph::in_memory().unwrap();
        let satya = g.upsert_entity("Student", "Satya").unwrap();
        info(&g, &satya.id, &[("program", "PhD")]);
        let chen = g.upsert_entity("Person", "Chen").unwrap();
        info(&g, &chen.id, &[("email", "chen@nus.edu"), ("affiliation", "NUS")]);
        let skipped = g.upsert_entity("Person", "Ravi").unwrap();
        info(&g, &skipped.id, &[(DETAILS_SKIPPED, "yes")]);
        let project = g.upsert_entity("Project", "Fuzzing").unwrap();
        let done = g.upsert_entity("Student", "Meera").unwrap();
        let all = [
            ("full_name", "Meera Iyer"), ("email", "m@x.in"), ("position", "PhD scholar"),
            ("affiliation", "IITG"), ("homepage", "https://x.in"), ("program", "PhD"), ("start", "2023"), ("thesis", "Types"),
        ];
        info(&g, &done.id, &all);

        let created: Vec<_> = [satya, chen, skipped, project, done].iter().map(|e| g.get(&e.id).unwrap().unwrap()).collect();
        let requests = details_requests(&created);
        let summary: Vec<_> = requests
            .iter()
            .map(|r| {
                (
                    r.entity.name.as_str(),
                    r.roles.len(),
                    r.fields.iter().map(|f| f.key).collect::<Vec<_>>(),
                    r.role_fields.get("student").map(|f| f.iter().map(|f| f.key).collect::<Vec<_>>()),
                )
            })
            .collect();
        assert_eq!(
            summary,
            vec![
                // A student: no role question; identity plus the student details still missing.
                ("Satya", 0, vec!["full_name", "email", "position", "affiliation", "homepage"], Some(vec!["start", "thesis"])),
                // Connection unknown: asked, with each role's details ready to show.
                ("Chen", ROLE_OPTIONS.len(), vec!["full_name", "position", "homepage"], Some(vec!["program", "start", "thesis"])),
            ]
        );
        // A page named in full isn't asked for a full name.
        let kavya = g.upsert_entity("Person", "Kavya Rao").unwrap();
        assert!(!details_request(&kavya).unwrap().fields.iter().any(|f| f.key == "full_name"));
        // Asking again from the page ignores Skip, and nothing is asked of complete people.
        assert!(details_request(&created[2]).is_some());
        assert!(details_request(&created[4]).is_none());
    }
}
