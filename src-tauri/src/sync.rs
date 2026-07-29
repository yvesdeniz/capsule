//! Background library sync.
//!
//! Runs off the UI path entirely: pages stream from Apple straight into SQLite,
//! and the UI learns about it through events. A sync failing must never leave
//! the app unusable — the local mirror simply stays as stale as it was.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

use crate::db::LibraryCounts;
use crate::AppState;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Progress {
    pub stage: &'static str,
    pub songs: u32,
    pub albums: u32,
    pub playlists: u32,
    pub done: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncFailed {
    pub reason: String,
    pub needs_auth: bool,
}

#[derive(Default)]
pub struct SyncGuard(Arc<AtomicBool>);

impl SyncGuard {
    pub fn try_begin(&self) -> Option<SyncLease> {
        if self.0.swap(true, Ordering::SeqCst) {
            None
        } else {
            Some(SyncLease(self.0.clone()))
        }
    }
}

pub struct SyncLease(Arc<AtomicBool>);

impl Drop for SyncLease {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

fn emit(app: &AppHandle, p: &Progress) {
    let _ = app.emit("library://progress", p);
}

pub async fn run(app: AppHandle) {
    let state = app.state::<AppState>();

    let Some(lease) = state.sync.try_begin() else {
        tracing::info!("sync already running; skipping");
        return;
    };

    let settings = { state.settings.lock().expect("settings mutex").clone() };
    let tokens = { state.tokens.lock().expect("tokens mutex").clone() };

    // Credential precedence lives in one place so sync and startup cannot
    // disagree about which server they are talking to.
    let (settings, navidrome_password) = crate::source::resolve(&settings);

    let client = match crate::source::connect(&settings, tokens, navidrome_password) {
        Ok(c) => c,
        Err(e) => {
            let needs_auth = e.needs_auth();
            tracing::warn!(error = %e, "cannot start sync");
            let _ =
                app.emit("library://failed", SyncFailed { reason: e.to_string(), needs_auth });
            return;
        }
    };

    // Artwork fetches need a signed URL, which needs the live client.
    if let crate::source::SourceClient::Navidrome(ref nd) = client {
        let shared = std::sync::Arc::new(nd.clone_for_artwork());
        *app.state::<AppState>().navidrome.lock().expect("navidrome mutex") = Some(shared);
    }

    let songs = AtomicU32::new(0);
    let albums = AtomicU32::new(0);
    let playlists = AtomicU32::new(0);
    let bump = |c: &AtomicU32, n: usize| {
        c.fetch_add(n as u32, Ordering::Relaxed);
    };

    let snapshot = |stage: &'static str, done: bool| Progress {
        stage,
        songs: songs.load(Ordering::Relaxed),
        albums: albums.load(Ordering::Relaxed),
        playlists: playlists.load(Ordering::Relaxed),
        done,
    };

    emit(&app, &snapshot("songs", false));

    let songs_res = client
        .library_songs(|rows| {
            if rows.is_empty() {
                return;
            }
            let state = app.state::<AppState>();
            let written = state.db.lock().expect("db mutex").upsert_songs(&rows);
            match written {
                Ok(n) => bump(&songs, n),
                Err(e) => tracing::error!(error = %e, "song batch write failed"),
            }
            emit(&app, &snapshot("songs", false));
        })
        .await;
    if let Err(e) = songs_res {
        return fail(&app, "songs", e, lease);
    }

    let albums_res = client
        .library_albums(|rows| {
            if rows.is_empty() {
                return;
            }
            let state = app.state::<AppState>();
            let written = state.db.lock().expect("db mutex").upsert_albums(&rows);
            match written {
                Ok(n) => bump(&albums, n),
                Err(e) => tracing::error!(error = %e, "album batch write failed"),
            }
            emit(&app, &snapshot("albums", false));
        })
        .await;
    if let Err(e) = albums_res {
        return fail(&app, "albums", e, lease);
    }

    let playlists_res = client
        .library_playlists(|rows| {
            if rows.is_empty() {
                return;
            }
            let state = app.state::<AppState>();
            let written = state.db.lock().expect("db mutex").upsert_playlists(&rows);
            match written {
                Ok(n) => bump(&playlists, n),
                Err(e) => tracing::error!(error = %e, "playlist batch write failed"),
            }
            emit(&app, &snapshot("playlists", false));
        })
        .await;
    if let Err(e) = playlists_res {
        return fail(&app, "playlists", e, lease);
    }

    // Apple fills playlist membership as part of its own paging; Subsonic
    // needs a getPlaylist call per playlist.
    if let crate::source::SourceClient::Navidrome(ref nd) = client {
        let ids: Vec<String> = {
            let state = app.state::<AppState>();
            let db = state.db.lock().expect("db mutex");
            db.playlist_ids().unwrap_or_default()
        };
        for pid in ids {
            match nd.playlist_track_ids(&pid).await {
                Ok(track_ids) => {
                    let state = app.state::<AppState>();
                    let mut db = state.db.lock().expect("db mutex");
                    if let Err(e) = db.set_playlist_tracks(&pid, &track_ids) {
                        tracing::error!(error = %e, %pid, "playlist track write failed");
                    }
                }
                Err(e) => tracing::warn!(error = %e, %pid, "playlist fetch failed; skipping"),
            }
        }
    }

    let artists_res = client
        .library_artists(|rows| {
            if rows.is_empty() {
                return;
            }
            let state = app.state::<AppState>();
            let written = state.db.lock().expect("db mutex").upsert_artists(&rows);
            if let Err(e) = written {
                tracing::error!(error = %e, "artist batch write failed");
            }
        })
        .await;
    if let Err(e) = artists_res {
        tracing::warn!(error = %e, "artist sync failed; continuing");
    }

    emit(&app, &snapshot("done", true));

    {
        let state = app.state::<AppState>();
        let db = state.db.lock().expect("db mutex");
        let _ = db.set_meta("last_sync_ok", "1");
    }

    tracing::info!(
        songs = songs.load(Ordering::Relaxed),
        albums = albums.load(Ordering::Relaxed),
        playlists = playlists.load(Ordering::Relaxed),
        "sync complete"
    );
    let _ = app.emit("library://updated", counts(&app));
    drop(lease);

    let handle = app.clone();
    tauri::async_runtime::spawn(async move { crate::artwork::prefetch(handle, 56).await });
}

fn fail(app: &AppHandle, stage: &str, e: crate::source::SourceError, lease: SyncLease) {
    let needs_auth = e.needs_auth();
    tracing::error!(stage, error = %e, "sync stage failed");
    let _ = app.emit("library://failed", SyncFailed { reason: e.to_string(), needs_auth });
    drop(lease);
}

pub fn counts(app: &AppHandle) -> LibraryCounts {
    let state = app.state::<AppState>();
    let db = state.db.lock().expect("db mutex");
    db.counts().unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guard_allows_one_sync_at_a_time() {
        let g = SyncGuard::default();
        let first = g.try_begin();
        assert!(first.is_some(), "first sync should start");
        assert!(g.try_begin().is_none(), "second concurrent sync must be refused");
        drop(first);
        assert!(g.try_begin().is_some(), "guard must release after the lease drops");
    }
}
