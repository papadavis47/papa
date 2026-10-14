//! The vault's password: deriving keys from it, and the key file that
//! keeps the vault's key wrapped under it.
//!
//! ```text
//!  password ──Argon2id(salt)──▶ wrapping key ─┐
//!                                             ▼
//!  vault.key: salt, nonce, wrapped key ──XChaCha20-Poly1305──▶ vault key
//!                                                                 │
//!  vault.db (every page encrypted by Turso, AEGIS-256) ◀──────────┘
//! ```
//!
//! The vault is encrypted with a random 256-bit **vault key**, not with the
//! password itself. The password only *wraps* (encrypts) that key, and the
//! result is stored next to the vault in `vault.key`. That buys three things:
//!
//! - **A wrong password fails cleanly.** Unwrapping is authenticated, so the
//!   wrong password gives [`Error::WrongPassword`] before Turso reads a page.
//! - **Changing the password is instant.** Only the small key file is
//!   rewritten; the vault keeps its key and isn't re-encrypted.
//! - **Phase 11 reuses it** for the key of each child's shared notes.
//!
//! Whether a vault is encrypted is read from the file itself (see
//! [`inspect`]), never from the settings: an encrypted Turso file starts
//! with `Turso`, a plain one with SQLite's `SQLite format 3`.

use std::fmt;
use std::io::{ErrorKind, Read as _};
use std::path::{Path, PathBuf};

use argon2::{Algorithm, Argon2, Params, Version};
use chacha20poly1305::aead::{Aead as _, KeyInit as _, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use serde::{Deserialize, Serialize};
use zeroize::{Zeroize as _, ZeroizeOnDrop, Zeroizing};

use crate::{Error, Result};

/// The shortest password Papa accepts, in characters.
pub const MIN_PASSWORD_CHARS: usize = 8;

/// Turso's cipher for the vault's pages: AEGIS-256, fast and with a
/// 256-bit key (PLAN.md §7).
pub(crate) const VAULT_CIPHER: &str = "aegis256";

/// Bound into every wrapped key, so a key wrapped for something else (a
/// share key, in Phase 11) can never be unwrapped as a vault key.
const WRAP_CONTEXT: &[u8] = b"papa vault key v1";

/// How hard Argon2id works on each unlock: 64 MiB of memory, three passes,
/// one lane. RFC 9106 recommends these for when 64 MiB is affordable. In a
/// release build it takes a fraction of a second; for someone guessing
/// passwords, every guess costs as much.
///
/// Tests use the smallest allowed values, so they stay quick. The values
/// used are stored in the key file, so they can be raised later without
/// breaking older vaults.
const KDF_COST: KdfCost = if cfg!(test) {
    KdfCost {
        memory_kib: 8,
        iterations: 1,
        parallelism: 1,
    }
} else {
    KdfCost {
        memory_kib: 64 * 1024,
        iterations: 3,
        parallelism: 1,
    }
};

/// A password, as typed. Its bytes are overwritten when it's dropped, so it
/// doesn't linger in freed memory.
///
/// `Debug` prints `Password(…)`: a password never ends up in a log by
/// accident.
#[derive(Clone, PartialEq, Eq)]
pub struct Password(Zeroizing<String>);

impl Password {
    pub fn new(password: impl Into<String>) -> Self {
        Self(Zeroizing::new(password.into()))
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Checks a new password against the rules: for now, only its length.
    /// Spaces count; nothing is trimmed.
    pub fn check_new(&self) -> Result<()> {
        if self.0.chars().count() < MIN_PASSWORD_CHARS {
            return Err(Error::PasswordTooShort);
        }
        Ok(())
    }

    fn as_bytes(&self) -> &[u8] {
        self.0.as_bytes()
    }
}

impl fmt::Debug for Password {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Password(…)")
    }
}

/// The key Turso encrypts the vault with. Never written anywhere
/// unwrapped, and zeroed when dropped.
#[derive(Clone, ZeroizeOnDrop)]
pub(crate) struct VaultKey([u8; 32]);

impl VaultKey {
    /// A new random key, for a vault that's being encrypted.
    pub(crate) fn generate() -> Result<Self> {
        let mut key = Self([0; 32]);
        getrandom::fill(&mut key.0).map_err(Error::Random)?;
        Ok(key)
    }

    /// The key as Turso takes it: 64 hex digits. Turso keeps a copy of its
    /// own while the vault is open, and zeroes it when the vault closes.
    pub(crate) fn hex(&self) -> Zeroizing<String> {
        Zeroizing::new(hex::encode(self.0))
    }
}

impl fmt::Debug for VaultKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("VaultKey(…)")
    }
}

/// What's at a vault path, judged from the file's first bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VaultKind {
    /// No vault yet: the first launch. Opening creates a plain one.
    Missing,
    /// A plain SQLite file.
    Plain,
    /// Encrypted by Turso: unlock it with the password.
    Encrypted,
}

