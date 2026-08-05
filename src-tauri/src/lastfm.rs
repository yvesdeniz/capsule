
use serde::Deserialize;

const BASE: &str = "https://ws.audioscrobbler.com/2.0/";

#[derive(Debug, Deserialize)]
struct AlbumResponse {
    album: Option<AlbumInfo>,
}

#[derive(Debug, Deserialize)]
struct AlbumInfo {
    #[serde(default)]
    image: Vec<Image>,
}

#[derive(Debug, Deserialize)]
struct Image {
    #[serde(rename = "#text", default)]
    url: String,
    #[serde(default)]
    size: String,
}

fn best_image(images: Vec<Image>) -> Option<String> {
    for want in ["extralarge", "large", "medium"] {
        if let Some(i) = images.iter().find(|i| i.size == want && !i.url.trim().is_empty()) {
            return Some(i.url.clone());
        }
    }
    images.into_iter().map(|i| i.url).find(|u| !u.trim().is_empty())
}

pub async fn album_art(
    http: &reqwest::Client,
    api_key: &str,
    artist: &str,
    album: &str,
) -> Option<String> {
    if api_key.trim().is_empty() || artist.trim().is_empty() || album.trim().is_empty() {
        return None;
    }
    let resp = http
        .get(BASE)
        .query(&[
            ("method", "album.getinfo"),
            ("api_key", api_key),
            ("artist", artist),
            ("album", album),
            ("format", "json"),
            ("autocorrect", "1"),
        ])
        .send()
        .await
        .ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let parsed: AlbumResponse = resp.json().await.ok()?;
    best_image(parsed.album?.image)
}

#[derive(Debug, thiserror::Error)]
pub enum LastfmError {
    #[error("not configured")]
    NotConfigured,
    #[error("last.fm is not linked yet")]
    NotAuthorized,
    #[error("last.fm said: {0}")]
    Api(String),
    #[error("could not reach last.fm")]
    Http(#[from] reqwest::Error),
    #[error("last.fm sent a malformed response")]
    Malformed,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Session {
    pub key: String,
    pub username: String,
}

pub fn sign(params: &[(&str, String)], shared_secret: &str) -> String {
    let mut sorted: Vec<&(&str, String)> = params.iter().collect();
    sorted.sort_by(|a, b| a.0.cmp(b.0));
    let mut buf = String::new();
    for (k, v) in sorted {
        buf.push_str(k);
        buf.push_str(v);
    }
    buf.push_str(shared_secret);
    format!("{:x}", md5::compute(buf))
}

pub fn authorize_url(api_key: &str, token: &str) -> String {
    format!(
        "https://www.last.fm/api/auth/?api_key={}&token={}",
        urlencoding::encode(api_key),
        urlencoding::encode(token)
    )
}

#[derive(Debug, Deserialize)]
struct ApiError {
    #[serde(default)]
    error: Option<i32>,
    #[serde(default)]
    message: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TokenResponse {
    #[serde(default)]
    token: Option<String>,
}

#[derive(Debug, Deserialize)]
struct SessionResponse {
    #[serde(default)]
    session: Option<SessionBody>,
}

#[derive(Debug, Deserialize)]
struct SessionBody {
    #[serde(default)]
    key: String,
    #[serde(default)]
    name: String,
}

async fn call<T: serde::de::DeserializeOwned>(
    http: &reqwest::Client,
    method: &str,
    api_key: &str,
    shared_secret: &str,
    mut params: Vec<(&'static str, String)>,
    post: bool,
) -> Result<T, LastfmError> {
    if api_key.trim().is_empty() || shared_secret.trim().is_empty() {
        return Err(LastfmError::NotConfigured);
    }
    params.push(("method", method.to_string()));
    params.push(("api_key", api_key.to_string()));
    let signature = sign(&params, shared_secret);
    params.push(("api_sig", signature));
    params.push(("format", "json".to_string()));

    let request = if post {
        http.post(BASE).form(&params)
    } else {
        http.get(BASE).query(&params)
    };
    let body = request.send().await?.text().await?;

    if let Ok(ApiError { error: Some(code), message }) = serde_json::from_str::<ApiError>(&body) {
        let text = message.unwrap_or_else(|| format!("error {code}"));
        return Err(match code {
            4 | 9 | 14 | 15 => LastfmError::NotAuthorized,
            _ => LastfmError::Api(text),
        });
    }

    serde_json::from_str(&body).map_err(|_| LastfmError::Malformed)
}

pub async fn request_token(
    http: &reqwest::Client,
    api_key: &str,
    shared_secret: &str,
) -> Result<String, LastfmError> {
    let parsed: TokenResponse =
        call(http, "auth.getToken", api_key, shared_secret, Vec::new(), false).await?;
    parsed.token.filter(|t| !t.trim().is_empty()).ok_or(LastfmError::Malformed)
}

pub async fn fetch_session(
    http: &reqwest::Client,
    api_key: &str,
    shared_secret: &str,
    token: &str,
) -> Result<Session, LastfmError> {
    let parsed: SessionResponse = call(
        http,
        "auth.getSession",
        api_key,
        shared_secret,
        vec![("token", token.to_string())],
        false,
    )
    .await?;
    let body = parsed.session.ok_or(LastfmError::Malformed)?;
    if body.key.trim().is_empty() {
        return Err(LastfmError::Malformed);
    }
    Ok(Session { key: body.key, username: body.name })
}

pub struct Auth<'a> {
    pub api_key: &'a str,
    pub shared_secret: &'a str,
    pub session_key: &'a str,
}

pub struct Play<'a> {
    pub artist: &'a str,
    pub title: &'a str,
    pub album: &'a str,
}

impl Play<'_> {
    fn params(&self, session_key: &str) -> Vec<(&'static str, String)> {
        let mut params = vec![
            ("artist", self.artist.to_string()),
            ("track", self.title.to_string()),
            ("sk", session_key.to_string()),
        ];
        if !self.album.trim().is_empty() {
            params.push(("album", self.album.to_string()));
        }
        params
    }
}

pub async fn update_now_playing(
    http: &reqwest::Client,
    auth: Auth<'_>,
    play: Play<'_>,
) -> Result<(), LastfmError> {
    let params = play.params(auth.session_key);
    let _: serde_json::Value =
        call(http, "track.updateNowPlaying", auth.api_key, auth.shared_secret, params, true)
            .await?;
    Ok(())
}

pub async fn scrobble(
    http: &reqwest::Client,
    auth: Auth<'_>,
    play: Play<'_>,
    started_at: u64,
) -> Result<(), LastfmError> {
    let mut params = play.params(auth.session_key);
    params.push(("timestamp", started_at.to_string()));
    let _: serde_json::Value =
        call(http, "track.scrobble", auth.api_key, auth.shared_secret, params, true).await?;
    Ok(())
}

pub fn scrobble_due(position_ms: u64, duration_ms: u64) -> bool {
    const MIN_TRACK_MS: u64 = 30_000;
    const ALWAYS_AT_MS: u64 = 4 * 60 * 1000;
    if duration_ms < MIN_TRACK_MS {
        return false;
    }
    position_ms >= (duration_ms / 2).min(ALWAYS_AT_MS)
}

pub fn started_at(now_unix: u64, position_ms: u64) -> u64 {
    now_unix.saturating_sub(position_ms / 1000)
}

pub fn now_unix() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn img(size: &str, url: &str) -> Image {
        Image { url: url.into(), size: size.into() }
    }

