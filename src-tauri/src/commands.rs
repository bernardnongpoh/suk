//! The only backend operations the frontend can call. Keep each one narrow, and list new ones
//! in build.rs and capabilities/default.json.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;
use tauri::ipc::Channel;
use tauri::{AppHandle, Emitter, Manager, State};

use crate::assistant::{self, Kind, Notes, Settings, SignIn, SignInPrompt};
use crate::claude::{self, Claude, Turn};
use crate::codex::{self, Codex};
use crate::gemini::{self, Gemini};
use crate::graph::{Affiliation, ChatRecord, Entity, Graph, GraphError, Link, LinkDetails, TaskQuery, ROLES};
use crate::links;
use crate::mcp::Endpoint;
use crate::pages::{self, DetailsRequest, Hit, SectionIdea, TaskItem, DETAILS_SKIPPED};
use crate::templates::{self, Template};
use crate::tidy::{self, TidyItems};
use crate::tools::{self, Activity, Proposal, Proposals};
use crate::calendar::{self, Feed};
use crate::google;
use crate::vault::{self, Vault};
use crate::watch::{self, AuthorCandidate, WatchInfo, Watcher};

#[derive(Clone, Serialize)]
pub struct ChatStatus {
    status: String,
}

#[derive(Serialize)]
pub struct ChatReply {
    text: String,
    /// Schedule proposals made during this turn, or updated by it.
    proposals: Vec<Proposal>,
    /// Extra help shown under the reply, e.g. how to connect Google Calendar.
    notice: Option<String>,
    /// Sidebar sections to offer for what this turn saved.
    sections: Vec<SectionIdea>,
    /// Forms for details missing from students and people this turn created.
    details: Vec<DetailsRequest>,
}

fn text(e: GraphError) -> String {
    e.to_string()
}

fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64)
}

fn data_dir(app: &AppHandle) -> Result<PathBuf, String> {
    app.path().app_data_dir().map_err(|e| e.to_string())
}

/// Shown when chat can't run because no assistant is set up.
const INSTALL_HELP: &str = "Open Settings to set up Claude Code or Codex; Suk can't answer without one.";

/// The assistant chosen when setting up the app, if any.
fn chosen(app: &AppHandle) -> Option<Kind> {
    Settings::load(&data_dir(app).ok()?).assistant
}

/// Sends a message, with any pending [App] notes, to the chosen assistant.
fn assistant_turn(app: &AppHandle, message: &str, on_status: &mut dyn FnMut(&str)) -> Result<Turn, String> {
    let kind = chosen(app).ok_or("No assistant is set up")?;
    let binary = assistant::find(kind).ok_or_else(|| format!("{} isn't installed", kind.label()))?;
    let mut content = app.state::<Notes>().with_message(message);
    let mcp = app.state::<Endpoint>().inner().clone();
    let dir = data_dir(app)?;
    // A new session (each day, or when the conversation grows long) starts with a short summary of
    // the last messages instead of re-reading the whole history.
    let brief = || pages::recent_brief(&app.state::<Graph>()).unwrap_or_default();
    let about_user = templates::instructions(Settings::load(&dir).template.as_deref());
    match kind {
        Kind::Claude => {
            let setup = claude::Setup { binary, workdir: dir.join("claude"), mcp, about_user };
            let claude = app.state::<Claude>();
            if claude.begins_fresh(&setup) {
                content = format!("{}{content}", brief());
            }
            claude.send(&setup, &content, on_status)
        }
        Kind::Codex => {
            let setup = codex::Setup { binary, workdir: dir.join("codex"), mcp, about_user };
            let codex = app.state::<Codex>();
            if codex.begins_fresh(&setup) {
                content = format!("{}{content}", brief());
            }
            codex.send(&setup, &content, on_status)
        }
        Kind::Gemini => {
            let setup = gemini::Setup { binary, workdir: dir.join("gemini"), mcp, about_user };
            let gemini = app.state::<Gemini>();
            if gemini.begins_fresh(&setup) {
                content = format!("{}{content}", brief());
            }
            gemini.send(&setup, &content, on_status)
        }
    }
}

/// Starts a new conversation: the assistant forgets the exchange so far and begins the next
/// message fresh, with a short summary. Everything said stays in the app.
#[tauri::command]
pub async fn new_conversation(app: AppHandle) -> Result<(), String> {
    let dir = data_dir(&app)?;
    let mcp = app.state::<Endpoint>().inner().clone();
    match chosen(&app) {
        Some(Kind::Gemini) => {
            let binary = assistant::find(Kind::Gemini).unwrap_or_default();
            app.state::<Gemini>().start_over(&gemini::Setup { binary, workdir: dir.join("gemini"), mcp, about_user: String::new() });
        }
        Some(Kind::Codex) => {
            let binary = assistant::find(Kind::Codex).unwrap_or_default();
            app.state::<Codex>().start_over(&codex::Setup { binary, workdir: dir.join("codex"), mcp, about_user: String::new() });
        }
        _ => {
            let binary = claude::find_binary().unwrap_or_default();
            app.state::<Claude>().start_over(&claude::Setup { binary, workdir: dir.join("claude"), mcp, about_user: String::new() });
        }
    }
    Ok(())
}

