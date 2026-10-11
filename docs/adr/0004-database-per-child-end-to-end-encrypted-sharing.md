# 0004. One cloud database per shared child, end-to-end encrypted

- **Status:** Accepted
- **Date:** 2026-10-11

## Context

A child must see only the notes their father shared with them. Turso Cloud has no row-level security; its recommended multi-tenant pattern is one database per tenant, with tokens scoped to a single database. The content is deeply personal, and some of it is about minors.

## Decision

- Each shared child gets their own Turso Cloud database.
- Everything stored there is encrypted on the client with XChaCha20-Poly1305: note titles, bodies, images and messages. Labels never leave the father's machine.
- The child's access key is turned into two keys with Argon2id + HKDF: an `auth_key` sent to the relay, and an `enc_key` that never leaves the device.
- Child tokens are read-only on notes and insert-only on messages, using Turso's fine-grained permissions.

## Consequences

- Isolation is enforced by the database boundary, not by our own query code.
- A breach of Turso or of the relay exposes only ciphertext and metadata.
- A lost access key cannot be recovered; the father regenerates it.
- Many small databases: we have to run migrations for each one, and watch plan limits as usage grows.
