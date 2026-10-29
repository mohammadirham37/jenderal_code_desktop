//! In-app browser preview: native child webviews laid over a placeholder
//! element in the frontend (the frontend reports its bounds).
//!
//! Preview webviews load arbitrary http(s) content, so they are kept apart
//! from the app: they have no IPC capability (capabilities are scoped to the
//! `main` webview and remote origins can never reach app commands), they may
//! only navigate to http(s), and they can never load the app's own origin.

use serde::Serialize;
use tauri::webview::{NewWindowResponse, PageLoadEvent, WebviewBuilder};
use tauri::{AppHandle, Emitter, LogicalPosition, LogicalSize, Manager, Url, WebviewUrl};

type CmdResult<T> = Result<T, String>;

const LABEL_PREFIX: &str = "preview-";

pub const EVENT_BROWSER_STATE: &str = "browser://state";

#[derive(Debug, Clone, Serialize)]
pub struct BrowserStateEvent {
    pub id: String,
    pub url: String,
    pub loading: bool,
}

pub fn is_preview_label(label: &str) -> bool {
    label.starts_with(LABEL_PREFIX)
}

/// Navigation rule for preview webviews: plain web content only, and never
/// the app's own (IPC-privileged) origins.
pub fn is_allowed_preview_navigation(url: &Url) -> bool {
    match url.scheme() {
        "http" | "https" => {
            let host = url.host_str().unwrap_or_default();
            let app_origin = matches!(host, "tauri.localhost" | "ipc.localhost" | "asset.localhost")
                || (cfg!(debug_assertions) && host == "localhost" && url.port() == Some(5173));
            !app_origin
        }
        "about" => url.as_str() == "about:blank",
        _ => false,
    }
}

fn label_for(id: &str) -> CmdResult<String> {
    if id.is_empty() || id.len() > 64 || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
        return Err(format!("invalid browser id: {id:?}"));
    }
    Ok(format!("{LABEL_PREFIX}{id}"))
}

/// Accepts `localhost:3000`, `example.com`, or a full http(s) URL.
pub fn normalize_url(input: &str) -> CmdResult<Url> {
    let s = input.trim();
    if s.is_empty() {
        return Err("empty URL".into());
    }
    let with_scheme = if s.contains("://") || s.starts_with("about:") {
        s.to_string()
    } else if s.starts_with("localhost") || s.starts_with("127.0.0.1") || s.starts_with("0.0.0.0") || s.starts_with("[::1]") {
        format!("http://{s}")
    } else {
        format!("https://{s}")
    };
    let url = Url::parse(&with_scheme).map_err(|e| format!("invalid URL {s:?}: {e}"))?;
    if !is_allowed_preview_navigation(&url) {
        return Err(format!("only http(s) pages can be previewed: {url}"));
    }
    Ok(url)
}

fn emit_state(app: &AppHandle, id: &str, url: &Url, loading: bool) {
    let _ = app.emit_to(
        "main",
        EVENT_BROWSER_STATE,
        BrowserStateEvent { id: id.to_string(), url: url.to_string(), loading },
    );
}

