use std::sync::Mutex;

use discord_rich_presence::activity::{Activity, Assets, Button, Timestamps};
use discord_rich_presence::{DiscordIpc, DiscordIpcClient};

use crate::player::{Status, Track};

const REPO_URL: &str = "https://github.com/yvesdeniz/capsule";

pub struct Presence {
    client: Mutex<Option<DiscordIpcClient>>,
    app_id: String,
}

impl Presence {
    pub fn new(app_id: String) -> Self {
        Self { client: Mutex::new(None), app_id }
    }

    fn ensure(&self, guard: &mut Option<DiscordIpcClient>) -> bool {
        if guard.is_some() {
            return true;
        }
        let mut client = DiscordIpcClient::new(&self.app_id);
        match client.connect() {
            Ok(()) => {
                tracing::info!("discord rich presence connected");
                *guard = Some(client);
                true
            }
            Err(e) => {
                tracing::debug!(error = %e, "discord not reachable");
                false
            }
        }
    }

    pub fn show(&self, track: &Track, status: Status, image: Option<&str>) {
        let mut guard = self.client.lock().expect("discord mutex");
        if !self.ensure(&mut guard) {
            return;
        }
        let Some(client) = guard.as_mut() else { return };

        let details = pad(&track.title);
        let state = pad(&track.artist);

        let mut assets = Assets::new();
        if let Some(url) = image {
            assets = assets.large_image(url);
        }
        let album = pad(&track.album);
        if !track.album.trim().is_empty() {
            assets = assets.large_text(&album);
        }

        let mut activity = Activity::new()
            .details(&details)
            .state(&state)
            .assets(assets)
            .buttons(vec![Button::new("Get capsule", REPO_URL)]);

        let started = start_epoch(status, track.duration_ms);
        if let Some(start) = started {
            activity = activity.timestamps(Timestamps::new().start(start));
        }

        if let Err(e) = client.set_activity(activity) {
            tracing::debug!(error = %e, "discord set_activity failed; dropping connection");
            *guard = None;
        }
    }

    pub fn clear(&self) {
        let mut guard = self.client.lock().expect("discord mutex");
        let Some(client) = guard.as_mut() else { return };
        if let Err(e) = client.clear_activity() {
            tracing::debug!(error = %e, "discord clear failed; dropping connection");
            *guard = None;
        }
    }
}

fn pad(s: &str) -> String {
    const ZWSP: char = '\u{200B}';
    if s.trim().is_empty() {
        "-".repeat(2)
    } else if s.chars().count() < 2 {
        format!("{s}{ZWSP}")
    } else {
        s.to_string()
    }
}

fn start_epoch(status: Status, _duration_ms: u64) -> Option<i64> {
    if status != Status::Playing {
        return None;
    }
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now().duration_since(UNIX_EPOCH).ok().map(|d| d.as_secs() as i64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn padding_survives_trimming() {
        let padded = pad("e");
        assert_eq!(padded.chars().count(), 2);
        assert_eq!(padded.trim().chars().count(), 2, "must not collapse back to one char");
    }

    #[test]
    fn a_one_letter_album_is_padded() {
        assert_eq!(pad("E").chars().count(), 2);
    }

    #[test]
    fn short_or_empty_values_are_padded_to_discords_minimum() {
        assert_eq!(pad("").chars().count(), 2);
        assert_eq!(pad("   ").chars().count(), 2);
        assert_eq!(pad("5").chars().count(), 2);
        assert_eq!(pad("1L"), "1L");
        assert_eq!(pad("Carhartt"), "Carhartt");
    }

    #[test]
    fn only_a_playing_track_gets_a_clock() {
        assert!(start_epoch(Status::Playing, 1000).is_some());
        assert!(start_epoch(Status::Paused, 1000).is_none());
        assert!(start_epoch(Status::Idle, 1000).is_none());
    }
}
