//! Resolves the Redmine URL and API key from environment variables or the
//! system credential store (macOS Keychain on macOS).
//!
//! Lookup order, per value:
//!   URL:     REDMINE_URL      -> keychain profile
//!   API key: REDMINE_API_KEY  -> keychain profile (only if its URL matches)
//!
//! A key is only ever sent to the URL it was paired with: REDMINE_API_KEY needs
//! REDMINE_URL, and a keychain key needs the URL stored next to it.

use std::env;

use serde::{Deserialize, Serialize};

use crate::error::{AppError, Result};

pub const SERVICE: &str = "redmine-cli";
pub const DEFAULT_PROFILE: &str = "default";
pub const ENV_URL: &str = "REDMINE_URL";
pub const ENV_API_KEY: &str = "REDMINE_API_KEY";
/// Set to any non-empty value other than "0" to never touch the keychain (CI, tests).
pub const ENV_NO_KEYCHAIN: &str = "REDMINE_NO_KEYCHAIN";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    Env,
    Keychain,
}

/// What is stored in one keychain item (account = profile name).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredProfile {
    pub url: String,
    pub api_key: String,
}

#[derive(Debug, Clone)]
pub struct Credentials {
    pub url: String,
    pub url_source: Source,
    pub api_key: Option<(String, Source)>,
}

pub fn normalize_url(raw: &str) -> Result<String> {
    let trimmed = raw.trim().trim_end_matches('/');
    let parsed = url::Url::parse(trimmed)
        .map_err(|e| AppError::Usage(format!("invalid Redmine URL {raw:?}: {e}")))?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err(AppError::Usage(format!(
            "invalid Redmine URL {raw:?}: scheme must be http or https"
        )));
    }
    Ok(trimmed.to_string())
}

/// A warning when the key would travel in cleartext to a non-loopback host.
pub fn cleartext_warning(url: &str) -> Option<String> {
    let parsed = url::Url::parse(url).ok()?;
    let loopback = match parsed.host()? {
        url::Host::Domain(d) => d.eq_ignore_ascii_case("localhost"),
        url::Host::Ipv4(ip) => ip.is_loopback(),
        url::Host::Ipv6(ip) => ip.is_loopback(),
    };
    (parsed.scheme() == "http" && !loopback)
        .then(|| format!("{url} uses http: the API key is sent unencrypted; prefer https"))
}

