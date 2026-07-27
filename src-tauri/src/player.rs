//! The single source of truth for playback.
//!
//! Deliberately free of Tauri, HTTP, and MusicKit types: this module is pure
//! data in, data out, which is what makes it testable. Everything with a
//! decision in it lives here; `engine-hook.js` stays dumb marshalling.
//!
//! Intents come in from the UI, engine events come in from MusicKit, and the
//! machine emits [`EngineCommand`]s describing what the playback backend should
//! be told to do. It never talks to a backend itself.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Track {
    pub id: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    #[serde(default)]
    pub duration_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Idle,
    Loading,
    Playing,
    Paused,
    Stalled,
    Ended,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Repeat {
    #[default]
    Off,
    All,
    One,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EngineCommand {
    SetQueue { ids: Vec<String>, start_index: usize },
    Play,
    Pause,
    Seek { ms: u64 },
    SetVolume { percent: u8 },
    SkipNext,
    SkipPrevious,
    SetShuffle { on: bool },
    SetRepeat { mode: u8 },
    Prewarm { id: String },
}

impl Repeat {
    fn musickit_mode(self) -> u8 {
        match self {
            Repeat::Off => 0,
            Repeat::One => 1,
            Repeat::All => 2,
        }
    }

    fn cycle(self) -> Self {
        match self {
            Repeat::Off => Repeat::All,
            Repeat::All => Repeat::One,
            Repeat::One => Repeat::Off,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlayerState {
    pub status: Status,
    pub queue: Vec<Track>,
    pub index: Option<usize>,
    pub position_ms: u64,
    pub volume: u8,
    pub shuffle: bool,
    pub repeat: Repeat,
}

impl Default for PlayerState {
    fn default() -> Self {
        Self {
            status: Status::Idle,
            queue: Vec::new(),
            index: None,
            position_ms: 0,
            volume: 100,
            shuffle: false,
            repeat: Repeat::Off,
        }
    }
}

impl PlayerState {
    pub fn current(&self) -> Option<&Track> {
        self.queue.get(self.index?)
    }

    pub fn is_active(&self) -> bool {
        matches!(self.status, Status::Playing | Status::Loading | Status::Stalled)
    }
}

pub fn map_musickit_state(raw: i64) -> Status {
    match raw {
        0 => Status::Idle,
        1 | 6 => Status::Loading, // loading, seeking
        2 => Status::Playing,
        3 => Status::Paused,
        4 => Status::Idle, // stopped
        5 | 10 => Status::Ended,
        8 | 9 => Status::Stalled, // waiting, stalled
        _ => Status::Loading,
    }
}

pub struct Player {
    state: PlayerState,
}

impl Default for Player {
    fn default() -> Self {
        Self::new()
    }
}

impl Player {
    pub fn new() -> Self {
        Self { state: PlayerState::default() }
    }

    pub fn state(&self) -> &PlayerState {
        &self.state
    }

    pub fn play_queue(&mut self, tracks: Vec<Track>, start_index: usize) -> Vec<EngineCommand> {
        if tracks.is_empty() {
            return Vec::new();
        }
        let start = start_index.min(tracks.len() - 1);
        let ids = tracks.iter().map(|t| t.id.clone()).collect();
        self.state.queue = tracks;
        self.state.index = Some(start);
        self.state.position_ms = 0;
        self.state.status = Status::Loading;
        vec![EngineCommand::SetQueue { ids, start_index: start }, EngineCommand::Play]
    }

    pub fn play(&mut self) -> Vec<EngineCommand> {
        if self.state.index.is_none() {
            return Vec::new();
        }
        vec![EngineCommand::Play]
    }

    pub fn pause(&mut self) -> Vec<EngineCommand> {
        if !self.state.is_active() {
            return Vec::new();
        }
        vec![EngineCommand::Pause]
    }

    pub fn toggle(&mut self) -> Vec<EngineCommand> {
        match self.state.status {
            Status::Playing | Status::Loading | Status::Stalled => self.pause(),
            Status::Paused | Status::Ended => self.play(),
            Status::Idle => self.play(),
        }
    }

    pub fn next_track(&mut self) -> Vec<EngineCommand> {
        let Some(i) = self.state.index else { return Vec::new() };
        let len = self.state.queue.len();
        if len == 0 {
            return Vec::new();
        }

        if i + 1 < len {
            self.state.index = Some(i + 1);
            self.state.position_ms = 0;
            self.state.status = Status::Loading;
            return vec![EngineCommand::SkipNext];
        }

        if self.state.repeat != Repeat::Off {
            return self.jump_to(0);
        }
        self.state.status = Status::Ended;
        self.state.position_ms = 0;
        Vec::new()
    }

    pub fn previous_track(&mut self) -> Vec<EngineCommand> {
        const RESTART_THRESHOLD_MS: u64 = 3_000;

        let Some(i) = self.state.index else { return Vec::new() };
        if self.state.queue.is_empty() {
            return Vec::new();
        }

        if self.state.position_ms > RESTART_THRESHOLD_MS {
            self.state.position_ms = 0;
            return vec![EngineCommand::Seek { ms: 0 }];
        }

        if i > 0 {
            self.state.index = Some(i - 1);
            self.state.position_ms = 0;
            self.state.status = Status::Loading;
            return vec![EngineCommand::SkipPrevious];
        }

        if self.state.repeat != Repeat::Off {
            return self.jump_to(self.state.queue.len() - 1);
        }
        self.state.position_ms = 0;
        vec![EngineCommand::Seek { ms: 0 }]
    }

    fn jump_to(&mut self, index: usize) -> Vec<EngineCommand> {
        self.state.index = Some(index);
        self.state.position_ms = 0;
        self.state.status = Status::Loading;
        let ids = self.state.queue.iter().map(|t| t.id.clone()).collect();
        vec![EngineCommand::SetQueue { ids, start_index: index }, EngineCommand::Play]
    }

    pub fn seek(&mut self, ms: u64) -> Vec<EngineCommand> {
        let Some(t) = self.state.current() else { return Vec::new() };
        let ms = if t.duration_ms > 0 { ms.min(t.duration_ms) } else { ms };
        self.state.position_ms = ms;
        vec![EngineCommand::Seek { ms }]
    }

    pub fn set_volume(&mut self, percent: u8) -> Vec<EngineCommand> {
        let v = percent.min(100);
        self.state.volume = v;
        vec![EngineCommand::SetVolume { percent: v }]
    }

    pub fn toggle_shuffle(&mut self) -> Vec<EngineCommand> {
        self.state.shuffle = !self.state.shuffle;
        vec![EngineCommand::SetShuffle { on: self.state.shuffle }]
    }

    pub fn cycle_repeat(&mut self) -> Vec<EngineCommand> {
        self.state.repeat = self.state.repeat.cycle();
        vec![EngineCommand::SetRepeat { mode: self.state.repeat.musickit_mode() }]
    }

    #[cfg(test)]
    pub fn set_repeat(&mut self, r: Repeat) {
        self.state.repeat = r;
    }

    pub fn on_playback_state(&mut self, raw: i64) -> Vec<EngineCommand> {
        const ITEM_ENDED: i64 = 5;
        const QUEUE_COMPLETED: i64 = 10;

        self.state.status = map_musickit_state(raw);

        match raw {
            ITEM_ENDED => {
                if self.state.repeat == Repeat::One {
                    self.state.position_ms = 0;
                    return vec![EngineCommand::Seek { ms: 0 }, EngineCommand::Play];
                }
                Vec::new()
            }
            QUEUE_COMPLETED => {
                if self.state.repeat != Repeat::Off {
                    return self.jump_to(0);
                }
                Vec::new()
            }
            _ => Vec::new(),
        }
    }

    pub fn on_position(&mut self, ms: u64) {
        self.state.position_ms = ms;
    }

    pub fn on_now_playing(&mut self, id: &str, duration_ms: u64) {
        if let Some(pos) = self.state.queue.iter().position(|t| t.id == id) {
            self.state.index = Some(pos);
        }
        if duration_ms > 0 {
            if let Some(i) = self.state.index {
                if let Some(t) = self.state.queue.get_mut(i) {
                    t.duration_ms = duration_ms;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(id: &str) -> Track {
        Track {
            id: id.into(),
            title: format!("t{id}"),
            artist: "a".into(),
            album: "b".into(),
            duration_ms: 200_000,
        }
    }

    fn loaded() -> Player {
        let mut p = Player::new();
        p.play_queue(vec![track("1"), track("2"), track("3")], 0);
        p
    }

    #[test]
    fn musickit_state_1_is_loading_not_playing() {
        assert_eq!(map_musickit_state(1), Status::Loading);
        assert_eq!(map_musickit_state(2), Status::Playing);
        assert_eq!(map_musickit_state(3), Status::Paused);
        assert_eq!(map_musickit_state(8), Status::Stalled);
        assert_eq!(map_musickit_state(10), Status::Ended);
    }

    #[test]
    fn play_queue_sets_queue_and_emits_commands() {
        let mut p = Player::new();
        let cmds = p.play_queue(vec![track("1"), track("2")], 1);
        assert_eq!(p.state().index, Some(1));
        assert_eq!(p.state().status, Status::Loading);
        assert_eq!(
            cmds,
            vec![
                EngineCommand::SetQueue { ids: vec!["1".into(), "2".into()], start_index: 1 },
                EngineCommand::Play
            ]
        );
    }

    #[test]
    fn play_queue_clamps_out_of_range_start() {
        let mut p = Player::new();
        p.play_queue(vec![track("1"), track("2")], 99);
        assert_eq!(p.state().index, Some(1));
    }

    #[test]
    fn empty_queue_is_a_noop_not_a_panic() {
        let mut p = Player::new();
        assert!(p.play_queue(vec![], 0).is_empty());
        assert!(p.next_track().is_empty());
        assert!(p.previous_track().is_empty());
        assert!(p.seek(1000).is_empty());
        assert_eq!(p.state().status, Status::Idle);
    }

    #[test]
    fn next_advances_and_stops_at_end_when_repeat_off() {
        let mut p = loaded();
        assert_eq!(p.next_track(), vec![EngineCommand::SkipNext]);
        assert_eq!(p.state().index, Some(1));
        assert_eq!(p.next_track(), vec![EngineCommand::SkipNext]);
        assert_eq!(p.state().index, Some(2));

        let cmds = p.next_track();
        assert!(cmds.is_empty(), "should not wrap with repeat off");
        assert_eq!(p.state().index, Some(2));
        assert_eq!(p.state().status, Status::Ended);
    }

    #[test]
    fn next_wraps_when_repeat_all() {
        let mut p = loaded();
        p.set_repeat(Repeat::All);
        p.next_track();
        p.next_track();
        let cmds = p.next_track();
        assert_eq!(p.state().index, Some(0));
        assert!(matches!(cmds.first(), Some(EngineCommand::SetQueue { .. })));
    }

    #[test]
    fn next_still_advances_under_repeat_one() {
        let mut p = loaded();
        p.set_repeat(Repeat::One);
        p.next_track();
        assert_eq!(p.state().index, Some(1));
    }

    #[test]
    fn previous_restarts_track_when_past_threshold() {
        let mut p = loaded();
        p.next_track();
        p.on_position(9_000);
        let cmds = p.previous_track();
        assert_eq!(cmds, vec![EngineCommand::Seek { ms: 0 }]);
        assert_eq!(p.state().index, Some(1), "should not change track");
        assert_eq!(p.state().position_ms, 0);
    }

    #[test]
    fn previous_goes_back_when_early_in_track() {
        let mut p = loaded();
        p.next_track();
        p.on_position(500);
        assert_eq!(p.previous_track(), vec![EngineCommand::SkipPrevious]);
        assert_eq!(p.state().index, Some(0));
    }

    #[test]
    fn previous_at_first_track_restarts_rather_than_underflowing() {
        let mut p = loaded();
        p.on_position(100);
        let cmds = p.previous_track();
        assert_eq!(p.state().index, Some(0));
        assert_eq!(cmds, vec![EngineCommand::Seek { ms: 0 }]);
    }

    #[test]
    fn previous_wraps_to_last_when_repeat_on() {
        let mut p = loaded();
        p.set_repeat(Repeat::All);
        p.on_position(100);
        p.previous_track();
        assert_eq!(p.state().index, Some(2));
    }

    #[test]
    fn seek_clamps_to_duration() {
        let mut p = loaded();
        let cmds = p.seek(999_999);
        assert_eq!(cmds, vec![EngineCommand::Seek { ms: 200_000 }]);
        assert_eq!(p.state().position_ms, 200_000);
    }

    #[test]
    fn volume_clamps_to_100() {
        let mut p = Player::new();
        let cmds = p.set_volume(250);
        assert_eq!(cmds, vec![EngineCommand::SetVolume { percent: 100 }]);
        assert_eq!(p.state().volume, 100);
    }

    #[test]
    fn toggle_pauses_while_playing_and_resumes_while_paused() {
        let mut p = loaded();
        p.on_playback_state(2); // playing
        assert_eq!(p.toggle(), vec![EngineCommand::Pause]);
        p.on_playback_state(3); // paused
        assert_eq!(p.toggle(), vec![EngineCommand::Play]);
    }

    #[test]
    fn toggle_with_nothing_queued_does_nothing() {
        let mut p = Player::new();
        assert!(p.toggle().is_empty());
    }

    #[test]
    fn item_ended_does_not_skip_because_musickit_auto_advances() {
        let mut p = loaded();
        let cmds = p.on_playback_state(5); // item ended
        assert!(cmds.is_empty(), "must not command a skip on natural item-end");
        assert_eq!(p.state().index, Some(0), "index follows nowPlaying, not this");
    }

    #[test]
    fn item_ended_under_repeat_one_replays_same_track() {
        let mut p = loaded();
        p.set_repeat(Repeat::One);
        let cmds = p.on_playback_state(5); // item ended
        assert_eq!(p.state().index, Some(0));
        assert_eq!(cmds, vec![EngineCommand::Seek { ms: 0 }, EngineCommand::Play]);
    }

    #[test]
    fn queue_completed_stops_when_repeat_off() {
        let mut p = loaded();
        let cmds = p.on_playback_state(10); // whole queue done
        assert!(cmds.is_empty());
    }

    #[test]
    fn queue_completed_wraps_when_repeat_all() {
        let mut p = loaded();
        p.set_repeat(Repeat::All);
        let cmds = p.on_playback_state(10);
        assert_eq!(p.state().index, Some(0));
        assert!(matches!(cmds.first(), Some(EngineCommand::SetQueue { .. })));
    }

    #[test]
    fn now_playing_resyncs_index_when_engine_disagrees() {
        let mut p = loaded();
        p.on_now_playing("3", 123_000);
        assert_eq!(p.state().index, Some(2));
        assert_eq!(p.state().current().unwrap().duration_ms, 123_000);
    }

    #[test]
    fn now_playing_for_unknown_id_leaves_index_alone() {
        let mut p = loaded();
        p.on_now_playing("does-not-exist", 1000);
        assert_eq!(p.state().index, Some(0));
    }
}
