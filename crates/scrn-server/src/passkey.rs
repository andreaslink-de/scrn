//! Passkeys (WebAuthn) with webauthn-rs.
//!
//! The relying party ID is the host of `SCRN_BASE_URL` and the origin is the
//! base URL itself, so `http://localhost:8081` works for local development.
//! Browsers do not allow IP addresses as relying party ID; with such a base
//! URL passkeys are switched off and only passwords work.
//!
//! The state of a running ceremony (the challenge) never leaves the server:
//! it is kept in memory under a random ceremony ID for at most
//! [`CEREMONY_TTL`] seconds and can be used once.

use std::collections::HashMap;
use std::sync::Mutex;

use webauthn_rs::prelude::{PasskeyAuthentication, PasskeyRegistration, Url, Uuid};
use webauthn_rs::{Webauthn, WebauthnBuilder};

use crate::crypto::{self, CryptoError};

/// Seconds a ceremony may take, matching the authenticator timeout.
pub const CEREMONY_TTL: i64 = 5 * 60;
/// Upper bound of pending ceremonies per kind, so a flood of start
/// requests cannot exhaust memory.
pub const MAX_PENDING: usize = 10_000;

/// Builds the WebAuthn configuration from the base URL. Returns `None` if
/// the URL has no domain name (an IP address), which WebAuthn forbids.
pub fn build(base_url: &str) -> Option<Webauthn> {
    let origin = Url::parse(base_url).ok()?;
    let rp_id = origin.domain()?.to_owned();
    WebauthnBuilder::new(&rp_id, &origin)
        .ok()?
        .rp_name("SCRN")
        .build()
        .ok()
}

/// The WebAuthn user handle of a user. It is derived from the numeric ID,
/// stable and free of personal data.
pub fn user_handle(user_id: i64) -> Uuid {
    // "SCRN" in the high bits keeps the value recognizable.
    Uuid::from_u64_pair(0x5343_524e_0000_0000, user_id.cast_unsigned())
}

/// A pending registration of a logged-in user.
pub struct Registration {
    pub user_id: i64,
    pub state: PasskeyRegistration,
}

/// A pending login of a user.
pub struct Authentication {
    pub user_id: i64,
    pub state: PasskeyAuthentication,
}

/// Passkey support: the configuration and the pending ceremonies.
pub struct Passkeys {
    pub webauthn: Webauthn,
    pub registrations: CeremonyStore<Registration>,
    pub authentications: CeremonyStore<Authentication>,
}

impl Passkeys {
    pub fn new(webauthn: Webauthn) -> Self {
        Self {
            webauthn,
            registrations: CeremonyStore::new(CEREMONY_TTL, MAX_PENDING),
            authentications: CeremonyStore::new(CEREMONY_TTL, MAX_PENDING),
        }
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum CeremonyError {
    #[error("too many pending passkey ceremonies")]
    Full,
    #[error("the random number generator failed")]
    Random,
}

impl From<CryptoError> for CeremonyError {
    fn from(_: CryptoError) -> Self {
        Self::Random
    }
}

/// Single-use values with a time to live, keyed by random IDs.
pub struct CeremonyStore<T> {
    ttl: i64,
    capacity: usize,
    entries: Mutex<HashMap<String, (i64, T)>>,
}

impl<T> CeremonyStore<T> {
    pub fn new(ttl: i64, capacity: usize) -> Self {
        Self {
            ttl,
            capacity,
            entries: Mutex::new(HashMap::new()),
        }
    }

    /// Stores `value` and returns its ID (64 hex characters).
    pub fn insert(&self, value: T, now: i64) -> Result<String, CeremonyError> {
        let id = crypto::hex_encode(&crypto::random_bytes::<32>()?);
        let mut entries = self.lock();
        if entries.len() >= self.capacity {
            entries.retain(|_, (expires, _)| *expires > now);
            if entries.len() >= self.capacity {
                return Err(CeremonyError::Full);
            }
        }
        entries.insert(id.clone(), (now + self.ttl, value));
        Ok(id)
    }

    /// Removes and returns the value if it has not expired. A second call
    /// with the same ID returns `None`.
    pub fn take(&self, id: &str, now: i64) -> Option<T> {
        let (expires, value) = self.lock().remove(id)?;
        (expires > now).then_some(value)
    }

    pub fn len(&self) -> usize {
        self.lock().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, (i64, T)>> {
        // The map stays consistent even if a holder panicked.
        self.entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relying_party_from_base_url() {
        assert!(build("https://scrn.dev").is_some());
        assert!(build("http://localhost:8080").is_some());
        assert!(build("https://sub.example.org:8443").is_some());
        // WebAuthn does not accept IP addresses as relying party ID.
        assert!(build("http://127.0.0.1:8080").is_none());
        assert!(build("http://[::1]:8080").is_none());
        assert!(build("not a url").is_none());
    }

    #[test]
    fn user_handles_are_stable_and_distinct() {
        assert_eq!(user_handle(1), user_handle(1));
        assert_ne!(user_handle(1), user_handle(2));
        assert!(user_handle(7).to_string().starts_with("5343524e-0000-0000"));
    }

    #[test]
    fn values_can_be_taken_once() {
        let store = CeremonyStore::new(60, 10);
        let id = store.insert("state", 100).unwrap();
        assert_eq!(id.len(), 64);
        assert_eq!(store.take(&id, 120), Some("state"));
        assert_eq!(store.take(&id, 121), None);
        assert_eq!(store.take("unknown", 121), None);
        assert!(store.is_empty());
    }

    #[test]
    fn values_expire() {
        let store = CeremonyStore::new(60, 10);
        let id = store.insert(1, 100).unwrap();
        assert_eq!(store.take(&id, 160), None);
        // Expired values are removed on access too.
        assert!(store.is_empty());
        let id = store.insert(2, 100).unwrap();
        assert_eq!(store.take(&id, 159), Some(2));
    }

    #[test]
    fn capacity_is_bounded_and_expired_entries_make_room() {
        let store = CeremonyStore::new(60, 2);
        store.insert(1, 100).unwrap();
        store.insert(2, 100).unwrap();
        assert_eq!(store.insert(3, 120), Err(CeremonyError::Full));
        // Once the first two expired there is room again.
        let id = store.insert(4, 160).unwrap();
        assert_eq!(store.len(), 1);
        assert_eq!(store.take(&id, 161), Some(4));
    }

    #[test]
    fn ids_are_random() {
        let store = CeremonyStore::new(60, 10);
        let a = store.insert((), 0).unwrap();
        let b = store.insert((), 0).unwrap();
        assert_ne!(a, b);
    }
}
