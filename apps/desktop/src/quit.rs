//! Quit confirmation: closing the main window or quitting the app (⌘Q)
//! asks first, naming the agent turns and terminals that would be stopped.

use crate::state::AppState;
use std::sync::atomic::{AtomicBool, Ordering};
use tauri::{AppHandle, Manager};
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};

/// Set once the user confirmed; the exit that follows must not ask again.
static CONFIRMED: AtomicBool = AtomicBool::new(false);
/// A confirmation dialog is showing (⌘Q pressed twice opens only one).
static ASKING: AtomicBool = AtomicBool::new(false);

pub fn confirmed() -> bool {
    CONFIRMED.load(Ordering::SeqCst)
}

pub const QUIT_MENU_ID: &str = "jcode-quit";

/// The default menu with the app menu's Quit (⌘Q) replaced by our own item:
/// the predefined one terminates the app directly on macOS, bypassing
/// `ExitRequested`, so it could never ask.
pub fn app_menu<R: tauri::Runtime>(app: &AppHandle<R>) -> tauri::Result<tauri::menu::Menu<R>> {
    use tauri::menu::{Menu, MenuItem, MenuItemKind};
    let menu = Menu::default(app)?;
    if cfg!(target_os = "macos") {
        if let Some(MenuItemKind::Submenu(app_menu)) = menu.items()?.into_iter().next() {
            // The predefined Quit is the app menu's last item.
            if let Some(last) = app_menu.items()?.pop().filter(|i| i.as_predefined_menuitem().is_some()) {
                app_menu.remove(&last)?;
            }
            app_menu.append(&MenuItem::with_id(
                app,
                QUIT_MENU_ID,
                "Quit JenderalCode",
                true,
                Some("CmdOrCtrl+Q"),
            )?)?;
        }
    }
    Ok(menu)
}

/// Show the confirmation dialog; quits the app when the user agrees.
pub fn ask(app: &AppHandle) {
    if ASKING.swap(true, Ordering::SeqCst) {
        return;
    }
    let message = match app.try_state::<AppState>() {
        Some(state) => describe(state.runtime.busy_count(), state.terminals.list().len()),
        None => describe(0, 0),
    };
    let handle = app.clone();
    app.dialog()
        .message(message)
        .title("Quit JenderalCode?")
        .kind(MessageDialogKind::Warning)
        .buttons(MessageDialogButtons::OkCancelCustom("Quit".into(), "Cancel".into()))
        .show(move |quit| {
            ASKING.store(false, Ordering::SeqCst);
            if quit {
                CONFIRMED.store(true, Ordering::SeqCst);
                handle.exit(0);
            }
        });
}

fn describe(turns: usize, terminals: usize) -> String {
    let mut running = Vec::new();
    if turns > 0 {
        running.push(format!("{turns} running agent turn{}", if turns == 1 { "" } else { "s" }));
    }
    if terminals > 0 {
        running.push(format!("{terminals} open terminal{}", if terminals == 1 { "" } else { "s" }));
    }
    if running.is_empty() {
        "Are you sure you want to quit?".into()
    } else {
        format!("{} will be stopped.", running.join(" and "))
    }
}

#[cfg(test)]
mod tests {
    use super::describe;

    #[test]
    fn names_what_will_be_stopped() {
        assert_eq!(describe(0, 0), "Are you sure you want to quit?");
        assert_eq!(describe(1, 0), "1 running agent turn will be stopped.");
        assert_eq!(describe(2, 3), "2 running agent turns and 3 open terminals will be stopped.");
    }
}
