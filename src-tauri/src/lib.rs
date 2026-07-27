//! capsule — a fast Apple Music desktop client.
//!
//! Rust owns all state. The React UI and the hidden `music.apple.com` engine
//! window are both just clients of it, which is what keeps the now-playing
//! view, the OS media controls and (later) Discord and Last.fm from ever
//! disagreeing with each other.

pub mod api;
pub mod artwork;
pub mod auth;
pub mod commands;
pub mod config;
pub mod db;
pub mod engine;
pub mod lyrics;
pub mod player;
pub mod settings;
#[cfg(target_os = "windows")]
pub mod smtc;
#[cfg(target_os = "windows")]
pub mod snap;
#[cfg(target_os = "windows")]
pub mod thumbbar;
pub mod sync;

use std::sync::atomic::AtomicBool;
use std::sync::Mutex;

use tauri::{Manager, WebviewUrl, WebviewWindowBuilder};

use player::Player;

pub struct AppState {
    pub player: Mutex<Player>,
    pub tokens: Mutex<Option<auth::Tokens>>,
    pub db: Mutex<db::Db>,
    pub settings: Mutex<settings::Settings>,
    pub data_dir: Mutex<Option<std::path::PathBuf>>,
    pub sync: sync::SyncGuard,
    pub login_prompted: AtomicBool,
    pub engine_ready: AtomicBool,
    #[cfg(target_os = "windows")]
    pub smtc: smtc::Smtc,
}

impl AppState {
    fn new(db: db::Db) -> Self {
        Self {
            player: Mutex::new(Player::new()),
            tokens: Mutex::new(None),
            db: Mutex::new(db),
            settings: Mutex::new(settings::Settings::default()),
            data_dir: Mutex::new(None),
            sync: sync::SyncGuard::default(),
            login_prompted: AtomicBool::new(false),
            engine_ready: AtomicBool::new(false),
            #[cfg(target_os = "windows")]
            smtc: smtc::Smtc::default(),
        }
    }
}

fn build_tray(app: &tauri::App) -> tauri::Result<()> {
    use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
    use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};

    let menu = Menu::with_items(
        app,
        &[
            &MenuItem::with_id(app, "playpause", "Play / Pause", true, None::<&str>)?,
            &MenuItem::with_id(app, "next", "Next", true, None::<&str>)?,
            &MenuItem::with_id(app, "prev", "Previous", true, None::<&str>)?,
            &PredefinedMenuItem::separator(app)?,
            &MenuItem::with_id(app, "show", "Show capsule", true, None::<&str>)?,
            &MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?,
        ],
    )?;

    let mut builder = TrayIconBuilder::new()
        .tooltip("capsule")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "playpause" => commands::apply(app, |p| p.toggle()),
            "next" => commands::apply(app, |p| p.next_track()),
            "prev" => commands::apply(app, |p| p.previous_track()),
            "show" => show_main(app),
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                show_main(tray.app_handle());
            }
        });

    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }
    builder.build(app)?;
    Ok(())
}

fn show_main(app: &tauri::AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
}

