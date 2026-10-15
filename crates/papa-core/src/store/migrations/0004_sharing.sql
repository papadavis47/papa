-- Version 4: sharing notes with children (Phase 11).
--
-- Everything the father's side of sharing needs to remember. The cloud
-- holds only ciphertext; the keys to it live here, in the vault, which is
-- encrypted at rest when it has a password.

-- The father's account on the relay. At most one row.
CREATE TABLE account (
    id            INTEGER PRIMARY KEY NOT NULL CHECK (id = 1),
    relay_url     TEXT NOT NULL,
    email         TEXT NOT NULL,
    -- Sent as `Authorization: Bearer …`. The relay stores only its hash.
    session_token TEXT NOT NULL,
    expires_at    INTEGER NOT NULL
) STRICT;

-- One row per child whose notes are shared: their cloud database and the
-- keys to it.
CREATE TABLE share (
    -- No ON DELETE: a shared child can't be deleted until sharing stops,
    -- or their notes would stay in the cloud with nobody to delete them.
    child_id       TEXT PRIMARY KEY NOT NULL REFERENCES child (id),
    -- The relay's id for the share, and where its database syncs.
    share_id       TEXT NOT NULL UNIQUE,
    relay_url      TEXT NOT NULL,
    database_url   TEXT NOT NULL,
    -- `PAPA-…`, so the father can show it to his child again.
    access_key     TEXT NOT NULL,
    -- Seals the notes. Random; a new access key brings a new one.
    content_key    BLOB NOT NULL,
    -- The content key wrapped under the access key, for the cloud's
    -- `share_key` row, which is how the child gets it.
    wrapped_nonce  BLOB NOT NULL,
    wrapped_key    BLOB NOT NULL,
    -- Counts the content keys; part of every sealed row's associated data.
    key_epoch      INTEGER NOT NULL CHECK (key_epoch > 0),
    -- 1 while the relay still has the previous access key: a new key was
    -- made, but telling the relay failed. The next sync retries.
    auth_key_stale INTEGER NOT NULL DEFAULT 0 CHECK (auth_key_stale IN (0, 1)),
    created_at     INTEGER NOT NULL,
    last_synced_at INTEGER
) STRICT;

-- Which notes each child can see. A subset of `note_child`: unassigning a
-- note from a child unshares it too.
CREATE TABLE note_share (
    note_id   TEXT NOT NULL REFERENCES note (id) ON DELETE CASCADE,
    child_id  TEXT NOT NULL REFERENCES share (child_id) ON DELETE CASCADE,
    shared_at INTEGER NOT NULL,
    PRIMARY KEY (note_id, child_id)
) STRICT;

CREATE INDEX note_share_by_child ON note_share (child_id);
