# Architecture

This is the living overview of how Papa fits together. [PLAN.md](../PLAN.md) has the full design and roadmap. The *why* behind each major choice is in the [decision records](adr/).

## Crates

```
papa (desktop app, gpui-kit) ──depends on──▶ papa-core ──▶ papa-proto
                             └─depends on──▶ papa-pdf
papa-relay (server, axum)    ──depends on──▶ papa-proto
```

- **papa** is the only crate that depends on `gpui-kit`. It holds the views, actions, keybindings, themes and Vim layer.
- **papa-core** has no UI code. It owns the models, the `Store` (local Turso database and migrations), crypto, the share projection and the settings file. It is unit-tested without a window.
- **papa-proto** holds the serde types for the relay API. The app and the server compile against the same definitions.
- **papa-pdf** turns Markdown into PDF. It is kept separate because the PDF toolchain is heavy to compile.
- **papa-relay** is a small HTTP service: father accounts, invite codes, creating one cloud database per shared child, and issuing short-lived, database-scoped tokens.

## Data flow (planned)

1. The father's vault is a local Turso database. It is the source of truth and works offline.
2. When a note is shared with a child, it is encrypted on the father's machine. It is then written to a local replica of that child's share database and pushed to Turso Cloud.
3. The child's app signs in through the relay (father's email + access key). It receives a read-only token scoped to its own database, pulls, and decrypts locally.
4. Messages are append-only encrypted rows in the same share database. Both sides push and pull them.

## Threading (planned)

GPUI runs its own executors. The `turso` crate is async and its sync feature expects tokio. `papa-core` therefore runs the database on a dedicated tokio runtime thread, an actor. The UI sends it commands over a channel and awaits replies in GPUI tasks, so the UI thread never blocks on I/O.

## Security model (planned)

- Optional encryption at rest for the local vault, keyed from the father's password (Argon2id).
- End-to-end encryption for everything in the cloud (XChaCha20-Poly1305). The relay and Turso only see ciphertext, ids and timestamps.
- Tokens are short-lived and scoped to a single database. No admin or Platform credentials ship in the app.
