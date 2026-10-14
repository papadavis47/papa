//! Random tokens, codes and hashes. The relay stores none of the secrets
//! it hands out or receives, only hashes of them:
//!
//! - **Father passwords** are low-entropy, so they get Argon2id: slow and
//!   memory-hard, to make guessing from a leaked database expensive.
//! - **Session tokens, invite codes and child auth keys** are long random
//!   values that can't be guessed, so a plain SHA-256 is enough, and it lets
//!   the relay find a row by the hash. A child's auth key has already been
//!   through Argon2id on the child's device (PLAN.md §2.2).

use std::sync::OnceLock;

use argon2::Argon2;
use argon2::password_hash::{PasswordHasher, PasswordVerifier};
use sha2::{Digest, Sha256};

use crate::error::RelayError;

/// The shortest father password the relay accepts.
pub const MIN_PASSWORD_CHARS: usize = 10;

pub fn random_bytes<const N: usize>() -> Result<[u8; N], RelayError> {
    let mut bytes = [0; N];
    getrandom::fill(&mut bytes).map_err(|e| RelayError::Internal(format!("random: {e}")))?;
    Ok(bytes)
}

pub fn sha256(bytes: &[u8]) -> Vec<u8> {
    Sha256::digest(bytes).to_vec()
}

/// A new session token: 256 random bits as hex.
pub fn new_token() -> Result<String, RelayError> {
    Ok(hex::encode(random_bytes::<32>()?))
}

/// A share database's name: random, so names reveal nothing about who
/// owns them. Turso allows lowercase letters, digits and dashes, 64 at most.
pub fn new_database_name() -> Result<String, RelayError> {
    Ok(format!("share-{}", hex::encode(random_bytes::<16>()?)))
}

/// Crockford's base 32: no I, L, O or U, so a code read aloud or retyped
/// from a text message survives.
const CODE_ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

/// A new invite code, `XXXX-XXXX-XXXX`: 60 random bits. Enough for a code
/// that works once, expires, and can only be tried a few times a minute.
pub fn new_invite_code() -> Result<String, RelayError> {
    let bytes = random_bytes::<12>()?;
    let chars: Vec<char> = bytes
        .iter()
        // 256 is a multiple of 32, so `% 32` picks every letter equally.
        .map(|b| CODE_ALPHABET[usize::from(b % 32)] as char)
        .collect();
    let groups: Vec<String> = chars.chunks(4).map(|c| c.iter().collect()).collect();
    Ok(groups.join("-"))
}

/// An invite code the way it's stored: upper case, without dashes or
/// spaces, and with the letters people confuse with digits read as digits.
pub fn normalize_invite_code(code: &str) -> String {
    code.chars()
        .filter(|c| !c.is_whitespace() && *c != '-')
        .map(|c| match c.to_ascii_uppercase() {
            'O' => '0',
            'I' | 'L' => '1',
            other => other,
        })
        .collect()
}

/// Emails are compared case-insensitively and without surrounding spaces.
/// Not a full RFC 5322 check: the relay never sends mail, so an address
/// only has to be something the father will recognize and retype.
pub fn normalize_email(email: &str) -> Result<String, RelayError> {
    let email = email.trim().to_lowercase();
    let valid = email.len() <= 254
        && !email.chars().any(char::is_whitespace)
        && email
            .split_once('@')
            .is_some_and(|(user, domain)| !user.is_empty() && domain.contains('.'));
    if valid {
        Ok(email)
    } else {
        Err(RelayError::BadRequest("that isn't an email address".into()))
    }
}

/// A child's auth key from its 64 hex digits.
pub fn parse_auth_key(hex_key: &str) -> Result<[u8; 32], RelayError> {
    let mut key = [0; 32];
    hex::decode_to_slice(hex_key, &mut key)
        .map_err(|_| RelayError::BadRequest("auth_key must be 64 hex digits".into()))?;
    Ok(key)
}

/// Hashes a father's password to a PHC string (`$argon2id$v=19$...`),
/// which carries its own salt and cost.
///
/// Argon2id takes tens of milliseconds of CPU on purpose. On an async
/// worker thread that would hold up every other request scheduled there,
/// so it runs on tokio's pool for blocking work.
pub async fn hash_password(password: String) -> Result<String, RelayError> {
    if password.chars().count() < MIN_PASSWORD_CHARS {
        return Err(RelayError::BadRequest(format!(
            "the password needs at least {MIN_PASSWORD_CHARS} characters"
        )));
    }
    tokio::task::spawn_blocking(move || {
        Argon2::default()
            .hash_password(password.as_bytes())
            .map(|hash| hash.to_string())
            .map_err(|e| RelayError::Internal(format!("hashing a password: {e}")))
    })
    .await
    .map_err(|e| RelayError::Internal(e.to_string()))?
}

/// Checks `password` against `hash`, or against a throwaway hash when the
/// account doesn't exist. Both cases take the same time, so timing a log-in
/// doesn't reveal which emails have accounts.
pub async fn verify_password(password: String, hash: Option<String>) -> Result<bool, RelayError> {
    tokio::task::spawn_blocking(move || {
        let known = hash.is_some();
        let hash = hash.unwrap_or_else(|| dummy_hash().to_owned());
        let matches = Argon2::default()
            .verify_password(password.as_bytes(), hash.as_str())
            .is_ok();
        known && matches
    })
    .await
    .map_err(|e| RelayError::Internal(e.to_string()))
}

fn dummy_hash() -> &'static str {
    static HASH: OnceLock<String> = OnceLock::new();
    HASH.get_or_init(|| {
        Argon2::default()
            .hash_password(b"no account has this password")
            .expect("hashing a constant can't fail")
            .to_string()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invite_codes_survive_retyping() {
        let code = new_invite_code().unwrap();
        assert_eq!(code.len(), 14, "{code}");
        assert!(
            code.chars()
                .all(|c| c == '-' || CODE_ALPHABET.contains(&(c as u8)))
        );

        let stored = normalize_invite_code(&code);
        let retyped = format!(" {} ", code.to_lowercase().replace('-', " "));
        assert_eq!(normalize_invite_code(&retyped), stored);
        assert_eq!(normalize_invite_code("o1l-I"), "0111");
    }

    #[test]
    fn emails_are_normalized_and_checked() {
        assert_eq!(
            normalize_email("  Dad@Example.COM ").unwrap(),
            "dad@example.com"
        );
        for bad in [
            "",
            "dad",
            "@example.com",
            "dad@localhost",
            "d ad@example.com",
        ] {
            assert!(normalize_email(bad).is_err(), "{bad:?}");
        }
    }

    #[tokio::test]
    async fn passwords_verify_only_against_their_own_hash() {
        let hash = hash_password("correct horse".into()).await.unwrap();
        assert!(hash.starts_with("$argon2id$"), "{hash}");
        assert!(
            verify_password("correct horse".into(), Some(hash.clone()))
                .await
                .unwrap()
        );
        assert!(
            !verify_password("correct hors".into(), Some(hash))
                .await
                .unwrap()
        );
        // No account: never a match, even for the dummy hash's own password.
        assert!(
            !verify_password("no account has this password".into(), None)
                .await
                .unwrap()
        );
        assert!(hash_password("too short".into()).await.is_err());
    }
}