fn init_tracing() {
    use tracing_subscriber::{fmt, prelude::*, EnvFilter};
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,capsule_lib=debug"));
    tracing_subscriber::registry().with(fmt::layer().compact()).with(filter).init();
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    engine::apply_webview_flags();
    init_tracing();

    let runtime = config::Runtime::from_env();
    tracing::info!("capsule starting ({})", config::describe());

    tauri::Builder::default()
        .register_asynchronous_uri_scheme_protocol(artwork::SCHEME, artwork::handle)
        .invoke_handler(tauri::generate_handler![
            commands::player_snapshot,
            commands::player_play,
            commands::player_pause,
            commands::player_toggle,
            commands::player_next,
            commands::player_previous,
            commands::player_seek,
            commands::player_set_volume,
            commands::player_toggle_shuffle,
            commands::player_cycle_repeat,
            commands::auth_status,
            commands::auth_show_login,
            commands::dev_load_recent,
            commands::engine_ready,
            commands::engine_tokens,
            commands::engine_event,
            commands::engine_log,
            commands::library_songs,
            commands::library_albums,
            commands::library_playlists,
            commands::library_album_songs,
            commands::library_search,
            commands::library_counts,
            commands::library_sync,
            commands::lyrics_for,
            commands::settings_get,
            commands::settings_set,
            commands::play_songs,
            commands::play_album,
            commands::play_playlist,
        ])
        .setup(move |app| {
            let handle = app.handle().clone();

            let db_path = db::default_db_path(app.path().app_data_dir().ok())?;
            tracing::info!(path = %db_path.display(), "opening library");
            let database = db::Db::open_at(&db_path)?;
            match database.counts() {
                Ok(c) => tracing::info!(
                    songs = c.songs,
                    albums = c.albums,
                    playlists = c.playlists,
                    "library on disk"
                ),
                Err(e) => tracing::warn!(error = %e, "could not count library"),
            }
            app.manage(AppState::new(database));

            if let Ok(dir) = app.path().app_data_dir() {
                let loaded = settings::load(&dir);
                tracing::info!(
                    source = ?loaded.source,
                    onboarded = loaded.onboarded,
                    lastfm = loaded.lastfm_enabled(),
                    discord = loaded.discord_enabled(),
                    "settings"
                );
                let state = app.state::<AppState>();
                *state.settings.lock().expect("settings mutex") = loaded;
                *state.data_dir.lock().expect("data dir mutex") = Some(dir);
            }

            match auth::load() {
                Ok(Some(t)) if t.is_complete() => {
                    tracing::info!(storefront = %t.storefront, "restored stored tokens");
                    *app.state::<AppState>().tokens.lock().expect("tokens mutex") = Some(t);
                }
                Ok(_) => tracing::info!("no stored tokens; first run"),
                Err(e) => tracing::warn!(error = %e, "could not read credential store"),
            }

            let glass = std::env::var("CAPSULE_GLASS").unwrap_or_else(|_| "none".into());
            let main = WebviewWindowBuilder::new(app, "main", WebviewUrl::default())
                .title("capsule")
                .inner_size(1160.0, 760.0)
                .min_inner_size(880.0, 560.0)
                .resizable(true)
                .decorations(false)
                .transparent(glass != "none")
                .initialization_script(format!(
                    r#"(function () {{
                        var v = {};
                        function set() {{
                            if (document.documentElement) {{
                                document.documentElement.dataset.glass = v;
                                return true;
                            }}
                            return false;
                        }}
                        if (!set()) {{
                            document.addEventListener('DOMContentLoaded', set);
                        }}
                    }})();"#,
                    serde_json::to_string(&glass).unwrap_or_else(|_| "\"none\"".into())
                ))
                .build()?;

            #[cfg(target_os = "windows")]
            {
                use window_vibrancy::{apply_acrylic, apply_mica};
                let applied = match glass.as_str() {
                    "acrylic-chrome" | "acrylic-all" => {
                        apply_acrylic(&main, Some((13, 16, 20, 24))).map(|_| "acrylic")
                    }
                    "mica" => apply_mica(&main, Some(true)).map(|_| "mica"),
                    _ => Ok("none"),
                };
                match applied {
                    Ok(m) => tracing::info!(material = m, variant = %glass, "window material"),
                    Err(e) => tracing::warn!(error = %e, variant = %glass, "material unavailable"),
                }

                snap::install(&main);

                thumbbar::install(&main, &handle);

                app.state::<AppState>().smtc.init(&handle);
            }

            build_tray(app)?;

            engine::spawn(&handle, runtime.show_engine_window)?;
            if runtime.show_engine_window {
                tracing::warn!("SHOW_ENGINE_WINDOW is set — engine webview is visible");
            }

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running capsule");
}