/// Tells whether the vault at `path` is encrypted, without opening it.
pub fn inspect(path: &Path) -> Result<VaultKind> {
    let mut start = [0u8; 16];
    let read = std::fs::File::open(path).and_then(|mut file| file.read(&mut start));
    match read {
        Err(err) if err.kind() == ErrorKind::NotFound => Ok(VaultKind::Missing),
        Err(reason) => Err(Error::ReadFile {
            path: path.to_owned(),
            reason,
        }),
        // Created but never written: the first launch was interrupted.
        Ok(0) => Ok(VaultKind::Missing),
        Ok(_) if start.starts_with(b"Turso") => Ok(VaultKind::Encrypted),
        Ok(_) => Ok(VaultKind::Plain),
    }
}

/// The key file of the vault at `vault`: `vault.key` next to `vault.db`.
pub fn key_path(vault: &Path) -> PathBuf {
    vault.with_extension("key")
}

/// Argon2id's cost parameters, as stored in the key file.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct KdfCost {
    memory_kib: u32,
    iterations: u32,
    parallelism: u32,
}

/// The contents of `vault.key`: everything needed to get the vault key back
/// from the password, and nothing that helps without it. JSON, so it can
/// be read (if not used) by hand.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct KeyFile {
    version: u32,
    /// How the vault itself is encrypted, by Turso.
    cipher: String,
    kdf: Kdf,
    wrap: Wrap,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Kdf {
    algorithm: String,
    #[serde(flatten)]
    cost: KdfCost,
    /// Random, so two vaults with the same password get different keys.
    #[serde(with = "hex")]
    salt: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Wrap {
    algorithm: String,
    /// Random, and new every time the key is wrapped.
    #[serde(with = "hex")]
    nonce: Vec<u8>,
    /// The vault key, encrypted, followed by the 16-byte tag that proves
    /// it wasn't tampered with (or unwrapped with the wrong password).
    #[serde(with = "hex")]
    key: Vec<u8>,
}

impl KeyFile {
    /// Wraps `key` under `password`, with a new salt and nonce.
    pub(crate) fn wrap(key: &VaultKey, password: &Password) -> Result<Self> {
        let mut salt = vec![0u8; 16];
        let mut nonce = [0u8; 24];
        getrandom::fill(&mut salt).map_err(Error::Random)?;
        getrandom::fill(&mut nonce).map_err(Error::Random)?;
        let wrapping = derive(password, &salt, KDF_COST)?;
        let wrapped = cipher(&wrapping)
            .encrypt(
                &XNonce::from(nonce),
                Payload {
                    msg: &key.0,
                    aad: WRAP_CONTEXT,
                },
            )
            .expect("encrypting 32 bytes in memory can't fail");
        Ok(Self {
            version: 1,
            cipher: VAULT_CIPHER.to_owned(),
            kdf: Kdf {
                algorithm: "argon2id".to_owned(),
                cost: KDF_COST,
                salt,
            },
            wrap: Wrap {
                algorithm: "xchacha20poly1305".to_owned(),
                nonce: nonce.to_vec(),
                key: wrapped,
            },
        })
    }

    /// Gets the vault key back. Fails with [`Error::WrongPassword`] if the
    /// password isn't the one it was wrapped with.
    pub(crate) fn unwrap(&self, password: &Password) -> Result<VaultKey> {
        let invalid = |reason: &str| Error::InvalidKeyFile(reason.to_owned());
        if self.version != 1 || self.cipher != VAULT_CIPHER {
            return Err(invalid("it was written by a newer version of Papa"));
        }
        if self.kdf.algorithm != "argon2id" || self.wrap.algorithm != "xchacha20poly1305" {
            return Err(invalid("unknown algorithm"));
        }
        let nonce = XNonce::try_from(self.wrap.nonce.as_slice())
            .map_err(|_| invalid("the nonce has the wrong length"))?;
        let wrapping = derive(password, &self.kdf.salt, self.kdf.cost)?;
        let mut plain = Zeroizing::new(
            cipher(&wrapping)
                .decrypt(
                    &nonce,
                    Payload {
                        msg: &self.wrap.key,
                        aad: WRAP_CONTEXT,
                    },
                )
                // The tag didn't match: almost always the wrong password.
                .map_err(|_| Error::WrongPassword)?,
        );
        let mut key = VaultKey([0; 32]);
        if plain.len() != key.0.len() {
            return Err(invalid("the key has the wrong length"));
        }
        key.0.copy_from_slice(&plain);
        plain.zeroize();
        Ok(key)
    }

    pub(crate) fn read(path: &Path) -> Result<Self> {
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(err) if err.kind() == ErrorKind::NotFound => {
                return Err(Error::KeyFileMissing(path.to_owned()));
            }
            Err(reason) => {
                return Err(Error::ReadFile {
                    path: path.to_owned(),
                    reason,
                });
            }
        };
        serde_json::from_str(&text).map_err(|err| Error::InvalidKeyFile(err.to_string()))
    }

    pub(crate) fn to_json(&self) -> String {
        let mut json =
            serde_json::to_string_pretty(self).expect("a key file is plain data and serializes");
        json.push('\n');
        json
    }

    /// Writes the key file through a temporary file and a rename, so a
    /// crash never leaves half of one.
    pub(crate) fn write(&self, path: &Path) -> Result<()> {
        crate::files::write_atomic(path, self.to_json().as_bytes())
    }
}

