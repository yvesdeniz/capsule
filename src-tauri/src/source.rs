//! Which backend the library comes from.
//!
//! An enum, not a trait object: sources are closed and known in advance, and
//! the MSRV (1.82) has no dyn-compatible async fn in traits.

use crate::db::{AlbumUpsert, ArtistUpsert, PlaylistUpsert, SongUpsert};
use crate::settings::{Settings, Source};
use crate::{api, auth, subsonic};

pub enum SourceClient {
    Apple(api::Client),
    Navidrome(subsonic::Client),
}

#[derive(Debug, thiserror::Error)]
pub enum ConnectError {
    #[error("not signed in")]
    NotSignedIn,
    #[error("no navidrome server configured")]
    NotConfigured,
    #[error("source not implemented yet: {0}")]
    Unsupported(&'static str),
    #[error("apple client: {0}")]
    Apple(#[from] api::ApiError),
    #[error("navidrome client: {0}")]
    Navidrome(#[from] subsonic::SubsonicError),
}

impl ConnectError {
    pub fn needs_auth(&self) -> bool {
        matches!(self, ConnectError::NotSignedIn)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SourceError {
    #[error(transparent)]
    Apple(#[from] api::ApiError),
    #[error(transparent)]
    Navidrome(#[from] subsonic::SubsonicError),
}

impl SourceError {
    pub fn needs_auth(&self) -> bool {
        matches!(
            self,
            SourceError::Apple(api::ApiError::Unauthorized)
                | SourceError::Navidrome(subsonic::SubsonicError::Unauthorized)
        )
    }
}

/// Settings with any environment override applied, plus the Navidrome
/// password.
///
/// The environment wins when fully specified, mirroring `CAPSULE_DB_PATH`,
/// otherwise the credential store. Sync and startup both go through here so
/// they cannot disagree about which server is being talked to.
pub fn resolve(settings: &Settings) -> (Settings, Option<String>) {
    if settings.source != Source::Navidrome {
        return (settings.clone(), None);
    }
    if let Some(env) = crate::config::navidrome_env() {
        let mut s = settings.clone();
        s.navidrome.url = env.url;
        s.navidrome.username = env.username;
        return (s, Some(env.password));
    }
    let password = auth::load_navidrome().ok().flatten().map(|c| c.password);
    (settings.clone(), password)
}

/// The Navidrome client for these settings, if the source is Navidrome and a
/// credential exists. Needed at startup because playback and artwork both
/// need a client without waiting for a sync to create one.
pub fn navidrome_client(settings: &Settings) -> Option<subsonic::Client> {
    let (resolved, password) = resolve(settings);
    match connect(&resolved, None, password) {
        Ok(SourceClient::Navidrome(c)) => Some(c),
        Ok(_) => None,
        Err(e) => {
            tracing::warn!(error = %e, "no navidrome client at startup");
            None
        }
    }
}

/// Credentials are passed in rather than read here so this stays
/// unit-testable without touching the credential store.
pub fn connect(
    settings: &Settings,
    tokens: Option<auth::Tokens>,
    navidrome_password: Option<String>,
) -> Result<SourceClient, ConnectError> {
    match settings.source {
        Source::Apple => {
            let tokens = tokens.filter(|t| t.is_complete()).ok_or(ConnectError::NotSignedIn)?;
            Ok(SourceClient::Apple(api::Client::new(tokens)?))
        }
        Source::Navidrome => {
            let cfg = &settings.navidrome;
            if cfg.url.trim().is_empty() || cfg.username.trim().is_empty() {
                return Err(ConnectError::NotConfigured);
            }
            let password =
                navidrome_password.filter(|p| !p.is_empty()).ok_or(ConnectError::NotSignedIn)?;
            Ok(SourceClient::Navidrome(subsonic::Client::new(subsonic::Credentials {
                base_url: cfg.url.clone(),
                username: cfg.username.clone(),
                password,
            })?))
        }
        Source::Local => Err(ConnectError::Unsupported("local")),
        Source::Spotify => Err(ConnectError::Unsupported("spotify")),
    }
}

impl SourceClient {
    pub async fn library_songs(
        &self,
        sink: impl FnMut(Vec<SongUpsert>),
    ) -> Result<(), SourceError> {
        match self {
            SourceClient::Apple(c) => Ok(c.library_songs(sink).await?),
            SourceClient::Navidrome(c) => {
                // Skipped albums are already logged per-album; sync only needs
                // to know the walk completed.
                let report = c.library_songs(sink).await?;
                if report.skipped_albums > 0 {
                    tracing::warn!(skipped = report.skipped_albums, "some albums were skipped");
                }
                Ok(())
            }
        }
    }

    pub async fn library_albums(
        &self,
        sink: impl FnMut(Vec<AlbumUpsert>),
    ) -> Result<(), SourceError> {
        match self {
            SourceClient::Apple(c) => Ok(c.library_albums(sink).await?),
            SourceClient::Navidrome(c) => Ok(c.library_albums(sink).await?),
        }
    }

    pub async fn library_playlists(
        &self,
        sink: impl FnMut(Vec<PlaylistUpsert>),
    ) -> Result<(), SourceError> {
        match self {
            SourceClient::Apple(c) => Ok(c.library_playlists(sink).await?),
            SourceClient::Navidrome(c) => Ok(c.library_playlists(sink).await?),
        }
    }

    pub async fn library_artists(
        &self,
        sink: impl FnMut(Vec<ArtistUpsert>),
    ) -> Result<(), SourceError> {
        match self {
            SourceClient::Apple(c) => Ok(c.library_artists(sink).await?),
            SourceClient::Navidrome(c) => Ok(c.library_artists(sink).await?),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::Navidrome;

    /// `SourceClient` holds reqwest clients and deliberately does not derive
    /// Debug, so tests unwrap by pattern rather than with `unwrap_err`.
    fn expect_err(r: Result<SourceClient, ConnectError>) -> ConnectError {
        match r {
            Err(e) => e,
            Ok(_) => panic!("expected connect to fail"),
        }
    }

    fn configured_navidrome() -> Settings {
        Settings {
            source: Source::Navidrome,
            navidrome: Navidrome {
                url: "https://m.example.com".into(),
                username: "deniz".into(),
            },
            ..Settings::default()
        }
    }

    #[test]
    fn navidrome_without_credentials_reports_needs_auth() {
        let err = expect_err(connect(&configured_navidrome(), None, None));
        assert!(err.needs_auth(), "missing password must prompt, not hard-fail");
    }

    #[test]
    fn navidrome_without_a_url_is_not_an_auth_problem() {
        let s = Settings { source: Source::Navidrome, ..Settings::default() };
        let err = expect_err(connect(&s, None, None));
        assert!(!err.needs_auth(), "an unconfigured server is not a credential problem");
        assert!(matches!(err, ConnectError::NotConfigured));
    }

    #[test]
    fn apple_without_tokens_reports_needs_auth() {
        let s = Settings::default();
        assert!(expect_err(connect(&s, None, None)).needs_auth());
    }

    #[test]
    fn navidrome_with_everything_builds_a_client() {
        match connect(&configured_navidrome(), None, Some("sesame".into())) {
            Ok(SourceClient::Navidrome(_)) => {}
            Ok(_) => panic!("expected a navidrome client"),
            Err(e) => panic!("expected success, got {e}"),
        }
    }

    #[test]
    fn unimplemented_sources_say_so() {
        let s = Settings { source: Source::Local, ..Settings::default() };
        assert!(matches!(connect(&s, None, None), Err(ConnectError::Unsupported("local"))));
    }
}
