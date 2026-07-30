//! capsule - a fast Apple Music desktop client.
//!
//! Rust owns all state. The React UI and the hidden `music.apple.com` engine
//! window are both just clients of it, which is what keeps the now-playing
//! view, the OS media controls and (later) Discord and Last.fm from ever
//! disagreeing with each other.

pub mod api;
pub mod audio;
pub mod artwork;
pub mod auth;
pub mod commands;
pub mod config;
pub mod db;
pub mod discord;
pub mod engine;
pub mod lastfm;
pub mod lyrics;
pub mod player;
pub mod settings;
pub mod source;
pub mod stream;
pub mod subsonic;
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

#[derive(Default)]
pub struct NowReported {
    pub track_id: Option<String>,
    pub scrobbled: bool,
}

pub struct AppState {
    pub player: Mutex<Player>,
    pub tokens: Mutex<Option<auth::Tokens>>,
    pub db: Mutex<db::Db>,
    pub settings: Mutex<settings::Settings>,
    pub data_dir: Mutex<Option<std::path::PathBuf>>,
    /// The live Navidrome client, when that source is active. Artwork fetches
    /// need it to sign cover URLs, which cannot be stored in the database.
    pub navidrome: Mutex<Option<std::sync::Arc<subsonic::Client>>>,
    /// The native playback engine, present when the active source plays in
    /// Rust. `None` on the Apple path, or when no output device exists.
    pub audio: Mutex<Option<std::sync::Arc<audio::Engine>>>,
    /// Rich Presence, when a client id is configured.
    pub discord: Mutex<Option<std::sync::Arc<discord::Presence>>>,
    /// What was last announced to Discord and the scrobbler, so a 250ms
    /// publish loop does not re-announce the same track forever.
    pub now_reported: Mutex<NowReported>,
    /// Bumped every time the library database is swapped.
    ///
    /// A sync captures this when it starts and abandons itself if it changes,
    /// so a source switch mid-sync cannot write one source's rows into
    /// another's file.
    pub db_generation: std::sync::atomic::AtomicU64,
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
            navidrome: Mutex::new(None),
            audio: Mutex::new(None),
            discord: Mutex::new(None),
            now_reported: Mutex::new(NowReported::default()),
            db_generation: std::sync::atomic::AtomicU64::new(0),
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
        .plugin(tauri_plugin_dialog::init())
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
            commands::navidrome_connect,
            commands::navidrome_status,
            commands::dev_load_recent,
            commands::dev_diagnostics,
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

            let data_dir = app.path().app_data_dir().ok();

            // Settings must load before the library: each source keeps its own
            // database file, so `source` decides which one we open.
            let loaded = data_dir.as_ref().map(|d| settings::load(d)).unwrap_or_default();
            let active_source = loaded.source;

            if let Some(dir) = data_dir.as_ref() {
                // Installs from before per-source files carry library.sqlite3;
                // claim it for Apple rather than orphaning it. Failing here
                // must not stop the app starting.
                if let Err(e) = db::migrate_legacy_db(dir) {
                    tracing::warn!(error = %e, "legacy db migration failed; continuing");
                }
            }

            let db_path = db::default_db_path(data_dir.clone(), loaded.source)?;
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

            if loaded.discord_enabled() {
                let id = loaded.discord_client_id().to_string();
                *app.state::<AppState>().discord.lock().expect("discord mutex") =
                    Some(std::sync::Arc::new(discord::Presence::new(id)));
            }
            audio::start_ticker(handle.clone());

            tracing::info!(
                source = ?loaded.source,
                onboarded = loaded.onboarded,
                lastfm = loaded.lastfm_enabled(),
                discord = loaded.discord_enabled(),
                "settings"
            );
            if let Some(dir) = data_dir {
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

            // The env var is a one-off override for comparing materials; the
            // setting is what persists, so `bun run app` looks the way the user
            // last chose rather than depending on how it was launched.
            let glass = std::env::var("CAPSULE_GLASS").unwrap_or_else(|_| {
                app.state::<AppState>().settings.lock().expect("settings mutex").appearance.glass.clone()
            });
            // Only the Windows block below reads this - acrylic, snap and the
            // thumbbar all need the window handle, and none of them exist
            // elsewhere.
            #[cfg_attr(not(target_os = "windows"), allow(unused_variables))]
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

            // Only the source that needs it gets a webview. Spawning it
            // unconditionally cost a second Chromium on music.apple.com during
            // every native session.
            commands::reconcile_backend(&handle, active_source);
            if runtime.show_engine_window {
                tracing::warn!("SHOW_ENGINE_WINDOW is set - engine webview is visible");
            }

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running capsule");
}
