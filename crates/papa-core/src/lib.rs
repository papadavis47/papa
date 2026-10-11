//! Domain logic for Papa, independent of any UI.
//!
//! Planned contents (see `PLAN.md`):
//! - models: children, notes, labels, attachments, messages
//! - `Store`: the local vault on the `turso` crate, with migrations
//! - crypto: key derivation and end-to-end encryption for shared notes
//! - sharing: projecting shared notes into per-child sync replicas
//! - settings: the per-machine settings file
