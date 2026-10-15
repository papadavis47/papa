//! The keys of a share, and sealing data with them.
//!
//! ```text
//!  access key ──Argon2id──▶ master ──HKDF "auth"──▶ auth_key ──▶ relay (as a hash)
//!                                  └─HKDF "enc"───▶ enc_key ─┐   never leaves the device
//!                                                            ▼
//!  share_key row: content key, wrapped (XChaCha20-Poly1305) under enc_key
//!                                                            │
//!  shared_note / shared_blob rows: sealed with the content key ◀┘
//! ```
//!
//! - **Two keys from one secret.** HKDF expands the master secret with two
//!   different labels. Knowing `auth_key`, which the relay sees, says
//!   nothing about `enc_key`: HKDF's outputs are independent.
//! - **A content key between them.** Notes are sealed with a random content
//!   key, and only that key is sealed with `enc_key`. A new access key then
//!   only needs a new wrapped key, and the father's device can seal notes
//!   without the access key at hand.
//! - **Associated data.** Every sealed value is bound to where it belongs:
//!   which share, which row, which key epoch. AEAD checks it along with the
//!   ciphertext, so a server that swaps two notes' rows, or serves an old
//!   row under a new epoch, gets a decryption error instead of the wrong
//!   note on the child's screen.

use std::fmt;

use argon2::{Algorithm, Argon2, Params, Version};
use chacha20poly1305::aead::{Aead as _, KeyInit as _, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use hkdf::Hkdf;
use papa_proto::Secret;
use sha2::Sha256;
use zeroize::{ZeroizeOnDrop, Zeroizing};

use super::AccessKey;
use crate::{Error, Result};

/// Argon2id's cost for access keys: 19 MiB and two passes, OWASP's
/// recommendation. Lower than the vault password's (64 MiB, three passes):
/// an access key is 130 random bits, so there's nothing to guess and the
/// slowness is only a second line of defence. The father's and the child's
/// devices must use the same values, so changing them needs a new version
/// of the derivation, not an edit.
const KDF_MEMORY_KIB: u32 = 19 * 1024;
const KDF_ITERATIONS: u32 = 2;

/// Argon2id needs a salt. A fixed one is fine here: a salt stops an
/// attacker from guessing many passwords with one computation, and with
/// 2¹³⁰ possible keys there's no guessing to share. A per-share salt would
/// also have to reach the child before sign-in, which nothing can carry.
const KDF_SALT: &[u8] = b"papa access key v1";

/// The key a child's app proves itself to the relay with. The relay keeps
/// only its SHA-256 (ADR 0014).
#[derive(Clone, ZeroizeOnDrop)]
pub struct AuthKey([u8; 32]);

impl AuthKey {
    /// As the relay takes it: 64 hex digits.
    pub fn to_secret(&self) -> Secret {
        Secret::new(hex::encode(self.0))
    }
}

impl fmt::Debug for AuthKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("AuthKey(…)")
    }
}

/// The key that unwraps a share's content key. It never leaves the device.
#[derive(Clone, ZeroizeOnDrop)]
pub struct EncKey([u8; 32]);

impl fmt::Debug for EncKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("EncKey(…)")
    }
}

/// The two keys of one access key.
#[derive(Clone, Debug)]
pub struct ChildKeys {
    pub auth: AuthKey,
    pub enc: EncKey,
}

impl ChildKeys {
    /// Derives the keys of `access_key`. Takes a moment, on purpose: run
    /// it off the UI thread.
    pub fn derive(access_key: &AccessKey) -> Result<Self> {
        let params = Params::new(KDF_MEMORY_KIB, KDF_ITERATIONS, 1, Some(32))
            .expect("the cost parameters are valid");
        let mut master = Zeroizing::new([0u8; 32]);
        Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
            .hash_password_into(access_key.secret(), KDF_SALT, master.as_mut())
            .expect("the salt and output lengths are valid");

        // No HKDF salt: the master secret is already uniformly random.
        let hkdf = Hkdf::<Sha256>::new(None, master.as_ref());
        let mut keys = Self {
            auth: AuthKey([0; 32]),
            enc: EncKey([0; 32]),
        };
        hkdf.expand(b"papa auth key v1", &mut keys.auth.0)
            .expect("32 bytes is a valid HKDF-SHA256 length");
        hkdf.expand(b"papa encryption key v1", &mut keys.enc.0)
            .expect("32 bytes is a valid HKDF-SHA256 length");
        Ok(keys)
    }
}

/// Encrypted bytes and the nonce they were sealed with. The ciphertext
/// ends with the 16-byte tag that authenticates it.
#[derive(Clone, PartialEq, Eq)]
pub struct Sealed {
    pub nonce: [u8; 24],
    pub ciphertext: Vec<u8>,
}

impl Sealed {
    /// Reads a nonce stored as a BLOB.
    pub fn from_parts(nonce: &[u8], ciphertext: Vec<u8>) -> Result<Self> {
        let nonce = nonce
            .try_into()
            .map_err(|_| Error::Decrypt("a nonce of the wrong length"))?;
        Ok(Self { nonce, ciphertext })
    }
}