/// Argon2id: the password and salt in, a 256-bit wrapping key out.
fn derive(password: &Password, salt: &[u8], cost: KdfCost) -> Result<Zeroizing<[u8; 32]>> {
    let params = Params::new(cost.memory_kib, cost.iterations, cost.parallelism, Some(32))
        .map_err(|err| Error::InvalidKeyFile(err.to_string()))?;
    let mut out = Zeroizing::new([0u8; 32]);
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
        .hash_password_into(password.as_bytes(), salt, out.as_mut())
        .map_err(|err| Error::InvalidKeyFile(err.to_string()))?;
    Ok(out)
}

fn cipher(key: &[u8; 32]) -> XChaCha20Poly1305 {
    XChaCha20Poly1305::new(&(*key).into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn password(text: &str) -> Password {
        Password::new(text)
    }

    #[test]
    fn the_right_password_unwraps_the_key() {
        let key = VaultKey::generate().unwrap();
        let file = KeyFile::wrap(&key, &password("correct horse")).unwrap();

        let unwrapped = file.unwrap(&password("correct horse")).unwrap();

        assert_eq!(unwrapped.0, key.0);
        assert_ne!(file.wrap.key[..32], key.0, "the key is stored encrypted");
    }

    #[test]
    fn a_wrong_password_is_refused() {
        let key = VaultKey::generate().unwrap();
        let file = KeyFile::wrap(&key, &password("correct horse")).unwrap();

        for wrong in ["correct hors", "Correct horse", "correct horse ", ""] {
            assert!(
                matches!(file.unwrap(&password(wrong)), Err(Error::WrongPassword)),
                "{wrong:?}"
            );
        }
    }

    #[test]
    fn a_tampered_key_file_is_refused() {
        let key = VaultKey::generate().unwrap();
        let mut file = KeyFile::wrap(&key, &password("correct horse")).unwrap();
        file.wrap.key[0] ^= 1;
        assert!(matches!(
            file.unwrap(&password("correct horse")),
            Err(Error::WrongPassword)
        ));
    }

    #[test]
    fn wrapping_twice_uses_a_new_salt_and_nonce() {
        let key = VaultKey::generate().unwrap();
        let a = KeyFile::wrap(&key, &password("correct horse")).unwrap();
        let b = KeyFile::wrap(&key, &password("correct horse")).unwrap();
        assert_ne!(a.kdf.salt, b.kdf.salt);
        assert_ne!(a.wrap.nonce, b.wrap.nonce);
        assert_ne!(a.wrap.key, b.wrap.key);
    }

    #[test]
    fn the_key_file_round_trips_through_json() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("vault.key");
        let key = VaultKey::generate().unwrap();
        let file = KeyFile::wrap(&key, &password("correct horse")).unwrap();

        file.write(&path).unwrap();
        let json = std::fs::read_to_string(&path).unwrap();
        let read = KeyFile::read(&path).unwrap();

        assert_eq!(read, file);
        assert!(json.contains("\"algorithm\": \"argon2id\""), "{json}");
        assert!(!json.contains(&hex::encode(key.0)), "no key in the clear");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600, "only the owner can read it");
        }
        assert_eq!(read.unwrap(&password("correct horse")).unwrap().0, key.0);
    }

    #[test]
    fn a_missing_key_file_says_which() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("vault.key");
        assert!(matches!(KeyFile::read(&path), Err(Error::KeyFileMissing(p)) if p == path));
    }

    #[test]
    fn short_passwords_are_refused() {
        assert!(matches!(
            password("1234567").check_new(),
            Err(Error::PasswordTooShort)
        ));
        // Characters, not bytes: “é” is two bytes.
        assert!(password("ééééééé").check_new().is_err());
        assert!(password("12345678").check_new().is_ok());
    }

    #[test]
    fn the_file_header_tells_plain_from_encrypted() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("vault.db");
        assert_eq!(inspect(&path).unwrap(), VaultKind::Missing);
        std::fs::write(&path, b"").unwrap();
        assert_eq!(inspect(&path).unwrap(), VaultKind::Missing);
        std::fs::write(&path, b"SQLite format 3\0rest").unwrap();
        assert_eq!(inspect(&path).unwrap(), VaultKind::Plain);
        std::fs::write(&path, b"Turso\0\x03\0rest").unwrap();
        assert_eq!(inspect(&path).unwrap(), VaultKind::Encrypted);
    }

    #[test]
    fn the_key_file_sits_next_to_the_vault() {
        assert_eq!(
            key_path(Path::new("/data/papa/vault.db")),
            Path::new("/data/papa/vault.key")
        );
    }
}
