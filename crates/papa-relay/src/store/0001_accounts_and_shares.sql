-- Version 1: father accounts, invites, sessions and shares.
--
-- Every row that belongs to someone is keyed by account, so one relay can
-- serve many families (ADR 0005). Secrets the relay hands out or receives
-- are stored only as hashes (see secrets.rs). Times are Unix milliseconds.

CREATE TABLE account (
    id            TEXT PRIMARY KEY NOT NULL,
    -- Lower case, trimmed: normalized before it gets here.
    email         TEXT NOT NULL UNIQUE,
    -- Argon2id PHC string.
    password_hash TEXT NOT NULL,
    created_at    INTEGER NOT NULL
) STRICT;

-- Minted with `papa-relay admin invite create`; each one makes one account.
CREATE TABLE invite (
    code_hash  BLOB PRIMARY KEY NOT NULL,
    -- Who it was for, to tell invites apart in `admin invite list`.
    note       TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    expires_at INTEGER NOT NULL,
    used_by    TEXT REFERENCES account (id),
    used_at    INTEGER
) STRICT;

CREATE TABLE session (
    token_hash BLOB PRIMARY KEY NOT NULL,
    account_id TEXT NOT NULL REFERENCES account (id) ON DELETE CASCADE,
    created_at INTEGER NOT NULL,
    expires_at INTEGER NOT NULL
) STRICT;

-- One cloud database per shared child. Which child is known only to the
-- father's vault.
CREATE TABLE share (
    id            TEXT PRIMARY KEY NOT NULL,
    -- No cascade: an account can't be deleted while it still has cloud
    -- databases, or they'd be left behind.
    account_id    TEXT NOT NULL REFERENCES account (id),
    database_name TEXT NOT NULL UNIQUE,
    database_url  TEXT NOT NULL,
    -- SHA-256 of the child's auth key; a child signs in by presenting it.
    auth_key_hash BLOB NOT NULL UNIQUE,
    created_at    INTEGER NOT NULL
) STRICT;

CREATE INDEX share_by_account ON share (account_id);
