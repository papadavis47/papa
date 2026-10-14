# 0013. Vault encryption with a wrapped key, and backups as one file

- **Status:** Accepted
- **Date:** 2026-10-14

## Context

Phase 9 adds an optional vault password, a lock screen, auto-lock, and export/import of backups. PLAN.md §7 asks for Turso's encryption at rest (AEGIS-256), a key derived from the password with Argon2id, kept only in memory and zeroized on lock. Forces at play:

- **Turso 0.8.2's encryption** takes a 256-bit key as hex in `EncryptionOpts` and encrypts every page, write-ahead log included. An encrypted file starts with `Turso`, a plain one with `SQLite format 3`. A wrong key fails with "Decryption failed for page=1"; no key fails with "file is not a database".
- **Turso can't change a vault's key in place.** There's no rekey, and `VACUUM INTO` from an encrypted vault writes a *plaintext* copy.
- **Passwords change**, and a father who changes his shouldn't wait for the whole vault, photos included, to be rewritten.
- **Phase 11** needs the same thing for each child's share: a random content key, wrapped under a key derived from a secret (PLAN.md §4: `wrapped_content_key`, `key_salt`).
- **Backups** must be independent of the running app (Turso's FAQ advises it while it's pre-1.0), consistent, and must not leak plaintext of an encrypted vault.
- **The settings window** is a second window, but the vault belongs to the main one.

## Decision

- **Two keys.** The vault is encrypted with a random 256-bit *vault key*. The password only wraps it:
  - Argon2id derives a *wrapping key* from the password and a random salt. The cost is 64 MiB, 3 passes, 1 lane (RFC 9106's second recommendation), stored with the salt so it can be raised later.
  - XChaCha20-Poly1305 encrypts the vault key, with the context string `papa vault key v1` as associated data.
  - The salt, nonce, cost and wrapped key go in `vault.key`, JSON, next to `vault.db`, created `0600`.
- **What it buys.**
  - A wrong password fails on the wrap's tag before Turso reads a page.
  - Changing the password rewrites only `vault.key`.
  - Phase 11 reuses the code.
- **The file says whether a vault is encrypted**, not the settings: `lock::inspect` reads its first bytes.
- **Setting the first password, or removing it, rewrites the vault.** Papa runs the migrations on a new file (`vault.db.new`), encrypted or plain, and copies every row of every table in one transaction. Then:
  1. When encrypting, it writes the key file.
  2. It closes the old vault after a `wal_checkpoint(TRUNCATE)` and deletes its write-ahead log.
  3. It renames the new file over the old one.
  4. When decrypting, it deletes the key file.

  A crash leaves a vault that opens: before the rename, the old one with at most a stray key file, which the next plain open deletes; after it, the new one.
- **Locking closes the store.** `Store::close` stops the database thread, dropping Turso's copy of the key, which Turso zeroes. The app also drops the editor, the lists, the selection and the search text. A generation counter discards store answers that arrive for a vault that has since closed.
- **Auto-lock** counts from the last key press (anywhere in the app), click, scroll or mouse move in the main window. A task checks every 5 s. The setting is `privacy.auto_lock_minutes`, 15 by default, 0 for never; only a vault with a password locks.
- **A backup is one `.papabackup` file:**
  - The line `PAPA BACKUP`, then a JSON header (version, date, schema version, the key file if any), then the vault file byte for byte.
  - It's copied on the database thread right after a checkpoint, so it's a consistent snapshot.
  - A plain vault gives a plain SQLite file after the header. An encrypted one stays encrypted: its pages as on disk, openable with the password the vault had that day. Nothing is decrypted to make it.
- **Restoring checks first, swaps last.**
  - Papa copies the backup next to the vault and opens it, which migrates an older one and refuses a newer one. It reads every row, so every page decrypts and authenticates. All of this happens while the current vault stays open, so a wrong password or a damaged file changes nothing.
  - Then it closes the vault, moves it aside to `vault-before-restore.db` (with its key file), and renames the copy into place.
  - Restore works from the lock screen too, and after a vault failed to open.
- **The Settings window sends actions to the main window** (`dispatch_to_main`) and reads a `VaultStatus` global the main window publishes. The dialogs and the store stay in one place.

## Consequences

- **Losing `vault.key` loses an encrypted vault**, password or not. Backups carry it, and the error names the missing file.
- **A password change doesn't change the vault key.** Someone holding an old copy of `vault.key` and the old password could still unlock the current vault file. Removing the password and setting it again re-encrypts with a new key.
- **Plaintext still passes through memory.** The UI's text fields, GPUI's strings, Turso's page cache while open, and freed memory after lock are not zeroized. The lock protects the file at rest and the screen, not a memory dump of a running machine.
- **Encrypted backups depend on Turso's encryption format**, like the vault itself. Plain backups depend on nothing but SQLite.
- **Encrypting and decrypting rewrite the whole vault.** With many photos that takes a moment; the dialog stays busy and can't be cancelled meanwhile.
- **No OS keychain yet** (PLAN.md §7 has it as optional). An encrypted vault asks for the password at every launch.
- **Turso's encryption is experimental.** It is gated behind the password, so a father who never sets one never uses it.
