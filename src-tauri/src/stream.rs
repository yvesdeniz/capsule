//! Streaming byte source: plays while downloading, caching to disk.
//!
//! Splitting the bookkeeping ([`CacheState`]) from the IO keeps the part worth
//! testing free of HTTP and of a sound device.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};

/// How much of a track has landed on disk, and whether the fetch died.
///
/// `available` is always a contiguous prefix from byte zero — sparse-range
/// tracking is the nastiest part of a streaming cache, so a seek past the
/// prefix abandons caching instead (see [`StreamingSource::seek`]).
#[derive(Debug, Default)]
pub struct CacheState {
    available: u64,
    total: Option<u64>,
    error: Option<String>,
}

impl CacheState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn available(&self) -> u64 {
        self.available
    }

    pub fn total(&self) -> Option<u64> {
        self.total
    }

    /// True only once the whole length is known and has arrived — a response
    /// without Content-Length is never "complete", so readers keep waiting
    /// rather than truncating the track.
    pub fn is_complete(&self) -> bool {
        matches!(self.total, Some(t) if self.available >= t)
    }

    pub fn note_written(&mut self, n: u64) {
        self.available += n;
    }

    pub fn set_total(&mut self, total: Option<u64>) {
        self.total = total;
    }

    /// Sticky: the first failure is the useful one, and a partial write
    /// arriving afterwards must not mask it.
    pub fn fail(&mut self, reason: String) {
        if self.error.is_none() {
            self.error = Some(reason);
        }
    }

    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }
}

pub type Shared = Arc<(Mutex<CacheState>, Condvar)>;

#[derive(Debug, thiserror::Error)]
pub enum StreamError {
    #[error("{0}")]
    Fetch(String),
    #[error("timed out waiting for audio data")]
    Timeout,
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

/// Returns a short count at the end of a complete stream — that's EOF, not an
/// error. The timeout exists so a stalled server cannot wedge the decoder
/// thread forever; the caller turns that into a playback error.
pub fn wait_for(shared: &Shared, want: u64, timeout: Duration) -> Result<u64, StreamError> {
    let (lock, cvar) = &**shared;
    let deadline = Instant::now() + timeout;
    let mut guard = lock.lock().expect("cache state mutex");

    loop {
        if let Some(e) = guard.error() {
            return Err(StreamError::Fetch(e.to_string()));
        }
        if guard.available() >= want || guard.is_complete() {
            return Ok(guard.available());
        }
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(StreamError::Timeout);
        }
        let (g, res) = cvar.wait_timeout(guard, left).expect("cache state mutex");
        guard = g;
        if res.timed_out() && guard.available() < want && !guard.is_complete() {
            if guard.error().is_some() {
                continue;
            }
            return Err(StreamError::Timeout);
        }
    }
}

/// How long a read waits for bytes before giving up and reporting a stall.
const READ_TIMEOUT: Duration = Duration::from_secs(20);

/// Audio cache ceiling: roughly a thousand transcoded tracks or a few hundred
/// FLACs, large enough that normal listening never hits it. A constant, not
/// a setting.
pub const CACHE_CAP_BYTES: u64 = 4 * 1024 * 1024 * 1024;

pub fn cache_dir(app_data: Option<PathBuf>) -> Option<PathBuf> {
    Some(app_data?.join("audio"))
}

pub fn cache_path(dir: &Path, key: &str) -> PathBuf {
    let mut h = Sha256::new();
    h.update(key.as_bytes());
    dir.join(format!("{:x}.dat", h.finalize()))
}

/// `keep` is the file currently being played; evicting it would pull the file
/// out from under the decoder mid-track.
pub fn prune(dir: &Path, cap_bytes: u64, keep: Option<&Path>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let mut files: Vec<(PathBuf, u64, std::time::SystemTime)> = entries
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let meta = e.metadata().ok()?;
            if !meta.is_file() {
                return None;
            }
            Some((e.path(), meta.len(), meta.modified().ok()?))
        })
        .collect();

    let mut total: u64 = files.iter().map(|(_, n, _)| n).sum();
    if total <= cap_bytes {
        return;
    }

    files.sort_by_key(|(_, _, t)| *t);
    for (path, size, _) in files {
        if total <= cap_bytes {
            break;
        }
        if keep.is_some_and(|k| k == path) {
            continue;
        }
        if std::fs::remove_file(&path).is_ok() {
            total = total.saturating_sub(size);
            tracing::debug!(path = %path.display(), "pruned audio cache file");
        }
    }
}

