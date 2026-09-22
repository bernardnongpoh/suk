//! What relationships mean: which kinds of pages each one can join, how to say one in a sentence,
//! and finding the page a loose description refers to.

use crate::graph::{Entity, Graph, GraphError};

/// Whether a relationship makes sense between entities of these kinds.
pub fn allowed(kind: &str, from: &str, to: &str) -> bool {
    let person = |k: &str| k == "Person";
    match kind {
        "WORKS_ON" => person(from) && to == "Project",
        "TAKES" => person(from) && to == "Course",
        // Students don't supervise each other; "Priya and Arjun are my students" once became
        // "Arjun supervises Priya".
        "SUPERVISES" => person(from) && person(to),
        "AUTHORED" => person(from) && to == "Document",
        "HAS_IDEA" => !matches!(from, "Idea" | "Task") && to == "Idea",
        "HAS_TASK" => !matches!(from, "Idea" | "Task") && to == "Task",
        "COLLABORATES_WITH" => person(to),
        "RELATED_TO" => !person(from) && !person(to),
        "SCHEDULED_FOR" => from == "Event" && to == "Task",
        "FOR" | "WAITING_ON" | "ASSIGNED_TO" => from == "Task" && person(to),
        "AFFILIATED_WITH" | "STUDIED_AT" => person(from) && to == "Organization",
        "PART_OF" => from == "Organization" && to == "Organization",
        // Only the notes editor makes these, from [[wikilinks]].
        "LINKS_TO" => false,
        _ => true,
    }
}

/// Exact name match first, then the entity sharing the most meaningful words with the
/// description ("the fuzzing project" finds "Compiler Fuzzing").
pub fn lookup(graph: &Graph, name: &str) -> Result<Option<Entity>, GraphError> {
    if let Some(entity) = graph.find_by_name(name)? {
        return Ok(Some(entity));
    }
    const IGNORED: &[&str] = &[
        "the", "a", "an", "my", "our", "his", "her", "their", "about", "on", "of", "for", "project",
        "idea", "course", "task",
    ];
    let words = |s: &str| -> Vec<String> {
        s.to_lowercase()
            .split(|c: char| !c.is_alphanumeric())
            .filter(|w| !w.is_empty() && !IGNORED.contains(w))
            .map(String::from)
            .collect()
    };
    let wanted = words(name);
    Ok(graph
        .recent_entities(500)?
        .into_iter()
        .map(|e| {
            let overlap = words(&e.name).iter().filter(|w| wanted.contains(w)).count();
            (overlap, e)
        })
        .filter(|(overlap, _)| *overlap > 0)
        .max_by_key(|(overlap, _)| *overlap)
        .map(|(_, e)| e))
}

/// A relationship's details in a few words: "Associate Professor, since 2019",
/// "MTech, until 2015", "Postdoc, 2020 to 2022-06".
pub fn details_text(detail: Option<&str>, since: Option<&str>, until: Option<&str>) -> Option<String> {
    let when = match (since, until) {
        (Some(s), Some(u)) => Some(format!("{s} to {u}")),
        (Some(s), None) => Some(format!("since {s}")),
        (None, Some(u)) => Some(format!("until {u}")),
        (None, None) => None,
    };
    let parts: Vec<String> = detail.map(String::from).into_iter().chain(when).collect();
    (!parts.is_empty()).then(|| parts.join(", "))
}

pub fn sentence(from: &str, kind: &str, to: &str) -> String {
    let verb = match kind {
        "WORKS_ON" => "works on",
        "SUPERVISES" => "supervises",
        "HAS_IDEA" => "has the idea",
        "HAS_TASK" => "has the task",
        "RELATED_TO" => "is related to",
        "COLLABORATES_WITH" => "collaborates with",
        "AUTHORED" => "authored",
        "MENTIONED_IN" => "is mentioned in",
        "ATTACHED_TO" => "is attached to",
        "TAKES" => "takes",
        "SCHEDULED_FOR" => "is scheduled for",
        "LINKS_TO" => "links to",
        "FOR" => "is for",
        "WAITING_ON" => "is waiting on",
        "ASSIGNED_TO" => "is assigned to",
        "AFFILIATED_WITH" => "is at",
        "STUDIED_AT" => "studied at",
        "PART_OF" => "is part of",
        // A relationship the user's own work needed: REVIEWS reads as "reviews".
        other => return format!("{from} {} {to}", other.to_lowercase().replace('_', " ")),
    };
    format!("{from} {verb} {to}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relationships_join_only_sensible_kinds() {
        assert!(allowed("WORKS_ON", "Person", "Project"));
        assert!(!allowed("WORKS_ON", "Project", "Person"));
        assert!(allowed("FOR", "Task", "Person"));
        assert!(!allowed("FOR", "Event", "Course"));
        assert!(!allowed("RELATED_TO", "Project", "Person"));
        assert!(!allowed("LINKS_TO", "Note", "Person"), "only the notes editor makes these");
        assert_eq!(sentence("Satya", "WORKS_ON", "Fuzzing"), "Satya works on Fuzzing");
        assert_eq!(sentence("Kavya", "IS_EXAMINER_FOR", "Satya"), "Kavya is examiner for Satya");
        assert_eq!(sentence("Review draft", "WAITING_ON", "Kavya"), "Review draft is waiting on Kavya");
        assert!(allowed("AFFILIATED_WITH", "Person", "Organization"));
        assert!(!allowed("AFFILIATED_WITH", "Person", "Project"));
        assert!(allowed("PART_OF", "Organization", "Organization"));
        assert_eq!(details_text(Some("Associate Professor"), Some("2019"), None).as_deref(), Some("Associate Professor, since 2019"));
        assert_eq!(details_text(Some("Postdoc"), Some("2020"), Some("2022-06")).as_deref(), Some("Postdoc, 2020 to 2022-06"));
        assert_eq!(details_text(None, None, Some("2015")).as_deref(), Some("until 2015"));
        assert_eq!(details_text(None, None, None), None);
    }

    #[test]
    fn lookup_finds_exact_names_then_the_best_word_match() {
        let g = Graph::in_memory().unwrap();
        g.upsert_entity("Project", "Compiler Fuzzing").unwrap();
        g.upsert_entity("Person", "Satya").unwrap();
        assert_eq!(lookup(&g, "satya").unwrap().unwrap().name, "Satya");
        assert_eq!(lookup(&g, "the fuzzing project").unwrap().unwrap().name, "Compiler Fuzzing");
        assert!(lookup(&g, "Priya").unwrap().is_none());
    }
}
