//! Short IDs of public screenshots: 8 base62 characters from the CSPRNG,
//! about 47.6 bits. They live directly under the domain (`/<short_id>`), so
//! they must never collide with fixed routes.

use crate::crypto::{self, CryptoError};

pub const LEN: usize = 8;

const ALPHABET: &[u8; 62] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";

/// First path segments used by fixed routes, now or later. A short ID that
/// starts with one of them (case-insensitively) is never generated, which
/// keeps even prefix-based routing and case-insensitive proxies safe.
pub const RESERVED_PREFIXES: &[&str] = &[
    "admin",
    "api",
    "app",
    "assets",
    "favicon",
    "healthz",
    "invite",
    "login",
    "logout",
    "robots",
    "setup",
    "static",
    "well-known",
];

/// Generates a short ID that does not start with a reserved prefix.
pub fn generate() -> Result<String, CryptoError> {
    loop {
        let id = random_id()?;
        if !is_reserved(&id) {
            return Ok(id);
        }
    }
}

fn random_id() -> Result<String, CryptoError> {
    let mut id = String::with_capacity(LEN);
    while id.len() < LEN {
        for b in crypto::random_bytes::<16>()? {
            // Rejection sampling: 248 = 4 × 62, so every digit is equally
            // likely.
            if b < 248 && id.len() < LEN {
                id.push(char::from(ALPHABET[usize::from(b % 62)]));
            }
        }
    }
    Ok(id)
}

pub fn is_reserved(id: &str) -> bool {
    let lower = id.to_ascii_lowercase();
    RESERVED_PREFIXES.iter().any(|p| lower.starts_with(p))
}

/// True if `s` has the shape of a short ID we could have issued.
pub fn is_well_formed(s: &str) -> bool {
    s.len() == LEN && s.bytes().all(|b| b.is_ascii_alphanumeric()) && !is_reserved(s)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn generated_ids_are_well_formed_and_distinct() {
        let ids: HashSet<String> = (0..1000).map(|_| generate().unwrap()).collect();
        assert_eq!(ids.len(), 1000);
        assert!(ids.iter().all(|id| is_well_formed(id)));
    }

    #[test]
    fn reserved_prefixes_are_detected_case_insensitively() {
        for id in ["apiXyz12", "APP00000", "Healthz1", "static00", "LoGiN123"] {
            assert!(is_reserved(id), "{id}");
            assert!(!is_well_formed(id), "{id}");
        }
        assert!(!is_reserved("aPx12345"));
        assert!(is_well_formed("aPx12345"));
    }

    #[test]
    fn malformed_ids() {
        for s in ["", "abc", "abcdefghi", "abcd-fgh", "abcdéfg"] {
            assert!(!is_well_formed(s), "{s}");
        }
    }
}
