//! User settings, in a TOML file the user can edit by hand.
//!
//! Distinct from [`crate::config`], which holds compile-time keys and debug
//! toggles. Anything a user should be able to change without a rebuild lives
//! here: which source they play from, where their music is, how their machine
//! is calibrated.
//!
//! Two rules, both inherited from `config`:
//!
//! * **Never panic.** A malformed file falls back to defaults with a warning.
//!   Losing settings is annoying; refusing to start is unusable.
//! * **Secrets do not live here.** Passwords, session keys and Apple tokens go
//!   to the credential store via [`crate::auth`]. This file sits in plaintext
//!   next to the library and should survive being pasted into a bug report.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub const FILE: &str = "config.toml";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    #[default]
    Apple,
    Spotify,
    Navidrome,
    Local,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Navidrome {
    pub url: String,
    pub username: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Local {
    pub folders: Vec<PathBuf>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct LyricsSettings {
    pub offset_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Lastfm {
    pub api_key: String,
    pub shared_secret: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Discord {
    pub client_id: String,
}

/// Window material. Kept as a string rather than an enum so an unrecognised
/// value degrades to "no glass" instead of invalidating the whole file — a
/// typo here should not cost the user their settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Appearance {
    pub glass: String,
}

impl Default for Appearance {
    fn default() -> Self {
        Self { glass: "none".into() }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub source: Source,
    pub onboarded: bool,
    pub appearance: Appearance,
    pub navidrome: Navidrome,
    pub local: Local,
    pub lyrics: LyricsSettings,
    pub lastfm: Lastfm,
    pub discord: Discord,
}

impl Settings {
    pub fn lastfm_enabled(&self) -> bool {
        !self.api_key().trim().is_empty() && !self.shared_secret().trim().is_empty()
    }

    pub fn discord_enabled(&self) -> bool {
        !self.discord_client_id().trim().is_empty()
    }

    pub fn api_key(&self) -> &str {
        pick(&self.lastfm.api_key, crate::config::Keys::LASTFM_API_KEY)
    }

    pub fn shared_secret(&self) -> &str {
        pick(&self.lastfm.shared_secret, crate::config::Keys::LASTFM_SHARED_SECRET)
    }

    pub fn discord_client_id(&self) -> &str {
        pick(&self.discord.client_id, crate::config::Keys::DISCORD_CLIENT_ID)
    }
}

fn pick<'a>(from_file: &'a str, baked_in: Option<&'a str>) -> &'a str {
    if !from_file.trim().is_empty() {
        return from_file;
    }
    baked_in.unwrap_or("")
}

pub fn path(dir: &Path) -> PathBuf {
    dir.join(FILE)
}

pub fn load(dir: &Path) -> Settings {
    let file = path(dir);
    let raw = match std::fs::read_to_string(&file) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Settings::default(),
        Err(e) => {
            tracing::warn!(error = %e, path = %file.display(), "could not read settings");
            return Settings::default();
        }
    };

    match toml::from_str(&raw) {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(error = %e, path = %file.display(), "settings are malformed; using defaults");
            Settings::default()
        }
    }
}

pub fn save(dir: &Path, settings: &Settings) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let body = toml::to_string_pretty(settings)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    std::fs::write(path(dir), body)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("capsule-settings-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn a_missing_file_yields_defaults() {
        let dir = tmp("missing");
        let s = load(&dir);
        assert_eq!(s.source, Source::Apple);
        assert!(!s.onboarded);
        assert_eq!(s.lyrics.offset_ms, 0);
    }

    #[test]
    fn round_trips_through_disk() {
        let dir = tmp("roundtrip");
        let s = Settings {
            source: Source::Navidrome,
            navidrome: Navidrome {
                url: "https://music.example.com".into(),
                username: "deniz".into(),
            },
            local: Local { folders: vec![PathBuf::from("D:/Music")] },
            lyrics: LyricsSettings { offset_ms: -180 },
            ..Settings::default()
        };
        save(&dir, &s).unwrap();

        assert_eq!(load(&dir), s);
    }

    #[test]
    fn a_partial_file_keeps_defaults_for_everything_else() {
        let dir = tmp("partial");
        std::fs::write(path(&dir), "source = \"local\"\n").unwrap();

        let s = load(&dir);
        assert_eq!(s.source, Source::Local);
        assert_eq!(s.lyrics.offset_ms, 0, "absent keys keep their default");
        assert!(s.navidrome.url.is_empty());
    }

    #[test]
    fn malformed_toml_falls_back_rather_than_panicking() {
        let dir = tmp("broken");
        std::fs::write(path(&dir), "source = [[[").unwrap();
        assert_eq!(load(&dir), Settings::default());

        assert_eq!(std::fs::read_to_string(path(&dir)).unwrap(), "source = [[[");
    }

    #[test]
    fn an_unknown_source_falls_back_rather_than_failing_the_whole_file() {
        let dir = tmp("unknown-source");
        std::fs::write(path(&dir), "source = \"tidal\"\n").unwrap();
        assert_eq!(load(&dir), Settings::default());
    }

    #[test]
    fn the_file_overrides_a_baked_in_key() {
        let s = Settings {
            lastfm: Lastfm { api_key: "from-file".into(), ..Lastfm::default() },
            ..Settings::default()
        };
        assert_eq!(s.api_key(), "from-file");
    }

    #[test]
    fn lastfm_needs_both_halves_of_a_registration() {
        let key_only = Settings {
            lastfm: Lastfm { api_key: "key".into(), ..Lastfm::default() },
            ..Settings::default()
        };
        assert!(!key_only.lastfm_enabled(), "an api key alone cannot sign calls");

        let both = Settings {
            lastfm: Lastfm { api_key: "key".into(), shared_secret: "secret".into() },
            ..Settings::default()
        };
        assert!(both.lastfm_enabled());
    }

    #[test]
    fn whitespace_is_not_a_key() {
        let s = Settings {
            discord: Discord { client_id: "   ".into() },
            ..Settings::default()
        };
        assert!(!s.discord_enabled());
    }
}
