//! The entire IPC surface. Keeping it in one file makes the
//! renderer-agnostic boundary auditable: if it isn't here, the UI can't do it.

use std::collections::HashSet;
use std::sync::atomic::Ordering;

use serde::Deserialize;
use tauri::{AppHandle, Emitter, Manager, State};

use crate::auth::{self, AuthStatus};
use crate::db::{AlbumRow, LibraryCounts, PlaylistRow, SongRow};
use crate::engine;
use crate::player::{EngineCommand, Player, PlayerState, Track};
use crate::{sync, AppState, NowReported};

pub fn apply(app: &AppHandle, f: impl FnOnce(&mut Player) -> Vec<EngineCommand>) {
    let state = app.state::<AppState>();
    let cmds = {
        let mut p = state.player.lock().expect("player mutex");
        f(&mut p)
    };
    commit(app, &state, cmds);
}

/// What an [`EngineCommand`] means to the native backend.
///
/// The commands are MusicKit-shaped because the webview needs them that way.
/// Natively most of them collapse: the player has already updated its own
/// index, so SetQueue, SkipNext and SkipPrevious all mean "load current".
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NativeAction {
    LoadCurrent,
    Play,
    Pause,
    Seek(u64),
    Volume(u8),
    Ignore,
}

pub fn native_action(cmd: &EngineCommand) -> NativeAction {
    match cmd {
        EngineCommand::SetQueue { .. }
        | EngineCommand::SkipNext
        | EngineCommand::SkipPrevious => NativeAction::LoadCurrent,
        EngineCommand::Play => NativeAction::Play,
        EngineCommand::Pause => NativeAction::Pause,
        EngineCommand::Seek { ms } => NativeAction::Seek(*ms),
        EngineCommand::SetVolume { percent } => NativeAction::Volume(*percent),
        EngineCommand::Prewarm { .. }
        | EngineCommand::SetShuffle { .. }
        | EngineCommand::SetRepeat { .. } => NativeAction::Ignore,
    }
}

fn commit(app: &AppHandle, state: &AppState, cmds: Vec<EngineCommand>) {
    let engine = state.audio.lock().expect("audio mutex").clone();
    for c in &cmds {
        tracing::debug!(?c, "engine command");
        match &engine {
            // Native path: the webview is not involved at all.
            Some(engine) => run_native(app, state, engine, c),
            None => {
                if let Err(e) = engine::send(app, c) {
                    tracing::warn!(?c, error = %e, "failed to send engine command");
                }
            }
        }
    }
    publish(app, state);
}

fn run_native(app: &AppHandle, state: &AppState, engine: &crate::audio::Engine, cmd: &EngineCommand) {
    let action = match native_action(cmd) {
        // Play on an empty sink does nothing: after the queue ends, or after a
        // stop, there is no source left to resume. Reload the current track
        // instead of no-opping forever.
        NativeAction::Play if engine.is_empty() && !engine.is_loading() => {
            NativeAction::LoadCurrent
        }
        other => other,
    };
    match action {
        NativeAction::Ignore => {}
        NativeAction::Play => engine.play(),
        NativeAction::Pause => engine.pause(),
        NativeAction::Seek(ms) => engine.seek(ms),
        NativeAction::Volume(p) => engine.set_volume(p),
        NativeAction::LoadCurrent => {
            let track =
                state.player.lock().expect("player mutex").state().current().cloned();
            let Some(track) = track else { return };

            // A local track's id is its path, so there is nothing to fetch.
            let source = state.settings.lock().expect("settings mutex").source;
            if source == crate::settings::Source::Local {
                if let Err(e) = engine.load_file(std::path::PathBuf::from(&track.id)) {
                    tracing::error!(error = %e, track = %track.id, "could not open local track");
                    let _ = app.emit("playback://error", e.to_string());
                    return;
                }
                engine.play();
                return;
            }

            let client = state.navidrome.lock().expect("navidrome mutex").clone();
            let Some(client) = client else {
                tracing::warn!("native load with no navidrome client");
                return;
            };

            let dir = {
                let data = state.data_dir.lock().expect("data dir mutex").clone();
                crate::stream::cache_dir(data)
            };
            let Some(dir) = dir else { return };
            let path = crate::stream::cache_path(&dir, &track.id);
            // Pass the file we are about to play: past the cap, eviction could
            // otherwise delete the very file rodio is reading.
            crate::stream::prune(&dir, crate::stream::CACHE_CAP_BYTES, Some(&path));
            let url = client.stream_url(&track.id);
            if let Err(e) = engine.load(url, path) {
                tracing::error!(error = %e, track = %track.id, "native load failed");
                let _ = app.emit("playback://error", e.to_string());
                return;
            }
            engine.play();
        }
    }
}

