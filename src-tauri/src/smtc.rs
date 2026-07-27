//! Windows System Media Transport Controls — the media flyout that shows the
//! current track with prev / play-pause / next, and receives hardware media
//! keys.
//!
//! Two hazards this module is built around:
//!
//! - The SMTC COM object is apartment-threaded and **not `Send`**. It is created
//!   on the main thread, its button callback is delivered via the main window's
//!   WndProc (main thread), and every update is funnelled back to the main
//!   thread with `run_on_main_thread`. The `unsafe impl Send` is sound only
//!   because of that discipline.
//! - souvlaki subclasses the main window to receive SMTC messages, and so does
//!   [`crate::snap`]. Install snap *first* so souvlaki's `SetWindowSubclass`
//!   chains on top of it; both then see their messages.

#![cfg(target_os = "windows")]

use std::sync::Mutex;
use std::time::Duration;

use souvlaki::{
    MediaControlEvent, MediaControls, MediaMetadata, MediaPlayback, MediaPosition, PlatformConfig,
};
use tauri::{AppHandle, Manager};

use crate::player::{PlayerState, Status};
use crate::AppState;

struct Controls(MediaControls);
// SAFETY: see the module docs — only ever touched on the main thread.
unsafe impl Send for Controls {}

#[derive(Default)]
pub struct Smtc {
    inner: Mutex<Option<Controls>>,
}

impl Smtc {
    pub fn init(&self, app: &AppHandle) {
        let Some(win) = app.get_webview_window("main") else {
            return;
        };
        let Ok(hwnd) = win.hwnd() else {
            tracing::warn!("no HWND; SMTC unavailable");
            return;
        };

        let config = PlatformConfig {
            dbus_name: "capsule",
            display_name: "capsule",
            hwnd: Some(hwnd.0),
        };

        let mut controls = match MediaControls::new(config) {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!(error = ?e, "SMTC init failed");
                return;
            }
        };

        let handle = app.clone();
        if let Err(e) = controls.attach(move |event| on_event(&handle, event)) {
            tracing::warn!(error = ?e, "SMTC attach failed");
            return;
        }

        *self.inner.lock().expect("smtc mutex") = Some(Controls(controls));
        tracing::info!("SMTC attached");
    }

    fn apply(&self, m: &Applied) {
        let mut guard = self.inner.lock().expect("smtc mutex");
        let Some(Controls(controls)) = guard.as_mut() else {
            return;
        };

        let _ = controls.set_metadata(MediaMetadata {
            title: Some(&m.title),
            artist: Some(&m.artist),
            album: Some(&m.album),
            cover_url: m.cover.as_deref(),
            duration: (m.duration_ms > 0).then(|| Duration::from_millis(m.duration_ms)),
        });

        let progress = Some(MediaPosition(Duration::from_millis(m.position_ms)));
        let playback = match m.status {
            Status::Playing | Status::Loading | Status::Stalled => {
                MediaPlayback::Playing { progress }
            }
            Status::Paused => MediaPlayback::Paused { progress },
            Status::Idle | Status::Ended => MediaPlayback::Stopped,
        };
        let _ = controls.set_playback(playback);
    }
}

struct Applied {
    title: String,
    artist: String,
    album: String,
    cover: Option<String>,
    duration_ms: u64,
    position_ms: u64,
    status: Status,
}

pub fn update(app: &AppHandle, snapshot: &PlayerState) {
    let track = snapshot.current();
    let applied = Applied {
        title: track.map(|t| t.title.clone()).unwrap_or_default(),
        artist: track.map(|t| t.artist.clone()).unwrap_or_default(),
        album: track.map(|t| t.album.clone()).unwrap_or_default(),
        cover: track.and_then(|t| cover_url(app, &t.id)),
        duration_ms: track.map(|t| t.duration_ms).unwrap_or(0),
        position_ms: snapshot.position_ms,
        status: snapshot.status,
    };

    let app = app.clone();
    let _ = app.clone().run_on_main_thread(move || {
        app.state::<AppState>().smtc.apply(&applied);
    });
}

fn cover_url(app: &AppHandle, track_id: &str) -> Option<String> {
    let state = app.state::<AppState>();
    let db = state.db.lock().expect("db mutex");
    let art = db.artwork_for(track_id).ok()??;
    Some(crate::artwork::resolve_template(&art.template, art.clamp(300)))
}

fn on_event(app: &AppHandle, event: MediaControlEvent) {
    use crate::commands::apply;
    match event {
        MediaControlEvent::Play => apply(app, |p| p.play()),
        MediaControlEvent::Pause => apply(app, |p| p.pause()),
        MediaControlEvent::Toggle => apply(app, |p| p.toggle()),
        MediaControlEvent::Next => apply(app, |p| p.next_track()),
        MediaControlEvent::Previous => apply(app, |p| p.previous_track()),
        MediaControlEvent::Stop => apply(app, |p| p.pause()),
        other => tracing::debug!(?other, "unhandled SMTC event"),
    }
}
