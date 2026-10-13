-- Version 3: images attached to notes.
--
-- The bytes live in the vault itself rather than as files beside it, so the
-- vault stays one file to back up, and Phase 9's encryption at rest covers
-- the photos too. See ADR 0009.

-- One row per image: what it is, but none of its bytes.
CREATE TABLE image (
    id         TEXT PRIMARY KEY NOT NULL,
    note_id    TEXT NOT NULL REFERENCES note (id) ON DELETE CASCADE,
    -- The original's MIME type.
    mime       TEXT NOT NULL,
    -- Upright size of the original, in pixels.
    width      INTEGER NOT NULL CHECK (width > 0),
    height     INTEGER NOT NULL CHECK (height > 0),
    -- Of the original, in hex. Attaching the same file twice to one note
    -- returns the image already there.
    sha256     TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    UNIQUE (note_id, sha256)
) STRICT;

-- A note's images in the order they were added. Without it, Turso's planner
-- answers "this note's images with their thumbnails" by scanning all of
-- `image_data`, originals included.
CREATE INDEX image_by_note ON image (note_id, id);

-- The bytes, one row per version of an image:
--   'thumb'    at most 400 px and upright, for the gallery;
--   'display'  at most 2048 px and upright, for the lightbox. Missing when
--              the original is already small and upright;
--   'original' the file as the father attached it, byte for byte.
--
-- Separate rows, not columns of `image`: Turso reads a row's whole record,
-- every BLOB in it, even when a query asks for one small column. With the
-- versions as columns, showing eight thumbnails read eight 7 MB originals
-- too (1.3 s in a debug build).
CREATE TABLE image_data (
    image_id TEXT NOT NULL REFERENCES image (id) ON DELETE CASCADE,
    kind     TEXT NOT NULL CHECK (kind IN ('thumb', 'display', 'original')),
    mime     TEXT NOT NULL,
    bytes    BLOB NOT NULL,
    PRIMARY KEY (image_id, kind)
) STRICT;
