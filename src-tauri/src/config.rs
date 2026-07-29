//! Build-time and runtime configuration.
//!
//! Two mechanisms, deliberately: integration keys are baked in with
//! `option_env!` so forks supply their own, while debug toggles and path
//! overrides are read from the environment at runtime so they can be flipped
//! without a rebuild. Absent values disable a feature — never panic.

/// Compile-time integration keys. `None` means the feature is simply off.
pub struct Keys;

impl Keys {
    pub const LASTFM_API_KEY: Option<&'static str> = option_env!("LASTFM_API_KEY");
    pub const LASTFM_SHARED_SECRET: Option<&'static str> = option_env!("LASTFM_SHARED_SECRET");
    pub const DISCORD_CLIENT_ID: Option<&'static str> = option_env!("DISCORD_CLIENT_ID");

    fn present(v: Option<&'static str>) -> bool {
        matches!(v, Some(s) if !s.trim().is_empty())
    }

    pub fn lastfm_enabled() -> bool {
        Self::present(Self::LASTFM_API_KEY) && Self::present(Self::LASTFM_SHARED_SECRET)
    }

    pub fn discord_enabled() -> bool {
        Self::present(Self::DISCORD_CLIENT_ID)
    }
}

fn env_flag(name: &str, default: bool) -> bool {
    match std::env::var(name) {
        Ok(v) => {
            let v = v.trim().to_ascii_lowercase();
            !(v == "false" || v == "0" || v == "no" || v.is_empty())
        }
        Err(_) => default,
    }
}

#[derive(Debug, Clone)]
pub struct Runtime {
    pub show_engine_window: bool,
}

impl Runtime {
    pub fn from_env() -> Self {
        Self { show_engine_window: env_flag("SHOW_ENGINE_WINDOW", false) }
    }
}

/// Navidrome connection from the environment, read via `std::env` rather
/// than `option_env!` like the integration keys above — changing which
/// server you point at must not require a rebuild. Mirrors how
/// `CAPSULE_DB_PATH` behaves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NavidromeEnv {
    pub url: String,
    pub username: String,
    pub password: String,
}

/// All three keys, or nothing — a partial set is ignored rather than merged
/// with stored settings, since pairing a development server URL with
/// production credentials by accident is worse than not applying the
/// override at all.
pub fn navidrome_env() -> Option<NavidromeEnv> {
    let get = |k: &str| {
        std::env::var(k).ok().map(|v| v.trim().to_string()).filter(|v| !v.is_empty())
    };
    match (get("NAVIDROME_URL"), get("NAVIDROME_USER"), get("NAVIDROME_PASSWORD")) {
        (Some(url), Some(username), Some(password)) => {
            Some(NavidromeEnv { url, username, password })
        }
        (url, username, password) => {
            let missing: Vec<&str> = [
                url.is_none().then_some("NAVIDROME_URL"),
                username.is_none().then_some("NAVIDROME_USER"),
                password.is_none().then_some("NAVIDROME_PASSWORD"),
            ]
            .into_iter()
            .flatten()
            .collect();
            if missing.len() < 3 {
                tracing::warn!(missing = ?missing, "partial NAVIDROME_* env; ignoring override");
            }
            None
        }
    }
}

pub fn describe() -> String {
    format!(
        "lastfm={} discord={} show_engine_window={}",
        Keys::lastfm_enabled(),
        Keys::discord_enabled(),
        Runtime::from_env().show_engine_window
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One test rather than three: these mutate process-wide environment, so
    /// splitting them lets the cases interleave and clobber each other.
    #[test]
    fn navidrome_env_requires_all_three_keys() {
        let keys = ["NAVIDROME_URL", "NAVIDROME_USER", "NAVIDROME_PASSWORD"];
        for k in keys {
            std::env::remove_var(k);
        }
        assert!(navidrome_env().is_none(), "nothing set");

        std::env::set_var("NAVIDROME_URL", "https://m.example.com");
        assert!(navidrome_env().is_none(), "partial set must be ignored entirely");

        std::env::set_var("NAVIDROME_USER", "deniz");
        assert!(navidrome_env().is_none(), "still partial");

        std::env::set_var("NAVIDROME_PASSWORD", "sesame");
        let env = navidrome_env().expect("all three set");
        assert_eq!(env.url, "https://m.example.com");
        assert_eq!(env.username, "deniz");
        assert_eq!(env.password, "sesame");

        // Whitespace-only counts as unset, not as a value.
        std::env::set_var("NAVIDROME_USER", "   ");
        assert!(navidrome_env().is_none(), "blank is not a username");

        for k in keys {
            std::env::remove_var(k);
        }
    }
}
