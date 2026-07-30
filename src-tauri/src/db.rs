//! Local mirror of the Apple Music library.
//!
//! This is the reason the app feels faster than Apple's: the UI reads only from
//! here, so browsing never waits on the network. Sync writes in the background
//! and the UI finds out via an event.
//!
//! Apple gives library items IDs like `l.AbCdEf` which are **not** catalog IDs.
//! Playback needs the catalog ID, so both are stored and `catalog_id` is what
//! the player queues.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

#[derive(Debug, thiserror::Error)]
pub enum DbError {
    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("could not resolve a data directory")]
    NoDataDir,
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

const MIGRATIONS: &[&str] = &[
    r#"
    CREATE TABLE albums (
        id            TEXT PRIMARY KEY,
        catalog_id    TEXT,
        name          TEXT NOT NULL,
        artist_name   TEXT NOT NULL DEFAULT '',
        artwork_url   TEXT,
        release_date  TEXT,
        track_count   INTEGER NOT NULL DEFAULT 0,
        added_at      TEXT
    );
    CREATE TABLE artists (
        id          TEXT PRIMARY KEY,
        name        TEXT NOT NULL,
        artwork_url TEXT
    );
    CREATE TABLE songs (
        id           TEXT PRIMARY KEY,
        catalog_id   TEXT,
        name         TEXT NOT NULL,
        artist_name  TEXT NOT NULL DEFAULT '',
        album_name   TEXT NOT NULL DEFAULT '',
        album_id     TEXT,
        duration_ms  INTEGER NOT NULL DEFAULT 0,
        artwork_url  TEXT,
        track_number INTEGER,
        disc_number  INTEGER,
        added_at     TEXT
    );
    CREATE TABLE playlists (
        id          TEXT PRIMARY KEY,
        name        TEXT NOT NULL,
        description TEXT,
        artwork_url TEXT,
        can_edit    INTEGER NOT NULL DEFAULT 0
    );
    CREATE TABLE playlist_tracks (
        playlist_id TEXT NOT NULL,
        position    INTEGER NOT NULL,
        song_id     TEXT NOT NULL,
        PRIMARY KEY (playlist_id, position)
    );
    CREATE TABLE meta (
        key   TEXT PRIMARY KEY,
        value TEXT NOT NULL
    );
    CREATE INDEX idx_songs_album   ON songs(album_id);
    CREATE INDEX idx_songs_artist  ON songs(artist_name);
    CREATE INDEX idx_songs_name    ON songs(name);
    CREATE INDEX idx_albums_artist ON albums(artist_name);
    "#,
    r#"
    ALTER TABLE songs     ADD COLUMN artwork_width  INTEGER;
    ALTER TABLE songs     ADD COLUMN artwork_height INTEGER;
    ALTER TABLE albums    ADD COLUMN artwork_width  INTEGER;
    ALTER TABLE albums    ADD COLUMN artwork_height INTEGER;
    ALTER TABLE playlists ADD COLUMN artwork_width  INTEGER;
    ALTER TABLE playlists ADD COLUMN artwork_height INTEGER;
    "#,
    r#"
    CREATE TABLE unresolvable (
        catalog_id TEXT PRIMARY KEY,
        seen_at    TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
    );
    "#,
    r#"
    CREATE TABLE lyrics (
        track_id   TEXT PRIMARY KEY,
        synced     TEXT,
        plain      TEXT,
        fetched_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
    );
    "#,
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SongLookup {
    pub name: String,
    pub artist_name: String,
    pub album_name: String,
    pub duration_ms: u64,
}

pub const MAX_ARTWORK: u32 = 3000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Artwork {
    pub template: String,
    pub max_side: Option<u32>,
}

impl Artwork {
    pub fn best_size(&self) -> u32 {
        self.max_side.unwrap_or(MAX_ARTWORK).min(MAX_ARTWORK)
    }

    pub fn clamp(&self, requested: u32) -> u32 {
        requested.min(self.best_size())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SongRow {
    pub id: String,
    pub catalog_id: Option<String>,
    pub name: String,
    pub artist_name: String,
    pub album_name: String,
    pub duration_ms: u64,
    pub artwork_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AlbumRow {
    pub id: String,
    pub name: String,
    pub artist_name: String,
    pub artwork_url: Option<String>,
    pub track_count: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PlaylistRow {
    pub id: String,
    pub name: String,
    pub artwork_url: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default)]
pub struct LibraryCounts {
    pub songs: u32,
    pub albums: u32,
    pub playlists: u32,
    pub artists: u32,
}

pub struct Db {
    conn: Connection,
}

impl Db {
    pub fn open_at(path: &Path) -> Result<Self, DbError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(path)?;
        Self::init(conn)
    }

    #[cfg(test)]
    pub fn open_in_memory() -> Result<Self, DbError> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> Result<Self, DbError> {
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        let mut db = Self { conn };
        db.migrate()?;
        Ok(db)
    }

    fn migrate(&mut self) -> Result<(), DbError> {
        let current: i64 = self.conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
        let target = MIGRATIONS.len() as i64;
        if current >= target {
            return Ok(());
        }
        for (i, sql) in MIGRATIONS.iter().enumerate().skip(current as usize) {
            let tx = self.conn.transaction()?;
            tx.execute_batch(sql)?;
            tx.pragma_update(None, "user_version", (i + 1) as i64)?;
            tx.commit()?;
            tracing::info!(version = i + 1, "applied db migration");
        }
        Ok(())
    }

    pub fn upsert_songs(&mut self, rows: &[SongUpsert]) -> Result<usize, DbError> {
        let tx = self.conn.transaction()?;
        {
            let mut stmt = tx.prepare(
                "INSERT INTO songs
                   (id, catalog_id, name, artist_name, album_name, album_id,
                    duration_ms, artwork_url, track_number, disc_number, added_at,
                    artwork_width, artwork_height)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13)
                 ON CONFLICT(id) DO UPDATE SET
                   catalog_id=excluded.catalog_id,
                   name=excluded.name,
                   artist_name=excluded.artist_name,
                   album_name=excluded.album_name,
                   album_id=excluded.album_id,
                   duration_ms=excluded.duration_ms,
                   artwork_url=excluded.artwork_url,
                   track_number=excluded.track_number,
                   disc_number=excluded.disc_number,
                   artwork_width=excluded.artwork_width,
                   artwork_height=excluded.artwork_height",
            )?;
            for r in rows {
                stmt.execute(params![
                    r.id,
                    r.catalog_id,
                    r.name,
                    r.artist_name,
                    r.album_name,
                    r.album_id,
                    r.duration_ms as i64,
                    r.artwork_url,
                    r.track_number,
                    r.disc_number,
                    r.added_at,
                    r.artwork_width,
                    r.artwork_height,
                ])?;
            }
        }
        tx.commit()?;
        Ok(rows.len())
    }

    pub fn upsert_albums(&mut self, rows: &[AlbumUpsert]) -> Result<usize, DbError> {
        let tx = self.conn.transaction()?;
        {
            let mut stmt = tx.prepare(
                "INSERT INTO albums
                   (id, catalog_id, name, artist_name, artwork_url, release_date,
                    track_count, added_at, artwork_width, artwork_height)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)
                 ON CONFLICT(id) DO UPDATE SET
                   catalog_id=excluded.catalog_id,
                   name=excluded.name,
                   artist_name=excluded.artist_name,
                   artwork_url=excluded.artwork_url,
                   release_date=excluded.release_date,
                   track_count=excluded.track_count,
                   artwork_width=excluded.artwork_width,
                   artwork_height=excluded.artwork_height",
            )?;
            for r in rows {
                stmt.execute(params![
                    r.id,
                    r.catalog_id,
                    r.name,
                    r.artist_name,
                    r.artwork_url,
                    r.release_date,
                    r.track_count as i64,
                    r.added_at,
                    r.artwork_width,
                    r.artwork_height,
                ])?;
            }
        }
        tx.commit()?;
        Ok(rows.len())
    }

    pub fn upsert_playlists(&mut self, rows: &[PlaylistUpsert]) -> Result<usize, DbError> {
        let tx = self.conn.transaction()?;
        {
            let mut stmt = tx.prepare(
                "INSERT INTO playlists
                   (id, name, description, artwork_url, can_edit, artwork_width, artwork_height)
                 VALUES (?1,?2,?3,?4,?5,?6,?7)
                 ON CONFLICT(id) DO UPDATE SET
                   name=excluded.name,
                   description=excluded.description,
                   artwork_url=excluded.artwork_url,
                   can_edit=excluded.can_edit,
                   artwork_width=excluded.artwork_width,
                   artwork_height=excluded.artwork_height",
            )?;
            for r in rows {
                stmt.execute(params![
                    r.id,
                    r.name,
                    r.description,
                    r.artwork_url,
                    r.can_edit as i64,
                    r.artwork_width,
                    r.artwork_height,
                ])?;
            }
        }
        tx.commit()?;
        Ok(rows.len())
    }

    pub fn upsert_artists(&mut self, rows: &[ArtistUpsert]) -> Result<usize, DbError> {
        let tx = self.conn.transaction()?;
        {
            let mut stmt = tx.prepare(
                "INSERT INTO artists (id, name, artwork_url) VALUES (?1, ?2, ?3)
                 ON CONFLICT(id) DO UPDATE SET name=excluded.name, artwork_url=excluded.artwork_url",
            )?;
            for r in rows {
                stmt.execute(params![r.id, r.name, r.artwork_url])?;
            }
        }
        tx.commit()?;
        Ok(rows.len())
    }

    /// Every playlist id currently mirrored. Sources that fill playlist
    /// membership in a second pass need this to know what to ask for.
    pub fn playlist_ids(&self) -> Result<Vec<String>, DbError> {
        let mut stmt = self.conn.prepare("SELECT id FROM playlists")?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        Ok(rows.filter_map(Result::ok).collect())
    }

    pub fn set_playlist_tracks(&mut self, playlist_id: &str, song_ids: &[String]) -> Result<(), DbError> {
        let tx = self.conn.transaction()?;
        {
            tx.execute("DELETE FROM playlist_tracks WHERE playlist_id = ?1", params![playlist_id])?;
            let mut stmt = tx.prepare(
                "INSERT INTO playlist_tracks (playlist_id, position, song_id) VALUES (?1, ?2, ?3)",
            )?;
            for (pos, song_id) in song_ids.iter().enumerate() {
                stmt.execute(params![playlist_id, pos as i64, song_id])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    pub fn playlist_songs(&self, playlist_id: &str) -> Result<Vec<SongRow>, DbError> {
        let mut stmt = self.conn.prepare(
            "SELECT s.id, s.catalog_id, s.name, s.artist_name, s.album_name, s.duration_ms, s.artwork_url
             FROM playlist_tracks pt
             JOIN songs s ON s.id = pt.song_id
             WHERE pt.playlist_id = ?1
             ORDER BY pt.position",
        )?;
        let rows = stmt
            .query_map(params![playlist_id], map_song)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn set_meta(&self, key: &str, value: &str) -> Result<(), DbError> {
        self.conn.execute(
            "INSERT INTO meta (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    pub fn meta(&self, key: &str) -> Result<Option<String>, DbError> {
        Ok(self
            .conn
            .query_row("SELECT value FROM meta WHERE key = ?1", params![key], |r| r.get(0))
            .optional()?)
    }

    pub fn mark_unresolvable(&self, ids: &[String]) -> Result<usize, DbError> {
        let mut stmt =
            self.conn.prepare("INSERT OR IGNORE INTO unresolvable (catalog_id) VALUES (?1)")?;
        let mut added = 0;
        for id in ids {
            added += stmt.execute(params![id])?;
        }
        Ok(added)
    }

    #[allow(clippy::type_complexity)]
    pub fn lyrics(&self, track_id: &str) -> Result<Option<(Option<String>, Option<String>)>, DbError> {
        let row = self
            .conn
            .query_row("SELECT synced, plain FROM lyrics WHERE track_id = ?1", params![track_id], |r| {
                Ok((r.get::<_, Option<String>>(0)?, r.get::<_, Option<String>>(1)?))
            })
            .optional()?;
        Ok(row)
    }

    pub fn save_lyrics(
        &self,
        track_id: &str,
        synced: Option<&str>,
        plain: Option<&str>,
    ) -> Result<(), DbError> {
        self.conn.execute(
            "INSERT INTO lyrics (track_id, synced, plain) VALUES (?1, ?2, ?3)
             ON CONFLICT(track_id) DO UPDATE SET
                synced = excluded.synced,
                plain = excluded.plain,
                fetched_at = CURRENT_TIMESTAMP",
            params![track_id, synced, plain],
        )?;
        Ok(())
    }

    pub fn song_for_lyrics(&self, catalog_id: &str) -> Result<Option<SongLookup>, DbError> {
        let row = self
            .conn
            .query_row(
                // Matched on either id: native sources have no catalog id, so
                // looking up by that alone silently returns nothing and lyrics
                // never resolve. Mirrors `artwork_for`.
                "SELECT name, artist_name, album_name, duration_ms
                 FROM songs WHERE id = ?1 OR catalog_id = ?1 LIMIT 1",
                params![catalog_id],
                |r| {
                    Ok(SongLookup {
                        name: r.get(0)?,
                        artist_name: r.get(1)?,
                        album_name: r.get(2)?,
                        duration_ms: r.get(3)?,
                    })
                },
            )
            .optional()?;
        Ok(row)
    }

    pub fn first_playable_catalog_id(&self) -> Result<Option<String>, DbError> {
        let id = self
            .conn
            .query_row(
                "SELECT s.catalog_id FROM songs s
                 LEFT JOIN unresolvable u ON u.catalog_id = s.catalog_id
                 WHERE s.catalog_id IS NOT NULL AND u.catalog_id IS NULL
                 LIMIT 1",
                [],
                |r| r.get::<_, String>(0),
            )
            .optional()?;
        Ok(id)
    }

    pub fn unresolvable_ids(&self) -> Result<HashSet<String>, DbError> {
        let mut stmt = self.conn.prepare("SELECT catalog_id FROM unresolvable")?;
        let ids = stmt
            .query_map([], |r| r.get::<_, String>(0))?
            .collect::<Result<HashSet<_>, _>>()?;
        Ok(ids)
    }

    pub fn songs(&self, limit: u32, offset: u32) -> Result<Vec<SongRow>, DbError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, catalog_id, name, artist_name, album_name, duration_ms, artwork_url
             FROM songs ORDER BY name COLLATE NOCASE LIMIT ?1 OFFSET ?2",
        )?;
        let rows = stmt
            .query_map(params![limit, offset], map_song)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn album_songs(&self, album_id: &str) -> Result<Vec<SongRow>, DbError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, catalog_id, name, artist_name, album_name, duration_ms, artwork_url
             FROM songs WHERE album_id = ?1
             ORDER BY COALESCE(disc_number,1), COALESCE(track_number,0)",
        )?;
        let rows = stmt
            .query_map(params![album_id], map_song)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn albums(&self, limit: u32, offset: u32) -> Result<Vec<AlbumRow>, DbError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, name, artist_name, artwork_url, track_count
             FROM albums ORDER BY artist_name COLLATE NOCASE, name COLLATE NOCASE
             LIMIT ?1 OFFSET ?2",
        )?;
        let rows = stmt
            .query_map(params![limit, offset], |r| {
                Ok(AlbumRow {
                    id: r.get(0)?,
                    name: r.get(1)?,
                    artist_name: r.get(2)?,
                    artwork_url: r.get(3)?,
                    track_count: r.get::<_, i64>(4)? as u32,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn playlists(&self) -> Result<Vec<PlaylistRow>, DbError> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, name, artwork_url FROM playlists ORDER BY name COLLATE NOCASE")?;
        let rows = stmt
            .query_map([], |r| {
                Ok(PlaylistRow { id: r.get(0)?, name: r.get(1)?, artwork_url: r.get(2)? })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn search(&self, query: &str, limit: u32) -> Result<Vec<SongRow>, DbError> {
        let escaped = query.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_");
        let pattern = format!("%{escaped}%");
        let mut stmt = self.conn.prepare(
            "SELECT id, catalog_id, name, artist_name, album_name, duration_ms, artwork_url
             FROM songs
             WHERE name LIKE ?1 ESCAPE '\\'
                OR artist_name LIKE ?1 ESCAPE '\\'
                OR album_name LIKE ?1 ESCAPE '\\'
             ORDER BY name COLLATE NOCASE LIMIT ?2",
        )?;
        let rows = stmt
            .query_map(params![pattern, limit], map_song)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn counts(&self) -> Result<LibraryCounts, DbError> {
        let one = |sql: &str| -> Result<u32, DbError> {
            Ok(self.conn.query_row(sql, [], |r| r.get::<_, i64>(0))? as u32)
        };
        Ok(LibraryCounts {
            songs: one("SELECT COUNT(*) FROM songs")?,
            albums: one("SELECT COUNT(*) FROM albums")?,
            playlists: one("SELECT COUNT(*) FROM playlists")?,
            artists: one("SELECT COUNT(*) FROM artists")?,
        })
    }

    pub fn all_artwork(&self) -> Result<Vec<Artwork>, DbError> {
        let mut stmt = self.conn.prepare(
            "SELECT artwork_url, MAX(COALESCE(artwork_width, 0), COALESCE(artwork_height, 0))
             FROM (
                 SELECT artwork_url, artwork_width, artwork_height FROM albums
                 UNION SELECT artwork_url, artwork_width, artwork_height FROM songs
                 UNION SELECT artwork_url, artwork_width, artwork_height FROM playlists
             )
             WHERE artwork_url IS NOT NULL AND artwork_url <> ''
             GROUP BY artwork_url",
        )?;
        let rows = stmt
            .query_map([], |r| {
                let side: i64 = r.get(1)?;
                Ok(Artwork {
                    template: r.get(0)?,
                    max_side: (side > 0).then_some(side as u32),
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn artwork_for(&self, id: &str) -> Result<Option<Artwork>, DbError> {
        let row = self
            .conn
            .query_row(
                "SELECT artwork_url, artwork_width, artwork_height FROM songs
                   WHERE id = ?1 OR catalog_id = ?1
                 UNION ALL
                 SELECT artwork_url, artwork_width, artwork_height FROM albums WHERE id = ?1
                 UNION ALL
                 SELECT artwork_url, artwork_width, artwork_height FROM playlists WHERE id = ?1
                 LIMIT 1",
                params![id],
                |r| {
                    let url: Option<String> = r.get(0)?;
                    let w: Option<i64> = r.get(1)?;
                    let h: Option<i64> = r.get(2)?;
                    Ok((url, w, h))
                },
            )
            .optional()?;

        Ok(row.and_then(|(url, w, h)| {
            let template = url.filter(|u| !u.is_empty())?;
            let side = w.unwrap_or(0).max(h.unwrap_or(0));
            Some(Artwork { template, max_side: (side > 0).then_some(side as u32) })
        }))
    }
}

fn map_song(r: &rusqlite::Row<'_>) -> rusqlite::Result<SongRow> {
    Ok(SongRow {
        id: r.get(0)?,
        catalog_id: r.get(1)?,
        name: r.get(2)?,
        artist_name: r.get(3)?,
        album_name: r.get(4)?,
        duration_ms: r.get::<_, i64>(5)?.max(0) as u64,
        artwork_url: r.get(6)?,
    })
}

#[derive(Debug, Clone, Default)]
pub struct SongUpsert {
    pub id: String,
    pub catalog_id: Option<String>,
    pub name: String,
    pub artist_name: String,
    pub album_name: String,
    pub album_id: Option<String>,
    pub duration_ms: u64,
    pub artwork_url: Option<String>,
    pub track_number: Option<i64>,
    pub disc_number: Option<i64>,
    pub added_at: Option<String>,
    pub artwork_width: Option<i64>,
    pub artwork_height: Option<i64>,
}

#[derive(Debug, Clone, Default)]
pub struct AlbumUpsert {
    pub id: String,
    pub catalog_id: Option<String>,
    pub name: String,
    pub artist_name: String,
    pub artwork_url: Option<String>,
    pub release_date: Option<String>,
    pub track_count: u32,
    pub added_at: Option<String>,
    pub artwork_width: Option<i64>,
    pub artwork_height: Option<i64>,
}

#[derive(Debug, Clone, Default)]
pub struct ArtistUpsert {
    pub id: String,
    pub name: String,
    pub artwork_url: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct PlaylistUpsert {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub artwork_url: Option<String>,
    pub can_edit: bool,
    pub artwork_width: Option<i64>,
    pub artwork_height: Option<i64>,
}

pub fn default_db_path(
    app_data: Option<PathBuf>,
    source: crate::settings::Source,
) -> Result<PathBuf, DbError> {
    if let Ok(p) = std::env::var("CAPSULE_DB_PATH") {
        if !p.trim().is_empty() {
            return Ok(PathBuf::from(p));
        }
    }
    let file = match source {
        crate::settings::Source::Apple => "library-apple.sqlite3",
        crate::settings::Source::Navidrome => "library-navidrome.sqlite3",
        crate::settings::Source::Spotify => "library-spotify.sqlite3",
        crate::settings::Source::Local => "library-local.sqlite3",
    };
    Ok(app_data.ok_or(DbError::NoDataDir)?.join(file))
}

/// One-time move of the pre-multi-source database.
///
/// Installs from before per-source files have `library.sqlite3`. Leaving it
/// behind would silently orphan the library and trigger a full re-sync, so it
/// is claimed for Apple, which is the only source that could have written it.
/// Never overwrites an existing target.
///
/// The `-wal` and `-shm` sidecars move with it. SQLite derives their names from
/// the database filename, so renaming the main file alone strands the write-ahead
/// log - and in WAL mode that log holds committed transactions that have not yet
/// been checkpointed. Losing it loses data.
pub fn migrate_legacy_db(dir: &Path) -> std::io::Result<()> {
    let legacy = dir.join("library.sqlite3");
    let target = dir.join("library-apple.sqlite3");
    if !legacy.exists() || target.exists() {
        return Ok(());
    }

    tracing::info!("migrating legacy library.sqlite3 to library-apple.sqlite3");
    std::fs::rename(&legacy, &target)?;

    // Sidecars are best-effort: a clean shutdown leaves none, and failing the
    // whole migration because one is missing would be worse than continuing.
    for suffix in ["-wal", "-shm"] {
        let from = dir.join(format!("library.sqlite3{suffix}"));
        let to = dir.join(format!("library-apple.sqlite3{suffix}"));
        if from.exists() && !to.exists() {
            if let Err(e) = std::fs::rename(&from, &to) {
                tracing::warn!(error = %e, suffix, "could not move sqlite sidecar");
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::Source;

    fn song(id: &str, name: &str, artist: &str) -> SongUpsert {
        SongUpsert {
            id: id.into(),
            catalog_id: Some(format!("cat-{id}")),
            name: name.into(),
            artist_name: artist.into(),
            album_name: "Album".into(),
            album_id: Some("al1".into()),
            duration_ms: 200_000,
            artwork_url: Some("https://ex/{w}x{h}.jpg".into()),
            track_number: Some(1),
            disc_number: Some(1),
            added_at: None,
            artwork_width: Some(1200),
            artwork_height: Some(1200),
        }
    }

    #[test]
    fn migrations_apply_and_are_idempotent() {
        let db = Db::open_in_memory().unwrap();
        let v: i64 = db.conn.pragma_query_value(None, "user_version", |r| r.get(0)).unwrap();
        assert_eq!(v, MIGRATIONS.len() as i64);
        let mut db2 = db;
        db2.migrate().unwrap();
        let v2: i64 = db2.conn.pragma_query_value(None, "user_version", |r| r.get(0)).unwrap();
        assert_eq!(v2, MIGRATIONS.len() as i64);
    }

    #[test]
    fn unresolvable_ids_round_trip_and_ignore_repeats() {
        let db = Db::open_in_memory().unwrap();
        assert!(db.unresolvable_ids().unwrap().is_empty(), "fresh db has none");

        let added = db.mark_unresolvable(&["1683303482".into(), "1800007403".into()]).unwrap();
        assert_eq!(added, 2);

        let again = db.mark_unresolvable(&["1683303482".into()]).unwrap();
        assert_eq!(again, 0, "already-known id is ignored");

        let ids = db.unresolvable_ids().unwrap();
        assert_eq!(ids.len(), 2);
        assert!(ids.contains("1683303482"));
    }

    #[test]
    fn upsert_is_idempotent_and_updates_in_place() {
        let mut db = Db::open_in_memory().unwrap();
        db.upsert_songs(&[song("s1", "First", "A")]).unwrap();
        db.upsert_songs(&[song("s1", "First Renamed", "A")]).unwrap();
        let rows = db.songs(10, 0).unwrap();
        assert_eq!(rows.len(), 1, "same id must not duplicate");
        assert_eq!(rows[0].name, "First Renamed");
    }

    #[test]
    fn catalog_id_is_preserved_because_playback_needs_it() {
        let mut db = Db::open_in_memory().unwrap();
        db.upsert_songs(&[song("l.abc", "T", "A")]).unwrap();
        let rows = db.songs(10, 0).unwrap();
        assert_eq!(rows[0].catalog_id.as_deref(), Some("cat-l.abc"));
    }

    #[test]
    fn album_songs_order_by_disc_then_track() {
        let mut db = Db::open_in_memory().unwrap();
        let mut a = song("s1", "one", "A");
        a.track_number = Some(2);
        a.disc_number = Some(1);
        let mut b = song("s2", "two", "A");
        b.track_number = Some(1);
        b.disc_number = Some(1);
        let mut c = song("s3", "three", "A");
        c.track_number = Some(1);
        c.disc_number = Some(2);
        db.upsert_songs(&[a, b, c]).unwrap();
        let ids: Vec<_> = db.album_songs("al1").unwrap().into_iter().map(|s| s.id).collect();
        assert_eq!(ids, vec!["s2", "s1", "s3"]);
    }

    #[test]
    fn search_matches_title_artist_and_album() {
        let mut db = Db::open_in_memory().unwrap();
        db.upsert_songs(&[song("s1", "Drama", "Bladee"), song("s2", "Obedient", "Ecco2k")])
            .unwrap();
        assert_eq!(db.search("blad", 10).unwrap().len(), 1);
        assert_eq!(db.search("obed", 10).unwrap().len(), 1);
        assert_eq!(db.search("Album", 10).unwrap().len(), 2);
        assert_eq!(db.search("nothing", 10).unwrap().len(), 0);
    }

    #[test]
    fn search_escapes_like_wildcards() {
        let mut db = Db::open_in_memory().unwrap();
        db.upsert_songs(&[song("s1", "100% Real", "A"), song("s2", "Other", "B")]).unwrap();
        assert_eq!(db.search("%", 10).unwrap().len(), 1);
        assert_eq!(db.search("100%", 10).unwrap().len(), 1);
    }

    #[test]
    fn counts_reflect_contents() {
        let mut db = Db::open_in_memory().unwrap();
        db.upsert_songs(&[song("s1", "a", "A"), song("s2", "b", "B")]).unwrap();
        db.upsert_albums(&[AlbumUpsert {
            id: "al1".into(),
            name: "Album".into(),
            artist_name: "A".into(),
            track_count: 2,
            ..Default::default()
        }])
        .unwrap();
        let c = db.counts().unwrap();
        assert_eq!((c.songs, c.albums), (2, 1));
    }

    #[test]
    fn meta_round_trips_and_overwrites() {
        let db = Db::open_in_memory().unwrap();
        assert_eq!(db.meta("last_sync").unwrap(), None);
        db.set_meta("last_sync", "t1").unwrap();
        db.set_meta("last_sync", "t2").unwrap();
        assert_eq!(db.meta("last_sync").unwrap().as_deref(), Some("t2"));
    }

    #[test]
    fn artwork_lookup_spans_tables() {
        let mut db = Db::open_in_memory().unwrap();
        db.upsert_playlists(&[PlaylistUpsert {
            id: "p1".into(),
            name: "Mix".into(),
            artwork_url: Some("https://pl/{w}x{h}.jpg".into()),
            ..Default::default()
        }])
        .unwrap();
        let art = db.artwork_for("p1").unwrap().unwrap();
        assert_eq!(art.template, "https://pl/{w}x{h}.jpg");
        assert_eq!(db.artwork_for("missing").unwrap(), None);
    }

    #[test]
    fn artwork_carries_its_true_maximum_size() {
        let mut db = Db::open_in_memory().unwrap();
        db.upsert_songs(&[song("s1", "a", "A")]).unwrap();
        let art = db.artwork_for("s1").unwrap().unwrap();
        assert_eq!(art.max_side, Some(1200));
        assert_eq!(art.clamp(3000), 1200);
        assert_eq!(art.clamp(56), 56);
    }

    #[test]
    fn missing_dimensions_leave_max_side_unknown() {
        let mut db = Db::open_in_memory().unwrap();
        let mut s = song("s1", "a", "A");
        s.artwork_width = None;
        s.artwork_height = None;
        db.upsert_songs(&[s]).unwrap();
        let art = db.artwork_for("s1").unwrap().unwrap();
        assert_eq!(art.max_side, None);
        assert_eq!(art.best_size(), MAX_ARTWORK);
    }

    #[test]
    fn artwork_templates_are_deduplicated_for_prefetch() {
        let mut db = Db::open_in_memory().unwrap();
        let mut a = song("s1", "a", "A");
        let mut b = song("s2", "b", "A");
        a.artwork_url = Some("https://same/{w}x{h}.jpg".into());
        b.artwork_url = Some("https://same/{w}x{h}.jpg".into());
        let mut c = song("s3", "c", "C");
        c.artwork_url = Some("https://other/{w}x{h}.jpg".into());
        db.upsert_songs(&[a, b, c]).unwrap();

        let mut t: Vec<String> =
            db.all_artwork().unwrap().into_iter().map(|a| a.template).collect();
        t.sort();
        assert_eq!(t, vec!["https://other/{w}x{h}.jpg", "https://same/{w}x{h}.jpg"]);
    }

    #[test]
    fn artwork_templates_skip_rows_without_art() {
        let mut db = Db::open_in_memory().unwrap();
        let mut a = song("s1", "a", "A");
        a.artwork_url = None;
        let mut b = song("s2", "b", "B");
        b.artwork_url = Some(String::new());
        db.upsert_songs(&[a, b]).unwrap();
        assert!(db.all_artwork().unwrap().is_empty());
    }

    #[test]
    fn queries_stay_fast_with_a_large_library() {
        let mut db = Db::open_in_memory().unwrap();
        let rows: Vec<SongUpsert> = (0..50_000)
            .map(|i| {
                let mut s = song(&format!("s{i}"), &format!("Track {i:05}"), "Artist");
                s.album_id = Some(format!("al{}", i % 500));
                s
            })
            .collect();
        db.upsert_songs(&rows).unwrap();
        assert_eq!(db.counts().unwrap().songs, 50_000);

        let t = std::time::Instant::now();
        let page = db.songs(200, 0).unwrap();
        let paged = t.elapsed();
        assert_eq!(page.len(), 200);

        let t = std::time::Instant::now();
        let exact = db.search("Track 04999", 100).unwrap();
        let searched = t.elapsed();
        assert_eq!(exact.len(), 1);
        assert_eq!(db.search("Track 0499", 100).unwrap().len(), 10);

        let t = std::time::Instant::now();
        let album = db.album_songs("al7").unwrap();
        let by_album = t.elapsed();
        assert_eq!(album.len(), 100);

        assert!(paged.as_millis() < 500, "paging took {paged:?}");
        assert!(searched.as_millis() < 2000, "search took {searched:?}");
        assert!(by_album.as_millis() < 500, "album lookup took {by_album:?}");
    }

    #[test]
    fn playlist_songs_come_back_in_playlist_order() {
        let mut db = Db::open_in_memory().unwrap();
        db.upsert_songs(&[song("a", "A", "x"), song("b", "B", "x"), song("c", "C", "x")]).unwrap();
        db.set_playlist_tracks("p1", &["c".into(), "a".into(), "b".into()]).unwrap();
        let ids: Vec<_> = db.playlist_songs("p1").unwrap().into_iter().map(|s| s.id).collect();
        assert_eq!(ids, vec!["c", "a", "b"]);
    }

    #[test]
    fn resyncing_a_playlist_replaces_rather_than_appends() {
        let mut db = Db::open_in_memory().unwrap();
        db.upsert_songs(&[song("a", "A", "x"), song("b", "B", "x")]).unwrap();
        db.set_playlist_tracks("p1", &["a".into(), "b".into()]).unwrap();
        db.set_playlist_tracks("p1", &["b".into()]).unwrap();
        let ids: Vec<_> = db.playlist_songs("p1").unwrap().into_iter().map(|s| s.id).collect();
        assert_eq!(ids, vec!["b"], "old rows must be cleared, not duplicated");
    }

    #[test]
    fn playlist_songs_skip_tracks_not_yet_upserted() {
        let mut db = Db::open_in_memory().unwrap();
        db.upsert_songs(&[song("a", "A", "x")]).unwrap();
        db.set_playlist_tracks("p1", &["a".into(), "missing".into()]).unwrap();
        assert_eq!(db.playlist_songs("p1").unwrap().len(), 1);
    }

    #[test]
    fn artists_upsert_and_count() {
        let mut db = Db::open_in_memory().unwrap();
        db.upsert_artists(&[
            ArtistUpsert { id: "r1".into(), name: "Bladee".into(), artwork_url: None },
            ArtistUpsert { id: "r2".into(), name: "Ecco2k".into(), artwork_url: None },
        ])
        .unwrap();
        assert_eq!(db.counts().unwrap().artists, 2);
        db.upsert_artists(&[ArtistUpsert {
            id: "r1".into(),
            name: "Bladee".into(),
            artwork_url: None,
        }])
        .unwrap();
        assert_eq!(db.counts().unwrap().artists, 2);
    }

    /// One test, not two: `CAPSULE_DB_PATH` is process-wide, and cargo runs
    /// tests in parallel - a separate override test races this one and makes
    /// both flaky.
    #[test]
    fn db_path_is_per_source_unless_overridden() {
        let data = Some(PathBuf::from(r"C:\appdata"));
        std::env::remove_var("CAPSULE_DB_PATH");

        let apple = default_db_path(data.clone(), Source::Apple).unwrap();
        let navi = default_db_path(data.clone(), Source::Navidrome).unwrap();
        assert_eq!(apple, PathBuf::from(r"C:\appdata\library-apple.sqlite3"));
        assert_eq!(navi, PathBuf::from(r"C:\appdata\library-navidrome.sqlite3"));
        assert_ne!(apple, navi);

        // The override is deliberately source-blind: it names one exact file.
        std::env::set_var("CAPSULE_DB_PATH", r"C:\tmp\custom.sqlite3");
        let overridden = default_db_path(data, Source::Apple).unwrap();
        std::env::remove_var("CAPSULE_DB_PATH");
        assert_eq!(overridden, PathBuf::from(r"C:\tmp\custom.sqlite3"));
    }

    #[test]
    fn legacy_db_is_renamed_to_apple_once() {
        let dir = std::env::temp_dir().join(format!("capsule-legacy-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("library.sqlite3"), b"legacy").unwrap();

        migrate_legacy_db(&dir).unwrap();

        assert!(!dir.join("library.sqlite3").exists());
        assert_eq!(std::fs::read(dir.join("library-apple.sqlite3")).unwrap(), b"legacy");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn migration_takes_the_wal_and_shm_with_it() {
        // SQLite derives sidecar names from the database filename. Renaming the
        // main file alone strands the write-ahead log, and in WAL mode that log
        // holds committed-but-uncheckpointed transactions - losing it loses data.
        let dir = std::env::temp_dir().join(format!("capsule-legacy-wal-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("library.sqlite3"), b"db").unwrap();
        std::fs::write(dir.join("library.sqlite3-wal"), b"wal").unwrap();
        std::fs::write(dir.join("library.sqlite3-shm"), b"shm").unwrap();

        migrate_legacy_db(&dir).unwrap();

        assert_eq!(std::fs::read(dir.join("library-apple.sqlite3-wal")).unwrap(), b"wal");
        assert_eq!(std::fs::read(dir.join("library-apple.sqlite3-shm")).unwrap(), b"shm");
        assert!(!dir.join("library.sqlite3-wal").exists(), "no orphaned wal left behind");
        assert!(!dir.join("library.sqlite3-shm").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn migration_without_sidecars_is_fine() {
        // A cleanly-closed database has no wal or shm; their absence must not
        // fail the migration.
        let dir = std::env::temp_dir().join(format!("capsule-legacy-nowal-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("library.sqlite3"), b"db").unwrap();

        migrate_legacy_db(&dir).unwrap();

        assert_eq!(std::fs::read(dir.join("library-apple.sqlite3")).unwrap(), b"db");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn migration_does_not_clobber_an_existing_apple_db() {
        let dir = std::env::temp_dir().join(format!("capsule-legacy2-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("library.sqlite3"), b"legacy").unwrap();
        std::fs::write(dir.join("library-apple.sqlite3"), b"current").unwrap();

        migrate_legacy_db(&dir).unwrap();

        assert_eq!(std::fs::read(dir.join("library-apple.sqlite3")).unwrap(), b"current");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
