//! A per-address limit on sign-up, log-in and child sign-in, so passwords,
//! invite codes and child keys can't be guessed at speed.
//!
//! A fixed one-minute window per client address: simple, and plenty for a
//! relay of this size. The counts live in memory, so a restart resets them.

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use axum::extract::{ConnectInfo, Request, State};
use axum::middleware::Next;
use axum::response::Response;

use crate::AppState;
use crate::error::RelayError;

const WINDOW: Duration = Duration::from_secs(60);

/// Beyond this many addresses, finished windows are swept out.
const SWEEP_ABOVE: usize = 10_000;

pub struct RateLimiter {
    per_window: u32,
    /// A plain `std` mutex: it's never held across an `.await`, and it's
    /// cheaper than tokio's.
    windows: Mutex<HashMap<IpAddr, Window>>,
}

struct Window {
    started: Instant,
    count: u32,
}

impl RateLimiter {
    pub fn new(per_minute: u32) -> Self {
        Self {
            per_window: per_minute,
            windows: Mutex::new(HashMap::new()),
        }
    }

    /// Counts an attempt from `ip`; `false` once it's over the limit.
    pub fn allow(&self, ip: IpAddr, now: Instant) -> bool {
        let mut windows = self.windows.lock().unwrap_or_else(|e| e.into_inner());
        if windows.len() > SWEEP_ABOVE {
            windows.retain(|_, w| now.duration_since(w.started) < WINDOW);
        }
        let window = windows.entry(ip).or_insert(Window {
            started: now,
            count: 0,
        });
        if now.duration_since(window.started) >= WINDOW {
            *window = Window {
                started: now,
                count: 0,
            };
        }
        window.count += 1;
        window.count <= self.per_window
    }
}

/// Middleware for the routes that take a password or key.
pub async fn limit(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    request: Request,
    next: Next,
) -> Result<Response, RelayError> {
    let ip = state
        .client_ip_header
        .as_deref()
        .and_then(|name| request.headers().get(name))
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.trim().parse().ok())
        .unwrap_or(peer.ip());
    if state.rate_limiter.allow(ip, Instant::now()) {
        Ok(next.run(request).await)
    } else {
        tracing::warn!(%ip, "rate limited");
        Err(RelayError::RateLimited)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_address_gets_its_own_window() {
        let limiter = RateLimiter::new(2);
        let a: IpAddr = "192.0.2.1".parse().unwrap();
        let b: IpAddr = "192.0.2.2".parse().unwrap();
        let start = Instant::now();

        assert!(limiter.allow(a, start));
        assert!(limiter.allow(a, start));
        assert!(!limiter.allow(a, start + Duration::from_secs(59)));
        assert!(limiter.allow(b, start), "another address isn't affected");
        assert!(
            limiter.allow(a, start + WINDOW),
            "a new minute starts afresh"
        );
    }
}