#[derive(Debug, Clone, Copy, serde::Deserialize)]
pub struct Bounds {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl Bounds {
    fn position(&self) -> LogicalPosition<f64> {
        LogicalPosition::new(self.x.max(0.0), self.y.max(0.0))
    }
    fn size(&self) -> LogicalSize<f64> {
        LogicalSize::new(self.width.max(1.0), self.height.max(1.0))
    }
}

/// Create the preview (or reuse it), load `url`, and place it at `bounds`.
#[tauri::command]
pub async fn browser_open(app: AppHandle, id: String, url: String, bounds: Bounds) -> CmdResult<String> {
    let label = label_for(&id)?;
    let url = normalize_url(&url)?;

    if let Some(view) = app.get_webview(&label) {
        view.set_position(bounds.position()).map_err(|e| e.to_string())?;
        view.set_size(bounds.size()).map_err(|e| e.to_string())?;
        view.navigate(url.clone()).map_err(|e| e.to_string())?;
        view.show().map_err(|e| e.to_string())?;
        return Ok(url.to_string());
    }

    let window = app.get_window("main").ok_or("main window not found")?;
    let nav_app = app.clone();
    let load_app = app.clone();
    let popup_app = app.clone();
    let (nav_id, load_id, popup_label) = (id.clone(), id.clone(), label.clone());

    let builder = WebviewBuilder::new(&label, WebviewUrl::External(url.clone()))
        // Separate, non-persistent storage: previews never share cookies or
        // local storage with the app itself.
        .incognito(true)
        .on_navigation(move |u| {
            let ok = is_allowed_preview_navigation(u);
            if ok {
                emit_state(&nav_app, &nav_id, u, true);
            } else {
                log::warn!("blocked preview navigation to {u}");
            }
            ok
        })
        .on_page_load(move |_view, payload| {
            let loading = matches!(payload.event(), PageLoadEvent::Started);
            emit_state(&load_app, &load_id, payload.url(), loading);
        })
        // `target=_blank` / window.open: open in the same preview instead of
        // spawning windows.
        .on_new_window(move |u, _features| {
            if is_allowed_preview_navigation(&u) {
                if let Some(view) = popup_app.get_webview(&popup_label) {
                    let _ = view.navigate(u);
                }
            }
            NewWindowResponse::Deny
        });

    window
        .add_child(builder, bounds.position(), bounds.size())
        .map_err(|e| e.to_string())?;
    Ok(url.to_string())
}

#[tauri::command]
pub async fn browser_set_bounds(app: AppHandle, id: String, bounds: Bounds) -> CmdResult<()> {
    let Some(view) = app.get_webview(&label_for(&id)?) else { return Ok(()) };
    view.set_position(bounds.position()).map_err(|e| e.to_string())?;
    view.set_size(bounds.size()).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn browser_set_visible(app: AppHandle, id: String, visible: bool) -> CmdResult<()> {
    let Some(view) = app.get_webview(&label_for(&id)?) else { return Ok(()) };
    if visible { view.show() } else { view.hide() }.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn browser_navigate(app: AppHandle, id: String, url: String) -> CmdResult<String> {
    let url = normalize_url(&url)?;
    let view = app.get_webview(&label_for(&id)?).ok_or("browser is not open")?;
    view.navigate(url.clone()).map_err(|e| e.to_string())?;
    Ok(url.to_string())
}

/// `back`, `forward`, `reload`, or `stop`.
#[tauri::command]
pub async fn browser_action(app: AppHandle, id: String, action: String) -> CmdResult<()> {
    let view = app.get_webview(&label_for(&id)?).ok_or("browser is not open")?;
    let result = match action.as_str() {
        "back" => view.eval("history.back()"),
        "forward" => view.eval("history.forward()"),
        "reload" => view.reload(),
        "stop" => view.eval("window.stop()"),
        other => return Err(format!("unknown browser action: {other}")),
    };
    result.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn browser_close(app: AppHandle, id: String) -> CmdResult<()> {
    if let Some(view) = app.get_webview(&label_for(&id)?) {
        view.close().map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_common_inputs() {
        assert_eq!(normalize_url("localhost:3000").unwrap().as_str(), "http://localhost:3000/");
        assert_eq!(normalize_url("127.0.0.1:8000/x").unwrap().as_str(), "http://127.0.0.1:8000/x");
        assert_eq!(normalize_url("example.com").unwrap().as_str(), "https://example.com/");
        assert_eq!(normalize_url(" https://a.dev/p?q=1 ").unwrap().as_str(), "https://a.dev/p?q=1");
    }

    #[test]
    fn rejects_non_web_and_app_origins() {
        for bad in ["", "file:///etc/passwd", "javascript:alert(1)", "tauri://localhost", "http://tauri.localhost/", "http://ipc.localhost/x", "data:text/html,hi"] {
            assert!(normalize_url(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn ids_are_sanitized() {
        assert!(label_for("main").is_ok());
        assert!(label_for("ws-1_a").is_ok());
        for bad in ["", "../x", "a b", "a/b"] {
            assert!(label_for(bad).is_err(), "{bad}");
        }
    }
}
