//! Native playback: a rodio sink fed by the streaming source.
//!
//! rodio 0.21's constructors differ from most published examples:
//! `OutputStreamBuilder::open_default_stream` and `Sink::connect_new`, not
//! `OutputStream::try_default` / `Sink::try_new`.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use crate::stream::{self, CacheState, Shared, StreamingSource};

#[derive(Debug, thiserror::Error)]
pub enum AudioError {
    #[error("no audio output device available: {0}")]
    Device(String),
    #[error("this track's format could not be decoded: {0}")]
    Decode(String),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

/// rodio takes gain as 0.0..=1.0; the UI speaks percent.
fn to_gain(percent: u8) -> f32 {
    (percent.min(100) as f32) / 100.0
}

/// Clears a flag however the scope exits, including on an early return.
struct ClearOnDrop(Arc<AtomicBool>);

impl Drop for ClearOnDrop {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

pub struct Engine {
    // Held for its lifetime: dropping the stream silences the sink.
    _stream: rodio::OutputStream,
    // Arc so the decode-setup thread can hold it; rodio's Sink is not Clone.
    sink: Arc<rodio::Sink>,
    current: Mutex<Option<Shared>>,
    /// True between `load` being called and the decoder reaching the sink,
    /// so the position ticker doesn't mistake the momentarily-empty sink for
    /// end-of-track.
    loading: Arc<AtomicBool>,
    /// A seek that arrived while the track was still being set up.
    pending_seek: Arc<Mutex<Option<u64>>>,
    /// Bumped on every `load`. A decode thread checks it before touching the
    /// sink: without it, a superseded load could append its track after a
    /// newer one was already requested, or steal the newer track's seek.
    generation: Arc<AtomicU64>,
}

impl Engine {
    pub fn new() -> Result<Self, AudioError> {
        let stream = rodio::OutputStreamBuilder::open_default_stream()
            .map_err(|e| AudioError::Device(e.to_string()))?;
        let sink = Arc::new(rodio::Sink::connect_new(stream.mixer()));
        Ok(Self {
            _stream: stream,
            sink,
            current: Mutex::new(None),
            loading: Arc::new(AtomicBool::new(false)),
            pending_seek: Arc::new(Mutex::new(None)),
            generation: Arc::new(AtomicU64::new(0)),
        })
    }

    /// `Decoder::new` blocks until the first bytes arrive, so it runs on its
    /// own thread rather than the IPC command thread.
    ///
    /// A decode failure is reported through the shared state, so the position
    /// ticker turns it into `playback://error` on the one existing path.
    pub fn load(&self, url: String, cache: PathBuf) -> Result<(), AudioError> {
        self.sink.stop();
        // Kept so a seek outside the buffer can restart the transfer.
        let url_for_seek = url.clone();

        let shared: Shared = Arc::new((Mutex::new(CacheState::new()), Condvar::new()));
        // Cache file is created synchronously so the reader can open it
        // immediately; only the transfer is asynchronous.
        let cancel = stream::Cancel::default();
        stream::spawn_fetch(url, cache.clone(), shared.clone(), 0, cancel.clone())?;
        *self.current.lock().expect("current mutex") = Some(shared.clone());
        self.loading.store(true, Ordering::SeqCst);
        let mine = self.generation.fetch_add(1, Ordering::SeqCst) + 1;

        let sink = self.sink.clone();
        let loading = self.loading.clone();
        let pending = self.pending_seek.clone();
        let generation = self.generation.clone();
        let refetch_url = url_for_seek;
        let cancel = cancel.clone();
        std::thread::spawn(move || {
            let _guard = ClearOnDrop(loading);
            let current = || generation.load(Ordering::SeqCst) == mine;

            let source = match StreamingSource::new(cache, shared.clone(), refetch_url, cancel) {
                Ok(s) => s,
                Err(e) => {
                    shared.0.lock().expect("cache state mutex").fail(e.to_string());
                    shared.1.notify_all();
                    return;
                }
            };
            if !current() {
                return;
            }
            match rodio::Decoder::new(source) {
                Ok(d) => {
                    // A newer load may have started while this one was probing
                    // the stream; appending now would play the wrong track.
                    if !current() {
                        return;
                    }
                    sink.append(d);
                    // Replay a seek made while this was still loading.
                    if let Some(ms) = pending.lock().expect("pending seek mutex").take() {
                        if let Err(e) = sink.try_seek(Duration::from_millis(ms)) {
                            tracing::warn!(error = ?e, "deferred seek failed");
                        }
                    }
                }
                Err(e) => {
                    if !current() {
                        return;
                    }
                    shared
                        .0
                        .lock()
                        .expect("cache state mutex")
                        .fail(format!("could not decode this track: {e}"));
                    shared.1.notify_all();
                }
            }
        });
        Ok(())
    }

    pub fn is_loading(&self) -> bool {
        self.loading.load(Ordering::SeqCst)
    }

    pub fn play(&self) {
        self.sink.play();
    }

