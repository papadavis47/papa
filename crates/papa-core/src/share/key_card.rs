//! What a father hands his child: the access key, and where to use it.
//!
//! ```text
//!  PAPA-7K3Q-M9XD-…-ABCD                       on Papa's own server
//!  PAPA-7K3Q-M9XD-…-ABCD@papa.example.org      on another one (https)
//!  PAPA-7K3Q-M9XD-…-ABCD@http://127.0.0.1:9000 on a plain-http one
//! ```
//!
//! The server travels with the key so that a child never types one
//! (PLAN.md §2.2). Papa's own server is left out, so the key most children
//! get is just the key. Only the secret symbols go into the keys derived
//! from it ([`ChildKeys`](super::ChildKeys)), so adding the server changed
//! nothing for keys already handed out.

use std::fmt;
use std::str::FromStr;

use super::AccessKey;
use crate::relay::{self, DEFAULT_RELAY_URL};
use crate::{Error, Result};

/// An access key and the relay it works on.
#[derive(Clone, PartialEq, Eq)]
pub struct KeyCard {
    pub access_key: AccessKey,
    /// The relay's address, as [`relay::clean_url`] leaves it.
    pub relay_url: String,
}

impl KeyCard {
    pub fn new(access_key: AccessKey, relay_url: &str) -> Self {
        Self {
            access_key,
            relay_url: relay_url.trim().trim_end_matches('/').to_owned(),
        }
    }
}

/// The key, then `@` and the server unless it's Papa's own. `https://` is
/// what a server is assumed to speak, so it's left out too.
impl fmt::Display for KeyCard {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.access_key.fmt(f)?;
        if self.relay_url != DEFAULT_RELAY_URL {
            let server = self
                .relay_url
                .strip_prefix("https://")
                .unwrap_or(&self.relay_url);
            write!(f, "@{server}")?;
        }
        Ok(())
    }
}

impl fmt::Debug for KeyCard {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "KeyCard({:?}, {})", self.access_key, self.relay_url)
    }
}

impl FromStr for KeyCard {
    type Err = Error;

    /// Reads a key as pasted: the key forgives what [`AccessKey`] forgives,
    /// and spaces around the `@` don't matter.
    fn from_str(text: &str) -> Result<Self> {
        let (key, server) = match text.split_once('@') {
            Some((key, server)) => (key, Some(server.trim())),
            None => (text, None),
        };
        let access_key = key.parse()?;
        let relay_url = match server {
            None => DEFAULT_RELAY_URL.to_owned(),
            Some(server) if server.starts_with("http://") || server.starts_with("https://") => {
                relay::clean_url(server)?
            }
            Some(server) => relay::clean_url(&format!("https://{server}"))?,
        };
        Ok(Self {
            access_key,
            relay_url,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key() -> AccessKey {
        AccessKey::from_secret(b"7K3QM9XD0123456789ABCDEFGH")
    }

    #[test]
    fn papas_own_server_is_left_out() {
        let card = KeyCard::new(key(), DEFAULT_RELAY_URL);
        assert_eq!(card.to_string(), key().to_string());

        let read: KeyCard = key().to_string().parse().unwrap();
        assert_eq!(read, card);
    }

    #[test]
    fn another_server_follows_the_key() {
        let https = KeyCard::new(key(), "https://papa.example.org/");
        assert_eq!(https.to_string(), format!("{}@papa.example.org", key()));

        let http = KeyCard::new(key(), "http://192.168.1.20:8787");
        assert_eq!(
            http.to_string(),
            format!("{}@http://192.168.1.20:8787", key())
        );

        for card in [https, http] {
            assert_eq!(card.to_string().parse::<KeyCard>().unwrap(), card);
        }
    }

    #[test]
    fn reading_forgives_spacing_and_case_around_the_server() {
        let text = format!(
            "  {} @ papa.example.org/\n",
            key().to_string().to_lowercase()
        );
        let card: KeyCard = text.parse().unwrap();
        assert_eq!(card.access_key, key());
        assert_eq!(card.relay_url, "https://papa.example.org");
    }

    #[test]
    fn a_bad_key_or_server_is_refused() {
        let key = key().to_string();
        assert!(matches!(
            format!("{}@papa.example.org", &key[..key.len() - 1]).parse::<KeyCard>(),
            Err(Error::InvalidAccessKey)
        ));
        for server in ["", "pa pa.example", "ftp://papa.example"] {
            assert!(
                matches!(
                    format!("{key}@{server}").parse::<KeyCard>(),
                    Err(Error::InvalidRelayUrl)
                ),
                "{server:?}"
            );
        }
    }

    #[test]
    fn debug_never_prints_the_key() {
        let card = KeyCard::new(key(), "https://papa.example.org");
        assert_eq!(
            format!("{card:?}"),
            "KeyCard(AccessKey(…), https://papa.example.org)"
        );
    }
}
