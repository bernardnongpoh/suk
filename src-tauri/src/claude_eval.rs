//! A simulated conversation with the real Claude Code CLI, through the same MCP server and runner
//! as the chat window, against an in-memory graph. Opt-in, since it uses the Claude account:
//! `cargo test --lib claude_conversation -- --ignored --nocapture --test-threads 1`

use std::sync::Arc;
use std::time::Instant;

use serde_json::Value;

use crate::claude::{self, Claude, Setup};
use crate::graph::{Graph, ENTITY_KINDS};
use crate::mcp::{self, Backend};
use crate::tools::{self, ItemState, Proposals};

struct Shared {
    graph: Arc<Graph>,
    proposals: Arc<Proposals>,
    activity: Arc<tools::Activity>,
}

impl Backend for Shared {
    fn call(&self, name: &str, args: Value) -> Result<String, String> {
        let ctx = tools::Ctx { graph: &self.graph, proposals: &self.proposals, activity: &self.activity, fetch: &crate::watch::Web };
        let result = tools::call(&ctx, name, args.clone());
        println!("      tool {name} {args} -> {}", match &result {
            Ok(t) => t.chars().take(160).collect::<String>(),
            Err(e) => format!("ERROR {e}"),
        });
        result
    }
}

fn dump(graph: &Graph) {
    for kind in ENTITY_KINDS {
        for e in graph.entities_of_kind(kind).unwrap() {
            println!("  {kind:<12} {} {:?}", e.name, e.info);
            for l in graph.links(&e.id).unwrap().iter().filter(|l| l.outgoing) {
                println!("  {:<12}   {} -> {}", "", l.kind, l.other.name);
            }
        }
    }
}

fn task(graph: &Graph, word: &str) -> Result<crate::graph::Entity, String> {
    graph
        .entities_of_kind("Task")
        .unwrap()
        .into_iter()
        .find(|t| t.name.to_lowercase().contains(word))
        .ok_or_else(|| format!("no task mentioning {word:?}"))
}

#[test]
#[ignore]
fn claude_conversation() {
    let graph = Arc::new(Graph::in_memory().unwrap());
    let proposals = Arc::new(Proposals::default());
    let endpoint = mcp::start(Shared { graph: graph.clone(), proposals: proposals.clone(), activity: Default::default() }).unwrap();
    let workdir = std::env::temp_dir().join(format!("suk-claude-{}", std::process::id()));
    let setup = Setup {
        binary: claude::find_binary().expect("claude not installed"),
        workdir: workdir.clone(),
        mcp: endpoint, about_user: String::new()
    };
    let claude = Claude::default();
    let failures = std::cell::RefCell::new(Vec::new());

    let say = |message: &str, check: &dyn Fn(&Graph, &str) -> Result<(), String>| {
        let start = Instant::now();
        println!("\nUSER: {message}");
        let turn = claude.send(&setup, message, &mut |s| println!("      [{s}]"));
        let text = match turn {
            Ok(turn) => turn.text,
            Err(e) => format!("ERROR {e}"),
        };
        println!("CLAUDE ({:.1}s): {text}", start.elapsed().as_secs_f32());
        if let Err(e) = check(&graph, &text.to_lowercase()) {
            println!("  FAIL: {e}");
            failures.borrow_mut().push(format!("{message}: {e}"));
        }
        text
    };

    say(
        "Amit Kumar joined this semester as my PhD student, email amit.k@example.edu. He'll work on LLM-assisted static analysis.",
        &|g, _| {
            let amit = g.find_by_name("Amit Kumar").unwrap().ok_or("Amit not saved")?;
            if !amit.tags.contains(&"student".to_string()) || amit.info.get("email").map(String::as_str) != Some("amit.k@example.edu") {
                return Err(format!("Amit stored as {amit:?}"));
            }
            if g.entities_of_kind("Person").unwrap().iter().any(|p| p.name.contains('@')) {
                return Err("saved the user as a person".into());
            }
            let works = g.links(&amit.id).unwrap().iter().any(|l| l.kind == "WORKS_ON" && l.other.name.to_lowercase().contains("static analysis"));
            works.then_some(()).ok_or("no WORKS_ON static analysis".into())
        },
    );
    say(
        "Prof Anjali Sharma from IISc Bangalore is collaborating with us on that project.",
        &|g, _| {
            let sharma = g.entities_of_kind("Person").unwrap().into_iter().find(|p| p.name.contains("Sharma")).ok_or("no Sharma")?;
            if !sharma.info.values().any(|v| v.contains("IISc")) {
                return Err(format!("affiliation missing: {sharma:?}"));
            }
            let linked = g.links(&sharma.id).unwrap().iter().any(|l| l.kind == "COLLABORATES_WITH");
            linked.then_some(()).ok_or("not linked to the project".into())
        },
    );
    say(
        "I have to review Amit's literature survey by Friday, it's high priority. The NBA accreditation report for the department is due on the 25th. And I need to prepare slides for lecture 7 of Software Analysis, which is tomorrow at 11.",
        &|g, _| {
            let survey = task(g, "survey")?;
            // "by Friday" is whichever Friday comes next, so check the weekday, not a fixed date.
            let due = survey.info.get("due").cloned().unwrap_or_default();
            let friday = chrono::NaiveDate::parse_from_str(&due[..due.len().min(10)], "%Y-%m-%d")
                .is_ok_and(|d| chrono::Datelike::weekday(&d) == chrono::Weekday::Fri && (d - chrono::Local::now().date_naive()).num_days() <= 7);
            if !friday {
                return Err(format!("survey should be due on the coming Friday: {survey:?}"));
            }
            let nba = task(g, "nba")?;
            let twenty_fifth = chrono::Local::now().format("%Y-%m-25").to_string();
            if !nba.info.get("due").is_some_and(|d| d.starts_with(&twenty_fifth)) {
                return Err(format!("NBA due: {nba:?}"));
            }
            task(g, "slides")?;
            Ok(())
        },
    );
    say("What's Amit's email, and who is he working with?", &|_, r| {
        (r.contains("amit.k@example.edu") && r.contains("sharma")).then_some(()).ok_or("missing email or Sharma".into())
    });
    let before = proposals.count();
    say("What are my priorities today?", &|_, r| {
        (r.contains("survey") && r.contains("slides")).then_some(()).ok_or("doesn't rank the urgent tasks".into())
    });
    let new = proposals.since(before);
    println!("  proposals: {new:#?}");
    if new.is_empty() {
        failures.borrow_mut().push("no schedule proposed".into());
    } else {
        let p = &new[0];
        let confirmed = proposals.confirm(&p.id, &[0]).unwrap();
        tools::record_confirmed(&graph, &confirmed).unwrap();
        let message = format!(
            "[App] The user confirmed proposal {}. Add these to their Google Calendar now, with exactly these titles and local times:\n- \"{}\" from {} to {}\nThen reply in one short sentence.",
            p.id, p.items[0].title, p.items[0].start, p.items[0].end
        );
        let calendar = claude.calendar_status();
        println!("  calendar status: {calendar:?}");
        say(&message, &|g, r| {
            let events = g.entities_of_kind("Event").unwrap();
            if events.len() != 1 {
                return Err(format!("expected only the app's event, got {events:?}"));
            }
            let claims_added = r.contains("added") && !r.contains("n't") && !r.contains("not ");
            if !claude.calendar_usable() && claims_added {
                return Err("claims the event was added without a calendar".into());
            }
            Ok(())
        });
        let state = proposals.get(&p.id).unwrap().states[0];
        println!("  item 0 state after confirm turn: {state:?}");
        assert_ne!(state, ItemState::Proposed);
    }
    say("Done with the lecture 7 slides.", &|g, _| {
        let slides = task(g, "slides")?;
        (slides.info.get("status").map(String::as_str) == Some("done")).then_some(()).ok_or(format!("not done: {slides:?}"))
    });

    println!("\nGRAPH:");
    dump(&graph);
    let _ = std::fs::remove_dir_all(&workdir);
    let failures = failures.into_inner();
    assert!(failures.is_empty(), "{failures:#?}");
}