/// Handles a message with Claude. `focus` is the id of the page the message was sent from, and
/// `mentions` the pages picked while typing. The exchange is stored, linked to the pages it
/// touched, and the reply offers sections and details forms for what was saved. If Claude can't
/// answer, the reply says why and nothing is stored.
#[tauri::command]
pub async fn send_message(
    app: AppHandle,
    message: String,
    focus: Option<String>,
    mentions: Option<Vec<String>>,
    on_status: Channel<ChatStatus>,
) -> Result<ChatReply, String> {
    let mentions = mentions.unwrap_or_default();
    // Claude takes seconds to answer; keep it off the async runtime.
    tauri::async_runtime::spawn_blocking(move || {
        let graph = app.state::<Graph>();
        let proposals = app.state::<Proposals>();
        let activity = app.state::<Activity>();
        let before = proposals.count();
        let started = now_ms();
        activity.begin(links::urls_in(&message));
        let focus = match focus.as_deref() {
            Some(id) => graph.get(id).map_err(text)?,
            None => None,
        };
        let mut mentioned = Vec::new();
        for id in &mentions {
            mentioned.extend(graph.get(id).map_err(text)?);
        }

        let content = pages::claude_message(&graph, focus.as_ref(), &mentioned, &message)?;
        let outcome = assistant_turn(&app, &content, &mut |status| {
            let _ = on_status.send(ChatStatus { status: status.into() });
        });
        let turn = match outcome {
            Ok(turn) => turn,
            Err(e) => {
                eprintln!("chat: {message:?} -> assistant failed: {e}");
                let missing = e.contains("isn't installed") || e.contains("No assistant");
                return Ok(ChatReply {
                    text: format!("{e}, so nothing was saved."),
                    proposals: Vec::new(),
                    notice: missing.then(|| INSTALL_HELP.to_string()),
                    sections: Vec::new(),
                    details: Vec::new(),
                });
            }
        };
        eprintln!("chat: {message:?} -> assistant: {:?}", turn.text);

        let (touched, suggested) = activity.take();
        let outcome = pages::record_turn(
            &graph,
            started,
            focus.as_ref(),
            &mentions,
            &touched,
            &suggested,
            &message,
            &turn.text,
        )
        .map_err(text)?;
        Ok(ChatReply {
            text: turn.text,
            proposals: proposals.since(before),
            notice: None,
            sections: outcome.sections,
            details: outcome.details,
        })
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Confirms the chosen items of a proposal, saves them to Today, and has Claude add them to
/// Google Calendar.
#[tauri::command]
pub async fn confirm_proposal(
    app: AppHandle,
    id: String,
    items: Vec<usize>,
    on_status: Channel<ChatStatus>,
) -> Result<ChatReply, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let graph = app.state::<Graph>();
        let proposals = app.state::<Proposals>();
        let proposal = proposals.confirm(&id, &items)?;
        tools::record_confirmed(&graph, &proposal).map_err(|e| e.to_string())?;

        let saved = "Saved to Today.";
        let reply = |text: String, proposal: Proposal, notice: Option<String>| {
            graph.add_message("agent", &text, None, &[]).map_err(|e| e.to_string())?;
            Ok(ChatReply { text, proposals: vec![proposal], notice, sections: vec![], details: vec![] })
        };
        match chosen(&app) {
            Some(Kind::Claude) if claude::find_binary().is_some() => {}
            Some(Kind::Codex) => {
                return reply(format!("{saved} Adding it to Google Calendar needs Claude Code, so it's only in the app."), proposal, None)
            }
            _ => return reply(format!("{saved} Claude Code isn't set up, so it wasn't added to Google Calendar."), proposal, Some(INSTALL_HELP.into())),
        }
        if !app.state::<Claude>().calendar_usable() {
            return reply(
                format!("{saved} Google Calendar isn't connected yet, so it wasn't added there."),
                proposal,
                Some(CALENDAR_HELP.into()),
            );
        }
        let outcome = assistant_turn(&app, &confirmation_message(&proposal), &mut |status| {
            let _ = on_status.send(ChatStatus { status: status.into() });
        });
        let (text, notice) = match outcome {
            Ok(turn) => {
                for input in &turn.created_events {
                    if let Some((pid, index)) = proposals.confirmed_item_for(input) {
                        proposals.mark_added(&pid, index);
                        if let Some(item) = proposals.get(&pid).and_then(|p| p.items.get(index).cloned()) {
                            tools::record_added(&graph, &item).map_err(|e| e.to_string())?;
                        }
                    }
                }
                (turn.text, None)
            }
            Err(e) => (format!("{saved} It wasn't added to Google Calendar."), Some(e)),
        };
        let updated = proposals.get(&id).ok_or("proposal disappeared")?;
        reply(text, updated, notice)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// How to connect Google Calendar; shown when it isn't.
const CALENDAR_HELP: &str = "To connect Google Calendar, run claude in Terminal, type /mcp, and choose \"claude.ai Google Calendar\".";

fn confirmation_message(proposal: &Proposal) -> String {
    let today = chrono::Local::now().format("%Y-%m-%d").to_string();
    let mut confirmed = Vec::new();
    let mut skipped = Vec::new();
    for (item, state) in proposal.items.iter().zip(&proposal.states) {
        let day = if item.start.starts_with(&today) { " (today)" } else { "" };
        let line = format!("- \"{}\" from {} to {}{day}", item.title, item.start, item.end);
        match state {
            tools::ItemState::Confirmed => confirmed.push(line),
            _ => skipped.push(line),
        }
    }
    let mut message = format!(
        "[App] The user confirmed proposal {}. Add these to their Google Calendar now, with exactly these titles and local times:\n{}",
        proposal.id,
        confirmed.join("\n")
    );
    if !skipped.is_empty() {
        message.push_str(&format!("\nThey left these out, so don't add them:\n{}", skipped.join("\n")));
    }
    message.push_str("\nThen reply in one short sentence.");
    message
}

#[tauri::command]
pub async fn dismiss_proposal(app: AppHandle, id: String) -> Result<Proposal, String> {
    let proposal = app.state::<Proposals>().dismiss(&id)?;
    app.state::<Notes>()
        .add(format!("[App] The user dismissed proposal {id} (\"Not now\")."));
    Ok(proposal)
}

#[tauri::command]
pub async fn list_entities(graph: State<'_, Graph>, kind: String) -> Result<Vec<Entity>, String> {
    graph.entities_of_kind(&kind).map_err(|e| e.to_string())
}

#[derive(Serialize)]
pub struct EntityDetail {
    entity: Entity,
    links: Vec<Link>,
    /// For a person, what is still unknown about them.
    details: Option<DetailsRequest>,
    /// For an organization, everyone there or in its departments, past and present.
    affiliations: Vec<Affiliation>,
    /// For a person whose affiliation is only text: that text, and the organization it names.
    unlinked_affiliation: Option<(String, Option<Entity>)>,
    /// The details this kind of page usually has, and how each is edited.
    fields: Vec<pages::DetailField>,
}

#[tauri::command]
pub async fn get_entity(graph: State<'_, Graph>, id: String) -> Result<EntityDetail, String> {
    let entity = graph
        .get(&id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("not found: {id}"))?;
    let links = graph.links(&id).map_err(|e| e.to_string())?;
    let details = pages::details_request(&entity);
    let affiliations = match entity.kind.as_str() {
        "Organization" => graph.affiliations_at(&id, true).map_err(text)?,
        _ => Vec::new(),
    };
    let unlinked_affiliation = match entity.kind.as_str() {
        "Person" if !links.iter().any(|l| l.outgoing && l.kind == "AFFILIATED_WITH") => match entity.info.get("affiliation") {
            Some(text_value) => Some((
                text_value.clone(),
                graph.find_by_name(text_value).map_err(text)?.filter(|e| e.kind == "Organization"),
            )),
            None => None,
        },
        _ => None,
    };
    let fields = pages::detail_fields(&entity);
    Ok(EntityDetail { entity, links, details, affiliations, unlinked_affiliation, fields })
}

/// The main chat's recent messages, or with `focus`, the messages about that page.
#[tauri::command]
pub async fn chat_history(graph: State<'_, Graph>, focus: Option<String>) -> Result<Vec<ChatRecord>, String> {
    match focus {
        Some(id) => graph.messages_about(&id, 200),
        None => graph.recent_messages(200),
    }
    .map_err(text)
}

/// Tasks from the database, soonest first: `status` is open, done or all; `about` limits them to
/// a person, project or course.
#[tauri::command]
pub async fn list_tasks(
    graph: State<'_, Graph>,
    status: String,
    about: Option<String>,
    assigned: Option<String>,
) -> Result<Vec<TaskItem>, String> {
    let done = match status.as_str() {
        "open" => Some(false),
        "done" => Some(true),
        _ => None,
    };
    let assigned = match assigned.as_deref() {
        Some("mine") => Some(false),
        Some("others") => Some(true),
        _ => None,
    };
    pages::task_items(&graph, &TaskQuery { done, linked_to: about, assigned, ..Default::default() }).map_err(text)
}

/// Assigns a task to a person, or takes it off them.
#[tauri::command]
pub async fn assign_task(app: AppHandle, task: String, person: String, assigned: bool) -> Result<(), String> {
    let graph = app.state::<Graph>();
    let (Some(t), Some(p)) = (graph.get(&task).map_err(text)?, graph.get(&person).map_err(text)?) else {
        return Err("page not found".into());
    };
    if t.kind != "Task" || p.kind != "Person" {
        return Err("only a task can be assigned, and only to a person".into());
    }
    if assigned {
        graph.link(&t.id, "ASSIGNED_TO", &p.id).map_err(text)?;
    } else {
        graph.unlink(&t.id, "ASSIGNED_TO", &p.id).map_err(text)?;
    }
    let what = if assigned { "assigned" } else { "unassigned" };
    let to = if assigned { "to" } else { "from" };
    app.state::<Notes>().add(format!("[App] The user {what} the task \"{}\" {to} {}.", t.name, p.name));
    Ok(())
}

#[derive(Serialize)]
pub struct UnlinkedAffiliation {
    person: Entity,
    text: String,
    /// The organization page the text already names, if any.
    organization: Option<Entity>,
}

/// People whose affiliation is still only text.
#[tauri::command]
pub async fn unlinked_affiliations(graph: State<'_, Graph>) -> Result<Vec<UnlinkedAffiliation>, String> {
    Ok(graph
        .unlinked_affiliations()
        .map_err(text)?
        .into_iter()
        .map(|(person, text, organization)| UnlinkedAffiliation { person, text, organization })
        .collect())
}

/// Turns a person's affiliation into a link to an organization page, creating it if needed.
/// `organization` is the page to link to, by any of its names.
#[tauri::command]
pub async fn link_affiliation(graph: State<'_, Graph>, person: String, organization: String) -> Result<Entity, String> {
    let entity = graph.get(&person).map_err(text)?.ok_or("page not found")?;
    let details = LinkDetails { detail: entity.info.get("position").cloned(), ..Default::default() };
    graph.affiliate(&person, "AFFILIATED_WITH", &organization, &details).map_err(text)
}

/// Replaces a page's other names.
#[tauri::command]
pub async fn set_aliases(graph: State<'_, Graph>, id: String, aliases: Vec<String>) -> Result<Entity, String> {
    graph.set_aliases(&id, &aliases).map_err(text)
}

/// Ticks a task off, or opens it again.
#[tauri::command]
pub async fn set_task_done(app: AppHandle, id: String, done: bool) -> Result<Entity, String> {
    let task = app.state::<Graph>().set_done(&id, done).map_err(text)?;
    let what = if done { "marked done" } else { "reopened" };
    app.state::<Notes>().add(format!("[App] The user {what} the task \"{}\".", task.name));
    Ok(task)
}

#[tauri::command]
pub async fn search(graph: State<'_, Graph>, query: String) -> Result<Vec<Hit>, String> {
    pages::search(&graph, &query, 30).map_err(text)
}

#[derive(Serialize)]
pub struct SidebarSection {
    tag: String,
    title: String,
    count: usize,
    icon: String,
}

#[derive(Serialize)]
pub struct Sidebar {
    /// Pages the user starred, by name.
    favorites: Vec<Entity>,
    sections: Vec<SidebarSection>,
    /// Tags in use that have no section yet.
    suggested: Vec<SectionIdea>,
}

#[tauri::command]
pub async fn get_sidebar(graph: State<'_, Graph>) -> Result<Sidebar, String> {
    let mut sections = Vec::new();
    for section in graph.sections().map_err(text)?.into_iter().filter(|s| s.pinned) {
        let count = graph.entities_with_tag(&section.tag).map_err(text)?.len();
        sections.push(SidebarSection { tag: section.tag, title: section.title, count, icon: section.icon });
    }
    let mut favorites = graph.entities_with_tag(crate::graph::FAVORITE).map_err(text)?;
    favorites.sort_by_key(|e| e.name.to_lowercase());
    Ok(Sidebar { favorites, sections, suggested: pages::unsectioned_tags(&graph).map_err(text)? })
}

/// Sets a page's emoji icon, or clears it with None.
#[tauri::command]
pub async fn set_icon(graph: State<'_, Graph>, id: String, icon: Option<String>) -> Result<Entity, String> {
    graph.update_info(&id, &BTreeMap::from([(crate::graph::ICON_KEY.to_string(), icon)])).map_err(text)
}

#[tauri::command]
pub async fn set_section_icon(graph: State<'_, Graph>, tag: String, icon: Option<String>) -> Result<(), String> {
    graph.set_section_icon(&tag, icon.as_deref()).map_err(text)
}

/// Stars or unstars a page, so it shows under Favorites in the sidebar.
#[tauri::command]
pub async fn set_favorite(graph: State<'_, Graph>, id: String, favorite: bool) -> Result<Entity, String> {
    let entity = graph.get(&id).map_err(text)?.ok_or("page not found")?;
    let mut tags: Vec<String> = entity.tags.into_iter().filter(|t| t != crate::graph::FAVORITE).collect();
    if favorite {
        tags.push(crate::graph::FAVORITE.into());
    }
    graph.set_tags(&id, &tags).map_err(text)
}

/// Adds a section to the sidebar, or renames one.
#[tauri::command]
pub async fn pin_section(graph: State<'_, Graph>, tag: String, title: String) -> Result<(), String> {
    graph.pin_section(&tag, &title).map(|_| ()).map_err(text)
}

/// Removes a section from the sidebar, or declines a suggested one.
#[tauri::command]
pub async fn dismiss_section(graph: State<'_, Graph>, tag: String) -> Result<(), String> {
    graph.dismiss_section(&tag).map(|_| ()).map_err(text)
}

#[tauri::command]
pub async fn list_tagged(graph: State<'_, Graph>, tag: String) -> Result<Vec<Entity>, String> {
    graph.entities_with_tag(&tag).map_err(text)
}

/// Changes a page's details from the page itself: a value sets a detail, None removes it. A
/// person's affiliation becomes a link to the organization. Claude is told what changed, so it
/// doesn't go on what an earlier message said.
#[tauri::command]
pub async fn update_details(app: AppHandle, id: String, changes: BTreeMap<String, Option<String>>) -> Result<Entity, String> {
    let graph = app.state::<Graph>();
    let before = graph.get(&id).map_err(text)?.ok_or("page not found")?;
    let mut changes: BTreeMap<String, Option<String>> =
        changes.into_iter().map(|(k, v)| (k, v.map(|v| v.trim().to_string()).filter(|v| !v.is_empty()))).collect();
    let affiliation = match before.kind.as_str() {
        "Person" => changes.remove("affiliation").flatten(),
        _ => None,
    };
    let mut entity = graph.update_info(&id, &changes).map_err(text)?;
    if let Some(organization) = &affiliation {
        let details = LinkDetails { detail: entity.info.get("position").cloned(), ..Default::default() };
        graph.affiliate(&id, "AFFILIATED_WITH", organization, &details).map_err(text)?;
        entity = graph.get(&id).map_err(text)?.ok_or("page not found")?;
        changes.insert("affiliation".into(), Some(organization.clone()));
    }
    let described: Vec<String> = changes
        .iter()
        .map(|(key, value)| match value {
            Some(value) => format!("{key}: {value}"),
            None => format!("{key} removed"),
        })
        .collect();
    if !described.is_empty() {
        app.state::<Notes>().add(format!(
            "[App] The user edited {}'s details on its page (already saved): {}.",
            entity.name,
            described.join("; ")
        ));
    }
    Ok(entity)
}

/// Saves the details form; empty fields are left out.
#[tauri::command]
pub async fn submit_details(
    app: AppHandle,
    id: String,
    values: BTreeMap<String, String>,
    roles: Option<Vec<String>>,
) -> Result<Entity, String> {
    let graph = app.state::<Graph>();
    let roles = roles.unwrap_or_default();
    if let Some(role) = roles.iter().find(|r| !ROLES.contains(&r.as_str())) {
        return Err(format!("unknown role {role}"));
    }
    let mut changes: BTreeMap<String, Option<String>> = values
        .into_iter()
        .filter(|(_, v)| !v.trim().is_empty())
        .map(|(k, v)| (k, Some(v)))
        .collect();
    // The affiliation becomes a link to the organization's page (found by any of its names).
    let affiliation = changes.remove("affiliation").flatten();
    let mut entity = graph.update_info(&id, &changes).map_err(text)?;
    if let Some(organization) = &affiliation {
        let details = LinkDetails { detail: entity.info.get("position").cloned(), ..Default::default() };
        graph.affiliate(&id, "AFFILIATED_WITH", organization, &details).map_err(text)?;
        entity = graph.get(&id).map_err(text)?.ok_or("page not found")?;
        changes.insert("affiliation".into(), Some(organization.clone()));
    }
    if !roles.is_empty() {
        entity = graph.add_tags(&id, &roles).map_err(text)?;
    }
    let mut given: Vec<String> = changes
        .iter()
        .map(|(k, v)| format!("{k}: {}", v.as_deref().unwrap_or_default()))
        .collect();
    if !roles.is_empty() {
        given.insert(0, format!("roles: {}", roles.join(", ")));
    }
    if !given.is_empty() {
        app.state::<Notes>().add(format!(
            "[App] The user filled in details for {} in the app (already saved): {}.",
            entity.name,
            given.join("; ")
        ));
    }
    Ok(entity)
}

#[derive(Serialize)]
pub struct ProfileFill {
    entity: Entity,
    text: String,
}

/// Fills a person's details from a profile link: Claude reads the page and saves what it says.
#[tauri::command]
pub async fn fill_profile(
    app: AppHandle,
    id: String,
    url: String,
    on_status: Channel<ChatStatus>,
) -> Result<ProfileFill, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let graph = app.state::<Graph>();
        let activity = app.state::<Activity>();
        let url = url.trim().to_string();
        links::check_public(&url)?;
        let entity = graph.get(&id).map_err(text)?.ok_or("page not found")?;
        let homepage = BTreeMap::from([("homepage".to_string(), Some(url.clone()))]);
        if let Some(missing) = match chosen(&app) {
            None => Some("No assistant is set up".to_string()),
            Some(kind) => assistant::find(kind).is_none().then(|| format!("{} isn't installed", kind.label())),
        } {
            let entity = graph.update_info(&id, &homepage).map_err(text)?;
            return Ok(ProfileFill { entity, text: format!("Saved the link. {missing}, so the page wasn't read.") });
        }
        let started = now_ms();
        activity.begin(vec![url.clone()]);
        let message = format!(
            "[App] The user gave this profile link for {name}: {url}\nRead it with read_link and save what it states about {name}: full_name, position, affiliation, department, email, homepage (this link), and research interests as a note. Don't add roles; the user chooses those. Reply in one short sentence saying what you filled in, or why the page couldn't be read.",
            name = entity.name
        );
        let content = pages::claude_message(&graph, Some(&entity), &[], &message)?;
        let turn = assistant_turn(&app, &content, &mut |status| {
            let _ = on_status.send(ChatStatus { status: status.into() });
        })?;
        let (touched, suggested) = activity.take();
        pages::record_turn(&graph, started, Some(&entity), &[], &touched, &suggested, &format!("Profile: {url}"), &turn.text)
            .map_err(text)?;
        let entity = graph.get(&id).map_err(text)?.ok_or("page not found")?;
        Ok(ProfileFill { entity, text: turn.text })
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Skips the details form, so it isn't shown for this page again.
#[tauri::command]
pub async fn skip_details(app: AppHandle, id: String) -> Result<Entity, String> {
    let graph = app.state::<Graph>();
    let changes = BTreeMap::from([(DETAILS_SKIPPED.to_string(), Some("yes".to_string()))]);
    let entity = graph.update_info(&id, &changes).map_err(text)?;
    app.state::<Notes>().add(format!(
        "[App] The user skipped giving details for {}; don't ask for them.",
        entity.name
    ));
    Ok(entity)
}

/// Saves a page's notes and updates the links its [[wikilinks]] make.
#[tauri::command]
pub async fn save_notes(graph: State<'_, Graph>, id: String, notes: String) -> Result<Entity, String> {
    let entity = graph.set_notes(&id, &notes).map_err(text)?;
    graph.sync_wikilinks(&id, &pages::wikilinks(&notes)).map_err(text)?;
    Ok(entity)
}

#[tauri::command]
pub async fn set_tags(graph: State<'_, Graph>, id: String, tags: Vec<String>) -> Result<Entity, String> {
    graph.set_tags(&id, &tags).map_err(text)
}

/// Opens the page with this name, creating it if there is none (like following a new
/// [[wikilink]] in Obsidian): a note unless `kind` says otherwise.
#[tauri::command]
pub async fn open_page(graph: State<'_, Graph>, name: String, kind: Option<String>) -> Result<Entity, String> {
    match graph.find_by_name(&name).map_err(text)? {
        Some(entity) => Ok(entity),
        None => graph.upsert_entity(kind.as_deref().unwrap_or("Note"), &name).map_err(text),
    }
}

#[tauri::command]
pub async fn rename_page(graph: State<'_, Graph>, id: String, name: String) -> Result<Entity, String> {
    graph.rename(&id, &name).map_err(text)
}

#[tauri::command]
pub async fn delete_page(app: AppHandle, id: String) -> Result<(), String> {
    let graph = app.state::<Graph>();
    let page = graph.get(&id).map_err(text)?;
    graph.delete(&id).map_err(text)?;
    // The page's file goes with it; a file left behind is read back as a new page.
    if let Some(page) = page {
        app.state::<Vault>().remove(&page)?;
    }
    Ok(())
}

#[derive(Serialize)]
pub struct VaultInfo {
    path: String,
    /// Whether Obsidian already knows the folder as a vault.
    registered: bool,
}

#[tauri::command]
pub async fn get_vault(vault: State<'_, Vault>) -> Result<VaultInfo, String> {
    Ok(VaultInfo {
        path: vault.root().display().to_string(),
        registered: vault::registered_in_obsidian(vault.root()),
    })
}

/// Opens a page's Markdown file (or the whole folder): in Obsidian when the folder is one of its
/// vaults, otherwise shown in Finder or the file manager.
#[tauri::command]
pub async fn open_page_file(app: AppHandle, id: Option<String>) -> Result<(), String> {
    let vault = app.state::<Vault>();
    let graph = app.state::<Graph>();
    let path = match id {
        Some(id) => {
            let entity = graph.get(&id).map_err(text)?.ok_or("page not found")?;
            vault.path_of(&entity)
        }
        None => vault.root().to_path_buf(),
    };
    if vault::registered_in_obsidian(vault.root()) {
        return open_target(&format!("obsidian://open?path={}", percent_encode(&path.display().to_string())));
    }
    reveal(&path)
}

/// Shows a file selected in Finder, or opens the folder it's in elsewhere.
fn reveal(path: &std::path::Path) -> Result<(), String> {
    if path.is_dir() {
        return open_target(&path.display().to_string());
    }
    if cfg!(target_os = "macos") {
        return std::process::Command::new("open").arg("-R").arg(path).status().map_err(|e| e.to_string()).map(|_| ());
    }
    open_target(&path.parent().unwrap_or(path).display().to_string())
}

fn percent_encode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// Opens a web link, such as a person's homepage, in the browser.
#[tauri::command]
pub async fn open_url(url: String) -> Result<(), String> {
    links::check_public(&url)?;
    open_target(&url)
}

/// Opens a URL or folder with the system's default handler.
fn open_target(target: &str) -> Result<(), String> {
    let mut command = if cfg!(target_os = "macos") {
        std::process::Command::new("open")
    } else if cfg!(windows) {
        let mut c = std::process::Command::new("cmd");
        c.args(["/C", "start", ""]);
        c
    } else {
        std::process::Command::new("xdg-open")
    };
    command.arg(target).status().map_err(|e| e.to_string()).map(|_| ())
}

#[derive(Serialize)]
pub struct AssistantStatus {
    /// The assistant chosen when setting up, if any.
    chosen: Option<Kind>,
    /// Whether the chosen assistant is installed and signed in, so the app can run.
    ready: bool,
    /// Claude Code, Codex, then Gemini CLI.
    assistants: Vec<assistant::Status>,
    /// Google Calendar connector status, known once Claude Code has answered a message.
    calendar_status: Option<String>,
    calendar_help: &'static str,
}

/// Whether Claude Code and Codex are installed and signed in, and which one the app uses.
#[tauri::command]
pub async fn assistant_status(app: AppHandle) -> Result<AssistantStatus, String> {
    let (claude, codex, gemini) = tauri::async_runtime::spawn_blocking(|| {
        let codex = std::thread::spawn(|| assistant::status(Kind::Codex));
        let gemini = std::thread::spawn(|| assistant::status(Kind::Gemini));
        let claude = assistant::status(Kind::Claude);
        (claude, codex.join().expect("status check"), gemini.join().expect("status check"))
    })
    .await
    .map_err(|e| e.to_string())?;
    let chosen = chosen(&app);
    let ready = match chosen {
        Some(Kind::Claude) => claude.ready(),
        Some(Kind::Codex) => codex.ready(),
        Some(Kind::Gemini) => gemini.ready(),
        None => false,
    };
    Ok(AssistantStatus {
        chosen,
        ready,
        assistants: vec![claude, codex, gemini],
        calendar_status: app.state::<Claude>().calendar_status(),
        calendar_help: CALENDAR_HELP,
    })
}

/// What Suk wants a decision on: new relationships and kinds of page, and possible duplicates.
#[tauri::command]
pub async fn tidy_items(graph: State<'_, Graph>) -> Result<TidyItems, String> {
    tidy::items(&graph).map_err(text)
}

/// Keeps a new relationship or kind of page as it is, so it isn't offered for tidying again.
#[tauri::command]
pub async fn keep_type(graph: State<'_, Graph>, what: String, name: String) -> Result<(), String> {
    check_what(&what)?;
    graph.decide(&format!("{what}:{name}")).map_err(text)
}

/// Renames a relationship or kind of page everywhere, or merges it into an existing one by
/// giving that one's name.
#[tauri::command]
pub async fn rename_type(app: AppHandle, what: String, name: String, new_name: String) -> Result<usize, String> {
    check_what(&what)?;
    let graph = app.state::<Graph>();
    let new_name = new_name.trim();
    let changed = match what.as_str() {
        "relation" => graph.rename_relation(&name, new_name).map_err(text)?,
        _ => {
            let pages = graph.entities_of_kind(&name).map_err(text)?;
            for page in &pages {
                graph.change_kind(&page.id, new_name).map_err(text)?;
            }
            pages.len()
        }
    };
    // Both names are settled now: the old one is gone, and the new one is what the user asked for.
    graph.decide(&format!("{what}:{name}")).map_err(text)?;
    graph.decide(&format!("{what}:{new_name}")).map_err(text)?;
    let _ = app.emit("pages-changed", ());
    Ok(changed)
}

/// Removes every relationship of a kind. The pages stay.
#[tauri::command]
pub async fn remove_relation_type(app: AppHandle, name: String) -> Result<usize, String> {
    let removed = app.state::<Graph>().remove_relation(&name).map_err(text)?;
    let _ = app.emit("pages-changed", ());
    Ok(removed)
}

/// Makes one page out of two, keeping everything both had.
#[tauri::command]
pub async fn merge_pages(app: AppHandle, keep: String, remove: String) -> Result<Entity, String> {
    let graph = app.state::<Graph>();
    let gone = graph.get(&remove).map_err(text)?.ok_or("page not found")?;
    let page = graph.merge(&keep, &remove).map_err(text)?;
    app.state::<Vault>().remove(&gone)?;
    app.state::<Notes>().add(format!(
        "[App] The user merged the page \"{}\" into \"{}\" in the app; they are one page now.",
        gone.name, page.name
    ));
    let _ = app.emit("pages-changed", ());
    Ok(page)
}

/// Remembers that two pages are different things, so they aren't offered as duplicates again.
#[tauri::command]
pub async fn not_duplicates(graph: State<'_, Graph>, a: String, b: String) -> Result<(), String> {
    graph.decide(&tidy::distinct_key(&a, &b)).map_err(text)
}

fn check_what(what: &str) -> Result<(), String> {
    match what {
        "relation" | "kind" => Ok(()),
        other => Err(format!("{other} is neither a relationship nor a kind of page")),
    }
}

/// Whether Suk can put things on Google Calendar, and as whom.
#[derive(Serialize)]
pub struct GoogleStatus {
    /// False until the app has a Google client ID to sign in with.
    client_set: bool,
    connected: bool,
    account: Option<String>,
    /// Tasks with a date and a time that Suk hasn't asked about yet.
    offers: Vec<calendar::Offer>,
}

/// The client Suk signs in with: its own, unless the user brought their own Google project.
fn google_client(app: &AppHandle) -> Result<google::AppClient, String> {
    let theirs = Settings::load(&data_dir(app)?).google_client.filter(google::AppClient::is_set);
    Ok(theirs.unwrap_or_else(google::AppClient::built_in))
}

fn google_calendar(app: &AppHandle) -> Result<google::Calendar, String> {
    Ok(google::Calendar::new(google_client(app)?, data_dir(app)?))
}

#[tauri::command]
pub async fn google_status(app: AppHandle) -> Result<GoogleStatus, String> {
    let dir = data_dir(&app)?;
    let session = google::Session::load(&dir);
    Ok(GoogleStatus {
        client_set: google_client(&app)?.is_set(),
        connected: session.is_some(),
        account: session.map(|s| s.account).filter(|a| !a.is_empty()),
        offers: calendar::offers(&app.state::<Graph>()).map_err(text)?,
    })
}

/// Saves the OAuth client the app signs in with, from Google Cloud Console.
#[tauri::command]
pub async fn set_google_client(app: AppHandle, id: String, secret: String) -> Result<(), String> {
    let client = google::AppClient { id: id.trim().to_string(), secret: secret.trim().to_string() };
    if !client.is_set() {
        return Err("That doesn't look like a client ID; it ends in .apps.googleusercontent.com.".into());
    }
    let dir = data_dir(&app)?;
    let mut settings = Settings::load(&dir);
    settings.google_client = Some(client);
    settings.save(&dir)
}

/// Opens Google in the browser and waits for the user to allow access. Returns the account.
#[tauri::command]
pub async fn connect_google(app: AppHandle) -> Result<String, String> {
    let calendar = google_calendar(&app)?;
    let session = tauri::async_runtime::spawn_blocking(move || calendar.sign_in(|url| { let _ = open_target(url); }))
        .await
        .map_err(|e| e.to_string())??;
    Ok(session.account)
}

/// Forgets the Google account. Events already added stay where they are.
#[tauri::command]
pub async fn disconnect_google(app: AppHandle) -> Result<(), String> {
    google::Session::forget(&data_dir(&app)?);
    Ok(())
}

/// Puts a task on Google Calendar, after the user has said yes to the question.
#[tauri::command]
pub async fn add_task_to_calendar(app: AppHandle, id: String) -> Result<String, String> {
    let graph = app.state::<Graph>();
    let task = graph.get(&id).map_err(text)?.ok_or("that task is gone")?;
    let due = task.info.get("due").filter(|d| d.contains('T')).ok_or("that task has no date and time")?;
    let (start, end, when) = calendar::window(due, google::DEFAULT_MINUTES).ok_or("that date and time can't be read")?;
    let block = google::Block {
        page_id: task.id.clone(),
        title: task.name.clone(),
        start,
        end,
        notes: Some(task.notes.clone()).filter(|n| !n.trim().is_empty()),
    };
    let calendar = google_calendar(&app)?;
    let (event_id, link) = tauri::async_runtime::spawn_blocking(move || calendar.add(&block))
        .await
        .map_err(|e| e.to_string())??;

    let mut info = BTreeMap::from([(calendar::ON_CALENDAR.to_string(), Some(event_id))]);
    if !link.is_empty() {
        info.insert("calendar_link".to_string(), Some(link.clone()));
    }
    graph.update_info(&task.id, &info).map_err(text)?;
    app.state::<Notes>().add(format!(
        "[App] The user put the task \"{}\" on their Google Calendar for {when}.",
        task.name
    ));
    let _ = app.emit("pages-changed", ());
    Ok(link)
}

/// Remembers that this task doesn't belong on the calendar, so the question stops coming back.
#[tauri::command]
pub async fn skip_task_calendar(app: AppHandle, id: String) -> Result<(), String> {
    let info = BTreeMap::from([(calendar::ON_CALENDAR.to_string(), Some(calendar::DECLINED.to_string()))]);
    app.state::<Graph>().update_info(&id, &info).map_err(text)?;
    let _ = app.emit("pages-changed", ());
    Ok(())
}

/// Takes a task's event back off the calendar, and lets the question be asked again.
#[tauri::command]
pub async fn remove_task_from_calendar(app: AppHandle, id: String) -> Result<(), String> {
    let graph = app.state::<Graph>();
    let task = graph.get(&id).map_err(text)?.ok_or("that task is gone")?;
    if let Some(event) = task.info.get(calendar::ON_CALENDAR).filter(|e| *e != calendar::DECLINED).cloned() {
        let calendar = google_calendar(&app)?;
        tauri::async_runtime::spawn_blocking(move || calendar.remove(&event)).await.map_err(|e| e.to_string())??;
    }
    let info = BTreeMap::from([
        (calendar::ON_CALENDAR.to_string(), None),
        ("calendar_link".to_string(), None),
    ]);
    graph.update_info(&task.id, &info).map_err(text)?;
    let _ = app.emit("pages-changed", ());
    Ok(())
}

/// Where the user's calendar app subscribes, and what is being published.
#[derive(Serialize)]
pub struct CalendarFeed {
    /// For apps that subscribe: opening this hands the link to the calendar app.
    webcal: String,
    /// The same link as plain http, for pasting into an app that asks for a URL.
    url: String,
    /// False while publishing is turned off.
    on: bool,
    /// How many confirmed blocks are on it.
    blocks: usize,
    /// Where a snapshot is saved for importing into Google Calendar.
    file: String,
}

#[tauri::command]
pub async fn calendar_feed(app: AppHandle) -> Result<CalendarFeed, String> {
    let feed = app.state::<Feed>();
    let settings = Settings::load(&data_dir(&app)?);
    Ok(CalendarFeed {
        webcal: feed.webcal(),
        url: feed.url(),
        on: !settings.calendar_off,
        blocks: calendar::slots(&app.state::<Graph>()).map_err(text)?.len(),
        file: app.state::<Vault>().root().join(calendar::FILE).display().to_string(),
    })
}

/// Turns publishing on or off. Off, the link answers nothing and the calendar app empties it.
#[tauri::command]
pub async fn set_calendar_feed(app: AppHandle, on: bool) -> Result<(), String> {
    let dir = data_dir(&app)?;
    let mut settings = Settings::load(&dir);
    settings.calendar_off = !on;
    settings.save(&dir)
}

/// Hands the link to the calendar app, which asks the user whether to subscribe.
#[tauri::command]
pub async fn subscribe_calendar(app: AppHandle) -> Result<(), String> {
    open_target(&app.state::<Feed>().webcal())
}

/// Saves the blocks as a file, for calendars that import rather than subscribe (Google), and
/// shows it in the file manager.
#[tauri::command]
pub async fn save_calendar_file(app: AppHandle) -> Result<String, String> {
    let path = app.state::<Vault>().root().join(calendar::FILE);
    let text = calendar::ics(&calendar::slots(&app.state::<Graph>()).map_err(text)?);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    std::fs::write(&path, text).map_err(|e| e.to_string())?;
    reveal(&path)?;
    Ok(path.display().to_string())
}

/// The starter templates, and the one in use.
#[tauri::command]
pub async fn templates(app: AppHandle) -> Result<(Vec<Template>, Option<String>), String> {
    let chosen = Settings::load(&data_dir(&app)?).template;
    Ok((templates::TEMPLATES.to_vec(), chosen))
}

/// Applies a starter template: adds its sidebar sections and tells the assistant what kind of
/// work the user does. Sections already there are left alone.
#[tauri::command]
pub async fn apply_template(app: AppHandle, id: String) -> Result<Vec<String>, String> {
    templates::find(&id).ok_or_else(|| format!("no template called {id}"))?;
    let added = templates::apply(&app.state::<Graph>(), &id).map_err(text)?;
    let dir = data_dir(&app)?;
    let mut settings = Settings::load(&dir);
    settings.template = Some(id);
    settings.save(&dir)?;
    Ok(added)
}

/// Chooses the assistant the app uses.
#[tauri::command]
pub async fn choose_assistant(app: AppHandle, kind: Kind) -> Result<(), String> {
    let dir = data_dir(&app)?;
    let mut settings = Settings::load(&dir);
    settings.assistant = Some(kind);
    settings.save(&dir)
}

#[derive(Clone, Serialize)]
pub struct InstallOutput {
    line: String,
}

/// Runs the assistant's official installer, streaming its output.
#[tauri::command]
pub async fn install_assistant(kind: Kind, on_output: Channel<InstallOutput>) -> Result<assistant::Status, String> {
    tauri::async_runtime::spawn_blocking(move || {
        assistant::install(kind, &mut |line| {
            let _ = on_output.send(InstallOutput { line: line.to_string() });
        })?;
        Ok(assistant::status(kind))
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Signs in to the assistant: opens its sign-in page and waits until signing in is done.
#[tauri::command]
pub async fn sign_in_assistant(app: AppHandle, kind: Kind, on_prompt: Channel<SignInPrompt>) -> Result<assistant::Status, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let mut opened = false;
        app.state::<SignIn>().run(kind, &mut |prompt| {
            if let (false, Some(url)) = (opened, &prompt.url) {
                opened = open_target(url).is_ok();
            }
            let _ = on_prompt.send(prompt.clone());
        })?;
        Ok(assistant::status(kind))
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Passes the code Claude's sign-in page shows back to the sign-in in progress.
#[tauri::command]
pub async fn submit_sign_in_code(app: AppHandle, code: String) -> Result<(), String> {
    app.state::<SignIn>().submit_code(&code)
}

#[tauri::command]
pub async fn cancel_sign_in(app: AppHandle) -> Result<(), String> {
    app.state::<SignIn>().cancel();
    Ok(())
}

#[derive(Clone, Serialize)]
struct UpdatesArrived {
    title: String,
    body: String,
    count: usize,
}

/// Checks followed people's sources (or all of one person's, with `only`), judges what's new with
/// Claude, and tells the app window about what's relevant.
pub fn watch_round(app: &AppHandle, only: Option<&str>) -> Result<watch::Round, String> {
    let graph = app.state::<Graph>();
    let kind = chosen(app);
    let binary = kind.and_then(assistant::find);
    let workdir = data_dir(app)?.join("updates");
    let ask = |prompt: &str, schema: &serde_json::Value| match (kind, &binary) {
        (Some(Kind::Claude), Some(binary)) => claude::ask_json(binary, &workdir, watch::JUDGE_SYSTEM, prompt, schema),
        (Some(Kind::Codex), Some(binary)) => codex::ask_json(binary, &workdir, watch::JUDGE_SYSTEM, prompt, schema),
        (Some(Kind::Gemini), Some(binary)) => gemini::ask_json(binary, &workdir, watch::JUDGE_SYSTEM, prompt, schema),
        _ => Err("No assistant is set up".to_string()),
    };
    let ask: Option<&dyn Fn(&str, &serde_json::Value) -> Result<serde_json::Value, String>> = binary.as_ref().map(|_| &ask as _);
    let round = app.state::<Watcher>().round(&graph, &watch::Web, ask, only, now_ms())?;
    for error in &round.errors {
        eprintln!("watch: {error}");
    }
    if round.new > 0 || round.judged > 0 {
        let _ = app.emit("updates-changed", ());
    }
    // Shown inside the app only, never as a system notification.
    if let Some((title, body)) = watch::notification(&round.notify) {
        let _ = app.emit("updates-arrived", UpdatesArrived { title, body, count: round.notify.len() });
    }
    Ok(round)
}

/// Follows or stops following a person. A newly followed person's sources are read right away,
/// so what's already there isn't reported as new later.
#[tauri::command]
pub async fn follow_person(app: AppHandle, id: String, follow: bool) -> Result<WatchInfo, String> {
    let graph = app.state::<Graph>();
    let person = graph.get(&id).map_err(text)?.filter(|e| e.kind == "Person").ok_or("person not found")?;
    let person = watch::set_following(&graph, &person, follow).map_err(text)?;
    if follow {
        let app = app.clone();
        std::thread::spawn(move || {
            if let Err(e) = watch_round(&app, Some(&id)) {
                eprintln!("watch: {e}");
            }
        });
    }
    watch::watch_info(&graph, &person).map_err(text)
}

/// Saves profile links (homepage, X, LinkedIn, Scholar, GitHub, OpenAlex…) under their keys.
#[tauri::command]
pub async fn add_profile_links(graph: State<'_, Graph>, id: String, urls: Vec<String>) -> Result<Entity, String> {
    let urls: Vec<String> = urls.iter().map(|u| u.trim().to_string()).filter(|u| !u.is_empty()).collect();
    for url in &urls {
        links::check_public(url)?;
    }
    watch::add_links(&graph, &id, &urls)
}

/// Removes one profile link, by its detail key.
#[tauri::command]
pub async fn remove_profile_link(graph: State<'_, Graph>, id: String, key: String) -> Result<Entity, String> {
    graph.update_info(&id, &BTreeMap::from([(key, None)])).map_err(text)
}

#[tauri::command]
pub async fn watch_info(graph: State<'_, Graph>, id: String) -> Result<WatchInfo, String> {
    let person = graph.get(&id).map_err(text)?.ok_or("person not found")?;
    watch::watch_info(&graph, &person).map_err(text)
}

/// OpenAlex authors who might be this person, for the user to pick.
#[tauri::command]
pub async fn openalex_candidates(app: AppHandle, id: String) -> Result<Vec<AuthorCandidate>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let person = app.state::<Graph>().get(&id).map_err(text)?.ok_or("person not found")?;
        let name = person.info.get("full_name").cloned().unwrap_or(person.name);
        watch::author_candidates(&watch::Web, &name)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[derive(Serialize)]
pub struct CheckSummary {
    new: usize,
    judged: usize,
    errors: Vec<String>,
}

/// Reads all of a person's sources now.
#[tauri::command]
pub async fn check_person_now(app: AppHandle, id: String) -> Result<CheckSummary, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let round = watch_round(&app, Some(&id))?;
        Ok(CheckSummary { new: round.new, judged: round.judged, errors: round.errors })
    })
    .await
    .map_err(|e| e.to_string())?
}

#[derive(Serialize)]
pub struct Update {
    activity: crate::graph::Activity,
    person: Entity,
}

/// Activity from followed people, newest first. Without `all`, only what's relevant to the
/// user's work. What was already there when following started is listed only for one person.
#[tauri::command]
pub async fn list_updates(graph: State<'_, Graph>, person: Option<String>, all: bool) -> Result<Vec<Update>, String> {
    let query = crate::graph::ActivityQuery { person: person.clone(), relevant_only: !all, limit: 200, ..Default::default() };
    let mut people: BTreeMap<String, Option<Entity>> = BTreeMap::new();
    let mut updates = Vec::new();
    for activity in graph.activities(&query).map_err(text)? {
        if activity.baseline && person.is_none() {
            continue;
        }
        if !people.contains_key(&activity.person) {
            people.insert(activity.person.clone(), graph.get(&activity.person).map_err(text)?);
        }
        if let Some(p) = &people[&activity.person] {
            updates.push(Update { person: p.clone(), activity });
        }
    }
    Ok(updates)
}

/// Marks updates seen: these ids, or all of them.
#[tauri::command]
pub async fn mark_updates_seen(app: AppHandle, ids: Option<Vec<String>>) -> Result<(), String> {
    app.state::<Graph>().mark_activities_seen(ids.as_deref()).map_err(text)?;
    let _ = app.emit("updates-changed", ());
    Ok(())
}
