# 0014. The relay: hashed secrets, two provisioning backends, and a local sync server for development

- **Status:** Proposed
- **Date:** 2026-10-14

## Context

Phase 10 builds `papa-relay` (ADR 0005): father accounts with invite codes, one cloud database per shared child (ADR 0004), and short-lived tokens for those databases. PLAN.md §10 left one question open, whether developing sync needs a Turso Cloud database. Forces at play:

- **Turso's own CLI runs a sync server.** `tursodb --sync-server <addr> --sync-dir <dir>` serves every database under the folder as `/db/<name>`, creates one the first time a client syncs with it, and speaks the same protocol as Turso Cloud. A test at the start of the phase pushed and pulled between two `turso` 0.8.2 replicas through it, both ways. It checks no tokens.
- **The Platform API** creates and deletes databases and mints a JWT for one database, with an expiry and full-access or read-only. Per-table permissions (`turso db tokens create -p`) exist, but the API reference doesn't document them; the official CLI sends a `fine_grained_permissions` field.
- **Secrets the relay holds:** father passwords (chosen by people, so guessable), and things the relay itself makes or receives that are long and random: session tokens, invite codes, and each child's `auth_key`. The `auth_key` is 256 bits that the child's app derives with Argon2id from a 128-bit access key.
- **A child signs in with Dad's email and their key**, with nothing that names the share. PLAN.md §7 says the relay stores "Argon2id hashes" of `auth_key`s. A salted slow hash can't be looked up, so the relay would have to try each of that father's shares in turn.
- **Turso locks its database file.** A second process, such as an admin command, can't open the relay's database while the relay runs.

## Decision

- **Local mode for development, no cloud.** `PAPA_RELAY_MODE=local` (the default) points at a `tursodb --sync-server`. Creating a share just returns `<url>/db/share-<hex>`. Revoking deletes that folder if `PAPA_RELAY_LOCAL_SYNC_DIR` is set. Tokens are `local-<father|child>-<random>`, so the app sends one in both modes. `PAPA_RELAY_MODE=cloud` uses the Platform API.
- **An enum, not a trait object, picks the backend** (`Provisioner::{Local, Cloud}`). There are exactly two, and every operation has to handle both.
- **Only father passwords get Argon2id**, as PHC strings, with the default cost (19 MiB, 2 passes), hashed on tokio's blocking pool. A log-in for an unknown email checks a dummy hash, so timing doesn't reveal which emails have accounts.
- **Everything else random is stored as plain SHA-256:** session tokens (256 bits, 30 days), invite codes (60 bits, single use, 14 days by default) and child `auth_key`s. A slow hash adds nothing when there's nothing to guess. A fast one lets the relay find a share by `sha256(auth_key)` and then check that it belongs to the father with that email. This replaces the "Argon2id hashes of child `auth_key`s" in PLAN.md §7.
- **Opaque session tokens, not JWTs.** One row per session: logging out deletes it, and nothing needs a signing key.
- **Database tokens last an hour.** The father gets full access. The child's token carries fine-grained permissions: read `shared_note` and `shared_blob`, and read and add to `message` and `receipt`.
- **Wrong email, wrong password and wrong key all reply `invalid_credentials`.** Sign-up, log-in and child sign-in are limited per client address: 10 a minute by default, in memory. Behind a proxy, `PAPA_RELAY_CLIENT_IP_HEADER` names the header that holds the client's address.
- **Admin commands go through a Unix socket.** It sits next to the database (`relay.db.admin.sock`, mode `0600`). `papa-relay admin invite create` sends one JSON line and reads one back. With no relay running, it opens the database itself. Admin access never touches the network and needs no password.
- **Configuration comes only from environment variables**, listed in `.env.example`. The Platform token is held in a `Secret` whose `Debug` prints `[redacted]`. Request bodies aren't logged.

## Consequences

- Phases 11–13 can be built and tested entirely offline: one `tursodb` process, one relay.
- Local mode enforces nothing at the database: any token reaches any share. That's fine on a developer's machine, and the reason it isn't an option for a deployed relay.
- **Not yet checked against Turso Cloud** (there are no credentials in this environment). The request shapes are tested against a fake Platform API. Unconfirmed: the `fine_grained_permissions` field, its action names (the docs say `data_add`, the CLI's help says `data_insert`), and whether a child limited to some tables can still push through `turso` sync, which writes its own bookkeeping. Phase 11 has to confirm this on a real database before relying on it.
- A leaked relay database exposes emails, password hashes (slow to attack), and which account owns which share URL. It exposes no usable tokens or keys, and no content: shares are encrypted end to end.
- Rate limits reset when the relay restarts and don't stretch across several relays. One relay is the plan (ADR 0005).
- The admin socket is Unix-only. On Windows the admin command needs the relay stopped.
- Before the relay opens beyond the family, these are still missing: account export and deletion (an account with shares can't be deleted yet; the schema refuses it), and a privacy policy.
