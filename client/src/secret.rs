//! The API token in the system keyring (Secret Service on Linux, the
//! Credential Manager on Windows, the Keychain on macOS). One token per
//! server address. The token never goes to the settings file or the log.

const SERVICE: &str = "SCRN";

#[derive(Debug, thiserror::Error)]
#[error("system keyring: {0}")]
pub struct SecretError(String);

fn entry(server_url: &str) -> Result<keyring::Entry, SecretError> {
    keyring::Entry::new(SERVICE, server_url).map_err(|e| SecretError(e.to_string()))
}

/// The stored token, or `None` if there is none for this server.
pub fn token(server_url: &str) -> Result<Option<String>, SecretError> {
    match entry(server_url)?.get_password() {
        Ok(token) => Ok(Some(token)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(err) => Err(SecretError(err.to_string())),
    }
}

pub fn set_token(server_url: &str, token: &str) -> Result<(), SecretError> {
    entry(server_url)?
        .set_password(token)
        .map_err(|e| SecretError(e.to_string()))
}

pub fn delete_token(server_url: &str) -> Result<(), SecretError> {
    match entry(server_url)?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(err) => Err(SecretError(err.to_string())),
    }
}

/// Tokens created by the server look like `scrn_` plus about 43 base62
/// characters; this catches pasting the wrong thing early.
pub fn looks_like_token(token: &str) -> bool {
    token.strip_prefix("scrn_").is_some_and(|rest| {
        rest.len() >= 32
            && rest
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Talks to the real keyring of the session: `cargo test -- --ignored`.
    #[test]
    #[ignore = "needs the system keyring"]
    fn tokens_round_trip_through_the_keyring() {
        let server = format!("https://keyring-test.invalid/{}", std::process::id());
        assert_eq!(token(&server).unwrap(), None);
        set_token(&server, "scrn_test").unwrap();
        assert_eq!(token(&server).unwrap().as_deref(), Some("scrn_test"));
        delete_token(&server).unwrap();
        assert_eq!(token(&server).unwrap(), None);
        delete_token(&server).unwrap();
    }

    /// The app reads the keyring from `spawn_blocking`; zbus then starts its
    /// own runtime there, which must not panic.
    #[tokio::test]
    #[ignore = "needs the system keyring"]
    async fn the_keyring_works_from_spawn_blocking() {
        let server = format!("https://keyring-test.invalid/b{}", std::process::id());
        let found = tokio::task::spawn_blocking(move || token(&server))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(found, None);
    }

    #[test]
    fn token_shape_is_checked() {
        assert!(looks_like_token(&format!("scrn_{}", "a".repeat(43))));
        assert!(looks_like_token(
            "scrn_Ab-_0123456789abcdefghijklmnopqrstuv"
        ));
        assert!(!looks_like_token("scrn_short"));
        assert!(!looks_like_token(&format!("token_{}", "a".repeat(43))));
        assert!(!looks_like_token(&format!("scrn_{} ", "a".repeat(43))));
    }
}