/// `from` is a byte offset for a ranged refetch after a seek past the cached
/// prefix; pass 0 for a fresh play.
///
/// The cache file is created synchronously, before the fetch is spawned, so
/// the caller can open a reader immediately — this runs on the IPC command
/// thread and must not block waiting on the response.
pub fn spawn_fetch(url: String, path: PathBuf, shared: Shared, from: u64) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    // The fetch reopens for writing rather than truncating, which on Windows
    // would collide with the reader's handle.
    File::create(&path)?;

    tauri::async_runtime::spawn(async move {
        use std::io::Write;

        let finish_err = |shared: &Shared, msg: String| {
            shared.0.lock().expect("cache state mutex").fail(msg);
            shared.1.notify_all();
        };

        let client = reqwest::Client::new();
        let mut req = client.get(&url);
        if from > 0 {
            req = req.header(reqwest::header::RANGE, format!("bytes={from}-"));
        }

        let resp = match req.send().await {
            Ok(r) => r,
            Err(_) => return finish_err(&shared, "server unreachable".into()),
        };
        if resp.status() == reqwest::StatusCode::UNAUTHORIZED
            || resp.status() == reqwest::StatusCode::FORBIDDEN
        {
            return finish_err(&shared, "server rejected the login".into());
        }
        if !resp.status().is_success() {
            return finish_err(&shared, format!("server returned {}", resp.status()));
        }

        {
            let mut g = shared.0.lock().expect("cache state mutex");
            g.set_total(resp.content_length());
        }
        shared.1.notify_all();

        let mut file = match std::fs::OpenOptions::new().write(true).open(&path) {
            Ok(f) => f,
            Err(e) => return finish_err(&shared, format!("cache write: {e}")),
        };

        let mut resp = resp;
        loop {
            match resp.chunk().await {
                Ok(Some(bytes)) => {
                    if let Err(e) = file.write_all(&bytes) {
                        return finish_err(&shared, format!("cache write: {e}"));
                    }
                    {
                        let mut g = shared.0.lock().expect("cache state mutex");
                        g.note_written(bytes.len() as u64);
                    }
                    shared.1.notify_all();
                }
                Ok(None) => break,
                Err(_) => return finish_err(&shared, "connection lost".into()),
            }
        }

        // A chunked response has no Content-Length; settling it here is what
        // lets the reader recognise EOF instead of waiting out its timeout.
        {
            let mut g = shared.0.lock().expect("cache state mutex");
            if g.total().is_none() {
                let landed = g.available();
                g.set_total(Some(landed));
            }
        }
        shared.1.notify_all();
    });
    Ok(())
}

/// A `Read + Seek` view over a file that is still being written, handed to
/// `rodio::Decoder`, which needs both. Reads block until the fetch catches
/// up.
pub struct StreamingSource {
    file: File,
    shared: Shared,
    pos: u64,
    abandoned: bool,
}

impl StreamingSource {
    pub fn new(path: PathBuf, shared: Shared) -> std::io::Result<Self> {
        Ok(Self { file: File::open(path)?, shared, pos: 0, abandoned: false })
    }

    /// True once a seek has taken us past the cached prefix; the partial
    /// file is no longer a faithful copy and must be discarded.
    pub fn caching_abandoned(&self) -> bool {
        self.abandoned
    }
}

impl Read for StreamingSource {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let want = self.pos + buf.len() as u64;
        let available = wait_for(&self.shared, want, READ_TIMEOUT)
            .map_err(|e| std::io::Error::other(e.to_string()))?;

        if available <= self.pos {
            return Ok(0); // EOF
        }
        let can = (available - self.pos).min(buf.len() as u64) as usize;
        self.file.seek(SeekFrom::Start(self.pos))?;
        let n = self.file.read(&mut buf[..can])?;
        self.pos += n as u64;
        Ok(n)
    }
}

