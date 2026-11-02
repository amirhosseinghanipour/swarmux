//! JEV API keys: from an environment variable or the OS keychain only.
//! They are never written to config.toml, the database or logs.

use std::fmt;

use swarmux_application::JevProvider;

/// Keychain service name; the account is the provider name.
const SERVICE: &str = "swarmux";

/// Where a key was found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeySource {
    Env(&'static str),
    Keychain,
}

impl fmt::Display for KeySource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            KeySource::Env(var) => write!(f, "${var}"),
            KeySource::Keychain => write!(f, "OS keychain (service \"{SERVICE}\")"),
        }
    }
}

/// The environment variable jev-layer reads for `provider`.
pub fn env_var(provider: JevProvider) -> Option<&'static str> {
    match provider {
        JevProvider::Off => None,
        JevProvider::TypeSafe => Some("TYPESAFE_API_KEY"),
        JevProvider::OpenRouter => Some("OPENROUTER_API_KEY"),
    }
}

/// The key for `provider`: its environment variable first, then the keychain.
pub fn find_key(provider: JevProvider) -> Option<(String, KeySource)> {
    let var = env_var(provider)?;
    if let Some(key) = std::env::var(var).ok().filter(|k| !k.trim().is_empty()) {
        return Some((key.trim().to_string(), KeySource::Env(var)));
    }
    let entry = keyring::Entry::new(SERVICE, provider.as_str()).ok()?;
    let key = entry.get_password().ok()?;
    let key = key.trim();
    (!key.is_empty()).then(|| (key.to_string(), KeySource::Keychain))
}

/// Saves `key` for `provider` in the OS keychain.
pub fn store_key(provider: JevProvider, key: &str) -> Result<(), String> {
    if provider == JevProvider::Off {
        return Err("choose a provider: typesafe or openrouter".into());
    }
    let key = key.trim();
    if key.is_empty() {
        return Err("the key is empty".into());
    }
    let entry =
        keyring::Entry::new(SERVICE, provider.as_str()).map_err(|e| redact(&e.to_string(), key))?;
    entry
        .set_password(key)
        .map_err(|e| redact(&e.to_string(), key))
}

/// `text` with every copy of `key` replaced.
pub fn redact(text: &str, key: &str) -> String {
    if key.len() < 4 {
        return text.to_string();
    }
    text.replace(key, "[redacted]")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_vars_match_jev_layer() {
        assert_eq!(env_var(JevProvider::TypeSafe), Some("TYPESAFE_API_KEY"));
        assert_eq!(env_var(JevProvider::OpenRouter), Some("OPENROUTER_API_KEY"));
        assert_eq!(env_var(JevProvider::Off), None);
        assert_eq!(find_key(JevProvider::Off), None);
    }

    #[test]
    fn redact_hides_the_key() {
        assert_eq!(
            redact("bad key sk-abc123 rejected sk-abc123", "sk-abc123"),
            "bad key [redacted] rejected [redacted]"
        );
        assert_eq!(redact("abc", ""), "abc");
    }

    #[test]
    fn empty_keys_and_off_are_refused() {
        assert!(store_key(JevProvider::TypeSafe, "  ").is_err());
        assert!(store_key(JevProvider::Off, "k").is_err());
    }

    #[test]
    fn sources_display_without_the_key() {
        assert_eq!(
            KeySource::Env("TYPESAFE_API_KEY").to_string(),
            "$TYPESAFE_API_KEY"
        );
        assert!(KeySource::Keychain.to_string().contains("keychain"));
    }
}
