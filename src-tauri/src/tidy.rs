//! What Suk noticed and wants a decision on: relationships and kinds of page it started using
//! because a message needed them, and pages that look like the same thing twice. Everything here
//! is worked out on this computer, without asking the assistant.

use serde::Serialize;

use crate::graph::{Entity, Graph, GraphError, ENTITY_KINDS, RELATION_KINDS, RESERVED_RELATIONS};
use crate::relations::sentence;

/// A relationship or kind of page in use that isn't one of the app's own.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct NewType {
    pub name: String,
    /// How it reads: "reviews" for REVIEWS, the name itself for a kind.
    pub label: String,
    pub count: usize,
    /// A few examples: sentences for relationships, page names for kinds.
    pub examples: Vec<String>,
}

/// Two pages that may be the same thing.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct DuplicatePair {
    pub keep: Entity,
    pub remove: Entity,
    pub reason: String,
}

#[derive(Debug, Default, Clone, Serialize, PartialEq)]
pub struct TidyItems {
    pub relations: Vec<NewType>,
    pub kinds: Vec<NewType>,
    pub duplicates: Vec<DuplicatePair>,
    pub count: usize,
}

/// "REVIEWS" reads as "reviews".
pub fn relation_label(kind: &str) -> String {
    kind.to_lowercase().replace('_', " ")
}

const EXAMPLES: usize = 3;
/// Beyond this many pages of a kind, duplicates aren't worth hunting through on every look.
const MAX_PAGES_TO_COMPARE: usize = 2000;

pub fn items(graph: &Graph) -> Result<TidyItems, GraphError> {
    let decided = graph.decisions()?;
    let kept = |key: String| !decided.contains(&key);

    let mut relations = Vec::new();
    for (kind, count) in graph.relation_counts()? {
        if RELATION_KINDS.contains(&kind.as_str()) || RESERVED_RELATIONS.contains(&kind.as_str()) || !kept(format!("relation:{kind}")) {
            continue;
        }
        let examples = graph
            .links_of_kind(&kind, EXAMPLES)?
            .into_iter()
            .map(|(from, to)| sentence(&from, &kind, &to))
            .collect();
        relations.push(NewType { label: relation_label(&kind), name: kind, count, examples });
    }

    let mut kinds = Vec::new();
    for (kind, count) in graph.kind_counts()? {
        if ENTITY_KINDS.contains(&kind.as_str()) || !kept(format!("kind:{kind}")) {
            continue;
        }
        let examples = graph.entities_of_kind(&kind)?.into_iter().take(EXAMPLES).map(|e| e.name).collect();
        kinds.push(NewType { label: kind.clone(), name: kind, count, examples });
    }

    let duplicates = duplicates(graph, &decided)?;
    let count = relations.len() + kinds.len() + duplicates.len();
    Ok(TidyItems { relations, kinds, duplicates, count })
}

/// Pages of the same kind whose names look like the same thing: one is the start of the other
/// ("Satya", "Satya Das"), or one is the other's initials ("S. Das").
fn duplicates(graph: &Graph, decided: &[String]) -> Result<Vec<DuplicatePair>, GraphError> {
    let pages: Vec<Entity> = graph.all_entities()?.into_iter().filter(|e| e.kind != "Task" && e.kind != "Note").take(MAX_PAGES_TO_COMPARE).collect();
    let mut found = Vec::new();
    for (i, a) in pages.iter().enumerate() {
        for b in pages.iter().skip(i + 1) {
            if a.kind != b.kind {
                continue;
            }
            // The page with more on it is the one kept, so nothing has to be carried over twice;
            // between equals, the shorter name is the one people use.
            let links_of = |e: &Entity| graph.links(&e.id).unwrap_or_default();
            // Pages already connected are two things that belong together, like a department and
            // its university, not the same thing written twice.
            if links_of(a).iter().any(|l| l.other.id == b.id) {
                continue;
            }
            let weight = |e: &Entity| -> (usize, usize) {
                (links_of(e).len() + e.info.len() + usize::from(!e.notes.trim().is_empty()), usize::MAX - e.name.len())
            };
            let (keep, remove) = if weight(a) >= weight(b) { (a, b) } else { (b, a) };
            if decided.contains(&distinct_key(&keep.id, &remove.id)) {
                continue;
            }
            if keep.aliases.iter().any(|alias| alias.eq_ignore_ascii_case(&remove.name)) {
                continue;
            }
            if let Some(reason) = looks_like(&keep.name, &remove.name) {
                found.push(DuplicatePair { keep: keep.clone(), remove: remove.clone(), reason });
            }
        }
    }
    Ok(found)
}