/// Push the current player state to the UI, the media flyout and the taskbar.
///
/// `pub(crate)` because the native position ticker must call it too: updating
/// the player without publishing leaves the transport frozen at 0:00 while
/// audio plays.
pub(crate) fn publish(app: &AppHandle, state: &AppState) {
    let snapshot = state.player.lock().expect("player mutex").state().clone();
    if let Err(e) = app.emit("player://state", &snapshot) {
        tracing::warn!(error = %e, "failed to emit player state");
    }
    #[cfg(target_os = "windows")]
    {
        crate::smtc::update(app, &snapshot);
        crate::thumbbar::refresh(app, snapshot.status);
    }
    report_now_playing(app, state, &snapshot);
}

/// Announce what is playing to Discord and to the scrobbler.
///
/// Lives here rather than in the audio ticker, since the ticker only runs for
/// native sources and would leave Apple playback unannounced. `publish` is the
/// one place both backends converge.
fn report_now_playing(app: &AppHandle, state: &AppState, snapshot: &PlayerState) {
    use crate::player::Status;

    let track = snapshot.current().cloned();
    let stopped = matches!(snapshot.status, Status::Idle | Status::Ended) || track.is_none();

    let mut last = state.now_reported.lock().expect("now reported mutex");

    if stopped {
        // Otherwise the presence card keeps claiming you're listening for as
        // long as the app stays open.
        if last.track_id.is_some() {
            last.track_id = None;
            last.scrobbled = false;
            let presence = state.discord.lock().expect("discord mutex").clone();
            if let Some(p) = presence {
                std::thread::spawn(move || p.clear());
            }
        }
        return;
    }

    let Some(track) = track else { return };
    let changed = last.track_id.as_deref() != Some(track.id.as_str());
    if changed {
        last.track_id = Some(track.id.clone());
        last.scrobbled = false;
        scrobble(app, track.id.clone(), false);
        present(app, track.clone(), snapshot.status);
    }
    if !last.scrobbled && crate::subsonic::scrobble_due(snapshot.position_ms, track.duration_ms) {
        last.scrobbled = true;
        scrobble(app, track.id.clone(), true);
    }
}

/// Report a play to the server, which forwards it to whatever the user linked
/// there. Only sources with a server that scrobbles have a client here.
fn scrobble(app: &AppHandle, track_id: String, submission: bool) {
    let client = { app.state::<AppState>().navidrome.lock().expect("navidrome mutex").clone() };
    let Some(client) = client else { return };
    tauri::async_runtime::spawn(async move {
        if let Err(e) = client.scrobble(&track_id, submission).await {
            tracing::debug!(error = %e, %track_id, submission, "scrobble rejected");
        }
    });
}

/// Resolve cover art, then hand the track to Discord.
fn present(app: &AppHandle, track: Track, status: crate::player::Status) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let state = app.state::<AppState>();
        let presence = { state.discord.lock().expect("discord mutex").clone() };
        let Some(presence) = presence else { return };

        let (from_server, api_key) = {
            let s = state.settings.lock().expect("settings mutex");
            (s.discord.serve_art_from_server, s.api_key().to_string())
        };

        let image = if from_server {
            let client = { state.navidrome.lock().expect("navidrome mutex").clone() };
            client.map(|c| c.cover_art_url(&track.id, 300))
        } else {
            let http = reqwest::Client::new();
            crate::lastfm::album_art(&http, &api_key, &track.artist, &track.album).await
        };

        // Discord's IPC is synchronous and can block indefinitely on a payload
        // it dislikes. On the async runtime that parks a worker, and the audio
        // download runs on the same runtime.
        std::thread::spawn(move || presence.show(&track, status, image.as_deref()));
    });
}

#[tauri::command]
pub fn player_snapshot(state: State<'_, AppState>) -> PlayerState {
    state.player.lock().expect("player mutex").state().clone()
}

#[tauri::command]
pub fn player_play(app: AppHandle, state: State<'_, AppState>) {
    let cmds = state.player.lock().expect("player mutex").play();
    commit(&app, &state, cmds);
}

#[tauri::command]
pub fn player_pause(app: AppHandle, state: State<'_, AppState>) {
    let cmds = state.player.lock().expect("player mutex").pause();
    commit(&app, &state, cmds);
}

#[tauri::command]
pub fn player_toggle(app: AppHandle, state: State<'_, AppState>) {
    let cmds = state.player.lock().expect("player mutex").toggle();
    commit(&app, &state, cmds);
}

#[tauri::command]
pub fn player_next(app: AppHandle, state: State<'_, AppState>) {
    let cmds = state.player.lock().expect("player mutex").next_track();
    commit(&app, &state, cmds);
}

#[tauri::command]
pub fn player_previous(app: AppHandle, state: State<'_, AppState>) {
    let cmds = state.player.lock().expect("player mutex").previous_track();
    commit(&app, &state, cmds);
}

#[tauri::command]
pub fn player_seek(app: AppHandle, state: State<'_, AppState>, ms: u64) {
    let cmds = state.player.lock().expect("player mutex").seek(ms);
    commit(&app, &state, cmds);
}