impl fmt::Debug for Sealed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Sealed(<{} bytes>)", self.ciphertext.len())
    }
}

/// Seals `plain` under `key`, bound to `aad`, with a new random nonce.
///
/// XChaCha20's nonce is 192 bits, so random nonces never repeat in
/// practice. (ChaCha20's 96 bits would start to risk it after a few billion
/// messages under one key.)
fn seal(key: &[u8; 32], aad: &[u8], plain: &[u8]) -> Result<Sealed> {
    let mut nonce = [0u8; 24];
    getrandom::fill(&mut nonce).map_err(Error::Random)?;
    let ciphertext = XChaCha20Poly1305::new(&(*key).into())
        .encrypt(&XNonce::from(nonce), Payload { msg: plain, aad })
        .expect("encrypting in memory can't fail");
    Ok(Sealed { nonce, ciphertext })
}

/// Opens what [`seal`] sealed. Fails if the key or `aad` differ, or a
/// single bit of the ciphertext changed.
fn open(key: &[u8; 32], aad: &[u8], sealed: &Sealed, what: &'static str) -> Result<Vec<u8>> {
    XChaCha20Poly1305::new(&(*key).into())
        .decrypt(
            &XNonce::from(sealed.nonce),
            Payload {
                msg: &sealed.ciphertext,
                aad,
            },
        )
        .map_err(|_| Error::Decrypt(what))
}

/// Where a sealed value belongs. Its bytes are the AEAD's associated data:
/// not secret and not stored, but checked when opening.
#[derive(Clone, Copy, Debug)]
pub enum Aad<'a> {
    /// The share's content key, wrapped under `enc_key`.
    ContentKey { share: &'a str, epoch: i64 },
    /// A note's text.
    Note {
        share: &'a str,
        note: &'a str,
        epoch: i64,
    },
    /// One version of an image (`kind`: "thumb" or "display").
    Blob {
        share: &'a str,
        image: &'a str,
        kind: &'a str,
        epoch: i64,
    },
}

impl Aad<'_> {
    fn bytes(self) -> Vec<u8> {
        // Ids are UUIDs and kinds are fixed words, so none holds a newline
        // and the fields can't run into each other.
        match self {
            Self::ContentKey { share, epoch } => {
                format!("papa share v1\ncontent key\n{share}\n{epoch}")
            }
            Self::Note { share, note, epoch } => {
                format!("papa share v1\nnote\n{share}\n{note}\n{epoch}")
            }
            Self::Blob {
                share,
                image,
                kind,
                epoch,
            } => format!("papa share v1\nblob\n{share}\n{image}\n{kind}\n{epoch}"),
        }
        .into_bytes()
    }
}

/// The random key a share's notes and images are sealed with. A new
/// access key brings a new content key too, so the old access key can't
/// read anything written after it was replaced.
#[derive(Clone, ZeroizeOnDrop)]
pub struct ContentKey([u8; 32]);

impl ContentKey {
    pub fn generate() -> Result<Self> {
        let mut key = Self([0; 32]);
        getrandom::fill(&mut key.0).map_err(Error::Random)?;
        Ok(key)
    }

    /// The key as stored in the father's vault.
    pub(crate) fn from_bytes(bytes: &[u8]) -> Result<Self> {
        let bytes = bytes.try_into().map_err(|_| Error::Corrupt {
            what: "content key",
            value: format!("{} bytes", bytes.len()),
        })?;
        Ok(Self(bytes))
    }

    pub(crate) fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    /// Seals this key under `enc`, for the share's `share_key` row.
    pub fn wrap(&self, enc: &EncKey, share: &str, epoch: i64) -> Result<Sealed> {
        seal(&enc.0, &Aad::ContentKey { share, epoch }.bytes(), &self.0)
    }

    /// Gets the content key back from a `share_key` row. Fails with
    /// [`Error::Decrypt`] for the wrong access key.
    pub fn unwrap(sealed: &Sealed, enc: &EncKey, share: &str, epoch: i64) -> Result<Self> {
        let aad = Aad::ContentKey { share, epoch }.bytes();
        let plain = Zeroizing::new(open(&enc.0, &aad, sealed, "the share's key")?);
        Self::from_bytes(&plain).map_err(|_| Error::Decrypt("the share's key"))
    }

    pub fn seal(&self, aad: Aad<'_>, plain: &[u8]) -> Result<Sealed> {
        seal(&self.0, &aad.bytes(), plain)
    }

    /// Opens a note or image. `what` names it in the error.
    pub fn open(&self, aad: Aad<'_>, sealed: &Sealed, what: &'static str) -> Result<Vec<u8>> {
        open(&self.0, &aad.bytes(), sealed, what)
    }
}

