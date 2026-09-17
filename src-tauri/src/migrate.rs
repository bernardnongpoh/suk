//! Moves data kept under the app's earlier name, Professor OS, to where Suk keeps it. Runs at
//! startup before anything is opened; each step happens only when the new place doesn't exist yet.

use std::path::Path;

const OLD_IDENTIFIER: &str = "com.professoros.app";
const OLD_DATABASE: &str = "professor.lbdb";
const OLD_FOLDER: &str = "Professor OS";

/// Moves the app data directory, the database file and the Markdown folder. Returns what moved.
pub fn from_old_name(data_dir: &Path, database: &str, documents: &Path, folder: &str) -> Vec<String> {
    let mut moved = Vec::new();
    let mut rename = |from: &Path, to: &Path| {
        if from.exists() && !to.exists() {
            match std::fs::rename(from, to) {
                Ok(()) => moved.push(format!("{} -> {}", from.display(), to.display())),
                Err(e) => eprintln!("migrate: couldn't move {}: {e}", from.display()),
            }
        }
    };
    if let Some(parent) = data_dir.parent() {
        rename(&parent.join(OLD_IDENTIFIER), data_dir);
    }
    rename(&data_dir.join(OLD_DATABASE), &data_dir.join(database));
    rename(&data_dir.join(format!("{OLD_DATABASE}.wal")), &data_dir.join(format!("{database}.wal")));
    rename(&documents.join(OLD_FOLDER), &documents.join(folder));
    moved
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn data_under_the_old_name_moves_once_and_new_data_is_never_replaced() {
        let root = std::env::temp_dir().join(format!("suk-migrate-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let support = root.join("Application Support");
        let documents = root.join("Documents");
        std::fs::create_dir_all(support.join(OLD_IDENTIFIER).join("claude")).unwrap();
        std::fs::write(support.join(OLD_IDENTIFIER).join(OLD_DATABASE), "db").unwrap();
        std::fs::write(support.join(OLD_IDENTIFIER).join("professor.lbdb.wal"), "wal").unwrap();
        std::fs::create_dir_all(documents.join(OLD_FOLDER).join("People")).unwrap();
        let data_dir = support.join("app.suk.desktop");

        let moved = from_old_name(&data_dir, "suk.lbdb", &documents, "Suk");
        assert_eq!(moved.len(), 4, "{moved:?}");
        assert_eq!(std::fs::read_to_string(data_dir.join("suk.lbdb")).unwrap(), "db");
        assert_eq!(std::fs::read_to_string(data_dir.join("suk.lbdb.wal")).unwrap(), "wal");
        assert!(data_dir.join("claude").is_dir() && documents.join("Suk/People").is_dir());
        assert!(!support.join(OLD_IDENTIFIER).exists() && !documents.join(OLD_FOLDER).exists());

        // An old folder appearing again doesn't replace what Suk already has.
        std::fs::create_dir_all(documents.join(OLD_FOLDER)).unwrap();
        assert!(from_old_name(&data_dir, "suk.lbdb", &documents, "Suk").is_empty());
        assert!(documents.join(OLD_FOLDER).exists() && documents.join("Suk/People").is_dir());
        std::fs::remove_dir_all(&root).unwrap();
    }
}
