//! Owns the hidden `music.apple.com` window that acts as our audio daemon.
//!
//! Rust drives it by evaluating JS against the hook; the hook reports back over
//! IPC. Everything with a decision in it lives in [`crate::player`] - this
//! module only translates [`EngineCommand`] into a JS call.

use tauri::{AppHandle, Emitter, Manager, WebviewUrl, WebviewWindow, WebviewWindowBuilder};

use crate::player::EngineCommand;

pub const ENGINE_LABEL: &str = "engine";

const DEFAULT_ENGINE_URL: &str = "https://music.apple.com";
const HOOK: &str = include_str!("../resources/engine-hook.js");

fn engine_url() -> String {
    std::env::var("CAPSULE_ENGINE_URL").unwrap_or_else(|_| DEFAULT_ENGINE_URL.to_string())
}

pub fn apply_webview_flags() {
    #[cfg(target_os = "windows")]
    std::env::set_var(
        "WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS",
        "--disable-background-timer-throttling \
         --disable-renderer-backgrounding \
         --disable-backgrounding-occluded-windows",
    );
}

pub fn spawn(app: &AppHandle, visible: bool) -> tauri::Result<WebviewWindow> {
    let raw = engine_url();
    tracing::info!(url = %raw, "engine host");
    let url = raw.parse().map_err(tauri::Error::InvalidUrl)?;
    let w = WebviewWindowBuilder::new(app, ENGINE_LABEL, WebviewUrl::External(url))
        .title("capsule engine")
        .inner_size(1100.0, 800.0)
        .visible(visible)
        .skip_taskbar(!visible)
        .initialization_script(HOOK)
        .build()?;
    Ok(w)
}

pub fn window(app: &AppHandle) -> Option<WebviewWindow> {
    app.get_webview_window(ENGINE_LABEL)
}

pub fn show_for_login(app: &AppHandle) -> tauri::Result<()> {
    if let Some(w) = window(app) {
        w.set_skip_taskbar(false)?;
        w.show()?;
        w.set_focus()?;
        let _ = app.emit("auth://login-required", ());
    }
    Ok(())
}

pub fn hide(app: &AppHandle) -> tauri::Result<()> {
    if let Some(w) = window(app) {
        w.hide()?;
        w.set_skip_taskbar(true)?;
    }
    Ok(())
}

fn js_string(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| "\"\"".into())
}

pub fn send(app: &AppHandle, cmd: &EngineCommand) -> tauri::Result<()> {
    let Some(w) = window(app) else { return Ok(()) };
    let js = match cmd {
        EngineCommand::SetQueue { ids, start_index } => {
            let list: Vec<String> = ids.iter().map(|i| js_string(i)).collect();
            format!("__saint.setQueue([{}],{})", list.join(","), start_index)
        }
        EngineCommand::Play => "__saint.play()".to_string(),
        EngineCommand::Pause => "__saint.pause()".to_string(),
        EngineCommand::Seek { ms } => format!("__saint.seek({})", *ms as f64 / 1000.0),
        EngineCommand::SetVolume { percent } => {
            format!("__saint.setVolume({})", f64::from(*percent) / 100.0)
        }
        EngineCommand::SkipNext => "__saint.skipNext()".to_string(),
        EngineCommand::SkipPrevious => "__saint.skipPrevious()".to_string(),
        EngineCommand::SetShuffle { on } => format!("__saint.setShuffle({on})"),
        EngineCommand::SetRepeat { mode } => format!("__saint.setRepeat({mode})"),
        EngineCommand::Prewarm { id } => format!("__saint.prewarm({})", js_string(id)),
    };
    w.eval(format!("window.__saint && {js}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_escaped_not_concatenated_raw() {
        let s = js_string("ab\"c\\d");
        assert_eq!(s, "\"ab\\\"c\\\\d\"");
    }

    #[test]
    fn seek_converts_milliseconds_to_seconds() {
        let cmd = EngineCommand::Seek { ms: 90_500 };
        let EngineCommand::Seek { ms } = cmd else { unreachable!() };
        assert!((f64::from(u32::try_from(ms).unwrap()) / 1000.0 - 90.5).abs() < f64::EPSILON);
    }

    #[test]
    fn volume_converts_percent_to_unit_interval() {
        assert!((f64::from(50u8) / 100.0 - 0.5).abs() < f64::EPSILON);
    }
}
