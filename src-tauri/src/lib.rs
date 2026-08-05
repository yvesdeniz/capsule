
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
pub mod local;
pub mod lyrics;
pub mod media_controls;
pub mod player;
pub mod settings;
pub mod source;
pub mod stream;
pub mod subsonic;
pub mod suspend;
#[cfg(target_os = "windows")]
pub mod snap;
#[cfg(target_os = "windows")]
pub mod thumbbar;
pub mod sync;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use tauri::{Emitter, Manager, WebviewUrl, WebviewWindowBuilder};

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
    pub navidrome: Mutex<Option<std::sync::Arc<subsonic::Client>>>,
    pub audio: Mutex<Option<std::sync::Arc<audio::Engine>>>,
    pub discord: Mutex<Option<std::sync::Arc<discord::Presence>>>,
    pub lastfm: Mutex<Option<lastfm::Session>>,
    pub now_reported: Mutex<NowReported>,
    pub db_generation: std::sync::atomic::AtomicU64,
    pub sync: sync::SyncGuard,
    pub login_prompted: AtomicBool,
    pub engine_ready: AtomicBool,
    pub media_controls: media_controls::Handle,
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
            lastfm: Mutex::new(None),
            now_reported: Mutex::new(NowReported::default()),
            db_generation: std::sync::atomic::AtomicU64::new(0),
            sync: sync::SyncGuard::default(),
            login_prompted: AtomicBool::new(false),
            engine_ready: AtomicBool::new(false),
            media_controls: media_controls::Handle::default(),
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

fn open_main_window(app: &tauri::AppHandle, glass: &str) -> tauri::Result<()> {
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
            serde_json::to_string(glass).unwrap_or_else(|_| "\"none\"".into())
        ))
        .build()?;

    {
        let app = app.clone();
        let suspended = Arc::new(AtomicBool::new(false));
        main.clone().on_window_event(move |event| match event {
            tauri::WindowEvent::CloseRequested { api, .. } => {
                api.prevent_close();
                if let Some(w) = app.get_webview_window("main") {
                    let _ = w.destroy();
                }
            }
            tauri::WindowEvent::Resized(size) => {
                let minimized = size.width == 0 && size.height == 0;
                let handle = app.clone();
                let suspended = suspended.clone();
                let _ = app.run_on_main_thread(move || {
                    let Some(w) = handle.get_webview_window("main") else {
                        return;
                    };
                    if minimized {
                        if w.is_minimized().unwrap_or(false)
                            && !suspended.swap(true, Ordering::SeqCst)
                        {
                            suspend::suspend(&w);
                        }
                    } else if suspended.swap(false, Ordering::SeqCst) {
                        suspend::resume(&w);
                        let state = handle.state::<AppState>();
                        commands::publish(&handle, &state);
                        let _ = handle.emit("library://updated", sync::counts(&handle));
                    }
                });
            }
            _ => {}
        });
    }

    #[cfg(target_os = "windows")]
    {
        use window_vibrancy::{apply_acrylic, apply_mica};
        let applied = match glass {
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
        thumbbar::install(&main, app);
    }

    app.state::<AppState>().media_controls.init(app);

    Ok(())
}

fn show_main(app: &tauri::AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        suspend::resume(&w);
        let _ = w.set_focus();
        return;
    }
    // Destroyed on close to free the webview; rebuild it.
    let glass = app.state::<AppState>().settings.lock().expect("settings mutex").appearance.glass.clone();
    if let Err(e) = open_main_window(app, &glass) {
        tracing::error!(error = %e, "could not reopen the main window");
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
            commands::lastfm_status,
            commands::lastfm_connect,
            commands::lastfm_disconnect,
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
            commands::library_playlist_songs,
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

            let loaded = data_dir.as_ref().map(|d| settings::load(d)).unwrap_or_default();
            let active_source = loaded.source;

            if let Some(dir) = data_dir.as_ref() {
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

            match auth::load_lastfm() {
                Ok(Some(s)) => {
                    tracing::info!(user = %s.username, "restored last.fm session");
                    *app.state::<AppState>().lastfm.lock().expect("lastfm mutex") = Some(s);
                }
                Ok(None) => {}
                Err(e) => tracing::warn!(error = %e, "could not read last.fm session"),
            }

            let glass = std::env::var("CAPSULE_GLASS").unwrap_or_else(|_| {
                app.state::<AppState>().settings.lock().expect("settings mutex").appearance.glass.clone()
            });

            engine::apply_webview_flags(matches!(
                active_source,
                settings::Source::Apple | settings::Source::Spotify
            ));
            open_main_window(&handle, &glass)?;

            build_tray(app)?;

            commands::reconcile_backend(&handle, active_source);
            if runtime.show_engine_window {
                tracing::warn!("SHOW_ENGINE_WINDOW is set - engine webview is visible");
            }

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running capsule");
}
