//! Workspace file watcher: emits `fs://changed` so the UI can refresh the
//! file tree when files change on disk (editor saves, agent edits, external
//! tools).
//!
//! Debounce is trailing: after the first event we wait for 300 ms of quiet,
//! but never longer than 1 s after that first event. Paths inside build and
//! VCS directories are ignored so `cargo build` / `npm install` don't flood
//! the UI.

use jcode_common::{FsChangedEvent, EVENT_FS_CHANGED};
use notify::{Config, Event, RecommendedWatcher, RecursiveMode, Watcher};
use std::collections::BTreeSet;
use std::path::{Component, Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter};

const QUIET: Duration = Duration::from_millis(300);
const MAX_LATENCY: Duration = Duration::from_secs(1);
const MAX_PATHS: usize = 200;

const IGNORED_DIRS: &[&str] = &[
    ".git",
    "target",
    "node_modules",
    "dist",
    "build",
    ".next",
    ".turbo",
    ".cache",
    "__pycache__",
    ".venv",
];

/// Workspace-relative, `/`-separated path, or `None` if the path is outside
/// the root or inside an ignored directory.
fn relevant_rel_path(root: &Path, path: &Path) -> Option<String> {
    let rel = path.strip_prefix(root).ok()?;
    let ignored = rel.components().any(|c| match c {
        Component::Normal(name) => name.to_str().is_some_and(|n| IGNORED_DIRS.contains(&n)),
        _ => false,
    });
    if ignored {
        return None;
    }
    Some(rel.to_string_lossy().replace('\\', "/"))
}

fn collect(root: &Path, pending: &mut BTreeSet<String>, ev: notify::Result<Event>) {
    match ev {
        Ok(ev) => {
            for p in ev.paths {
                if pending.len() >= MAX_PATHS {
                    break;
                }
                if let Some(rel) = relevant_rel_path(root, &p) {
                    pending.insert(rel);
                }
            }
        }
        // A single notify error (e.g. a transient permission issue) must not
        // stop watching.
        Err(e) => log::warn!("file watcher error: {e}"),
    }
}

/// Start watching `root`. Returns the watcher (keep it alive; dropping stops
/// watching) and spawns a debounce thread that coalesces bursts of events.
pub fn start(app: AppHandle, root: PathBuf) -> notify::Result<RecommendedWatcher> {
    let (tx, rx) = mpsc::channel::<notify::Result<Event>>();
    let mut watcher = RecommendedWatcher::new(tx, Config::default())?;
    watcher.watch(&root, RecursiveMode::Recursive)?;

    // Events report canonical paths on some platforms (e.g. /private/var on
    // macOS); strip either form.
    let canonical_root = std::fs::canonicalize(&root).unwrap_or_else(|_| root.clone());
    std::thread::spawn(move || {
        let mut pending: BTreeSet<String> = BTreeSet::new();
        loop {
            // Block until a first event arrives; exit when the watcher is dropped.
            let Ok(first) = rx.recv() else { return };
            collect_both(&root, &canonical_root, &mut pending, first);
            if pending.is_empty() {
                continue; // only ignored paths / errors
            }
            let started = Instant::now();
            loop {
                let remaining = MAX_LATENCY.saturating_sub(started.elapsed());
                if remaining.is_zero() {
                    break;
                }
                match rx.recv_timeout(QUIET.min(remaining)) {
                    Ok(ev) => collect_both(&root, &canonical_root, &mut pending, ev),
                    Err(mpsc::RecvTimeoutError::Timeout) => break,
                    Err(mpsc::RecvTimeoutError::Disconnected) => return,
                }
            }
            let paths: Vec<String> = std::mem::take(&mut pending).into_iter().collect();
            let _ = app.emit(
                EVENT_FS_CHANGED,
                FsChangedEvent {
                    root: root.to_string_lossy().to_string(),
                    paths,
                },
            );
        }
    });
    Ok(watcher)
}

fn collect_both(
    root: &Path,
    canonical_root: &Path,
    pending: &mut BTreeSet<String>,
    ev: notify::Result<Event>,
) {
    match ev {
        Ok(mut ev) => {
            // Normalize canonical paths back onto the workspace root.
            for p in ev.paths.iter_mut() {
                if !p.starts_with(root) {
                    if let Ok(rel) = p.strip_prefix(canonical_root) {
                        *p = root.join(rel);
                    }
                }
            }
            collect(root, pending, Ok(ev));
        }
        Err(e) => collect(root, pending, Err(e)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ignores_build_and_vcs_dirs() {
        let root = Path::new("/ws");
        assert_eq!(relevant_rel_path(root, Path::new("/ws/src/a.rs")).as_deref(), Some("src/a.rs"));
        assert_eq!(relevant_rel_path(root, Path::new("/ws/.git/index")), None);
        assert_eq!(relevant_rel_path(root, Path::new("/ws/target/debug/x")), None);
        assert_eq!(relevant_rel_path(root, Path::new("/ws/web/node_modules/a/b.js")), None);
        assert_eq!(relevant_rel_path(root, Path::new("/elsewhere/a")), None);
        // Only components inside the workspace count.
        let root = Path::new("/home/u/build/proj");
        assert_eq!(
            relevant_rel_path(root, Path::new("/home/u/build/proj/main.c")).as_deref(),
            Some("main.c")
        );
    }

    #[test]
    fn errors_do_not_panic_and_paths_dedupe() {
        let root = Path::new("/ws");
        let mut pending = BTreeSet::new();
        collect(root, &mut pending, Err(notify::Error::generic("boom")));
        let ev = Event::new(notify::EventKind::Any)
            .add_path(PathBuf::from("/ws/a"))
            .add_path(PathBuf::from("/ws/a"));
        collect(root, &mut pending, Ok(ev));
        assert_eq!(pending.len(), 1);
    }
}
