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
use crate::{sync, AppState};

pub fn apply(app: &AppHandle, f: impl FnOnce(&mut Player) -> Vec<EngineCommand>) {
    let state = app.state::<AppState>();
    let cmds = {
        let mut p = state.player.lock().expect("player mutex");
        f(&mut p)
    };
    commit(app, &state, cmds);
}

fn commit(app: &AppHandle, state: &AppState, cmds: Vec<EngineCommand>) {
    for c in &cmds {
        tracing::debug!(?c, "engine command");
        if let Err(e) = engine::send(app, c) {
            tracing::warn!(?c, error = %e, "failed to send engine command");
        }
    }
    publish(app, state);
}

fn publish(app: &AppHandle, state: &AppState) {
    let snapshot = state.player.lock().expect("player mutex").state().clone();
    if let Err(e) = app.emit("player://state", &snapshot) {
        tracing::warn!(error = %e, "failed to emit player state");
    }
    #[cfg(target_os = "windows")]
    {
        crate::smtc::update(app, &snapshot);
        crate::thumbbar::refresh(app, snapshot.status);
    }
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
                tracing::info!("not signed in — opening Apple Music login");
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
    state: State<'_, AppState>,
    settings: crate::settings::Settings,
) -> Result<(), String> {
    let dir = state.data_dir.lock().expect("data dir mutex").clone();
    let Some(dir) = dir else {
        return Err("no data directory; settings cannot be saved".into());
    };

    crate::settings::save(&dir, &settings).map_err(|e| e.to_string())?;
    *state.settings.lock().expect("settings mutex") = settings;
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

fn enqueue(app: &AppHandle, songs: Vec<SongRow>, start_index: usize) -> bool {
    let state = app.state::<AppState>();

    let dead = state
        .db
        .lock()
        .expect("db mutex")
        .unresolvable_ids()
        .unwrap_or_else(|e| {
            tracing::warn!(error = %e, "could not read unresolvable ids; queueing unfiltered");
            HashSet::new()
        });

    let clicked = songs.get(start_index).and_then(|s| s.catalog_id.clone());

    let playable: Vec<Track> = songs
        .into_iter()
        .filter_map(|s| {
            let id = s.catalog_id.clone()?;
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
        tracing::warn!("play_songs: no rows had a catalog id");
    }
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

#[tauri::command]
pub fn dev_load_recent(app: AppHandle) -> Result<(), String> {
    let Some(w) = engine::window(&app) else {
        return Err("engine window not available".into());
    };
    w.eval("window.__saint && __saint.loadRecent()").map_err(|e| e.to_string())
}