/// Why two names may be the same person or thing, if they do.
fn looks_like(a: &str, b: &str) -> Option<String> {
    let words = |name: &str| name.to_lowercase().split_whitespace().map(|w| w.trim_end_matches(',').to_string()).collect::<Vec<_>>();
    let (first, second) = (words(a), words(b));
    let (raw_a, raw_b) = (a.to_lowercase(), b.to_lowercase());
    if first.is_empty() || second.is_empty() || first == second {
        return None;
    }
    let (short, long) = if first.len() <= second.len() { (&first, &second) } else { (&second, &first) };
    // Names read back the way the user wrote them, not lowercased for comparing.
    let (shown_short, shown_long) = if first.len() <= second.len() { (a, b) } else { (b, a) };
    // "Satya" and "Satya Das", or "Kavya Rao" and "Dr Kavya Rao". A comma says the longer name
    // is one thing inside another ("CSE, IIT Guwahati"), which is not the same thing twice.
    if short.iter().all(|word| long.contains(word)) {
        let longer = if raw_a.len() >= raw_b.len() { &raw_a } else { &raw_b };
        let named_inside = longer.ends_with(&format!(", {}", short.join(" ")));
        return (!named_inside).then(|| format!("\"{shown_short}\" is part of \"{shown_long}\""));
    }
    // "S. Das" and "Satya Das": same last word, and the first letters match.
    let initial = |word: &String| word.chars().next().unwrap_or(' ');
    if short.len() == long.len()
        && short.last() == long.last()
        && short.iter().zip(long.iter()).all(|(s, l)| s == l || (s.trim_end_matches('.').chars().count() == 1 && initial(s) == initial(l)))
    {
        return Some(format!("\"{shown_short}\" may be short for \"{shown_long}\""));
    }
    None
}

/// How the decision that two pages are different is remembered, whichever way round they come.
pub fn distinct_key(a: &str, b: &str) -> String {
    let (first, second) = if a <= b { (a, b) } else { (b, a) };
    format!("distinct:{first}|{second}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_that_look_like_the_same_thing() {
        assert_eq!(looks_like("Satya", "Satya Das").unwrap(), "\"Satya\" is part of \"Satya Das\"");
        assert!(looks_like("Kavya Rao", "Dr Kavya Rao").is_some());
        assert!(looks_like("S. Das", "Satya Das").unwrap().contains("may be short for"));
        assert!(looks_like("Satya Das", "Satya Das").is_none());
        assert!(looks_like("Satya Das", "Kavya Rao").is_none());
        assert!(looks_like("Fuzzing", "Compiler Testing").is_none());
        assert!(looks_like("IITG", "CSE, IITG").is_none(), "a department is not its university");
        assert!(looks_like("NIT Silchar", "EEE, NIT Silchar").is_none());
        assert_eq!(distinct_key("b", "a"), distinct_key("a", "b"));
    }

    #[test]
    fn new_types_and_duplicates_are_listed_until_decided() {
        let g = Graph::in_memory().unwrap();
        let kavya = g.upsert_entity("Person", "Kavya Rao").unwrap();
        let grant = g.upsert_entity("Grant", "SERB CRG").unwrap();
        g.link(&kavya.id, "REVIEWS", &grant.id).unwrap();
        let project = g.upsert_entity("Project", "Fuzzing").unwrap();
        g.link(&kavya.id, "WORKS_ON", &project.id).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(2));
        let twin = g.upsert_entity("Person", "Dr Kavya Rao").unwrap();

        let found = items(&g).unwrap();
        assert_eq!(found.relations.len(), 1, "only the new relationship, not WORKS_ON");
        assert_eq!(found.relations[0].label, "reviews");
        assert_eq!(found.relations[0].examples, vec!["Kavya Rao reviews SERB CRG"]);
        assert_eq!(found.kinds.iter().map(|k| k.name.as_str()).collect::<Vec<_>>(), vec!["Grant"]);
        assert_eq!(found.kinds[0].examples, vec!["SERB CRG"]);
        assert_eq!(found.duplicates.len(), 1);
        assert_eq!((found.duplicates[0].keep.id.as_str(), found.duplicates[0].remove.id.as_str()), (kavya.id.as_str(), twin.id.as_str()));
        assert_eq!(found.count, 3);

        // A department and its university look alike but belong together.
        let uni = g.upsert_entity("Organization", "IIT Guwahati").unwrap();
        let dept = g.upsert_entity("Organization", "IIT Guwahati CSE").unwrap();
        assert_eq!(items(&g).unwrap().duplicates.len(), 2, "alike until they are connected");
        g.link(&dept.id, "PART_OF", &uni.id).unwrap();
        assert_eq!(items(&g).unwrap().duplicates.len(), 1, "connected pages are not the same thing twice");

        // Decisions are remembered.
        g.decide("relation:REVIEWS").unwrap();
        g.decide("kind:Grant").unwrap();
        g.decide(&distinct_key(&kavya.id, &twin.id)).unwrap();
        assert_eq!(items(&g).unwrap(), TidyItems::default());
    }
}
