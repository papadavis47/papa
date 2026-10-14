//! Settings, all from environment variables (`.env.example` lists them).
//! Secrets never come from files in the repository or from flags, which
//! would show up in `ps`.

use std::env;
use std::net::SocketAddr;
use std::path::PathBuf;

use papa_proto::Secret;

use crate::provision::{LocalSync, Provisioner, TursoCloud};

#[derive(Debug)]
pub struct Config {
    /// `PAPA_RELAY_ADDR`. Default `127.0.0.1:8787`.
    pub addr: SocketAddr,
    /// `PAPA_RELAY_DB`, the relay's own database. Default `relay.db`.
    pub db_path: PathBuf,
    /// `PAPA_RELAY_AUTH_PER_MINUTE`: sign-ups, log-ins and child sign-ins
    /// allowed per client address per minute. Default 10.
    pub auth_per_minute: u32,
    /// `PAPA_RELAY_CLIENT_IP_HEADER`: behind a proxy, the header holding the
    /// client's address (e.g. `Fly-Client-IP`). Unset: the TCP peer.
    pub client_ip_header: Option<String>,
    pub backend: Backend,
}

/// Where share databases live: `PAPA_RELAY_MODE=local` (the default) or
/// `cloud`.
#[derive(Debug)]
pub enum Backend {
    Local {
        /// `PAPA_RELAY_LOCAL_SYNC_URL`. Default `http://127.0.0.1:8080`.
        sync_url: String,
        /// `PAPA_RELAY_LOCAL_SYNC_DIR`: the server's `--sync-dir`, so that
        /// revoking a share can delete its files. Unset: they stay.
        sync_dir: Option<PathBuf>,
    },
    Cloud {
        /// `TURSO_API_URL`. Default `https://api.turso.tech`.
        api_url: String,
        /// `TURSO_ORG`: the organization's slug.
        org: String,
        /// `TURSO_GROUP`. Default `default`.
        group: String,
        /// `TURSO_PLATFORM_TOKEN`: a Platform API token for the org.
        token: Secret,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("{name} is required when PAPA_RELAY_MODE=cloud")]
    Missing { name: &'static str },
    #[error("{name}={value:?} is not valid: {reason}")]
    Invalid {
        name: &'static str,
        value: String,
        reason: String,
    },
}

impl Config {
    pub fn from_env() -> Result<Self, ConfigError> {
        Self::from_lookup(|name| env::var(name).ok().filter(|v| !v.is_empty()))
    }

    /// Reads settings through `lookup`, so tests don't touch the process
    /// environment (which every test thread shares).
    pub fn from_lookup(lookup: impl Fn(&str) -> Option<String>) -> Result<Self, ConfigError> {
        let addr = parse(&lookup, "PAPA_RELAY_ADDR", "127.0.0.1:8787")?;
        let auth_per_minute = parse(&lookup, "PAPA_RELAY_AUTH_PER_MINUTE", "10")?;
        let mode = lookup("PAPA_RELAY_MODE").unwrap_or_else(|| "local".into());
        let backend = match mode.as_str() {
            "local" => Backend::Local {
                sync_url: lookup("PAPA_RELAY_LOCAL_SYNC_URL")
                    .unwrap_or_else(|| "http://127.0.0.1:8080".into()),
                sync_dir: lookup("PAPA_RELAY_LOCAL_SYNC_DIR").map(PathBuf::from),
            },
            "cloud" => {
                let required = |name| lookup(name).ok_or(ConfigError::Missing { name });
                Backend::Cloud {
                    api_url: lookup("TURSO_API_URL")
                        .unwrap_or_else(|| "https://api.turso.tech".into()),
                    org: required("TURSO_ORG")?,
                    group: lookup("TURSO_GROUP").unwrap_or_else(|| "default".into()),
                    token: Secret::new(required("TURSO_PLATFORM_TOKEN")?),
                }
            }
            _ => {
                return Err(ConfigError::Invalid {
                    name: "PAPA_RELAY_MODE",
                    value: mode,
                    reason: "expected `local` or `cloud`".into(),
                });
            }
        };
        Ok(Self {
            addr,
            db_path: lookup("PAPA_RELAY_DB")
                .unwrap_or_else(|| "relay.db".into())
                .into(),
            auth_per_minute,
            client_ip_header: lookup("PAPA_RELAY_CLIENT_IP_HEADER"),
            backend,
        })
    }

    pub fn provisioner(&self) -> Provisioner {
        match &self.backend {
            Backend::Local { sync_url, sync_dir } => {
                Provisioner::Local(LocalSync::new(sync_url, sync_dir.clone()))
            }
            Backend::Cloud {
                api_url,
                org,
                group,
                token,
            } => Provisioner::Cloud(TursoCloud::new(api_url, org, group, token.clone())),
        }
    }
}

fn parse<T>(
    lookup: &impl Fn(&str) -> Option<String>,
    name: &'static str,
    default: &str,
) -> Result<T, ConfigError>
where
    T: std::str::FromStr,
    T::Err: std::fmt::Display,
{
    let value = lookup(name).unwrap_or_else(|| default.into());
    value.parse().map_err(|e: T::Err| ConfigError::Invalid {
        name,
        reason: e.to_string(),
        value,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(vars: &[(&str, &str)]) -> Result<Config, ConfigError> {
        Config::from_lookup(|name| {
            vars.iter()
                .find(|(n, _)| *n == name)
                .map(|(_, v)| v.to_string())
        })
    }

    #[test]
    fn cloud_mode_needs_the_org_and_platform_token() {
        let error = config(&[("PAPA_RELAY_MODE", "cloud"), ("TURSO_ORG", "papa")]).unwrap_err();
        assert!(
            error.to_string().contains("TURSO_PLATFORM_TOKEN"),
            "{error}"
        );

        let config = config(&[
            ("PAPA_RELAY_MODE", "cloud"),
            ("TURSO_ORG", "papa"),
            ("TURSO_PLATFORM_TOKEN", "platform-secret"),
        ])
        .unwrap();
        assert!(matches!(config.backend, Backend::Cloud { ref group, .. } if group == "default"));
        assert!(!format!("{config:?}").contains("platform-secret"));
    }

    #[test]
    fn unknown_mode_and_bad_numbers_are_refused() {
        assert!(config(&[("PAPA_RELAY_MODE", "Cloud")]).is_err());
        assert!(config(&[("PAPA_RELAY_AUTH_PER_MINUTE", "ten")]).is_err());
    }
}
