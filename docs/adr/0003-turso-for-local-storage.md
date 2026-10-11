# 0003. Turso (`turso` crate) for local storage

- **Status:** Accepted
- **Date:** 2026-10-11

## Context

The father's vault must work offline, be fast, and live in a single file. Sharing later needs sync with a cloud database. Options considered: SQLite via `rusqlite`, `libsql` (Turso's SQLite fork, now receiving only maintenance work), and `turso` (Turso's Rust rewrite of SQLite).

## Decision

Use the `turso` crate, pinned to an exact version.

- It is pure Rust (no C toolchain) with an async API.
- It has built-in sync with Turso Cloud, which the sharing design relies on.
- It offers encryption at rest and full-text search, both experimental today.

## Consequences

- `turso` is pre-1.0, so expect API changes and some rough edges. All access goes through a `Store` type in `papa-core`, which makes swapping engines a one-module change.
- Experimental features (encryption, FTS) sit behind settings with plain fallbacks.
- "Export backup" is required, as Turso itself advises keeping independent backups before 1.0.
- Supabase announced in October 2026 that it is acquiring Turso. Plans or pricing may change, which is another reason for the `Store` abstraction.
