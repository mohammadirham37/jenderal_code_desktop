//! JenderalCode Desktop — Tauri application entry point.

mod agent_detect;
mod agent_usage;
mod browser;
mod commands;
mod quit;
mod sink;
mod state;
mod terminal;
mod watcher;

use jcode_provider_core::SwitchableCredentialStore;
use jcode_storage::Db;
use state::AppState;
use std::sync::{Arc, Mutex, RwLock};
use tauri::Manager;

/// Holds the active workspace file watcher (dropping it stops watching).
pub struct WatcherStore(pub Mutex<Option<notify::RecommendedWatcher>>);

/// For every provider credential that is missing from the active store but
/// exists in the OS keychain, copy it over. Silently skips when the keychain
/// is unavailable or the user cancels a prompt.
async fn migrate_legacy_keychain_keys(
    db: &jcode_storage::Db,
    active: &jcode_provider_core::SwitchableCredentialStore,
) {
    use jcode_provider_core::CredentialStore as _;
    let Ok(providers) = db.list_providers() else {
        return;
    };
    let keyring = jcode_provider_core::KeyringCredentialStore;
    for p in providers {
        let Some(r) = &p.credential_ref else { continue };
        if active.get(r).await.unwrap_or(None).is_some() {
            continue; // already present in the active store
        }
        if let Ok(Some(key)) = keyring.get(r).await {
            if !key.is_empty() {
                let _ = active.set(r, &key).await;
                let _ = keyring.delete(r).await; // avoid future prompts
            }
        }
    }
}

/// Only the app's own origin (and the Vite dev server in debug builds) may
/// be loaded in a webview. Anything else (a link in model output, an
/// injected `location = ...`) is blocked.
fn is_allowed_navigation(url: &tauri::Url) -> bool {
    let host = url.host_str();
    match url.scheme() {
        "tauri" | "asset" => host == Some("localhost"),
        "http" | "https" => match host {
            Some("tauri.localhost") | Some("asset.localhost") => true,
            Some("localhost") => cfg!(debug_assertions) && url.port() == Some(5173),
            _ => false,
        },
        "about" => url.as_str() == "about:blank",
        _ => false,
    }
}

fn navigation_guard<R: tauri::Runtime>() -> tauri::plugin::TauriPlugin<R> {
    tauri::plugin::Builder::new("navigation-guard")
        .on_navigation(|webview, url| {
            // Browser previews follow their own (http/https-only) rule.
            let allowed = if browser::is_preview_label(webview.label()) {
                browser::is_allowed_preview_navigation(url)
            } else {
                is_allowed_navigation(url)
            };
            if !allowed {
                log::warn!("blocked webview navigation to {url}");
            }
            allowed
        })
        .build()
}