    #[test]
    fn prefers_the_largest_populated_size() {
        let images = vec![
            img("small", "https://x/s.png"),
            img("medium", "https://x/m.png"),
            img("extralarge", "https://x/xl.png"),
        ];
        assert_eq!(best_image(images).as_deref(), Some("https://x/xl.png"));
    }

    #[test]
    fn skips_sizes_last_fm_left_blank() {
        let images = vec![img("extralarge", ""), img("large", "  "), img("medium", "https://x/m.png")];
        assert_eq!(best_image(images).as_deref(), Some("https://x/m.png"));
    }

    #[test]
    fn no_usable_image_is_none_not_an_empty_string() {
        assert_eq!(best_image(vec![img("large", ""), img("medium", "")]), None);
        assert_eq!(best_image(vec![]), None);
    }

    #[test]
    fn a_play_counts_at_the_halfway_mark() {
        let three_min = 180_000;
        assert!(!scrobble_due(89_000, three_min));
        assert!(scrobble_due(90_000, three_min));
    }

    #[test]
    fn long_tracks_count_at_four_minutes_not_halfway() {
        // A 20-minute mix should not need 10 minutes to register.
        let twenty_min = 1_200_000;
        assert!(!scrobble_due(239_000, twenty_min));
        assert!(scrobble_due(240_000, twenty_min));
    }

    #[test]
    fn tracks_under_thirty_seconds_never_count() {
        assert!(!scrobble_due(29_000, 29_000));
        assert!(!scrobble_due(1_000_000, 12_000));
    }

    #[test]
    fn a_track_of_unknown_length_never_counts() {
        assert!(!scrobble_due(500_000, 0));
    }

    #[test]
    fn signing_sorts_by_name_then_appends_the_secret() {
        let params =
            vec![("track", "b".to_string()), ("api_key", "k".to_string()), ("artist", "a".to_string())];
        let expected = format!("{:x}", md5::compute("api_keykartistatrackbsecret"));
        assert_eq!(sign(&params, "secret"), expected);
    }

    #[test]
    fn signing_is_order_independent() {
        let one = vec![("b", "2".to_string()), ("a", "1".to_string())];
        let two = vec![("a", "1".to_string()), ("b", "2".to_string())];
        assert_eq!(sign(&one, "s"), sign(&two, "s"));
    }

    #[test]
    fn the_timestamp_is_when_the_track_started() {
        assert_eq!(started_at(1_000_000, 90_000), 999_910);
    }

    #[test]
    fn a_position_beyond_the_clock_cannot_underflow() {
        assert_eq!(started_at(10, 90_000), 0);
    }

    #[test]
    fn the_authorize_url_escapes_its_parameters() {
        let url = authorize_url("a b", "t&x");
        assert!(url.contains("api_key=a%20b"), "{url}");
        assert!(url.contains("token=t%26x"), "{url}");
    }
}