fn non_empty_env(name: &str) -> Option<String> {
    env::var(name)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

pub fn keychain_disabled() -> bool {
    env::var(ENV_NO_KEYCHAIN).is_ok_and(|v| !v.is_empty() && v != "0")
}

fn entry(profile: &str) -> Result<keyring::Entry> {
    Ok(keyring::Entry::new(SERVICE, profile)?)
}

pub fn load_profile(profile: &str) -> Result<Option<StoredProfile>> {
    if keychain_disabled() {
        return Ok(None);
    }
    match entry(profile)?.get_password() {
        Ok(raw) => serde_json::from_str(&raw).map(Some).map_err(|e| {
            AppError::Credential(format!(
                "keychain item {SERVICE}/{profile} is not valid: {e}; run `redmine auth login` again"
            ))
        }),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

pub fn store_profile(profile: &str, stored: &StoredProfile) -> Result<()> {
    if keychain_disabled() {
        return Err(AppError::Credential(format!(
            "keychain is disabled by {ENV_NO_KEYCHAIN}"
        )));
    }
    let raw = serde_json::to_string(stored).expect("serializing a profile cannot fail");
    Ok(entry(profile)?.set_password(&raw)?)
}

/// Returns whether an item existed.
pub fn delete_profile(profile: &str) -> Result<bool> {
    if keychain_disabled() {
        return Err(AppError::Credential(format!(
            "keychain is disabled by {ENV_NO_KEYCHAIN}"
        )));
    }
    match entry(profile)?.delete_credential() {
        Ok(()) => Ok(true),
        Err(keyring::Error::NoEntry) => Ok(false),
        Err(e) => Err(e.into()),
    }
}

pub fn resolve(profile: &str) -> Result<Credentials> {
    resolve_with(
        profile,
        non_empty_env(ENV_URL),
        non_empty_env(ENV_API_KEY),
        load_profile,
    )
}

fn resolve_with(
    profile: &str,
    env_url: Option<String>,
    env_key: Option<String>,
    load: impl FnOnce(&str) -> Result<Option<StoredProfile>>,
) -> Result<Credentials> {
    let env_url = env_url.map(|u| normalize_url(&u)).transpose()?;

    // The env key has no URL of its own; never let it fall through to a keychain URL.
    if env_key.is_some() && env_url.is_none() {
        return Err(AppError::Config(format!(
            "{ENV_API_KEY} is set but {ENV_URL} is not; set both \
             (a key from the environment is only sent to {ENV_URL})"
        )));
    }

    // Both from env: never touch the keychain (no prompts in CI / agent sandboxes).
    let stored = match (&env_url, &env_key) {
        (Some(_), Some(_)) => None,
        _ => load(profile)?,
    };

    let (url, url_source) = match (env_url, &stored) {
        (Some(u), _) => (u, Source::Env),
        (None, Some(s)) => (normalize_url(&s.url)?, Source::Keychain),
        (None, None) => {
            return Err(AppError::Config(format!(
                "no Redmine URL: set {ENV_URL} or run `redmine auth login` (profile {profile:?})"
            )));
        }
    };

    let api_key = match (env_key, stored) {
        (Some(k), _) => Some((k, Source::Env)),
        (None, Some(s)) => {
            // Never send a stored key to a server other than the one it was saved for.
            if normalize_url(&s.url)? != url {
                return Err(AppError::Config(format!(
                    "keychain profile {profile:?} is for {}, but {ENV_URL} is {url}; \
                     refusing to send the stored key to a different server. \
                     Set {ENV_API_KEY}, unset {ENV_URL}, or use --profile",
                    s.url
                )));
            }
            Some((s.api_key, Source::Keychain))
        }
        (None, None) => None,
    };

    Ok(Credentials {
        url,
        url_source,
        api_key,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stored(url: &str) -> Result<Option<StoredProfile>> {
        Ok(Some(StoredProfile {
            url: url.into(),
            api_key: "stored-key".into(),
        }))
    }

    fn key_and_url(c: &Credentials) -> (&str, &str) {
        (c.api_key.as_ref().unwrap().0.as_str(), c.url.as_str())
    }

    #[test]
    fn cleartext_warning_only_for_remote_http() {
        assert!(cleartext_warning("http://redmine.example").is_some());
        assert!(cleartext_warning("http://10.0.0.5:3000").is_some());
        assert!(cleartext_warning("https://redmine.example").is_none());
        assert!(cleartext_warning("http://localhost:3000").is_none());
        assert!(cleartext_warning("http://127.0.0.1:18080").is_none());
        assert!(cleartext_warning("http://[::1]:3000").is_none());
    }

    #[test]
    fn env_key_is_never_sent_to_a_keychain_url() {
        let err = resolve_with("p", None, Some("env-key".into()), |_| {
            stored("https://other.example")
        })
        .unwrap_err();
        assert_eq!(err.exit_code(), 3);
    }

    #[test]
    fn keychain_key_is_never_sent_to_a_different_env_url() {
        let err = resolve_with("p", Some("https://evil.example".into()), None, |_| {
            stored("https://r.example")
        })
        .unwrap_err();
        assert_eq!(err.exit_code(), 3);
    }

    #[test]
    fn matching_sources_resolve() {
        let c = resolve_with("p", None, None, |_| stored("https://r.example/")).unwrap();
        assert_eq!(key_and_url(&c), ("stored-key", "https://r.example"));

        let c = resolve_with("p", Some("https://r.example".into()), None, |_| {
            stored("https://r.example")
        })
        .unwrap();
        assert_eq!(key_and_url(&c), ("stored-key", "https://r.example"));

        let c = resolve_with(
            "p",
            Some("https://e.example".into()),
            Some("env-key".into()),
            |_| panic!("keychain must not be read when both env vars are set"),
        )
        .unwrap();
        assert_eq!(key_and_url(&c), ("env-key", "https://e.example"));
    }
}