    pub fn pause(&self) {
        self.sink.pause();
    }

    /// A seek before the decoder exists cannot be applied, so it is held and
    /// replayed once the track is in the sink. Dropping it snaps the bar back
    /// on the next tick.
    pub fn seek(&self, ms: u64) {
        if self.is_loading() {
            *self.pending_seek.lock().expect("pending seek mutex") = Some(ms);
            return;
        }
        if let Err(e) = self.sink.try_seek(Duration::from_millis(ms)) {
            tracing::warn!(error = ?e, "seek not supported for this stream");
        }
    }

    pub fn set_volume(&self, percent: u8) {
        self.sink.set_volume(to_gain(percent));
    }

    pub fn stop(&self) {
        self.sink.stop();
        *self.current.lock().expect("current mutex") = None;
        *self.pending_seek.lock().expect("pending seek mutex") = None;
    }

    pub fn position_ms(&self) -> u64 {
        self.sink.get_pos().as_millis() as u64
    }

    pub fn is_empty(&self) -> bool {
        self.sink.empty()
    }

    pub fn is_paused(&self) -> bool {
        self.sink.is_paused()
    }

    /// The fetch or decode error for the current track, if it died.
    pub fn current_error(&self) -> Option<String> {
        let guard = self.current.lock().expect("current mutex");
        let shared = guard.as_ref()?;
        let state = shared.0.lock().expect("cache state mutex");
        state.error().map(|s| s.to_string())
    }
}

/// Edge-triggered on purpose: a level check would call next_track on every
/// tick while idle.
fn track_ended(was_playing: bool, sink_empty: bool) -> bool {
    was_playing && sink_empty
}



/// rodio has no non-blocking completion callback, so this polls. 250ms is
/// fine for lyrics because `offset_ms` calibration already corrects reported
/// position against real output.
pub fn start_ticker(app: tauri::AppHandle) {
    tauri::async_runtime::spawn(async move {
        use tauri::{Emitter, Manager};

        let mut was_playing = false;
        loop {
            tokio::time::sleep(Duration::from_millis(250)).await;

            let engine = {
                let state = app.state::<crate::AppState>();
                let guard = state.audio.lock().expect("audio mutex");
                guard.clone()
            };
            let Some(engine) = engine else {
                was_playing = false;
                continue;
            };

            if let Some(err) = engine.current_error() {
                tracing::warn!(%err, "playback stream failed");
                let _ = app.emit("playback://error", err);
                engine.stop();
                was_playing = false;
                // Stopping alone leaves status at Loading, showing a spinner over
                // a track that will never start; this marks it stopped instead of
                // auto-advancing, since that would tear through an entire
                // broken-server queue.
                let state = app.state::<crate::AppState>();
                state.player.lock().expect("player mutex").set_status(crate::player::Status::Idle);
                crate::commands::publish(&app, &state);
                continue;
            }

            if engine.is_loading() {
                was_playing = false;
                continue;
            }

            let empty = engine.is_empty();
            if !empty {
                let ms = engine.position_ms();
                let state = app.state::<crate::AppState>();
                {
                    let mut p = state.player.lock().expect("player mutex");
                    // Nothing else reports native playback state.
                    p.set_status(if engine.is_paused() {
                        crate::player::Status::Paused
                    } else {
                        crate::player::Status::Playing
                    });
                    p.on_position(ms);
                }
                // Transport, media flyout, and taskbar all redraw from
                // player://state.
                crate::commands::publish(&app, &state);
            }

            if track_ended(was_playing, empty) {
                // The same path the Next button uses, so repeat and
                // end-of-queue behave identically however the track ended.
                crate::commands::apply(&app, |p| p.next_track());

                // At the end of the queue, `next_track` leaves the sink empty with
                // an index still set; without this reset, Play would call
                // sink.play() on nothing forever.
                let ended = {
                    let state = app.state::<crate::AppState>();
                    let status = state.player.lock().expect("player mutex").state().status;
                    status == crate::player::Status::Ended
                };
                if ended {
                    engine.stop();
                }
            }
            was_playing = !empty;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn volume_maps_percent_to_unit_and_clamps() {
        assert_eq!(to_gain(0), 0.0);
        assert_eq!(to_gain(100), 1.0);
        assert_eq!(to_gain(50), 0.5);
        assert_eq!(to_gain(150), 1.0, "over 100 must clamp, not distort");
    }

    #[test]
    fn end_of_track_is_only_detected_on_the_playing_to_empty_edge() {
        assert!(track_ended(true, true), "was playing, sink now empty");
        assert!(!track_ended(false, true), "already idle; not an end");
        assert!(!track_ended(true, false), "still has audio queued");
        assert!(!track_ended(false, false));
    }

    /// Opening a real device needs hardware CI does not have.
    #[test]
    #[ignore]
    fn engine_opens_the_default_device() {
        let e = Engine::new().expect("default output device");
        assert!(e.is_empty());
        assert_eq!(e.position_ms(), 0);
        assert!(!e.is_loading());
    }
}
