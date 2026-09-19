//! Starter templates: the sidebar sections a kind of work needs, and a line telling the assistant
//! what the user does. Chosen when setting up, and changeable later in Settings.

use serde::Serialize;

use crate::graph::{Graph, GraphError};

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Template {
    pub id: &'static str,
    pub name: &'static str,
    /// One line about who it's for.
    pub about: &'static str,
    /// What it adds, for the person choosing.
    pub adds: &'static [&'static str],
}

/// Sidebar sections each template starts with, as (tag, title).
fn sections(id: &str) -> &'static [(&'static str, &'static str)] {
    match id {
        "academic" => &[
            ("student", "Students"),
            ("project", "Projects"),
            ("course", "Courses"),
            ("research-area", "Research areas"),
        ],
        "general" => &[("person", "People"), ("project", "Projects")],
        _ => &[],
    }
}

/// Added to the assistant's instructions, so it uses the right words for this kind of work.
pub fn instructions(id: Option<&str>) -> String {
    match id {
        Some("academic") => "\n\nThe user is an academic. Their work is supervising students (programme, thesis, progress, funding), research projects and papers, the courses they teach (code, semester, schedule), grants and funders, conferences and reviewing, and department admin such as committees and accreditation. Use those words, and keep students, courses and research areas as pages of their own."
            .to_string(),
        _ => String::new(),
    }
}

pub const TEMPLATES: &[Template] = &[
    Template {
        id: "academic",
        name: "Academic",
        about: "Professors, postdocs and researchers",
        adds: &["Students, Projects, Courses and Research areas in the sidebar", "The assistant knows about theses, courses, grants and committees"],
    },
    Template {
        id: "general",
        name: "General",
        about: "Anyone running projects and people",
        adds: &["People and Projects in the sidebar"],
    },
];

pub fn find(id: &str) -> Option<&'static Template> {
    TEMPLATES.iter().find(|t| t.id == id)
}

/// Adds the template's sections. Sections already there keep their name, and nothing is removed,
/// so switching templates later only adds.
pub fn apply(graph: &Graph, id: &str) -> Result<Vec<String>, GraphError> {
    let existing: Vec<String> = graph.sections()?.into_iter().map(|s| s.tag).collect();
    let mut added = Vec::new();
    for (tag, title) in sections(id) {
        if !existing.contains(&tag.to_string()) {
            graph.pin_section(tag, title)?;
            added.push((*title).to_string());
        }
    }
    Ok(added)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_template_adds_its_sections_once_and_tells_the_assistant_who_the_user_is() {
        let g = Graph::in_memory().unwrap();
        g.pin_section("project", "My projects").unwrap();
        let added = apply(&g, "academic").unwrap();
        assert_eq!(added, vec!["Students", "Courses", "Research areas"]);
        let sections: Vec<(String, String)> = g.sections().unwrap().into_iter().map(|s| (s.tag, s.title)).collect();
        assert!(sections.contains(&("project".into(), "My projects".into())), "a section already there is left alone");
        assert!(sections.contains(&("student".into(), "Students".into())));
        assert!(apply(&g, "academic").unwrap().is_empty(), "applying it again adds nothing");

        assert!(instructions(Some("academic")).contains("thesis"));
        assert_eq!(instructions(Some("general")), "");
        assert_eq!(instructions(None), "");
        assert_eq!(find("academic").unwrap().name, "Academic");
        assert!(find("nothing").is_none());
    }
}