/// The user mentions a new student, fills in the details form, adds context, then finds the
/// student with search and asks about him from his page. Checks what was stored, what the app
/// offered, what Claude replied, and the page's file in the Obsidian vault.
#[test]
#[ignore]
fn claude_pages_and_focus() {
    use crate::pages::{self, record_turn};
    use std::collections::BTreeMap;

    let graph = Arc::new(Graph::in_memory().unwrap());
    let proposals = Arc::new(Proposals::default());
    let activity = Arc::new(tools::Activity::default());
    let endpoint = mcp::start(Shared { graph: graph.clone(), proposals: proposals.clone(), activity: activity.clone() }).unwrap();
    let workdir = std::env::temp_dir().join(format!("suk-claude-pages-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&workdir);
    let setup = Setup { binary: claude::find_binary().expect("claude not installed"), workdir: workdir.clone(), mcp: endpoint , about_user: String::new()};
    let claude = Claude::default();
    // [App] notes the app sends before the next message.
    let notes = crate::assistant::Notes::default();
    let failures = std::cell::RefCell::new(Vec::new());
    let fail = |step: &str, e: String| {
        println!("  FAIL: {e}");
        failures.borrow_mut().push(format!("{step}: {e}"));
    };

    // Sends like commands::send_message: focus record, then store the turn and compute offers.
    let say = |message: &str, focus: Option<&crate::graph::Entity>| {
        let start = Instant::now();
        std::thread::sleep(std::time::Duration::from_millis(2));
        let started = crate::graph::tests_now();
        activity.begin(crate::links::urls_in(message));
        println!("\nUSER{}: {message}", focus.map(|f| format!(" (on {}'s page)", f.name)).unwrap_or_default());
        let content = match focus {
            Some(page) => pages::claude_message(&graph, Some(page), &[], message).unwrap(),
            None => message.to_string(),
        };
        let text = match claude.send(&setup, &notes.with_message(&content), &mut |s| println!("      [{s}]")) {
            Ok(turn) => turn.text,
            Err(e) => format!("ERROR {e}"),
        };
        println!("CLAUDE ({:.1}s): {text}", start.elapsed().as_secs_f32());
        let (touched, suggested) = activity.take();
        let outcome = record_turn(&graph, started, focus, &[], &touched, &suggested, message, &text).unwrap();
        for s in &outcome.sections {
            println!("  OFFER section {:?} (#{}) for {:?}", s.title, s.tag, s.members);
        }
        for d in &outcome.details {
            println!("  OFFER details for {}: {:?}", d.entity.name, d.fields.iter().map(|f| f.key).collect::<Vec<_>>());
        }
        (text.to_lowercase(), outcome)
    };

    let step = "new student";
    let (reply, outcome) = say("I have a student Satya working on Fuzzing.", None);
    match graph.find_by_name("Satya").unwrap() {
        Some(s) if s.tags.contains(&"student".to_string()) => {
            if !graph.links(&s.id).unwrap().iter().any(|l| l.kind == "WORKS_ON" && l.other.name.to_lowercase().contains("fuzz")) {
                fail(step, "Satya not linked to a fuzzing project".into());
            }
        }
        other => fail(step, format!("Satya stored as {other:?}")),
    }
    if !outcome.sections.iter().any(|s| s.tag == "student") {
        fail(step, "no Students section offered".into());
    }
    if !outcome.details.iter().any(|d| d.entity.name == "Satya" && d.fields.iter().any(|f| f.key == "email")) {
        fail(step, "no details form for Satya".into());
    }
    if reply.contains("email") || reply.contains("full name") {
        fail(step, "Claude asked for details the form already asks for".into());
    }
    if graph.messages_about("person:satya", 10).unwrap().len() != 2 {
        fail(step, "conversation not stored on Satya".into());
    }

    // The user adds the section and fills in the form (as the Add and Save buttons do).
    graph.pin_section("student", "Students").unwrap();
    let satya = graph.find_by_name("Satya").unwrap().expect("Satya");
    let values: BTreeMap<String, Option<String>> = [
        ("full_name", "Satya Prakash Das"),
        ("email", "satya.das@example.edu"),
        ("program", "PhD"),
        ("relationship", "PhD advisee"),
    ]
    .iter()
    .map(|(k, v)| (k.to_string(), Some(v.to_string())))
    .collect();
    graph.update_info(&satya.id, &values).unwrap();
    notes.add("[App] The user filled in details for Satya in the app (already saved): full_name: Satya Prakash Das; email: satya.das@example.edu; program: PhD; relationship: PhD advisee.".into());

    let step = "context";
    let (_, outcome) = say(
        "Met Satya today. He wants to start with the Rust compiler, and he's anxious about his qualifying exam in November.",
        None,
    );
    let satya = graph.get(&satya.id).unwrap().unwrap();
    let notes = satya.notes.to_lowercase();
    if !notes.contains("rust") || !(notes.contains("exam") || notes.contains("qualif")) {
        fail(step, format!("context not kept in Satya's notes: {:?}", satya.notes));
    }
    if outcome.sections.iter().any(|s| s.tag == "student") || outcome.details.iter().any(|d| d.entity.name == "Satya") {
        fail(step, "offered the Students section or Satya's form again".into());
    }

    let step = "search";
    let hits = pages::search(&graph, "satya", 10).unwrap();
    println!("\nSEARCH satya: {:?}", hits.iter().map(|h| (&h.entity.name, &h.field)).collect::<Vec<_>>());
    if hits.first().map(|h| h.entity.name.as_str()) != Some("Satya") {
        fail(step, "Satya is not the top result".into());
    }
    if pages::search(&graph, "prakash", 10).unwrap().first().map(|h| h.entity.name.as_str()) != Some("Satya") {
        fail(step, "full name doesn't find Satya".into());
    }

    let step = "focus question";
    let (reply, _) = say("What's his email, and what is he worried about?", Some(&satya));
    if !reply.contains("satya.das@example.edu") || !(reply.contains("exam") || reply.contains("qualif")) {
        fail(step, "missing email or the exam worry".into());
    }
    if reply.contains("fuzzing") && reply.contains("amit") {
        fail(step, "brought up unrelated things".into());
    }

    let step = "focus update";
    let satya = graph.get(&satya.id).unwrap().unwrap();
    let (_, outcome) = say("He's also joining the program analysis reading group.", Some(&satya));
    let satya = graph.get(&satya.id).unwrap().unwrap();
    let in_group = satya.tags.iter().any(|t| t.contains("reading"))
        || graph.links(&satya.id).unwrap().iter().any(|l| l.other.name.to_lowercase().contains("reading"))
        || satya.notes.to_lowercase().contains("reading group");
    if !in_group {
        fail(step, format!("reading group not recorded on Satya: {satya:?}"));
    }
    println!("  sections offered: {:?}", outcome.sections.iter().map(|s| &s.title).collect::<Vec<_>>());
    if graph.messages_about(&satya.id, 20).unwrap().iter().filter(|m| m.focus.is_some()).count() != 4 {
        fail(step, "page conversation not stored on Satya".into());
    }

    // The page as a file in an Obsidian vault.
    let vault_dir = workdir.join("vault");
    let vault = crate::vault::Vault::new(&vault_dir);
    vault.sync(&graph).unwrap();
    let file = std::fs::read_to_string(vault.path_of(&satya)).unwrap_or_default();
    println!("\nVAULT {}:\n{file}", vault.path_of(&satya).display());
    if !file.contains("email: satya.das@example.edu") || !file.contains("[[") {
        fail("vault", "file lacks details or links".into());
    }

    println!("\nGRAPH:");
    dump(&graph);
    let _ = std::fs::remove_dir_all(&workdir);
    let failures = failures.into_inner();
    assert!(failures.is_empty(), "{failures:#?}");
}

/// People as the user mentions them: details given in chat are filled in, a new name becomes
/// a person the app asks about, a pasted profile link fills in details, and a person picked while
/// typing is used rather than duplicated.
#[test]
#[ignore]
fn claude_people() {
    use crate::graph::role_of;
    use crate::pages::{self, record_turn};
    use std::io::{Read, Write};

    // A profile page served locally, standing in for a university page.
    let server = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let profile_url = format!("http://{}/people/kavya-rao", server.local_addr().unwrap());
    std::thread::spawn(move || {
        for stream in server.incoming().flatten() {
            let mut stream = stream;
            let mut buf = [0u8; 2048];
            let _ = stream.read(&mut buf);
            let body = "<html><head><title>Kavya Rao | Department of CSA, IISc</title></head><body>\
                <h1>Dr. Kavya Rao</h1><p>Associate Professor, Department of Computer Science and Automation</p>\
                <p>Indian Institute of Science, Bangalore</p><p>Email: kavya [at] iisc [dot] ac [dot] in</p>\
                <h2>Research interests</h2><ul><li>Program synthesis</li><li>Verified compilers</li></ul></body></html>";
            let _ = write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
        }
    });

    let graph = Arc::new(Graph::in_memory().unwrap());
    let proposals = Arc::new(Proposals::default());
    let activity = Arc::new(tools::Activity::default());
    activity.allow_local_links.store(true, std::sync::atomic::Ordering::Relaxed);
    let endpoint = mcp::start(Shared { graph: graph.clone(), proposals: proposals.clone(), activity: activity.clone() }).unwrap();
    let workdir = std::env::temp_dir().join(format!("suk-claude-people-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&workdir);
    let setup = Setup { binary: claude::find_binary().expect("claude not installed"), workdir: workdir.clone(), mcp: endpoint , about_user: String::new()};
    let claude = Claude::default();
    let failures = std::cell::RefCell::new(Vec::new());
    let fail = |step: &str, e: String| {
        println!("  FAIL: {e}");
        failures.borrow_mut().push(format!("{step}: {e}"));
    };

    let say = |message: &str, mentioned: &[crate::graph::Entity]| {
        let start = Instant::now();
        std::thread::sleep(std::time::Duration::from_millis(2));
        let started = crate::graph::tests_now();
        activity.begin(crate::links::urls_in(message));
        println!("\nUSER: {message}{}", if mentioned.is_empty() { String::new() } else { format!("  [picked: {:?}]", mentioned.iter().map(|m| &m.name).collect::<Vec<_>>()) });
        let content = pages::claude_message(&graph, None, mentioned, message).unwrap();
        let text = match claude.send(&setup, &content, &mut |s| println!("      [{s}]")) {
            Ok(turn) => turn.text,
            Err(e) => format!("ERROR {e}"),
        };
        println!("CLAUDE ({:.1}s): {text}", start.elapsed().as_secs_f32());
        let (touched, suggested) = activity.take();
        let ids: Vec<String> = mentioned.iter().map(|m| m.id.clone()).collect();
        let outcome = record_turn(&graph, started, None, &ids, &touched, &suggested, message, &text).unwrap();
        for s in &outcome.sections {
            println!("  OFFER section {:?} for {:?}", s.title, s.members);
        }
        for d in &outcome.details {
            println!("  OFFER form for {}: roles asked: {}, fields {:?}, student fields {:?}", d.entity.name, !d.roles.is_empty(),
                d.fields.iter().map(|f| f.key).collect::<Vec<_>>(), d.role_fields.get("student").map(|f| f.iter().map(|f| f.key).collect::<Vec<_>>()));
        }
        (text.to_lowercase(), outcome)
    };
    let person = |word: &str| graph.entities_of_kind("Person").unwrap().into_iter().find(|p| p.name.to_lowercase().contains(word));

    let step = "details in chat";
    let (reply, outcome) = say("Satya Prakash Das has been my PhD student since 2024, email satya.das@example.edu. He works on Fuzzing.", &[]);
    match person("satya") {
        Some(p) => {
            if role_of(&p) != Some("student") { fail(step, format!("no student role: {p:?}")); }
            for (key, want) in [("email", "satya.das@example.edu"), ("program", "phd"), ("start", "2024")] {
                if !p.info.get(key).is_some_and(|v| v.to_lowercase().contains(want)) { fail(step, format!("{key} not {want}: {:?}", p.info)); }
            }
            if let Some(form) = outcome.details.iter().find(|d| d.entity.id == p.id) {
                if !form.roles.is_empty() || form.fields.iter().any(|f| f.key == "email") {
                    fail(step, "form asks for what the message gave".into());
                }
            }
        }
        None => fail(step, "Satya not saved as a person".into()),
    }
    if reply.contains("?") { fail(step, "asked a question instead of saving".into()); }

    let step = "new name";
    let (reply, outcome) = say("I met Dr. Kavya Rao from IISc at the PLDI workshop today.", &[]);
    match person("kavya") {
        Some(p) => {
            if !p.info.values().any(|v| v.contains("IISc")) { fail(step, format!("affiliation missing: {:?}", p.info)); }
            if !outcome.details.iter().any(|d| d.entity.id == p.id) { fail(step, "no form offered for Kavya".into()); }
        }
        None => fail(step, "Kavya not saved as a person".into()),
    }
    if reply.contains("email") { fail(step, "asked for email in the reply".into()); }

    let step = "profile link";
    let (_, _) = say(&format!("Here's her page: {profile_url}"), &[]);
    match person("kavya") {
        Some(p) => {
            println!("  Kavya now: {:?} notes: {:?}", p.info, p.notes);
            if p.info.get("email").map(String::as_str) != Some("kavya@iisc.ac.in") { fail(step, "email not filled from the page".into()); }
            if !p.info.get("position").is_some_and(|v| v.contains("Associate Professor")) { fail(step, "position not filled".into()); }
            if !p.info.get("homepage").is_some_and(|v| v.contains("kavya-rao")) { fail(step, "homepage not set to the link".into()); }
            if !p.notes.to_lowercase().contains("synthesis") { fail(step, "research interests not noted".into()); }
        }
        None => fail(step, "Kavya gone".into()),
    }

    let step = "picked while typing";
    let satya = person("satya").expect("Satya");
    let people_before = graph.entities_of_kind("Person").unwrap().len();
    let (_, _) = say(&format!("{} will present his fuzzing results at the reading group on Friday.", satya.name), &[satya.clone()]);
    if graph.entities_of_kind("Person").unwrap().len() != people_before { fail(step, "created another person".into()); }
    if graph.messages_about(&satya.id, 20).unwrap().len() < 4 { fail(step, "exchange not stored on Satya".into()); }

    println!("\nGRAPH:");
    dump(&graph);
    let _ = std::fs::remove_dir_all(&workdir);
    let failures = failures.into_inner();
    assert!(failures.is_empty(), "{failures:#?}");
}

/// Tasks as the user mentions them: linked to the person they're for, the person being
/// waited on, and their course; planning reads them from the database; ticking one off is seen.
#[test]
#[ignore]
fn claude_tasks() {
    use crate::graph::TaskQuery;
    use crate::pages::{self, record_turn};

    let graph = Arc::new(Graph::in_memory().unwrap());
    let proposals = Arc::new(Proposals::default());
    let activity = Arc::new(tools::Activity::default());
    let endpoint = mcp::start(Shared { graph: graph.clone(), proposals: proposals.clone(), activity: activity.clone() }).unwrap();
    let workdir = std::env::temp_dir().join(format!("suk-claude-tasks-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&workdir);
    let setup = Setup { binary: claude::find_binary().expect("claude not installed"), workdir: workdir.clone(), mcp: endpoint , about_user: String::new()};
    let claude = Claude::default();
    // [App] notes the app sends before the next message.
    let notes = crate::assistant::Notes::default();
    let failures = std::cell::RefCell::new(Vec::new());
    let fail = |step: &str, e: String| {
        println!("  FAIL: {e}");
        failures.borrow_mut().push(format!("{step}: {e}"));
    };
    let say = |message: &str| {
        let start = Instant::now();
        std::thread::sleep(std::time::Duration::from_millis(2));
        let started = crate::graph::tests_now();
        activity.begin(vec![]);
        println!("\nUSER: {message}");
        let content = pages::claude_message(&graph, None, &[], message).unwrap();
        let text = match claude.send(&setup, &notes.with_message(&content), &mut |s| println!("      [{s}]")) {
            Ok(turn) => turn.text,
            Err(e) => format!("ERROR {e}"),
        };
        println!("CLAUDE ({:.1}s): {text}", start.elapsed().as_secs_f32());
        let (touched, suggested) = activity.take();
        record_turn(&graph, started, None, &[], &touched, &suggested, message, &text).unwrap();
        text.to_lowercase()
    };
    let task = |word: &str| {
        graph.tasks(&TaskQuery::default()).unwrap().into_iter().find(|t| t.name.to_lowercase().contains(word))
    };
    let linked = |task_id: &str, kind: &str, word: &str| {
        graph.links(task_id).unwrap().iter().any(|l| l.kind == kind && l.other.name.to_lowercase().contains(word))
    };

    say("Satya is my PhD student working on fuzzing. Kavya Rao from IISc is collaborating with us on the grant proposal. I teach Software Analysis.");

    let step = "task for a student";
    say("I need to review Satya's literature survey by Friday, it's high priority.");
    match task("survey") {
        Some(t) => {
            if t.info.get("due").map(String::as_str) != Some("2026-09-18") { fail(step, format!("due: {:?}", t.info)); }
            if t.info.get("priority").map(String::as_str) != Some("high") { fail(step, format!("priority: {:?}", t.info)); }
            if !linked(&t.id, "FOR", "satya") { fail(step, "not linked FOR Satya".into()); }
        }
        None => fail(step, "no survey task".into()),
    }

    let step = "waiting on someone";
    say("I'm waiting on Kavya's comments on the grant draft before I can submit it on the 25th.");
    match graph.tasks(&TaskQuery::default()).unwrap().into_iter().find(|t| linked(&t.id, "WAITING_ON", "kavya")) {
        Some(t) => println!("  waiting task: {} {:?}", t.name, t.info),
        None => fail(step, "no task WAITING_ON Kavya".into()),
    }

    let step = "task in a course";
    say("Also prepare the slides for lecture 7 of Software Analysis, the lecture is tomorrow at 11.");
    match task("slides") {
        Some(t) => {
            let in_course = graph.links(&t.id).unwrap().iter().any(|l| l.kind == "HAS_TASK" && !l.outgoing && l.other.kind == "Course");
            if !in_course { fail(step, "slides not under the course".into()); }
        }
        None => fail(step, "no slides task".into()),
    }
    let satya = graph.find_by_name("Satya").unwrap().expect("Satya");
    let for_satya = graph.tasks(&TaskQuery { linked_to: Some(satya.id.clone()), open_only: true, ..Default::default() }).unwrap();
    println!("  open tasks on Satya's page: {:?}", for_satya.iter().map(|t| &t.name).collect::<Vec<_>>());
    println!("  database order: {:?}", graph.tasks(&TaskQuery { open_only: true, ..Default::default() }).unwrap().iter().map(|t| (&t.name, t.info.get("due"))).collect::<Vec<_>>());

    let step = "planning";
    let reply = say("What should I focus on today?");
    let slides_at = reply.find("slides");
    let survey_at = reply.find("survey");
    if slides_at.is_none() || survey_at.is_none() { fail(step, "doesn't rank the slides and the survey".into()); }

    let step = "ticked off";
    if let Some(t) = task("survey") {
        graph.set_done(&t.id, true).unwrap();
        notes.add(format!("[App] The user marked done the task \"{}\".", t.name));
    }
    let reply = say("Is there anything I still owe Satya?");
    if reply.contains("survey") && !(reply.contains("done") || reply.contains("finished") || reply.contains("completed") || reply.contains("nothing") || reply.contains("no open")) {
        fail(step, "still lists the survey as open".into());
    }

    println!("\nGRAPH:");
    dump(&graph);
    let _ = std::fs::remove_dir_all(&workdir);
    let failures = failures.into_inner();
    assert!(failures.is_empty(), "{failures:#?}");
}

/// "Update the main heading too": Claude renames the page instead of refusing or making a new one.
#[test]
#[ignore]
fn claude_rename() {
    use crate::pages::{self, record_turn};

    let graph = Arc::new(Graph::in_memory().unwrap());
    let proposals = Arc::new(Proposals::default());
    let activity = Arc::new(tools::Activity::default());
    let endpoint = mcp::start(Shared { graph: graph.clone(), proposals: proposals.clone(), activity: activity.clone() }).unwrap();
    let workdir = std::env::temp_dir().join(format!("suk-claude-rename-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&workdir);
    let setup = Setup { binary: claude::find_binary().expect("claude not installed"), workdir: workdir.clone(), mcp: endpoint , about_user: String::new()};
    let claude = Claude::default();
    let say = |message: &str| {
        std::thread::sleep(std::time::Duration::from_millis(2));
        let started = crate::graph::tests_now();
        activity.begin(vec![]);
        println!("\nUSER: {message}");
        let content = pages::claude_message(&graph, None, &[], message).unwrap();
        let text = claude.send(&setup, &content, &mut |_| {}).map(|t| t.text).unwrap_or_else(|e| format!("ERROR {e}"));
        println!("CLAUDE: {text}");
        let (touched, suggested) = activity.take();
        record_turn(&graph, started, None, &[], &touched, &suggested, message, &text).unwrap();
    };

    say("Tenzin is my new MTech student working on program repair.");
    let before = graph.find_by_name("Tenzin").unwrap().expect("Tenzin saved");
    say("His full name is Tenzin Norbu Bhutia. Update the main heading too.");

    let after = graph.get(&before.id).unwrap().expect("same page still there");
    println!("\npage: {} {:?} {:?}", after.name, after.tags, after.info);
    let people = graph.entities_of_kind("Person").unwrap();
    let _ = std::fs::remove_dir_all(&workdir);
    assert_eq!(after.name, "Tenzin Norbu Bhutia", "heading not renamed");
    assert_eq!(people.len(), 1, "made another page: {people:?}");
    assert!(graph.links(&after.id).unwrap().iter().any(|l| l.kind == "WORKS_ON"), "lost the project link");
    assert!(after.tags.contains(&"student".to_string()));
}

/// Organizations as the user talks about them: one page per institution whatever it's
/// called, departments inside it, positions and dates on the links, moves kept as history, and
/// "who do I know at IITG" answered through departments.
#[test]
#[ignore]
fn claude_organizations() {
    use crate::pages::{self, record_turn};

    let graph = Arc::new(Graph::in_memory().unwrap());
    let proposals = Arc::new(Proposals::default());
    let activity = Arc::new(tools::Activity::default());
    let endpoint = mcp::start(Shared { graph: graph.clone(), proposals: proposals.clone(), activity: activity.clone() }).unwrap();
    let workdir = std::env::temp_dir().join(format!("suk-claude-orgs-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&workdir);
    let setup = Setup { binary: claude::find_binary().expect("claude not installed"), workdir: workdir.clone(), mcp: endpoint , about_user: String::new()};
    let claude = Claude::default();
    let failures = std::cell::RefCell::new(Vec::new());
    let fail = |step: &str, e: String| {
        println!("  FAIL: {e}");
        failures.borrow_mut().push(format!("{step}: {e}"));
    };
    let say = |message: &str| {
        std::thread::sleep(std::time::Duration::from_millis(2));
        let started = crate::graph::tests_now();
        activity.begin(vec![]);
        println!("\nUSER: {message}");
        let content = pages::claude_message(&graph, None, &[], message).unwrap();
        let text = claude.send(&setup, &content, &mut |_| {}).map(|t| t.text).unwrap_or_else(|e| format!("ERROR {e}"));
        println!("CLAUDE: {text}");
        let (touched, suggested) = activity.take();
        record_turn(&graph, started, None, &[], &touched, &suggested, message, &text).unwrap();
        text.to_lowercase()
    };
    let orgs = || graph.entities_of_kind("Organization").unwrap();
    let iitg_pages = || orgs().into_iter().filter(|o| {
        let names: Vec<String> = std::iter::once(o.name.clone()).chain(o.aliases.clone()).map(|n| n.to_lowercase()).collect();
        names.iter().any(|n| n == "iitg" || n == "iit guwahati" || n == "indian institute of technology guwahati")
    }).count();

    say("Satya is my PhD student in the CSE department at IITG. He started in 2024.");
    let step = "department";
    match graph.find_by_name("IITG").unwrap() {
        Some(iitg) if iitg.kind == "Organization" => {
            let at: Vec<String> = graph.affiliations_at(&iitg.id, false).unwrap().into_iter().map(|a| a.person.name).collect();
            if !at.iter().any(|n| n.contains("Satya")) { fail(step, format!("Satya not found at IITG: {at:?}")); }
            let satya = graph.find_by_name("Satya").unwrap().unwrap();
            if !graph.links(&satya.id).unwrap().iter().any(|l| l.kind == "AFFILIATED_WITH") { fail(step, "a current student should be AFFILIATED_WITH".into()); }
            if !satya.info.get("affiliation").is_some_and(|a| a.contains("IIT Guwahati") || a.contains("CSE")) { fail(step, format!("shown affiliation: {:?}", satya.info.get("affiliation"))); }
        }
        other => fail(step, format!("IITG isn't an organization page: {other:?}")),
    }

    say("Kavya Rao is an Associate Professor at IISc since 2019. She did her MTech at IIT Guwahati.");
    let step = "same institution by another name";
    if iitg_pages() != 1 { fail(step, format!("IITG pages: {:?}", orgs().iter().map(|o| (&o.name, &o.aliases)).collect::<Vec<_>>())); }
    if let Some(kavya) = graph.find_by_name("Kavya Rao").unwrap() {
        let links = graph.links(&kavya.id).unwrap();
        let iisc = links.iter().find(|l| l.kind == "AFFILIATED_WITH" && l.other.name.contains("IISc") || l.kind == "AFFILIATED_WITH" && l.other.aliases.iter().any(|a| a == "IISc"));
        match iisc {
            Some(l) if l.detail.as_deref().is_some_and(|d| d.contains("Associate Professor")) && l.since.as_deref() == Some("2019") => {}
            other => fail(step, format!("IISc link: {other:?}")),
        }
        if !links.iter().any(|l| l.kind == "STUDIED_AT") { fail(step, "no STUDIED_AT".into()); }
    } else {
        fail(step, "no Kavya".into());
    }

    say("Arun was a postdoc at IITG until June 2022. He's now an engineer at Google.");
    let step = "moved";
    if let Some(arun) = graph.find_by_name("Arun").unwrap() {
        let links = graph.links(&arun.id).unwrap();
        let old = links.iter().find(|l| l.kind == "AFFILIATED_WITH" && l.until.is_some());
        if old.is_none() { fail(step, format!("old position has no end: {links:?}")); }
        if arun.info.get("affiliation").map(String::as_str) != Some("Google") { fail(step, format!("shown affiliation: {:?}", arun.info.get("affiliation"))); }
    } else {
        fail(step, "no Arun".into());
    }
    if iitg_pages() != 1 { fail(step, "IITG duplicated".into()); }

    let step = "who at";
    let reply = say("Who do I know at IITG?");
    for name in ["satya", "kavya", "arun"] {
        if !reply.contains(name) { fail(step, format!("{name} missing from the answer")); }
    }

    println!("\nORGANIZATIONS:");
    for o in orgs() {
        println!("  {} {:?}", o.name, o.aliases);
        for l in graph.links(&o.id).unwrap() {
            println!("    {} {} {} {:?} {:?} {:?}", if l.outgoing { "->" } else { "<-" }, l.kind, l.other.name, l.detail, l.since, l.until);
        }
    }
    let _ = std::fs::remove_dir_all(&workdir);
    let failures = failures.into_inner();
    assert!(failures.is_empty(), "{failures:#?}");
}

/// Delegated tasks: "Rohan and Kabir will present…" is assigned to both, planning treats it as
/// theirs, and "what have I given Rohan" finds it.
#[test]
#[ignore]
fn claude_assigned_tasks() {
    use crate::graph::TaskQuery;
    use crate::pages::{self, record_turn};

    let graph = Arc::new(Graph::in_memory().unwrap());
    let proposals = Arc::new(Proposals::default());
    let activity = Arc::new(tools::Activity::default());
    let endpoint = mcp::start(Shared { graph: graph.clone(), proposals: proposals.clone(), activity: activity.clone() }).unwrap();
    let workdir = std::env::temp_dir().join(format!("suk-claude-assign-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&workdir);
    let setup = Setup { binary: claude::find_binary().expect("claude not installed"), workdir: workdir.clone(), mcp: endpoint , about_user: String::new()};
    let claude = Claude::default();
    let say = |message: &str| {
        std::thread::sleep(std::time::Duration::from_millis(2));
        let started = crate::graph::tests_now();
        activity.begin(vec![]);
        println!("\nUSER: {message}");
        let content = pages::claude_message(&graph, None, &[], message).unwrap();
        let text = claude.send(&setup, &content, &mut |_| {}).map(|t| t.text).unwrap_or_else(|e| format!("ERROR {e}"));
        println!("CLAUDE: {text}");
        let (touched, suggested) = activity.take();
        record_turn(&graph, started, None, &[], &touched, &suggested, message, &text).unwrap();
        text.to_lowercase()
    };

    say("Rohan Das and Kabir Mehta are my BTech students working on Solidity Compiler Fuzzing.");
    say("They will present the state of the art on Solidity compiler fuzzing at our meeting on the 24th. I also need to review their literature list before that.");
    let assigned = graph.tasks(&TaskQuery { assigned: Some(true), ..Default::default() }).unwrap();
    let mine = graph.tasks(&TaskQuery { assigned: Some(false), ..Default::default() }).unwrap();
    println!("\nassigned: {:?}\nmine: {:?}", assigned.iter().map(|t| &t.name).collect::<Vec<_>>(), mine.iter().map(|t| &t.name).collect::<Vec<_>>());
    let mut failures = Vec::new();
    match assigned.iter().find(|t| t.name.to_lowercase().contains("state of the art")) {
        Some(t) => {
            let people: Vec<String> = graph.links(&t.id).unwrap().into_iter().filter(|l| l.kind == "ASSIGNED_TO").map(|l| l.other.name).collect();
            println!("presentation assigned to {people:?}, due {:?}", t.info.get("due"));
            if people.len() != 2 { failures.push(format!("assigned to {people:?}")); }
        }
        None => failures.push("presentation not assigned".into()),
    }
    if !mine.iter().any(|t| t.name.to_lowercase().contains("review")) { failures.push("the review isn't the user's own task".into()); }

    let reply = say("What have I given Rohan to do?");
    if !reply.contains("state of the art") && !reply.contains("present") { failures.push("doesn't list the presentation".into()); }
    let _ = std::fs::remove_dir_all(&workdir);
    assert!(failures.is_empty(), "{failures:#?}");
}

/// Following a researcher from pasted links, picking their OpenAlex author, a real check of their
/// papers and homepage, and Claude judging new items against the user's work.
/// Uses the network (OpenAlex, the homepage) as well as the Claude account.
#[test]
#[ignore]
fn claude_following() {
    use crate::graph::{Activity, ActivityQuery};
    use crate::pages::{self, record_turn};
    use crate::watch::{self, Watcher};

    let graph = Arc::new(Graph::in_memory().unwrap());
    let project = graph.upsert_entity("Project", "Solidity Compiler Fuzzing").unwrap();
    graph.set_notes(&project.id, "BTech project with Rohan and Kabir: grammar-based fuzzing of the Solidity compiler (solc) to find miscompilations.").unwrap();
    graph.upsert_entity("ResearchArea", "Compiler Testing").unwrap();
    graph.upsert_entity("Idea", "LLM-generated test programs for smart contract compilers").unwrap();

    let proposals = Arc::new(Proposals::default());
    let activity = Arc::new(tools::Activity::default());
    let endpoint = mcp::start(Shared { graph: graph.clone(), proposals: proposals.clone(), activity: activity.clone() }).unwrap();
    let workdir = std::env::temp_dir().join(format!("suk-claude-follow-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&workdir);
    let binary = claude::find_binary().expect("claude not installed");
    let setup = Setup { binary: binary.clone(), workdir: workdir.clone(), mcp: endpoint , about_user: String::new()};
    let claude = Claude::default();
    let say = |message: &str| {
        std::thread::sleep(std::time::Duration::from_millis(2));
        let started = crate::graph::tests_now();
        activity.begin(crate::links::urls_in(message));
        println!("\nUSER: {message}");
        let content = pages::claude_message(&graph, None, &[], message).unwrap();
        let text = claude.send(&setup, &content, &mut |s| println!("      [{s}]")).map(|t| t.text).unwrap_or_else(|e| format!("ERROR {e}"));
        println!("CLAUDE: {text}");
        let (touched, suggested) = activity.take();
        record_turn(&graph, started, None, &[], &touched, &suggested, message, &text).unwrap();
        text.to_lowercase()
    };
    let mut failures = Vec::new();

    let reply = say("I want to keep track of Andreas Zeller's work, tell me when he does something related to my research. https://x.com/AndreasZeller https://andreas-zeller.info/ https://www.linkedin.com/in/andreaszeller/");
    let person = graph.find_by_name("Andreas Zeller").unwrap();
    match &person {
        Some(p) => {
            println!("info {:?} tags {:?}", p.info, p.tags);
            if !p.tags.iter().any(|t| t == watch::FOLLOWING) { failures.push("not followed".to_string()); }
            for key in ["twitter", "homepage", "linkedin"] {
                if !p.info.contains_key(key) { failures.push(format!("{key} not saved")); }
            }
            if p.info.contains_key("openalex") { failures.push("picked an OpenAlex author without asking".into()); }
        }
        None => failures.push("person not saved".into()),
    }
    if !reply.contains("linkedin") { failures.push("doesn't say LinkedIn can't be checked".into()); }
    if !reply.contains("cispa") && !reply.contains("saarland") { failures.push("doesn't ask which author (CISPA)".into()); }

    say("The CISPA one.");
    let person = graph.find_by_name("Andreas Zeller").unwrap().expect("person");
    if person.info.get("openalex").map(String::as_str) != Some("A5051672229") {
        failures.push(format!("openalex is {:?}", person.info.get("openalex")));
    }

    let ask = |prompt: &str, schema: &Value| claude::ask_json(&binary, &workdir.join("judge"), watch::JUDGE_SYSTEM, prompt, schema);
    let watcher = Watcher::default();
    let first = watcher.round(&graph, &watch::Web, Some(&ask), None, crate::graph::tests_now()).unwrap();
    let baseline = graph.activities(&ActivityQuery { person: Some(person.id.clone()), ..Default::default() }).unwrap();
    println!("\nfirst round {first:?}; {} baseline items, e.g. {:?}", baseline.len(), baseline.iter().take(3).map(|a| (&a.published, &a.title)).collect::<Vec<_>>());
    if first.new != 0 || baseline.is_empty() { failures.push(format!("baseline: new {}, stored {}", first.new, baseline.len())); }
    if !first.errors.is_empty() { failures.push(format!("errors {:?}", first.errors)); }

    let item = |id: &str, title: &str, summary: &str| Activity {
        id: id.into(),
        person: person.id.clone(),
        source: "openalex".into(),
        kind: "paper".into(),
        title: title.into(),
        url: format!("https://doi.org/10.0/{id}"),
        summary: summary.into(),
        published: "2026-09-16".into(),
        found_at: crate::graph::tests_now(),
        baseline: false,
        relevance: String::new(),
        reason: String::new(),
        related: vec![],
        seen: false,
        notified: false,
    };
    graph.add_activity(&item("t-solc", "Finding Miscompilations in Solidity Compilers with Grammar-Based Fuzzing", "ICSE 2027 — We generate Solidity programs from a grammar and compare solc optimization levels, finding 14 miscompilations.")).unwrap();
    graph.add_activity(&item("t-debug", "Explaining Failures with Input Grammars", "FSE — Learning which input features cause a program failure.")).unwrap();
    graph.add_activity(&item("t-bio", "Protein Structure Prediction at Scale", "Nature — A deep learning model for folding proteins.")).unwrap();
    let round = watcher.round(&graph, &watch::Web, Some(&ask), Some(&person.id), crate::graph::tests_now()).unwrap();
    let judged = graph.activities(&ActivityQuery { person: Some(person.id.clone()), ..Default::default() }).unwrap();
    for a in judged.iter().filter(|a| a.id.starts_with("t-")) {
        println!("{:<8} {:<6} {} — {} {:?}", a.id, a.relevance, a.title, a.reason, a.related);
    }
    let relevance = |id: &str| judged.iter().find(|a| a.id == id).map(|a| a.relevance.clone()).unwrap_or_default();
    if relevance("t-solc") != "high" { failures.push(format!("solc paper judged {}", relevance("t-solc"))); }
    if !["none", "low"].contains(&relevance("t-bio").as_str()) { failures.push(format!("protein paper judged {}", relevance("t-bio"))); }
    let notified: Vec<&str> = round.notify.iter().map(|(a, _)| a.id.as_str()).collect();
    println!("notify {notified:?} -> {:?}", watch::notification(&round.notify));
    if !notified.contains(&"t-solc") || notified.contains(&"t-bio") { failures.push(format!("notified {notified:?}")); }

    let reply = say("Anything new from people I follow?");
    if !reply.contains("solidity") { failures.push("doesn't mention the Solidity paper".into()); }
    if reply.contains("protein") { failures.push("mentions the unrelated paper".into()); }

    let _ = std::fs::remove_dir_all(&workdir);
    assert!(failures.is_empty(), "{failures:#?}");
}

/// Project statuses from how the user talks about projects, and listing by status.
#[test]
#[ignore]
fn claude_project_status() {
    use crate::pages::{self, record_turn};

    let graph = Arc::new(Graph::in_memory().unwrap());
    let proposals = Arc::new(Proposals::default());
    let activity = Arc::new(tools::Activity::default());
    let endpoint = mcp::start(Shared { graph: graph.clone(), proposals: proposals.clone(), activity: activity.clone() }).unwrap();
    let workdir = std::env::temp_dir().join(format!("suk-claude-status-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&workdir);
    let setup = Setup { binary: claude::find_binary().expect("claude not installed"), workdir: workdir.clone(), mcp: endpoint , about_user: String::new()};
    let claude = Claude::default();
    let say = |message: &str| {
        std::thread::sleep(std::time::Duration::from_millis(2));
        let started = crate::graph::tests_now();
        activity.begin(vec![]);
        println!("\nUSER: {message}");
        let content = pages::claude_message(&graph, None, &[], message).unwrap();
        let text = claude.send(&setup, &content, &mut |_| {}).map(|t| t.text).unwrap_or_else(|e| format!("ERROR {e}"));
        println!("CLAUDE: {text}");
        let (touched, suggested) = activity.take();
        record_turn(&graph, started, None, &[], &touched, &suggested, message, &text).unwrap();
        text.to_lowercase()
    };
    let status = |word: &str| {
        graph
            .entities_of_kind("Project")
            .unwrap()
            .into_iter()
            .find(|p| p.name.to_lowercase().contains(word))
            .map(|p| p.info.get("status").cloned().unwrap_or_default())
    };
    let mut failures = Vec::new();
    let expect = |word: &str, want: &str, failures: &mut Vec<String>| {
        let got = status(word);
        println!("  {word}: {got:?}");
        if got.as_deref() != Some(want) {
            failures.push(format!("{word}: {got:?}, want {want}"));
        }
    };

    say("Rohan Das and Kabir Mehta are my BTech students working on Solidity Compiler Fuzzing.");
    expect("solidity", "in-progress", &mut failures);
    say("Next semester I plan to start a project on LLM-based program repair with Vikram from Qualcomm.");
    expect("repair", "planned", &mut failures);
    say("Also, our Grammar Inference project is done, the paper got accepted at FSE.");
    expect("grammar", "completed", &mut failures);
    let reply = say("Which of my projects are ongoing right now?");
    if !reply.contains("solidity") || reply.contains("grammar inference") {
        failures.push("ongoing projects answer is wrong".into());
    }
    say("We've started the program repair project this week.");
    expect("repair", "in-progress", &mut failures);

    // New projects get an icon from Claude; people don't.
    for project in graph.entities_of_kind("Project").unwrap() {
        println!("  icon {:?} for {}", project.info.get("icon"), project.name);
        if !project.info.get("icon").is_some_and(|i| crate::graph::check_icon(i).is_ok()) {
            failures.push(format!("no icon for {}", project.name));
        }
    }
    if let Some(person) = graph.entities_of_kind("Person").unwrap().into_iter().find(|p| p.info.contains_key("icon")) {
        failures.push(format!("{} was given an icon", person.name));
    }

    let _ = std::fs::remove_dir_all(&workdir);
    assert!(failures.is_empty(), "{failures:#?}");
}

/// A pasted student isn't guessed to be the answer to an unrelated open request, and a correction
/// ("this is wrong") fixes what was saved: the task's name, its links and its notes.
#[test]
#[ignore]
fn claude_corrections() {
    use crate::pages::{self, record_turn};

    fn start(tag: &str) -> (Arc<Graph>, Arc<tools::Activity>, Setup, Claude, std::path::PathBuf) {
        let graph = Arc::new(Graph::in_memory().unwrap());
        let activity = Arc::new(tools::Activity::default());
        let endpoint = mcp::start(Shared { graph: graph.clone(), proposals: Default::default(), activity: activity.clone() }).unwrap();
        let workdir = std::env::temp_dir().join(format!("suk-claude-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&workdir);
        let setup = Setup { binary: claude::find_binary().expect("claude not installed"), workdir: workdir.clone(), mcp: endpoint , about_user: String::new()};
        (graph, activity, setup, Claude::default(), workdir)
    }
    fn say(graph: &Graph, activity: &tools::Activity, setup: &Setup, claude: &Claude, message: &str) -> String {
        std::thread::sleep(std::time::Duration::from_millis(2));
        let started = crate::graph::tests_now();
        activity.begin(vec![]);
        println!("\nUSER: {message}");
        let content = pages::claude_message(graph, None, &[], message).unwrap();
        let text = claude.send(setup, &content, &mut |_| {}).map(|t| t.text).unwrap_or_else(|e| format!("ERROR {e}"));
        println!("CLAUDE: {text}");
        let (touched, suggested) = activity.take();
        record_turn(graph, started, None, &[], &touched, &suggested, message, &text).unwrap();
        text.to_lowercase()
    }
    /// Faculty-advisory tasks tied to Nisha: by name, notes, or a link to her or her project.
    fn wrongly_tied(graph: &Graph) -> Vec<String> {
        let mut wrong = Vec::new();
        for task in graph.entities_of_kind("Task").unwrap() {
            let text = format!("{} {} {:?}", task.name, task.notes, task.info).to_lowercase();
            let advisory = text.contains("advis");
            let links: Vec<String> = graph.links(&task.id).unwrap().into_iter().map(|l| l.other.name.to_lowercase()).collect();
            println!("  task {:?} info {:?} notes {:?} links {links:?}", task.name, task.info, task.notes);
            let tied = text.contains("nisha") || text.contains("her faculty") || links.iter().any(|n| n.contains("nisha") || n.contains("auto build"));
            if advisory && tied {
                wrong.push(task.name);
            }
        }
        wrong
    }
    let mut failures = Vec::new();

    // The conversation that went wrong.
    let (graph, activity, setup, claude, workdir) = start("tie");
    say(&graph, &activity, &setup, &claude, "Follow up on email sent to students whom I am the Faculty Advisory");
    say(&graph, &activity, &setup, &claude, "NISHA VERMA\nComputer Science and Engineering\n•\nMTech nisha.verma@example.edu work on Auto Build System Analysis");
    let wrong = wrongly_tied(&graph);
    if !wrong.is_empty() {
        failures.push(format!("guessed Nisha is a faculty-advisory student: {wrong:?}"));
    }
    for task in graph.entities_of_kind("Task").unwrap() {
        let name = task.name.to_lowercase();
        if name.contains("nisha") && name.contains("email") {
            failures.push(format!("guessed the email follow-up is about Nisha: {}", task.name));
        }
    }

    // Correcting it later in the same conversation, whatever was saved.
    say(&graph, &activity, &setup, &claude, "she is my MTP");
    let reply = say(&graph, &activity, &setup, &claude, "This is wrong Follow up on email to Nisha Verma (Faculty Advisory)\nTask  Nisha is my MTP students not Faculty Advisory");
    let wrong = wrongly_tied(&graph);
    if !wrong.is_empty() {
        failures.push(format!("not corrected: {wrong:?}"));
    }
    // The follow-up is about the faculty-advisory students, so no task about emailing Nisha remains.
    for task in graph.entities_of_kind("Task").unwrap() {
        let name = task.name.to_lowercase();
        if name.contains("nisha") && name.contains("email") {
            failures.push(format!("still a task about emailing Nisha: {}", task.name));
        }
    }
    let nisha = graph.find_by_name("Nisha Verma").unwrap().expect("Nisha kept");
    if !nisha.tags.contains(&"student".to_string()) {
        failures.push(format!("Nisha lost her student role: {:?}", nisha.tags));
    }
    if reply.starts_with("error") {
        failures.push(reply);
    }
    let _ = std::fs::remove_dir_all(&workdir);
    assert!(failures.is_empty(), "{failures:#?}");
}

/// A fixed ten-message conversation, reporting the tokens each message used, so changes to the
/// instructions, tools or sessions can be compared. Also checks the basics were recorded.
#[test]
#[ignore]
fn claude_token_budget() {
    use crate::claude::Usage;
    use crate::pages::{self, record_turn};

    let graph = Arc::new(Graph::in_memory().unwrap());
    let proposals = Arc::new(Proposals::default());
    let activity = Arc::new(tools::Activity::default());
    let endpoint = mcp::start(Shared { graph: graph.clone(), proposals: proposals.clone(), activity: activity.clone() }).unwrap();
    let workdir = std::env::temp_dir().join(format!("suk-claude-budget-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&workdir);
    let setup = Setup { binary: claude::find_binary().expect("claude not installed"), workdir: workdir.clone(), mcp: endpoint , about_user: String::new()};
    let claude = Claude::default();
    let script = [
        "Satya Das joined as my PhD student this semester, email satya@example.edu. He'll work on compiler fuzzing with Kavya Rao from IISc.",
        "I need to review Satya's literature survey by Friday, it's high priority.",
        "Kavya is sending the grant budget next week; I'm waiting on her.",
        "Note for the compiler fuzzing project: we decided to target LLVM first.",
        "Meera Iyer is my MTech student; she's helping Satya with the fuzzing infrastructure.",
        "What does Satya have going on?",
        "The NBA accreditation report is due on the 30th.",
        "Which of my tasks are due this week?",
        "I finished reviewing Satya's survey.",
        "Who is working on compiler fuzzing?",
    ];
    let mut total = Usage::default();
    println!("\n{:<4} {:>5} {:>9} {:>9} {:>7} {:>7}  message", "#", "calls", "input", "cached", "output", "context");
    for (i, message) in script.iter().enumerate() {
        std::thread::sleep(std::time::Duration::from_millis(2));
        let started = crate::graph::tests_now();
        activity.begin(vec![]);
        let mut content = pages::claude_message(&graph, None, &[], message).unwrap();
        if claude.begins_fresh(&setup) {
            content = format!("{}{content}", pages::recent_brief(&graph).unwrap());
        }
        let turn = claude.send(&setup, &content, &mut |_| {}).expect("turn");
        let (touched, suggested) = activity.take();
        record_turn(&graph, started, None, &[], &touched, &suggested, message, &turn.text).unwrap();
        let u = turn.usage;
        total += u;
        println!("{:<4} {:>5} {:>9} {:>9} {:>7} {:>7}  {}", i + 1, u.calls, u.input, u.cached, u.output, u.last_context, &message[..message.len().min(50)]);
        println!("       reply: {}", turn.text.replace('\n', " ").chars().take(160).collect::<String>());
    }
    println!("{:<4} {:>5} {:>9} {:>9} {:>7}", "all", total.calls, total.input, total.cached, total.output);

    let mut failures = Vec::new();
    let satya = graph.find_by_name("Satya Das").unwrap().or_else(|| graph.find_by_name("Satya").unwrap());
    match &satya {
        Some(s) if s.tags.contains(&"student".to_string()) => {}
        other => failures.push(format!("Satya not a student: {other:?}")),
    }
    let tasks = graph.tasks(&crate::graph::TaskQuery::default()).unwrap();
    if !tasks.iter().any(|t| t.name.to_lowercase().contains("survey") && t.info.get("status").map(String::as_str) == Some("done")) {
        failures.push("survey review not done".into());
    }
    if !tasks.iter().any(|t| t.name.to_lowercase().contains("nba")) {
        failures.push("NBA task missing".into());
    }
    let _ = std::fs::remove_dir_all(&workdir);
    assert!(failures.is_empty(), "{failures:#?}");
}
