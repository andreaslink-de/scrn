//! Random tokens, hashing and password handling.

use argon2::Argon2;
use argon2::password_hash::phc::PasswordHash;
use argon2::password_hash::{PasswordHasher, PasswordVerifier};
use sha2::{Digest, Sha256 as Sha256Hasher};

use crate::repo::Sha256;

/// Errors of the crypto helpers. They never contain secret material.
#[derive(Debug, thiserror::Error)]
pub enum CryptoError {
    #[error("the operating system random number generator failed")]
    Random,
    #[error("password hashing failed")]
    Hash,
}

/// Fills an array with bytes from the operating system CSPRNG.
pub fn random_bytes<const N: usize>() -> Result<[u8; N], CryptoError> {
    let mut buf = [0u8; N];
    getrandom::fill(&mut buf).map_err(|_| CryptoError::Random)?;
    Ok(buf)
}

pub fn sha256(data: &[u8]) -> Sha256 {
    Sha256Hasher::digest(data).into()
}

/// A random 256-bit secret, encoded as 64 lowercase hex characters, together
/// with the SHA-256 hash of the encoded string. Only the hash is stored.
pub struct SecretToken {
    pub token: String,
    pub hash: Sha256,
}

impl SecretToken {
    pub fn generate() -> Result<Self, CryptoError> {
        let token = hex_encode(&random_bytes::<32>()?);
        let hash = sha256(token.as_bytes());
        Ok(Self { token, hash })
    }
}

/// Hashes a token received from a client for lookup. Returns `None` for
/// strings that cannot be a token we issued, so obviously invalid input
/// never reaches the database.
pub fn hash_hex_token(token: &str) -> Option<Sha256> {
    if token.len() == 64
        && token
            .bytes()
            .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
    {
        Some(sha256(token.as_bytes()))
    } else {
        None
    }
}

/// Prefix of API tokens. It makes leaked tokens easy to recognize for
/// secret scanners.
pub const API_TOKEN_PREFIX: &str = "scrn_";
/// 62^43 > 2^256, so 43 base62 digits hold any 32-byte value.
const API_TOKEN_DIGITS: usize = 43;
const BASE62: &[u8; 62] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";

/// A new API token: `scrn_` followed by 32 random bytes in base62, together
/// with the SHA-256 hash of the whole string. Only the hash is stored.
pub struct ApiTokenSecret {
    pub token: String,
    pub hash: Sha256,
}

impl ApiTokenSecret {
    pub fn generate() -> Result<Self, CryptoError> {
        let token = format!("{API_TOKEN_PREFIX}{}", base62_fixed(&random_bytes::<32>()?));
        let hash = sha256(token.as_bytes());
        Ok(Self { token, hash })
    }
}

/// Hashes an API token received from a client for lookup. Returns `None` for
/// strings that do not have the token format.
pub fn hash_api_token(token: &str) -> Option<Sha256> {
    let digits = token.strip_prefix(API_TOKEN_PREFIX)?;
    if digits.len() == API_TOKEN_DIGITS && digits.bytes().all(|b| b.is_ascii_alphanumeric()) {
        Some(sha256(token.as_bytes()))
    } else {
        None
    }
}

/// Encodes 32 bytes as a big-endian number in exactly 43 base62 digits,
/// padded with leading zeros.
fn base62_fixed(bytes: &[u8; 32]) -> String {
    let mut num = *bytes;
    let mut digits = [0u8; API_TOKEN_DIGITS];
    for digit in digits.iter_mut().rev() {
        // Long division of the 256-bit number by 62.
        let mut rem = 0u32;
        for byte in &mut num {
            let acc = (rem << 8) | u32::from(*byte);
            // acc < 62 * 256, so the quotient fits into a byte.
            *byte = u8::try_from(acc / 62).unwrap_or(u8::MAX);
            rem = acc % 62;
        }
        *digit = BASE62[usize::try_from(rem).unwrap_or(0)];
    }
    digits.iter().copied().map(char::from).collect()
}

