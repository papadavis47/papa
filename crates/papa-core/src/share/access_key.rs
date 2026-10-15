//! The access key a father hands his child: `PAPA-7K3Q-M9XD-…`.
//!
//! 26 random symbols carry the secret, 130 bits of it. Two more are a
//! checksum, so a mistyped key is caught on the child's device before it
//! costs a round trip to the relay. The symbols are Crockford's base 32:
//! digits and capitals without `I`, `L`, `O` and `U`, so nothing looks like
//! anything else. Reading a key back forgives case, spaces, dashes, and
//! `O`/`I`/`L` typed for `0`/`1`/`1`.

use std::fmt;
use std::str::FromStr;

use sha2::{Digest as _, Sha256};
use zeroize::{Zeroize as _, ZeroizeOnDrop};

use crate::{Error, Result};

/// Crockford's base 32, in order: symbol `n` stands for the value `n`.
const ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

/// Symbols of secret: 26 × 5 bits = 130 bits.
const SECRET_SYMBOLS: usize = 26;
/// Symbols of checksum: 10 bits, so one typo in 1024 slips through.
const CHECK_SYMBOLS: usize = 2;
const SYMBOLS: usize = SECRET_SYMBOLS + CHECK_SYMBOLS;

/// What every key starts with, so it's recognisable when pasted.
const PREFIX: &str = "PAPA";

/// A child's access key. Its symbols are wiped from memory when it's
/// dropped, and `Debug` doesn't print them.
#[derive(Clone, PartialEq, Eq, ZeroizeOnDrop)]
pub struct AccessKey([u8; SYMBOLS]);

impl AccessKey {
    /// A new random key.
    pub fn generate() -> Result<Self> {
        let mut random = [0u8; SECRET_SYMBOLS];
        getrandom::fill(&mut random).map_err(Error::Random)?;
        let mut symbols = [0u8; SYMBOLS];
        for (symbol, byte) in symbols.iter_mut().zip(&random) {
            // 256 is a multiple of 32, so keeping the low 5 bits of a
            // random byte gives every symbol the same chance.
            *symbol = ALPHABET[usize::from(byte & 31)];
        }
        random.zeroize();
        let check = checksum(&symbols[..SECRET_SYMBOLS]);
        symbols[SECRET_SYMBOLS..].copy_from_slice(&check);
        Ok(Self(symbols))
    }

    /// A key with these 26 secret symbols, for tests that need the same
    /// key every run.
    #[cfg(test)]
    pub(crate) fn from_secret(secret: &[u8; SECRET_SYMBOLS]) -> Self {
        let mut symbols = [0u8; SYMBOLS];
        symbols[..SECRET_SYMBOLS].copy_from_slice(secret);
        let check = checksum(secret);
        symbols[SECRET_SYMBOLS..].copy_from_slice(&check);
        Self(symbols)
    }

    /// The secret symbols, without the checksum: what the keys are derived
    /// from (see [`ChildKeys`](super::ChildKeys)).
    pub(crate) fn secret(&self) -> &[u8] {
        &self.0[..SECRET_SYMBOLS]
    }
}

/// The two checksum symbols of `secret`: the first 10 bits of its SHA-256.
fn checksum(secret: &[u8]) -> [u8; CHECK_SYMBOLS] {
    let hash = Sha256::new()
        .chain_update(b"papa access key check v1\n")
        .chain_update(secret)
        .finalize();
    let bits = u16::from_be_bytes([hash[0], hash[1]]) >> 6;
    [
        ALPHABET[usize::from(bits >> 5)],
        ALPHABET[usize::from(bits & 31)],
    ]
}

/// `PAPA-XXXX-XXXX-XXXX-XXXX-XXXX-XXXX-XXXX`: seven groups of four.
impl fmt::Display for AccessKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(PREFIX)?;
        for group in self.0.chunks(4) {
            f.write_str("-")?;
            // Every symbol comes from `ALPHABET`, which is ASCII.
            f.write_str(std::str::from_utf8(group).map_err(|_| fmt::Error)?)?;
        }
        Ok(())
    }
}

