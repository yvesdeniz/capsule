//! Token storage.
//!
//! We never see an Apple password: sign-in happens on Apple's own login page
//! inside the engine window, and we only ever receive the two tokens MusicKit
//! exposes afterwards. Those go to Windows Credential Manager, not to disk.

use keyring::Entry;
use serde::{Deserialize, Serialize};

const SERVICE: &str = "capsule";
const ACCOUNT: &str = "apple-music-tokens";

#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    #[error("credential store unavailable: {0}")]
    Store(#[from] keyring::Error),
    #[error("stored credential was malformed: {0}")]
    Malformed(#[from] serde_json::Error),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Tokens {
    pub developer_token: String,
    pub music_user_token: String,
    #[serde(default)]
    pub storefront: String,
}

impl Tokens {
    pub fn is_complete(&self) -> bool {
        !self.developer_token.trim().is_empty() && !self.music_user_token.trim().is_empty()
    }
}

fn entry() -> Result<Entry, AuthError> {
    Ok(Entry::new(SERVICE, ACCOUNT)?)
}

pub fn save(tokens: &Tokens) -> Result<(), AuthError> {
    let json = serde_json::to_string(tokens)?;
    entry()?.set_password(&json)?;
    Ok(())
}

pub fn load() -> Result<Option<Tokens>, AuthError> {
    match entry()?.get_password() {
        Ok(json) => Ok(Some(serde_json::from_str(&json)?)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(AuthError::Store(e)),
    }
}

pub fn clear() -> Result<(), AuthError> {
    match entry()?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(AuthError::Store(e)),
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthStatus {
    pub authenticated: bool,
    pub storefront: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn incomplete_tokens_are_rejected() {
        let t = Tokens {
            developer_token: "abc".into(),
            music_user_token: "   ".into(),
            storefront: "us".into(),
        };
        assert!(!t.is_complete());
    }

    #[test]
    fn complete_tokens_are_accepted() {
        let t = Tokens {
            developer_token: "abc".into(),
            music_user_token: "def".into(),
            storefront: "tr".into(),
        };
        assert!(t.is_complete());
    }

    #[test]
    fn tokens_round_trip_through_json() {
        let t = Tokens {
            developer_token: "d".into(),
            music_user_token: "m".into(),
            storefront: "tr".into(),
        };
        let s = serde_json::to_string(&t).unwrap();
        assert_eq!(serde_json::from_str::<Tokens>(&s).unwrap(), t);
    }

    #[test]
    fn storefront_defaults_when_absent_from_stored_json() {
        let t: Tokens =
            serde_json::from_str(r#"{"developer_token":"d","music_user_token":"m"}"#).unwrap();
        assert_eq!(t.storefront, "");
        assert!(t.is_complete());
    }
}