impl Seek for StreamingSource {
    fn seek(&mut self, from: SeekFrom) -> std::io::Result<u64> {
        let total = self.shared.0.lock().expect("cache state mutex").total();
        let target = match from {
            SeekFrom::Start(n) => n,
            SeekFrom::Current(d) => self.pos.saturating_add_signed(d),
            SeekFrom::End(d) => {
                let total = total.ok_or_else(|| {
                    std::io::Error::new(
                        std::io::ErrorKind::Unsupported,
                        "cannot seek from end without a known length",
                    )
                })?;
                total.saturating_add_signed(d)
            }
        };

        let available = self.shared.0.lock().expect("cache state mutex").available();
        if target > available {
            // Past the prefix. Caching this play is over; the caller refetches
            // from `target` and deletes the partial file.
            self.abandoned = true;
        }
        self.pos = target;
        Ok(target)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn temp_file(name: &str, bytes: &[u8]) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("capsule-stream-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join(name);
        let mut f = File::create(&p).unwrap();
        f.write_all(bytes).unwrap();
        p
    }

    fn shared() -> Shared {
        Arc::new((Mutex::new(CacheState::new()), Condvar::new()))
    }

    #[test]
    fn starts_empty_and_incomplete() {
        let s = CacheState::new();
        assert_eq!(s.available(), 0);
        assert_eq!(s.total(), None);
        assert!(!s.is_complete());
        assert_eq!(s.error(), None);
    }

    #[test]
    fn written_bytes_accumulate() {
        let mut s = CacheState::new();
        s.note_written(1024);
        s.note_written(512);
        assert_eq!(s.available(), 1536);
    }

    #[test]
    fn complete_only_when_available_reaches_total() {
        let mut s = CacheState::new();
        s.set_total(Some(1000));
        s.note_written(999);
        assert!(!s.is_complete(), "one byte short is not complete");
        s.note_written(1);
        assert!(s.is_complete());
    }

    #[test]
    fn unknown_total_is_never_complete() {
        let mut s = CacheState::new();
        s.note_written(10_000);
        assert!(!s.is_complete());
    }

    #[test]
    fn failure_is_recorded_and_sticky() {
        let mut s = CacheState::new();
        s.fail("server unreachable".into());
        assert_eq!(s.error(), Some("server unreachable"));
        s.note_written(10);
        assert_eq!(s.error(), Some("server unreachable"), "a late write must not clear the error");
    }

    #[test]
    fn returns_immediately_when_bytes_are_already_there() {
        let s = shared();
        s.0.lock().unwrap().note_written(500);
        let got = wait_for(&s, 100, Duration::from_millis(50)).unwrap();
        assert_eq!(got, 500);
    }

    #[test]
    fn wakes_when_bytes_arrive() {
        let s = shared();
        let writer = s.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(30));
            writer.0.lock().unwrap().note_written(2048);
            writer.1.notify_all();
        });
        let got = wait_for(&s, 1024, Duration::from_secs(2)).unwrap();
        assert_eq!(got, 2048, "must observe the bytes the writer added");
    }

    #[test]
    fn returns_short_at_end_of_a_complete_stream() {
        let s = shared();
        {
            let mut g = s.0.lock().unwrap();
            g.set_total(Some(100));
            g.note_written(100);
        }
        let got = wait_for(&s, 500, Duration::from_millis(50)).unwrap();
        assert_eq!(got, 100);
    }

    #[test]
    fn surfaces_a_fetch_failure_rather_than_hanging() {
        let s = shared();
        let writer = s.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(20));
            writer.0.lock().unwrap().fail("server unreachable".into());
            writer.1.notify_all();
        });
        match wait_for(&s, 1024, Duration::from_secs(2)) {
            Err(StreamError::Fetch(m)) => assert_eq!(m, "server unreachable"),
            other => panic!("expected a fetch error, got {other:?}"),
        }
    }

    #[test]
    fn times_out_instead_of_blocking_forever() {
        let s = shared();
        assert!(matches!(
            wait_for(&s, 1024, Duration::from_millis(60)),
            Err(StreamError::Timeout)
        ));
    }

    fn complete(s: &Shared, n: u64) {
        let mut g = s.0.lock().unwrap();
        g.set_total(Some(n));
        g.note_written(n);
    }

    #[test]
    fn reads_bytes_that_have_landed() {
        let p = temp_file("read.dat", b"0123456789");
        let s = shared();
        complete(&s, 10);
        let mut src = StreamingSource::new(p, s).unwrap();
        let mut buf = [0u8; 4];
        src.read_exact(&mut buf).unwrap();
        assert_eq!(&buf, b"0123");
    }

    #[test]
    fn seek_inside_the_prefix_does_not_abandon_caching() {
        let p = temp_file("seek-in.dat", b"0123456789");
        let s = shared();
        complete(&s, 10);
        let mut src = StreamingSource::new(p, s).unwrap();
        assert_eq!(src.seek(SeekFrom::Start(6)).unwrap(), 6);
        let mut buf = [0u8; 2];
        src.read_exact(&mut buf).unwrap();
        assert_eq!(&buf, b"67");
        assert!(!src.caching_abandoned(), "a seek within cached bytes is free");
    }

    #[test]
    fn seek_beyond_the_prefix_abandons_caching() {
        let p = temp_file("seek-out.dat", b"01234");
        let s = shared();
        {
            let mut g = s.0.lock().unwrap();
            g.set_total(Some(1_000_000));
            g.note_written(5);
        }
        let mut src = StreamingSource::new(p, s).unwrap();
        src.seek(SeekFrom::Start(900_000)).unwrap();
        assert!(src.caching_abandoned());
    }

    #[test]
    fn seek_from_end_uses_the_known_total() {
        // symphonia probes with SeekFrom::End; without a total this must fail
        // rather than silently reporting zero.
        let p = temp_file("seek-end.dat", b"0123456789");
        let s = shared();
        complete(&s, 10);
        let mut src = StreamingSource::new(p, s).unwrap();
        assert_eq!(src.seek(SeekFrom::End(-2)).unwrap(), 8);
    }

    #[test]
    fn seek_from_end_without_a_total_is_an_error_not_a_guess() {
        let p = temp_file("seek-end-none.dat", b"0123456789");
        let s = shared();
        s.0.lock().unwrap().note_written(10);
        let mut src = StreamingSource::new(p, s).unwrap();
        assert!(src.seek(SeekFrom::End(-2)).is_err());
    }

    #[test]
    fn cache_path_is_stable_and_distinct_per_track() {
        let dir = Path::new(r"C:\cache");
        assert_eq!(cache_path(dir, "tr-1"), cache_path(dir, "tr-1"));
        assert_ne!(cache_path(dir, "tr-1"), cache_path(dir, "tr-2"));
    }

    #[test]
    fn prune_evicts_oldest_first_and_spares_the_playing_file() {
        let dir = std::env::temp_dir().join(format!("capsule-prune-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let old = dir.join("old.dat");
        let mid = dir.join("mid.dat");
        let playing = dir.join("playing.dat");
        for p in [&old, &mid, &playing] {
            std::fs::write(p, vec![0u8; 400]).unwrap();
            // Space the mtimes so ordering is unambiguous.
            std::thread::sleep(Duration::from_millis(20));
        }

        prune(&dir, 500, Some(&playing));

        assert!(playing.exists(), "the in-use file must never be evicted");
        assert!(!old.exists(), "oldest goes first");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn prune_does_nothing_below_the_cap() {
        let dir = std::env::temp_dir().join(format!("capsule-prune2-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("a.dat");
        std::fs::write(&f, vec![0u8; 100]).unwrap();
        prune(&dir, 10_000, None);
        assert!(f.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_fetch_failure_surfaces_as_an_io_error() {
        let p = temp_file("fail.dat", b"");
        let s = shared();
        s.0.lock().unwrap().fail("server unreachable".into());
        let mut src = StreamingSource::new(p, s).unwrap();
        let mut buf = [0u8; 4];
        assert!(src.read(&mut buf).is_err());
    }
}