impl fmt::Debug for AccessKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("AccessKey(…)")
    }
}

impl FromStr for AccessKey {
    type Err = Error;

    fn from_str(text: &str) -> Result<Self> {
        let mut symbols = Vec::with_capacity(SYMBOLS + PREFIX.len());
        for c in text.chars() {
            let symbol = match c.to_ascii_uppercase() {
                '-' | ' ' | '\t' | '\n' | '\r' => continue,
                'O' => b'0',
                'I' | 'L' => b'1',
                c if c.is_ascii() && ALPHABET.contains(&(c as u8)) => c as u8,
                _ => {
                    symbols.zeroize();
                    return Err(Error::InvalidAccessKey);
                }
            };
            symbols.push(symbol);
        }
        // "PAPA" is made of valid symbols, so it's told apart by length.
        let start =
            if symbols.len() == SYMBOLS + PREFIX.len() && symbols.starts_with(PREFIX.as_bytes()) {
                PREFIX.len()
            } else {
                0
            };
        let result = match <[u8; SYMBOLS]>::try_from(&symbols[start..]) {
            Ok(key) if checksum(&key[..SECRET_SYMBOLS]) == key[SECRET_SYMBOLS..] => Ok(Self(key)),
            _ => Err(Error::InvalidAccessKey),
        };
        symbols.zeroize();
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_key_reads_back_as_itself() {
        let key = AccessKey::generate().unwrap();
        let text = key.to_string();

        assert_eq!(text.len(), "PAPA".len() + 7 * 5, "{text}");
        assert!(text.starts_with("PAPA-"));
        assert_eq!(text.parse::<AccessKey>().unwrap(), key);
    }

    #[test]
    fn keys_are_random() {
        let a = AccessKey::generate().unwrap();
        let b = AccessKey::generate().unwrap();
        assert_ne!(a.secret(), b.secret());
    }

    #[test]
    fn reading_forgives_case_spacing_and_lookalikes() {
        let key = AccessKey::generate().unwrap();
        let text = key.to_string();

        let lower = text.to_lowercase();
        let spaced = text.replace('-', " ");
        let bare = text.trim_start_matches("PAPA-").replace('-', "");
        // Typing O for 0 and l for 1 still works.
        let lookalikes = text.replace('0', "O").replace('1', "l");
        for variant in [lower, spaced, bare, lookalikes, format!("  {text}\n")] {
            assert_eq!(variant.parse::<AccessKey>().unwrap(), key, "{variant}");
        }
    }

    #[test]
    fn a_typo_is_caught_by_the_checksum() {
        // A fixed key: by chance, one typo in 1024 slips through, and a
        // random key would make this test fail once in a few thousand runs.
        let key = AccessKey::from_secret(b"7K3QM9XD0123456789ABCDEFGH");
        let text = key.to_string();
        let mut missed = Vec::new();
        // Change each symbol of the secret in turn to another one.
        for (i, c) in text.char_indices().filter(|(i, _)| *i >= 5) {
            if c == '-' {
                continue;
            }
            let other = if c == 'Z' { 'Y' } else { 'Z' };
            let typo = format!("{}{other}{}", &text[..i], &text[i + 1..]);
            if typo.parse::<AccessKey>().is_ok() {
                missed.push(typo);
            }
        }
        assert_eq!(missed, Vec::<String>::new());
    }

    #[test]
    fn malformed_keys_are_refused() {
        let key = AccessKey::generate().unwrap().to_string();
        for bad in [
            "",
            "PAPA",
            &key[..key.len() - 1],
            &format!("{key}0"),
            &key.replace('-', "_"),
            // U isn't in the alphabet.
            &format!("{}U", &key[..key.len() - 1]),
        ] {
            assert!(
                matches!(bad.parse::<AccessKey>(), Err(Error::InvalidAccessKey)),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn debug_never_prints_the_key() {
        let key = AccessKey::generate().unwrap();
        let debug = format!("{key:?}");
        assert_eq!(debug, "AccessKey(…)");
    }
}
