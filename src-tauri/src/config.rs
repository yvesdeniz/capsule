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

pub fn describe() -> String {
    format!(
        "lastfm={} discord={} show_engine_window={}",
        Keys::lastfm_enabled(),
        Keys::discord_enabled(),
        Runtime::from_env().show_engine_window
    )
}
