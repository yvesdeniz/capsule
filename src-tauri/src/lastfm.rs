//! Last.fm cover-art lookup for Discord Rich Presence.
//!
//! Not scrobbling - Navidrome does that, using credentials it already holds.
//! This exists only because Discord's image bot needs a publicly fetchable URL,
//! and the alternative is handing out a signed Subsonic URL.
//!
//! Every failure returns `None`. Presence without art beats no presence.

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

/// The largest usable image from Last.fm's size-ordered list.
///
/// Last.fm returns entries with empty URLs for sizes it lacks, so picking by
/// position rather than by content yields a blank image about as often as not.
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
}