fn main() {
    // Agent commands need the user's login-shell PATH (Homebrew, nvm, ...);
    // resolve it off the startup path.
    std::thread::spawn(jcode_tool_terminal::user_path::init_from_login_shell);
    tauri::Builder::default()
        .plugin(navigation_guard())
        .plugin(tauri_plugin_dialog::init())
        .menu(quit::app_menu)
        .on_menu_event(|app, event| {
            if event.id() == quit::QUIT_MENU_ID {
                quit::ask(app);
            }
        })
        .setup(|app| {
            let handle = app.handle().clone();
            let data_dir = handle
                .path()
                .app_data_dir()
                .expect("app data dir must be resolvable");
            std::fs::create_dir_all(&data_dir).ok();

            // Database.
            let db = Arc::new(
                Db::open(data_dir.join("jenderalcode.db")).expect("opening application database"),
            );

            // Credentials: a 0600 file in the app data dir is the default —
            // it never triggers the macOS Security Agent password prompts
            // that unsigned builds hit on every keychain read. The OS
            // keychain remains available as an opt-in in Settings (and is
            // the recommended choice for signed release builds).
            let settings0 = db.get_settings().unwrap_or_default();
            let creds: Arc<SwitchableCredentialStore> = Arc::new(
                SwitchableCredentialStore::new(
                    &settings0.credential_storage,
                    data_dir.join("credentials.json"),
                ),
            );

            let sink = Arc::new(sink::TauriEventSink::new(handle.clone()));
            let runtime = Arc::new(jcode_agent_runtime::AgentRuntime::new(
                db.clone(),
                creds.clone(),
                sink,
                data_dir.join("checkpoints"),
            ));

            // Seed built-in profiles + mock provider and apply retention.
            tauri::async_runtime::block_on(jcode_agent_runtime::seed_defaults(&db))
                .expect("seeding defaults");
            if let Ok(settings) = db.get_settings() {
                if let Some(days) = settings.retention_days {
                    let _ = db.apply_retention(days);
                }
            }

            // One-time migration: older builds stored API keys in the OS
            // keychain, which makes unsigned builds prompt for the login
            // password on every read. Move any such keys into the active
            // store (the 0600 file by default). At most one OS prompt per
            // key, then never again.
            tauri::async_runtime::block_on(migrate_legacy_keychain_keys(&db, &creds));

            app.manage(WatcherStore(Mutex::new(None)));
            app.manage(AppState {
                db,
                creds,
                runtime,
                terminals: terminal::TerminalManager::default(),
                active_workspace: RwLock::new(None),
                data_dir,
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            // Workspaces
            commands::list_workspaces,
            commands::open_workspace,
            commands::get_active_workspace,
            commands::close_workspace,
            commands::remove_workspace,
            // Filesystem
            commands::fs_list,
            commands::fs_read,
            commands::fs_write,
            commands::fs_create,
            commands::fs_delete,
            commands::fs_rename,
            commands::fs_search,
            commands::fs_glob,
            // Credentials & providers
            commands::get_credential_backend,
            commands::switch_credential_storage,
            commands::list_providers,
            commands::save_provider,
            commands::delete_provider,
            commands::test_provider,
            commands::refresh_models,
            commands::list_models,
            commands::add_custom_model,
            commands::delete_model,
            commands::provider_presets,
            // Agent profiles
            commands::list_agent_profiles,
            commands::save_agent_profile,
            commands::delete_agent_profile,
            // Sessions & chat
            commands::list_sessions,
            commands::search_sessions,
            commands::create_session,
            commands::get_session_messages,
            commands::get_session_tool_runs,
            commands::get_session_approvals,
            commands::get_session_detail,
            commands::send_message,
            commands::steer_message,
            commands::cancel_generation,
            commands::decide_approval,
            commands::set_session_model,
            commands::set_session_agent,
            commands::set_policy_mode,
            commands::delete_session,
            commands::rename_session,
            commands::session_is_busy,
            commands::list_checkpoints,
            commands::restore_checkpoints,
            // Git
            commands::git_status,
            commands::git_diff,
            commands::git_diff_all,
            commands::git_stage,
            commands::git_unstage,
            commands::git_discard,
            commands::git_commit,
            commands::git_branches,
            commands::git_checkout_branch,
            commands::git_create_branch,
            commands::git_current_branch,
            commands::git_log,
            // Usage & settings
            commands::get_data_dir,
            commands::set_zoom,
            commands::usage_summary,
            commands::get_session_usage,
            browser::browser_open,
            browser::browser_set_bounds,
            browser::browser_set_visible,
            browser::browser_navigate,
            browser::browser_action,
            browser::browser_close,
            commands::get_settings,
            commands::save_settings,
            commands::apply_retention,
            commands::clear_history,
            commands::export_history,
            // Terminals
            commands::terminal_create,
            commands::terminal_write,
            commands::terminal_resize,
            commands::terminal_kill,
            commands::terminal_list,
            commands::detect_agent_clis,
            commands::agent_usage,
            commands::list_skills,
            commands::search_skills,
            commands::install_skill,
            commands::remove_skill,
            // External agents
            commands::cli_agent_presets,
            commands::run_external_agent,
        ])
        .on_window_event(|window, event| match event {
            tauri::WindowEvent::CloseRequested { api, .. } if !quit::confirmed() => {
                api.prevent_close();
                quit::ask(window.app_handle());
            }
            tauri::WindowEvent::Destroyed => {
                if let Some(state) = window.app_handle().try_state::<AppState>() {
                    state.terminals.kill_all();
                    state.runtime.shutdown();
                }
            }
            _ => {}
        })
        .build(tauri::generate_context!())
        .expect("building JenderalCode Desktop")
        .run(|app, event| {
            // ⌘Q / the app menu's Quit (code `None`); `exit(0)` after the
            // user confirmed passes through.
            if let tauri::RunEvent::ExitRequested { api, code: None, .. } = event {
                if !quit::confirmed() {
                    api.prevent_exit();
                    quit::ask(app);
                }
            }
        });
}

#[cfg(test)]
mod tests {
    use super::is_allowed_navigation;

    fn ok(u: &str) -> bool {
        is_allowed_navigation(&tauri::Url::parse(u).unwrap())
    }

    #[test]
    fn navigation_allowlist() {
        assert!(ok("tauri://localhost/index.html"));
        assert!(ok("http://tauri.localhost/"));
        assert!(ok("https://tauri.localhost/x"));
        assert!(!ok("https://evil.example.com/"));
        assert!(!ok("http://localhost:8080/"));
        assert!(!ok("file:///etc/passwd"));
        assert!(!ok("https://tauri.localhost.evil.com/"));
        assert_eq!(ok("http://localhost:5173/"), cfg!(debug_assertions));
    }
}
