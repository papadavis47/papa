-- Version 5: messages between the father and his children (Phase 13).
--
-- The vault keeps the whole conversation with each child, both ways, in
-- plain text like the notes (the vault itself may be encrypted). The
-- share's database only carries messages, sealed, between the two apps.

CREATE TABLE message (
    -- Made by whoever wrote the message: the same id in the share.
    id         TEXT PRIMARY KEY NOT NULL,
    -- No ON DELETE CASCADE on `share`: the conversation outlives sharing,
    -- as a note does. Deleting the child deletes it.
    child_id   TEXT NOT NULL REFERENCES child (id) ON DELETE CASCADE,
    sender     TEXT NOT NULL CHECK (sender IN ('father', 'child')),
    body       TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    -- The father's: when a push took it to the cloud, NULL until then.
    -- The child's: when it arrived here.
    sent_at    INTEGER,
    -- When the recipient read it: for the father's, from the child's
    -- receipt; for the child's, when the father opened the conversation.
    read_at    INTEGER
) STRICT;

CREATE INDEX message_by_child ON message (child_id, created_at);