pub fn hex_encode(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(char::from(DIGITS[usize::from(b >> 4)]));
        out.push(char::from(DIGITS[usize::from(b & 0x0f)]));
    }
    out
}

/// Compares two byte strings in time that depends only on their lengths.
pub fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Hashes a password with argon2id (default parameters: 19 MiB, t=2, p=1)
/// and a random salt. CPU heavy: call it from a blocking task.
pub fn hash_password(password: &str) -> Result<String, CryptoError> {
    Argon2::default()
        .hash_password(password.as_bytes())
        .map(|hash| hash.to_string())
        .map_err(|_| CryptoError::Hash)
}

/// Verifies a password against a stored PHC string. Malformed hashes count
/// as a mismatch. CPU heavy: call it from a blocking task.
pub fn verify_password(password: &str, phc: &str) -> bool {
    match PasswordHash::new(phc) {
        Ok(parsed) => Argon2::default()
            .verify_password(password.as_bytes(), &parsed)
            .is_ok(),
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secret_tokens_are_random_hex_and_hash_matches() {
        let a = SecretToken::generate().unwrap();
        let b = SecretToken::generate().unwrap();
        assert_ne!(a.token, b.token);
        assert_eq!(a.token.len(), 64);
        assert_eq!(hash_hex_token(&a.token), Some(a.hash));
    }

    #[test]
    fn hash_hex_token_rejects_foreign_input() {
        for input in ["", "abc", &"A".repeat(64), &"g".repeat(64), &"a".repeat(65)] {
            assert_eq!(hash_hex_token(input), None, "{input}");
        }
    }

    #[test]
    fn api_tokens_have_the_scrn_format_and_hash_matches() {
        let a = ApiTokenSecret::generate().unwrap();
        let b = ApiTokenSecret::generate().unwrap();
        assert_ne!(a.token, b.token);
        assert!(a.token.starts_with("scrn_"));
        assert_eq!(a.token.len(), 5 + 43);
        assert_eq!(hash_api_token(&a.token), Some(a.hash));
    }

    #[test]
    fn hash_api_token_rejects_foreign_input() {
        let digits = "a".repeat(43);
        for input in [
            String::new(),
            digits.clone(),
            format!("scrn_{}", "a".repeat(42)),
            format!("scrn_{}", "a".repeat(44)),
            format!("scrn_{}-", "a".repeat(42)),
            format!("SCRN_{digits}"),
            "a".repeat(64),
        ] {
            assert_eq!(hash_api_token(&input), None, "{input}");
        }
        assert!(hash_api_token(&format!("scrn_{digits}")).is_some());
    }

    #[test]
    fn base62_encoding_is_fixed_width() {
        assert_eq!(base62_fixed(&[0; 32]), "0".repeat(43));
        let mut one = [0u8; 32];
        one[31] = 61;
        assert_eq!(base62_fixed(&one), format!("{}z", "0".repeat(42)));
        one[31] = 62;
        assert_eq!(base62_fixed(&one), format!("{}10", "0".repeat(41)));
        // 2^256 - 1 still fits into 43 digits.
        let max = base62_fixed(&[0xff; 32]);
        assert_eq!(max.len(), 43);
        assert_ne!(max.as_bytes()[0], b'0');
    }

    #[test]
    fn hex_encoding() {
        assert_eq!(hex_encode(&[0x00, 0x0f, 0xa5, 0xff]), "000fa5ff");
    }

    #[test]
    fn sha256_known_value() {
        assert_eq!(
            hex_encode(&sha256(b"abc")),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn constant_time_comparison() {
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abc", b"abd"));
        assert!(!constant_time_eq(b"abc", b"abcd"));
    }

    #[test]
    fn password_round_trip_uses_argon2id() {
        let hash = hash_password("correct horse battery").unwrap();
        assert!(hash.starts_with("$argon2id$v=19$m=19456,t=2,p=1$"));
        assert!(verify_password("correct horse battery", &hash));
        assert!(!verify_password("wrong horse battery", &hash));
        assert!(!verify_password("correct horse battery", "not a hash"));
    }
}
