-- Version 2: notes, and which children and labels they belong to.

-- Times are Unix milliseconds. A deleted note stays as a tombstone: its text
-- is wiped and `deleted_at` is set, so sharing (Phase 11) can tell a child's
-- app that the note is gone.
CREATE TABLE note (
    id         TEXT PRIMARY KEY NOT NULL,
    title      TEXT NOT NULL,
    body_md    TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    deleted_at INTEGER
) STRICT;

-- The note list, newest first. Partial: tombstones are never listed.
CREATE INDEX note_live_by_created ON note (created_at) WHERE deleted_at IS NULL;

-- Many-to-many: a note can be about several children. Deleting a child or a
-- note deletes its links, but never the other side.
CREATE TABLE note_child (
    note_id  TEXT NOT NULL REFERENCES note (id) ON DELETE CASCADE,
    child_id TEXT NOT NULL REFERENCES child (id) ON DELETE CASCADE,
    PRIMARY KEY (note_id, child_id)
) STRICT;

-- The primary key finds a note's children; this finds a child's notes.
CREATE INDEX note_child_by_child ON note_child (child_id);

CREATE TABLE note_label (
    note_id  TEXT NOT NULL REFERENCES note (id) ON DELETE CASCADE,
    label_id TEXT NOT NULL REFERENCES label (id) ON DELETE CASCADE,
    PRIMARY KEY (note_id, label_id)
) STRICT;

CREATE INDEX note_label_by_label ON note_label (label_id);
