# 0007. The vault runs as an actor on its own thread

- **Status:** Accepted
- **Date:** 2026-10-12

## Context

GPUI drives the UI from its own executors, and the UI thread must never block on disk I/O. The `turso` crate is async, but its local engine does file I/O inside `poll`, and its sync feature (needed for sharing, Phase 11) expects a tokio runtime. GPUI doesn't run tokio. Options considered:

- Call `turso` directly from GPUI tasks. Simple, but blocking I/O would run on the UI thread, and sync would have no tokio runtime.
- Run a multi-threaded tokio runtime and share the connection behind a mutex. Works, but needs locking, and nothing stops two requests from interleaving between "check" and "write".
- Run one thread that owns the connection and receives requests over a channel (an actor).

## Decision

`papa-core` runs the vault as an actor:

- `Store` is a cloneable handle holding a tokio `mpsc` sender. Each method sends a `Request` enum variant carrying a `oneshot` reply channel, and awaits the reply.
- A dedicated `papa-store` OS thread runs a single-threaded tokio runtime. It owns the only `turso` connection and handles requests one at a time, in order.
- The thread exits when the last `Store` is dropped and the channel closes.
- Only the `store::db` module writes SQL. The schema is versioned with `PRAGMA user_version` and upgraded by numbered, append-only migrations, each in one transaction with its version bump.

## Consequences

- The UI awaits the store from ordinary GPUI tasks (`cx.spawn_in`); a `oneshot` receiver works on any executor.
- Requests are serialised, so check-then-write sequences (such as "is this label name taken?") can't race inside the app, without locks.
- Adding an operation means touching three places: the `Request` enum, the `Store` method and the `Db` method. The compiler flags a missing `match` arm.
- One slow query delays the ones queued behind it. For a single-user journal that's acceptable; read-heavy work could later get a second, read-only connection.
- `turso` is used with default features off, so it doesn't install mimalloc as the whole app's global allocator; full-text search is left for later too (Phase 3 deferred it; see ADR 0008).
