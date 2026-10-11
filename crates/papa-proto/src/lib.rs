//! Request and response types shared by the Papa app and `papa-relay`.
//!
//! Keeping them in one crate means the client and server can't drift apart:
//! a change to the API is a compile error on both sides.
