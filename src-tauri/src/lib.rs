mod assistant;
mod claude;
#[cfg(test)]
mod claude_eval;
mod codex;
mod commands;
mod graph;
mod links;
mod mcp;
mod migrate;
mod pages;
mod relations;
mod tools;
mod vault;
mod watch;

use std::time::Duration;

use tauri::{AppHandle, Emitter, Manager};

/// Tool calls from Claude, run against the app's database and proposals.
impl mcp::Backend for AppHandle {
    fn call(&self, name: &str, args: serde_json::Value) -> Result<String, String> {
        let ctx = tools::Ctx {
            graph: &self.state::<graph::Graph>(),
            proposals: &self.state::<tools::Proposals>(),
            activity: &self.state::<tools::Activity>(),
            fetch: &watch::Web,
        };
        tools::call(&ctx, name, args)
    }
}

/// Keeps the Obsidian vault and the graph in step, and tells the UI when a file edited in
/// Obsidian changed a page.
fn start_vault_sync(app: AppHandle) {
    std::thread::spawn(move || loop {
        let report = app.state::<vault::Vault>().sync(&app.state::<graph::Graph>());
        match report {
            Ok(report) => {
                if !report.imported.is_empty() || !report.deleted.is_empty() {
                    eprintln!("vault: {report:?}");
                    // The assistant may remember these pages as they were; have it look again.
                    let changed: Vec<String> = report.imported.iter().chain(&report.deleted).cloned().collect();
                    app.state::<assistant::Notes>().add(format!(
                        "[App] These pages were changed in their Markdown files outside the app: {}. Look them up again rather than relying on earlier messages.",
                        changed.join(", ")
                    ));
                    let _ = app.emit("pages-changed", ());
                }
            }
            Err(e) => eprintln!("vault: sync failed: {e}"),
        }
        std::thread::sleep(Duration::from_secs(2));
    });
}

/// Checks people the user follows: shortly after launch, then every minute for sources that
/// are due (each is read at most every few hours).
fn start_watching(app: AppHandle) {
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(20));
        loop {
            if let Err(e) = commands::watch_round(&app, None) {
                eprintln!("watch: {e}");
            }
            std::thread::sleep(Duration::from_secs(60));
        }
    });
}

/// The database file in the app data directory.
const DATABASE: &str = "suk.lbdb";
/// The folder in Documents that holds every page as a Markdown file.
const FOLDER: &str = "Suk";

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            let data_dir = app.path().app_data_dir()?;
            // Linux systems without a desktop setup have no known Documents folder; use ~/Documents.
            let documents = app.path().document_dir().or_else(|_| app.path().home_dir().map(|home| home.join("Documents")))?;
            for moved in migrate::from_old_name(&data_dir, DATABASE, &documents, FOLDER) {
                eprintln!("migrate: moved {moved}");
            }
            std::fs::create_dir_all(&data_dir)?;
            let graph = graph::Graph::open(data_dir.join(DATABASE))?;
            app.manage(graph);
            app.manage(tools::Proposals::default());
            app.manage(tools::Activity::default());
            let vault_root = documents.join(FOLDER);
            app.manage(vault::Vault::new(vault_root));
            start_vault_sync(app.handle().clone());
            app.manage(claude::Claude::default());
            app.manage(codex::Codex::default());
            app.manage(assistant::Notes::default());
            app.manage(assistant::SignIn::default());
            let endpoint = mcp::start(app.handle().clone())?;
            eprintln!("mcp: tools for Claude at {}", endpoint.url);
            app.manage(endpoint);
            app.manage(watch::Watcher::default());
            start_watching(app.handle().clone());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::send_message,
            commands::confirm_proposal,
            commands::dismiss_proposal,
            commands::chat_history,
            commands::list_tasks,
            commands::set_task_done,
            commands::assign_task,
            commands::unlinked_affiliations,
            commands::link_affiliation,
            commands::set_aliases,
            commands::search,
            commands::get_sidebar,
            commands::pin_section,
            commands::dismiss_section,
            commands::list_tagged,
            commands::submit_details,
            commands::update_details,
            commands::skip_details,
            commands::fill_profile,
            commands::save_notes,
            commands::set_tags,
            commands::open_page,
            commands::rename_page,
            commands::delete_page,
            commands::get_vault,
            commands::open_page_file,
            commands::open_url,
            commands::assistant_status,
            commands::choose_assistant,
            commands::install_assistant,
            commands::sign_in_assistant,
            commands::submit_sign_in_code,
            commands::cancel_sign_in,
            commands::list_entities,
            commands::get_entity,
            commands::follow_person,
            commands::add_profile_links,
            commands::remove_profile_link,
            commands::watch_info,
            commands::openalex_candidates,
            commands::check_person_now,
            commands::list_updates,
            commands::mark_updates_seen,
            commands::set_icon,
            commands::set_section_icon,
            commands::set_favorite,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