#[tauri::command]
pub fn player_set_volume(app: AppHandle, state: State<'_, AppState>, percent: u8) {
    let cmds = state.player.lock().expect("player mutex").set_volume(percent);
    commit(&app, &state, cmds);
}

#[tauri::command]
pub fn player_toggle_shuffle(app: AppHandle, state: State<'_, AppState>) {
    let cmds = state.player.lock().expect("player mutex").toggle_shuffle();
    commit(&app, &state, cmds);
}

#[tauri::command]
pub fn player_cycle_repeat(app: AppHandle, state: State<'_, AppState>) {
    let cmds = state.player.lock().expect("player mutex").cycle_repeat();
    commit(&app, &state, cmds);
}

#[tauri::command]
pub fn auth_status(state: State<'_, AppState>) -> AuthStatus {
    let tokens = state.tokens.lock().expect("tokens mutex");
    match tokens.as_ref() {
        Some(t) if t.is_complete() => {
            AuthStatus { authenticated: true, storefront: Some(t.storefront.clone()) }
        }
        _ => AuthStatus { authenticated: false, storefront: None },
    }
}

#[tauri::command]
pub fn auth_show_login(app: AppHandle) -> Result<(), String> {
    engine::show_for_login(&app).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn engine_log(msg: String) {
    tracing::info!(target: "engine", "{msg}");
}

#[derive(Debug, Deserialize)]
pub struct ReadyPayload {
    pub ok: bool,
    #[serde(default)]
    pub reason: String,
}

#[tauri::command]
pub fn engine_ready(app: AppHandle, state: State<'_, AppState>, payload: ReadyPayload) {
    if payload.ok {
        tracing::info!("engine ready and authorized");
        state.engine_ready.store(true, Ordering::Relaxed);
        state.login_prompted.store(false, Ordering::Relaxed);
        let _ = engine::hide(&app);
        let _ = app.emit("engine://ready", ());

        let warm_with = {
            let db = state.db.lock().expect("db mutex");
            db.first_playable_catalog_id().unwrap_or_else(|e| {
                tracing::warn!(error = %e, "could not pick a prewarm track");
                None
            })
        };
        match warm_with {
            Some(id) => {
                tracing::info!(track = %id, "prewarming playback");
                let _ = engine::send(&app, &EngineCommand::Prewarm { id });
            }
            None => tracing::debug!("no synced track to prewarm with"),
        }
        return;
    }

    if payload.reason != "unauthorized" && state.engine_ready.load(Ordering::Relaxed) {
        tracing::debug!(reason = %payload.reason, "ignoring not-found after engine was ready");
        return;
    }

    match payload.reason.as_str() {
        "unauthorized" => {
            if !state.login_prompted.swap(true, Ordering::Relaxed) {
                tracing::info!("not signed in - opening Apple Music login");
                let _ = engine::show_for_login(&app);
            }
        }
        reason => {
            tracing::error!(reason, "engine unavailable");
            let _ = app.emit("engine://unavailable", reason.to_string());
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct TokensPayload {
    pub dev: String,
    pub user: String,
    #[serde(default)]
    pub storefront: String,
}

#[tauri::command]
pub fn engine_tokens(app: AppHandle, state: State<'_, AppState>, payload: TokensPayload) {
    let tokens = auth::Tokens {
        developer_token: payload.dev,
        music_user_token: payload.user,
        storefront: payload.storefront,
    };

    if !tokens.is_complete() {
        tracing::warn!("engine reported incomplete tokens; ignoring");
        return;
    }

    if let Err(e) = auth::save(&tokens) {
        tracing::warn!(error = %e, "could not persist tokens; continuing in-memory");
    }

    tracing::info!(storefront = %tokens.storefront, "tokens harvested");
    *state.tokens.lock().expect("tokens mutex") = Some(tokens);
    let _ = app.emit("auth://authenticated", ());

    let handle = app.clone();
    tauri::async_runtime::spawn(async move { sync::run(handle).await });
}

#[derive(Debug, Deserialize)]
pub struct EventPayload {
    pub kind: String,
    #[serde(default)]
    pub data: serde_json::Value,
}

#[tauri::command]
pub fn engine_event(app: AppHandle, state: State<'_, AppState>, payload: EventPayload) {
    if payload.kind == "unresolvable" {
        let ids: Vec<String> = payload
            .data
            .get("ids")
            .cloned()
            .and_then(|v| serde_json::from_value(v).ok())
            .unwrap_or_default();
        if !ids.is_empty() {
            let db = state.db.lock().expect("db mutex");
            match db.mark_unresolvable(&ids) {
                Ok(added) => {
                    tracing::info!(added, reported = ids.len(), "recorded unresolvable catalog ids")
                }
                Err(e) => tracing::warn!(error = %e, "could not record unresolvable ids"),
            }
        }
        return;
    }

    let follow_up = {
        let mut p = state.player.lock().expect("player mutex");
        match payload.kind.as_str() {
            "playbackState" => {
                let raw = payload.data.get("state").and_then(serde_json::Value::as_i64);
                raw.map(|r| p.on_playback_state(r)).unwrap_or_default()
            }
            "position" => {
                if let Some(ms) = payload.data.get("ms").and_then(serde_json::Value::as_u64) {
                    p.on_position(ms);
                }
                Vec::new()
            }
            "nowPlaying" => {
                if let Some(id) = payload.data.get("id").and_then(serde_json::Value::as_str) {
                    let dur = payload
                        .data
                        .get("durationMs")
                        .and_then(serde_json::Value::as_u64)
                        .unwrap_or(0);
                    p.on_now_playing(id, dur);
                }
                Vec::new()
            }
            "authorization" => {
                let ok = payload
                    .data
                    .get("authorized")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false);
                if !ok {
                    tracing::warn!("engine lost authorization");
                    let _ = app.emit("auth://lost", ());
                }
                Vec::new()
            }
            "recentTracks" => {
                let tracks: Vec<Track> = payload
                    .data
                    .get("tracks")
                    .cloned()
                    .and_then(|v| serde_json::from_value(v).ok())
                    .unwrap_or_default();
                if tracks.is_empty() {
                    tracing::warn!("engine returned no recent tracks");
                    Vec::new()
                } else {
                    tracing::info!(count = tracks.len(), "loaded recent tracks");
                    p.play_queue(tracks, 0)
                }
            }
            "error" => {
                tracing::warn!(data = %payload.data, "engine reported error");
                Vec::new()
            }
            other => {
                tracing::debug!(kind = other, "unhandled engine event");
                Vec::new()
            }
        }
    };

    commit(&app, &state, follow_up);
}

fn db_err(e: crate::db::DbError) -> String {
    tracing::error!(error = %e, "library query failed");
    e.to_string()
}

#[tauri::command]
pub fn library_songs(
    state: State<'_, AppState>,
    limit: Option<u32>,
    offset: Option<u32>,
) -> Result<Vec<SongRow>, String> {
    let db = state.db.lock().expect("db mutex");
    db.songs(limit.unwrap_or(200).min(1000), offset.unwrap_or(0)).map_err(db_err)
}

#[tauri::command]
pub fn library_albums(
    state: State<'_, AppState>,
    limit: Option<u32>,
    offset: Option<u32>,
) -> Result<Vec<AlbumRow>, String> {
    let db = state.db.lock().expect("db mutex");
    db.albums(limit.unwrap_or(200).min(1000), offset.unwrap_or(0)).map_err(db_err)
}

#[tauri::command]
pub fn library_playlists(state: State<'_, AppState>) -> Result<Vec<PlaylistRow>, String> {
    let db = state.db.lock().expect("db mutex");
    db.playlists().map_err(db_err)
}

#[tauri::command]
pub fn library_album_songs(
    state: State<'_, AppState>,
    album_id: String,
) -> Result<Vec<SongRow>, String> {
    let db = state.db.lock().expect("db mutex");
    db.album_songs(&album_id).map_err(db_err)
}

#[tauri::command]
pub fn library_playlist_songs(
    state: State<'_, AppState>,
    playlist_id: String,
) -> Result<Vec<SongRow>, String> {
    let db = state.db.lock().expect("db mutex");
    db.playlist_songs(&playlist_id).map_err(db_err)
}

#[tauri::command]
pub fn library_search(
    state: State<'_, AppState>,
    query: String,
    limit: Option<u32>,
) -> Result<Vec<SongRow>, String> {
    if query.trim().is_empty() {
        return Ok(Vec::new());
    }
    let db = state.db.lock().expect("db mutex");
    db.search(query.trim(), limit.unwrap_or(100).min(500)).map_err(db_err)
}

#[tauri::command]
pub fn settings_get(state: State<'_, AppState>) -> crate::settings::Settings {
    state.settings.lock().expect("settings mutex").clone()
}

#[tauri::command]
pub fn settings_set(
    app: AppHandle,
    state: State<'_, AppState>,
    settings: crate::settings::Settings,
) -> Result<(), String> {
    let dir = state.data_dir.lock().expect("data dir mutex").clone();
    let Some(dir) = dir else {
        return Err("no data directory; settings cannot be saved".into());
    };

    let previous = state.settings.lock().expect("settings mutex").source;
    crate::settings::save(&dir, &settings).map_err(|e| e.to_string())?;
    let source = settings.source;
    *state.settings.lock().expect("settings mutex") = settings;

    // Each source has its own database file. Leaving the old handle open would
    // show the previous source's library and write the new one's rows into it.
    if source != previous {
        use_database_for(&app, source)?;
        // Without this the old source's backend keeps handling playback, with
        // the new source's track ids.
        reconcile_backend(&app, source);
        let _ = app.emit("library://updated", sync::counts(&app));
    }
    Ok(())
}

#[derive(Debug, serde::Serialize)]
pub struct Lyrics {
    pub lines: Vec<crate::lyrics::Line>,
    pub plain: Option<String>,
}

#[tauri::command]
pub async fn lyrics_for(app: AppHandle, track_id: String) -> Result<Lyrics, String> {
    let state = app.state::<AppState>();

    let cached = {
        let db = state.db.lock().expect("db mutex");
        db.lyrics(&track_id).map_err(db_err)?
    };
    if let Some((synced, plain)) = cached {
        let lines = synced.as_deref().map(crate::lyrics::parse_lrc).unwrap_or_default();
        return Ok(Lyrics { lines, plain });
    }

    let Some(song) = ({
        let db = state.db.lock().expect("db mutex");
        db.song_for_lyrics(&track_id).map_err(db_err)?
    }) else {
        tracing::warn!(track = %track_id, "no library row for this track; cannot look up lyrics");
        return Ok(Lyrics { lines: Vec::new(), plain: None });
    };

    let fetched = match crate::lyrics::fetch(&song).await {
        Ok(f) => f,
        Err(e) => {
            tracing::warn!(error = %e, track = %track_id, "lyrics lookup failed");
            return Ok(Lyrics { lines: Vec::new(), plain: None });
        }
    };
    tracing::debug!(
        synced = fetched.synced.is_some(),
        plain = fetched.plain.is_some(),
        "lyrics fetched"
    );

    {
        let db = state.db.lock().expect("db mutex");
        if let Err(e) = db.save_lyrics(&track_id, fetched.synced.as_deref(), fetched.plain.as_deref())
        {
            tracing::warn!(error = %e, "could not cache lyrics");
        }
    }

    Ok(Lyrics {
        lines: fetched.synced.as_deref().map(crate::lyrics::parse_lrc).unwrap_or_default(),
        plain: fetched.plain,
    })
}

#[tauri::command]
pub fn library_counts(state: State<'_, AppState>) -> Result<LibraryCounts, String> {
    let db = state.db.lock().expect("db mutex");
    db.counts().map_err(db_err)
}

#[tauri::command]
pub fn library_sync(app: AppHandle) {
    tauri::async_runtime::spawn(async move { sync::run(app).await });
}

/// Which id the playback backend addresses a track by.
///
/// MusicKit plays from Apple's catalog, so that path needs `catalog_id` and a
/// track without one is genuinely unplayable. Native sources address tracks by
/// their own library id - and they never have a catalog id, so filtering on one
/// would silently discard the entire library.
fn playable_id(source: crate::settings::Source, s: &SongRow) -> Option<String> {
    use crate::settings::Source;
    match source {
        Source::Apple | Source::Spotify => s.catalog_id.clone(),
        Source::Navidrome | Source::Local => Some(s.id.clone()),
    }
}

fn enqueue(app: &AppHandle, songs: Vec<SongRow>, start_index: usize) -> bool {
    let state = app.state::<AppState>();

    let source = state.settings.lock().expect("settings mutex").source;

    let dead = state
        .db
        .lock()
        .expect("db mutex")
        .unresolvable_ids()
        .unwrap_or_else(|e| {
            tracing::warn!(error = %e, "could not read unresolvable ids; queueing unfiltered");
            HashSet::new()
        });

    let clicked = songs.get(start_index).and_then(|s| playable_id(source, s));

    let playable: Vec<Track> = songs
        .into_iter()
        .filter_map(|s| {
            let id = playable_id(source, &s)?;
            if dead.contains(&id) {
                return None;
            }
            Some(Track {
                id,
                title: s.name,
                artist: s.artist_name,
                album: s.album_name,
                duration_ms: s.duration_ms,
            })
        })
        .collect();

    if playable.is_empty() {
        return false;
    }

    let start = clicked
        .and_then(|id| playable.iter().position(|t| t.id == id))
        .unwrap_or_else(|| start_index.min(playable.len() - 1));
    let cmds = state.player.lock().expect("player mutex").play_queue(playable, start);
    commit(app, &state, cmds);
    true
}

#[tauri::command]
pub fn play_songs(app: AppHandle, songs: Vec<SongRow>, start_index: Option<usize>) {
    if !enqueue(&app, songs, start_index.unwrap_or(0)) {
        tracing::warn!("play_songs: nothing in the selection was playable");
    }
}

/// Whether the active source is served by Apple's catalog API.
fn uses_apple_catalog(app: &AppHandle) -> bool {
    let source = app.state::<AppState>().settings.lock().expect("settings mutex").source;
    matches!(source, crate::settings::Source::Apple)
}

fn api_client(app: &AppHandle) -> Result<crate::api::Client, String> {
    let tokens = { app.state::<AppState>().tokens.lock().expect("tokens mutex").clone() };
    let tokens = tokens.filter(|t| t.is_complete()).ok_or("not signed in")?;
    crate::api::Client::new(tokens).map_err(|e| e.to_string())
}

async fn ensure_album_songs(app: &AppHandle, album_id: &str) -> Result<Vec<SongRow>, String> {
    let cached = { app.state::<AppState>().db.lock().expect("db mutex").album_songs(album_id) };
    if let Ok(rows) = cached {
        if !rows.is_empty() {
            return Ok(rows);
        }
    }
    // Only Apple backfills on demand. Other sources mirror everything during a
    // sync, so an empty album means the sync is incomplete - querying the
    // Apple API here would misreport it as not being signed in.
    if !uses_apple_catalog(app) {
        return Err("no tracks for this album yet - try syncing your library".into());
    }
    let fetched = api_client(app)?.album_tracks(album_id).await.map_err(|e| e.to_string())?;
    if fetched.is_empty() {
        return Err("album has no tracks".into());
    }
    {
        let state = app.state::<AppState>();
        let mut db = state.db.lock().expect("db mutex");
        db.upsert_songs(&fetched).map_err(db_err)?;
    }
    app.state::<AppState>().db.lock().expect("db mutex").album_songs(album_id).map_err(db_err)
}

async fn ensure_playlist_songs(app: &AppHandle, playlist_id: &str) -> Result<Vec<SongRow>, String> {
    let cached = { app.state::<AppState>().db.lock().expect("db mutex").playlist_songs(playlist_id) };
    if let Ok(rows) = cached {
        if !rows.is_empty() {
            return Ok(rows);
        }
    }
    if !uses_apple_catalog(app) {
        return Err("no tracks for this playlist yet - try syncing your library".into());
    }
    let fetched = api_client(app)?.playlist_tracks(playlist_id).await.map_err(|e| e.to_string())?;
    if fetched.is_empty() {
        return Err("playlist has no tracks".into());
    }
    let ids: Vec<String> = fetched.iter().map(|s| s.id.clone()).collect();
    {
        let state = app.state::<AppState>();
        let mut db = state.db.lock().expect("db mutex");
        db.upsert_songs(&fetched).map_err(db_err)?;
        db.set_playlist_tracks(playlist_id, &ids).map_err(db_err)?;
    }
    app.state::<AppState>().db.lock().expect("db mutex").playlist_songs(playlist_id).map_err(db_err)
}

#[tauri::command]
pub async fn play_album(app: AppHandle, album_id: String) -> Result<(), String> {
    let songs = ensure_album_songs(&app, &album_id).await?;
    if !enqueue(&app, songs, 0) {
        return Err("no tracks in this album are playable".into());
    }
    Ok(())
}

#[tauri::command]
pub async fn play_playlist(app: AppHandle, playlist_id: String) -> Result<(), String> {
    let songs = ensure_playlist_songs(&app, &playlist_id).await?;
    if !enqueue(&app, songs, 0) {
        return Err("no tracks in this playlist are playable".into());
    }
    Ok(())
}

/// Point the library at the database belonging to `source`.
///
/// Each source has its own file, and the handle is opened once at startup from
/// whatever source was configured then. Switching source at runtime without
/// this writes the new source's rows into the previous source's database -
/// which corrupts both and defeats the per-source split entirely.
/// Make the playback backend match the active source.
///
/// Sources need different machinery: Apple drives a hidden MusicKit webview,
/// Navidrome decodes natively. Creating both is what made a Navidrome session
/// carry a second Chromium loading music.apple.com - around half the app's
/// memory, doing nothing. Leaving stale state behind is worse: after switching,
/// commands route to the wrong backend and ids from one source get fed to the
/// other.
///
/// Called on startup and on every source change, and safe to call twice.
pub(crate) fn reconcile_backend(app: &AppHandle, source: crate::settings::Source) {
    use crate::settings::Source;
    let state = app.state::<AppState>();
    let webview_source = matches!(source, Source::Apple | Source::Spotify);

    // A queue from the previous source is meaningless here: its ids belong to
    // a catalog this backend cannot address. Stop, forget it, and tell the UI
    // and Discord, or both keep announcing a track that is not playing.
    if let Some(engine) = state.audio.lock().expect("audio mutex").clone() {
        engine.stop();
    }
    state.player.lock().expect("player mutex").reset_queue();
    *state.now_reported.lock().expect("now reported mutex") = NowReported::default();
    if let Some(presence) = state.discord.lock().expect("discord mutex").clone() {
        std::thread::spawn(move || presence.clear());
    }

    if webview_source {
        if crate::engine::window(app).is_none() {
            let visible = crate::config::Runtime::from_env().show_engine_window;
            if let Err(e) = crate::engine::spawn(app, visible) {
                tracing::error!(error = %e, "could not start the playback engine");
            }
        }
        *state.audio.lock().expect("audio mutex") = None;
        *state.navidrome.lock().expect("navidrome mutex") = None;
        return;
    }

    // Native source: tear the webview down rather than leaving it resident.
    if let Some(w) = crate::engine::window(app) {
        if let Err(e) = w.close() {
            tracing::warn!(error = %e, "could not close the engine webview");
        }
        state.engine_ready.store(false, Ordering::Relaxed);
    }

    let settings = state.settings.lock().expect("settings mutex").clone();
    // Local files need no client at all; only Navidrome talks to a server.
    *state.navidrome.lock().expect("navidrome mutex") =
        crate::source::navidrome_client(&settings).map(std::sync::Arc::new);

    let has_engine = state.audio.lock().expect("audio mutex").is_some();
    if !has_engine {
        match crate::audio::Engine::new() {
            Ok(e) => {
                *state.audio.lock().expect("audio mutex") = Some(std::sync::Arc::new(e));
            }
            Err(e) => {
                tracing::error!(error = %e, "no audio output; playback disabled");
                let _ = app.emit("playback://error", e.to_string());
            }
        }
    }
}

fn use_database_for(app: &AppHandle, source: crate::settings::Source) -> Result<(), String> {
    let state = app.state::<AppState>();
    let dir = state.data_dir.lock().expect("data dir mutex").clone();
    let path = crate::db::default_db_path(dir, source).map_err(|e| e.to_string())?;
    let opened = crate::db::Db::open_at(&path).map_err(|e| e.to_string())?;
    tracing::info!(path = %path.display(), ?source, "switching library database");
    *state.db.lock().expect("db mutex") = opened;
    // Any sync still running was writing into the file we just replaced. Bump
    // the generation so it abandons itself rather than pouring the previous
    // source's rows into this one.
    state.db_generation.fetch_add(1, Ordering::SeqCst);
    Ok(())
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NavidromeStatus {
    pub configured: bool,
    pub url: String,
    pub username: String,
    pub insecure: bool,
}

#[tauri::command]
pub fn navidrome_status(state: State<'_, AppState>) -> NavidromeStatus {
    let settings = state.settings.lock().expect("settings mutex").clone();
    // Either store counts as configured: the env override exists precisely so
    // a dev machine can skip the connect screen.
    let has_password = crate::config::navidrome_env().is_some()
        || auth::load_navidrome().ok().flatten().is_some();
    NavidromeStatus {
        insecure: crate::subsonic::is_insecure(&settings.navidrome.url),
        configured: has_password
            && !settings.navidrome.url.trim().is_empty()
            && !settings.navidrome.username.trim().is_empty(),
        url: settings.navidrome.url,
        username: settings.navidrome.username,
    }
}

/// Verify a Navidrome login, then persist it.
///
/// Order matters: ping first, write second. Storing credentials we have not
/// verified would leave the app in a state where sync fails for a reason the
/// user has no way to see.
#[tauri::command]
pub async fn navidrome_connect(
    app: AppHandle,
    url: String,
    username: String,
    password: String,
) -> Result<(), String> {
    let client = crate::subsonic::Client::new(crate::subsonic::Credentials {
        base_url: url,
        username: username.clone(),
        password: password.clone(),
    })
    .map_err(|e| e.to_string())?;

    client.ping().await.map_err(|e| match e {
        crate::subsonic::SubsonicError::Unauthorized => "wrong username or password".to_string(),
        crate::subsonic::SubsonicError::Http(_) => "server unreachable".to_string(),
        other => other.to_string(),
    })?;

    auth::save_navidrome(&auth::NavidromeCredentials { password }).map_err(|e| e.to_string())?;

    {
        let state = app.state::<AppState>();
        let dir = state.data_dir.lock().expect("data dir mutex").clone();
        let mut settings = state.settings.lock().expect("settings mutex");
        settings.source = crate::settings::Source::Navidrome;
        settings.navidrome.url = client.base_url().to_string();
        settings.navidrome.username = username;
        match dir {
            Some(dir) => {
                if let Err(e) = crate::settings::save(&dir, &settings) {
                    tracing::error!(error = %e, "failed to persist navidrome settings");
                }
            }
            None => tracing::warn!("no data directory; navidrome settings not persisted"),
        }
    }

    // Must happen before the sync: otherwise Navidrome's rows land in whichever
    // database was open at launch.
    use_database_for(&app, crate::settings::Source::Navidrome)?;
    reconcile_backend(&app, crate::settings::Source::Navidrome);
    let _ = app.emit("library://updated", sync::counts(&app));

    let handle = app.clone();
    tauri::async_runtime::spawn(async move { sync::run(handle).await });
    Ok(())
}

/// Everything worth knowing in a bug report, as pasteable text.
///
/// Deliberately redacted: the server URL and username identify a private host,
/// and no credential ever appears. What remains is what actually explains a
/// fault - which source, which backend, what the library looks like, what the
/// player thinks it is doing.
#[tauri::command]
pub fn dev_diagnostics(app: AppHandle) -> String {
    let state = app.state::<AppState>();
    let settings = state.settings.lock().expect("settings mutex").clone();
    let player = state.player.lock().expect("player mutex").state().clone();
    let counts = sync::counts(&app);

    let has_audio = state.audio.lock().expect("audio mutex").is_some();
    let has_navidrome = state.navidrome.lock().expect("navidrome mutex").is_some();
    let has_discord = state.discord.lock().expect("discord mutex").is_some();
    let engine_window = crate::engine::window(&app).is_some();
    let db_path = {
        let dir = state.data_dir.lock().expect("data dir mutex").clone();
        crate::db::default_db_path(dir, settings.source)
            .map(|p| p.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default())
            .unwrap_or_else(|_| "unknown".into())
    };

    format!(
        "capsule {}\n\
         source: {:?}   glass: {}\n\
         backend: audio={} navidrome={} engine_webview={} discord={}\n\
         database: {}\n\
         library: {} songs, {} albums, {} playlists, {} artists\n\
         player: {:?}  queue={} index={:?} position={}ms volume={} shuffle={} repeat={:?}\n\
         navidrome: configured={} https={}\n\
         lastfm_key={} discord_id={} serve_art_from_server={}\n\
         onboarded: {}  developer: {}",
        env!("CARGO_PKG_VERSION"),
        settings.source,
        settings.appearance.glass,
        has_audio,
        has_navidrome,
        engine_window,
        has_discord,
        db_path,
        counts.songs,
        counts.albums,
        counts.playlists,
        counts.artists,
        player.status,
        player.queue.len(),
        player.index,
        player.position_ms,
        player.volume,
        player.shuffle,
        player.repeat,
        !settings.navidrome.url.trim().is_empty(),
        !crate::subsonic::is_insecure(&settings.navidrome.url),
        !settings.api_key().trim().is_empty(),
        !settings.discord_client_id().trim().is_empty(),
        settings.discord.serve_art_from_server,
        settings.onboarded,
        settings.developer,
    )
}

#[tauri::command]
pub fn dev_load_recent(app: AppHandle) -> Result<(), String> {
    let Some(w) = engine::window(&app) else {
        return Err("engine window not available".into());
    };
    w.eval("window.__saint && __saint.loadRecent()").map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::player::EngineCommand;

    #[test]
    fn musickit_only_commands_are_no_ops_natively() {
        // Rust already applied shuffle and repeat to its own queue; forwarding
        // them would double-apply. Prewarm has no DRM path to warm.
        assert_eq!(native_action(&EngineCommand::Prewarm { id: "x".into() }), NativeAction::Ignore);
        assert_eq!(native_action(&EngineCommand::SetShuffle { on: true }), NativeAction::Ignore);
        assert_eq!(native_action(&EngineCommand::SetRepeat { mode: 2 }), NativeAction::Ignore);
    }

    #[test]
    fn transport_commands_map_directly() {
        assert_eq!(native_action(&EngineCommand::Play), NativeAction::Play);
        assert_eq!(native_action(&EngineCommand::Pause), NativeAction::Pause);
        assert_eq!(native_action(&EngineCommand::Seek { ms: 5000 }), NativeAction::Seek(5000));
        assert_eq!(
            native_action(&EngineCommand::SetVolume { percent: 40 }),
            NativeAction::Volume(40)
        );
    }

    fn row(id: &str, catalog: Option<&str>) -> SongRow {
        SongRow {
            id: id.into(),
            catalog_id: catalog.map(|c| c.into()),
            name: "t".into(),
            artist_name: "a".into(),
            album_name: "al".into(),
            duration_ms: 1000,
            artwork_url: None,
        }
    }

    #[test]
    fn native_sources_queue_by_library_id_not_catalog_id() {
        use crate::settings::Source;
        // Navidrome tracks have no catalog id - that is an Apple concept. If
        // the queue filters on one, the entire library becomes unplayable and
        // the transport just says "Nothing queued".
        let navidrome_track = row("tr-1", None);
        assert_eq!(playable_id(Source::Navidrome, &navidrome_track).as_deref(), Some("tr-1"));
        assert_eq!(playable_id(Source::Local, &navidrome_track).as_deref(), Some("tr-1"));
    }

    #[test]
    fn apple_queues_by_catalog_id_and_rejects_rows_without_one() {
        use crate::settings::Source;
        assert_eq!(
            playable_id(Source::Apple, &row("lib-1", Some("cat-1"))).as_deref(),
            Some("cat-1")
        );
        assert_eq!(playable_id(Source::Apple, &row("lib-1", None)), None);
    }

    #[test]
    fn queue_and_skip_commands_load_the_current_index() {
        assert_eq!(
            native_action(&EngineCommand::SetQueue { ids: vec!["a".into()], start_index: 0 }),
            NativeAction::LoadCurrent
        );
        assert_eq!(native_action(&EngineCommand::SkipNext), NativeAction::LoadCurrent);
        assert_eq!(native_action(&EngineCommand::SkipPrevious), NativeAction::LoadCurrent);
    }
}
