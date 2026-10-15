# 0015. Sharing: access keys, sealed rows, and a projection that is safe to repeat

- **Status:** Accepted
- **Date:** 2026-10-15

## Context

Phase 11 builds the father's side of sharing (ADR 0004): a key for each child, an encrypted copy of the shared notes in that child's cloud database, and pushing it with Turso's sync. Forces at play:

- **The child signs in with Dad's email and a key, nothing else.** Whatever the child's app derives from the key, it has to derive before it has talked to anyone: no salt or setting can come from the server first.
- **The server must learn nothing it can use.** The relay sees the `auth_key`'s hash (ADR 0014). Turso Cloud sees the share database. Neither may read a note, a label, or which note is which.
- **Pushes fail.** Laptops sleep, networks drop, the app quits mid-sync. Turso's sync keeps a log of local changes and sends it on `push`, but a failed push leaves the local replica *ahead* of the cloud.
- **A new key must lock the old one out**, including of notes written after it was replaced.
- **Turso's sync replicates the database file itself** (CDC on the client, pages on the server), so deleting a row deletes it for the child too. And `PRAGMA user_version` lives in the file header, which isn't synced.

## Decision

- **The access key** is 26 random Crockford base-32 symbols (130 bits) plus 2 checksum symbols, shown as `PAPA-XXXX-…` in seven groups of four. Reading it back forgives case, spaces, dashes, and `O`/`I`/`L` typed for `0`/`1`. The checksum catches a typo before it costs a round trip.
- **Two keys from one.** Argon2id (19 MiB, 2 passes, a fixed salt) turns the key into a master secret, and HKDF-SHA256 expands that with two labels into `auth_key` (sent to the relay) and `enc_key` (never leaves the device). The salt is fixed because nothing could carry a per-share one to the child before sign-in; with 2¹³⁰ keys there's no guessing for a salt to slow down. A test pins the outputs for a fixed key, checked against Python's `argon2-cffi`: changing the derivation would lock out every child.
- **A random content key per share and epoch**, wrapped under `enc_key` in the share's `share_key` row. Notes and images are sealed with the content key (XChaCha20-Poly1305, random 192-bit nonces). A new access key brings a new content key and the next epoch, so the old key can't read anything resealed after it.
- **Every sealed value is bound to its address** through the AEAD's associated data: share id, row id, image version and key epoch. A server that swaps rows, or serves an old row as new, makes decryption fail instead of showing the wrong note.
- **The share database** holds `share_schema` (its version, in a table because the header doesn't sync), `share_key`, `shared_note` (id, the note's `updated_at`, epoch, sealed JSON of title, body, dates and image ids) and `shared_blob` (thumbnail and display copy per image, sealed; originals stay home). Labels and the other children a note is about never leave. Deletions are plain `DELETE`s, which sync. The relay's child tokens can read the first four tables.
- **The projection is idempotent.** `Store::share_changes` compares the vault with a manifest of the replica (ids, `updated_at`, epochs; no content) and returns only what differs, sealed on the vault's thread. Running it on a matching replica returns nothing. Images go in batches of about 16 MB, pushed one at a time.
- **Push every time, even with nothing new.** The manifest describes the local replica, which is ahead of the cloud after a failed push. Turso remembers what it hasn't sent; pushing is how it gets sent.
- **The vault is written first.** A new key is stored (marked `auth_key_stale`) before the relay hears of it; if telling the relay fails, the next sync retries. Stopping deletes the cloud copy first, then the replica and the vault's record, so a failure can be retried.
- **Sharing is an actor of its own** (`papa-sharing` thread): the relay client and Turso's sync need tokio's IO, and a slow network mustn't hold up the vault's thread. It talks to the vault through a `Store` clone.
- **One TLS provider for the process.** Turso's sync brings `rustls` with `aws-lc-rs`; the relay's `reqwest` brings `ring`. With both compiled in, rustls refuses to pick and the first connection panics, inside Turso's IO thread, where it hangs the sync instead of failing. `papa-core` installs `aws-lc-rs` as the process default, and sync operations time out after two minutes.
- **A note is shared only with a child it's about**, whose sharing is on. Unassigning the child unshares it. A shared child can't be deleted until sharing stops.

## Consequences

- A sync cut short finishes on the next one, with no recovery code: a test stops the sync server mid-way, starts it again, and the child sees the edit.
- Each sync reads every shared note's id and version from the vault and the replica. That's cheap for thousands of notes; the bodies are read and sealed only when they changed.
- `updated_at`, epochs, ids and sizes are visible to the cloud. The access key's derivation can't change without a versioned scheme.
- The access key doesn't carry the relay's address yet. PLAN.md §2.2 wants it to, so children never type a server; Phase 12, which reads keys, decides the format.
- **Still not checked against Turso Cloud** (no credentials here): whether a child token with fine-grained permissions on these four tables can bootstrap and pull through sync. Local mode checks no tokens. This has to be tried before the relay runs in cloud mode.
