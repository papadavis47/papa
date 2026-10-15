//! Picks the TLS crypto library for the whole process.
//!
//! Two HTTP clients run in Papa: `reqwest` for the relay, and the one
//! inside Turso's sync. Both use `rustls`, which leaves the actual
//! cryptography to a "provider", and two are built into the workspace:
//! `aws-lc-rs` (which Turso's sync asks for) and `ring` (which the relay's
//! own HTTP client asks for). With two to choose from, rustls refuses to
//! guess, and the first connection panics. Installing one as the process
//! default settles it.

use std::sync::Once;

/// Installs `aws-lc-rs` as the process's TLS provider. Safe to call any
/// number of times, from any thread.
pub(crate) fn init() {
    static INIT: Once = Once::new();
    INIT.call_once(|| {
        // Fails only if a provider is already installed, which is just
        // as good.
        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    });
}