impl fmt::Debug for ContentKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ContentKey(…)")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHARE: &str = "0199d3c4-0000-7000-8000-000000000001";

    fn note(note: &str, epoch: i64) -> Aad<'_> {
        Aad::Note {
            share: SHARE,
            note,
            epoch,
        }
    }

    #[test]
    fn the_same_access_key_always_gives_the_same_keys() {
        let access = AccessKey::generate().unwrap();
        let a = ChildKeys::derive(&access).unwrap();
        let b = ChildKeys::derive(&access.to_string().parse().unwrap()).unwrap();

        assert_eq!(a.auth.0, b.auth.0);
        assert_eq!(a.enc.0, b.enc.0);
    }

    /// Pins the derivation. If it ever changes, every access key already
    /// handed out stops working: this test failing means "don't".
    ///
    /// The expected values come from Python's `argon2-cffi` and the
    /// standard library's HMAC (HKDF written out by hand), not from this
    /// code.
    #[test]
    fn the_derivation_never_changes() {
        let access = AccessKey::from_secret(b"7K3QM9XD0123456789ABCDEFGH");
        let keys = ChildKeys::derive(&access).unwrap();
        assert_eq!(
            hex::encode(keys.auth.0),
            "92d6373d42d5c2ab6906610b7f886cd65c449989b0e7b4b0c27c0faf5a519cdc"
        );
        assert_eq!(
            hex::encode(keys.enc.0),
            "70d2e14892b9b50badbfea0de52a2b950ec9ab6d039b8e813a81bd10f57355af"
        );
    }

    #[test]
    fn the_two_keys_differ_and_so_do_other_access_keys() {
        let a = ChildKeys::derive(&AccessKey::generate().unwrap()).unwrap();
        let b = ChildKeys::derive(&AccessKey::generate().unwrap()).unwrap();

        assert_ne!(a.auth.0, a.enc.0, "auth and enc keys are independent");
        assert_ne!(a.auth.0, b.auth.0);
        assert_ne!(a.enc.0, b.enc.0);
        assert_eq!(a.auth.to_secret().expose().len(), 64);
    }

    #[test]
    fn a_wrapped_content_key_opens_only_with_the_right_access_key() {
        let keys = ChildKeys::derive(&AccessKey::generate().unwrap()).unwrap();
        let other = ChildKeys::derive(&AccessKey::generate().unwrap()).unwrap();
        let content = ContentKey::generate().unwrap();
        let wrapped = content.wrap(&keys.enc, SHARE, 1).unwrap();

        let unwrapped = ContentKey::unwrap(&wrapped, &keys.enc, SHARE, 1).unwrap();
        assert_eq!(unwrapped.0, content.0);

        assert!(matches!(
            ContentKey::unwrap(&wrapped, &other.enc, SHARE, 1),
            Err(Error::Decrypt(_))
        ));
        // Presented as another epoch's, or another share's: refused too.
        assert!(ContentKey::unwrap(&wrapped, &keys.enc, SHARE, 2).is_err());
        assert!(ContentKey::unwrap(&wrapped, &keys.enc, "another share", 1).is_err());
    }

    #[test]
    fn a_sealed_note_opens_only_at_its_own_address() {
        let key = ContentKey::generate().unwrap();
        let sealed = key.seal(note("note-a", 1), b"First bike ride").unwrap();

        assert_eq!(
            key.open(note("note-a", 1), &sealed, "a note").unwrap(),
            b"First bike ride"
        );
        assert_ne!(sealed.ciphertext[..15], b"First bike ride"[..]);
        // Swapped into another note's row, or replayed under a new epoch.
        assert!(key.open(note("note-b", 1), &sealed, "a note").is_err());
        assert!(key.open(note("note-a", 2), &sealed, "a note").is_err());
        let blob = Aad::Blob {
            share: SHARE,
            image: "note-a",
            kind: "thumb",
            epoch: 1,
        };
        assert!(key.open(blob, &sealed, "an image").is_err());
    }

    #[test]
    fn tampering_and_other_keys_are_detected() {
        let key = ContentKey::generate().unwrap();
        let sealed = key.seal(note("n", 1), b"School play").unwrap();

        let mut flipped = sealed.clone();
        flipped.ciphertext[0] ^= 1;
        assert!(key.open(note("n", 1), &flipped, "a note").is_err());

        let mut renonced = sealed.clone();
        renonced.nonce[0] ^= 1;
        assert!(key.open(note("n", 1), &renonced, "a note").is_err());

        let other = ContentKey::generate().unwrap();
        assert!(other.open(note("n", 1), &sealed, "a note").is_err());
    }

    #[test]
    fn sealing_twice_uses_a_new_nonce() {
        let key = ContentKey::generate().unwrap();
        let a = key.seal(note("n", 1), b"same").unwrap();
        let b = key.seal(note("n", 1), b"same").unwrap();
        assert_ne!(a.nonce, b.nonce);
        assert_ne!(a.ciphertext, b.ciphertext);
    }

    #[test]
    fn keys_never_debug_print() {
        let keys = ChildKeys::derive(&AccessKey::generate().unwrap()).unwrap();
        let debug = format!("{keys:?} {:?}", ContentKey::generate().unwrap());
        assert_eq!(
            debug,
            "ChildKeys { auth: AuthKey(…), enc: EncKey(…) } ContentKey(…)"
        );
    }
}
