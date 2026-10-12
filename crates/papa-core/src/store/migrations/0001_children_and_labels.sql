-- Version 1: children and labels.
--
-- STRICT tables reject values of the wrong type instead of silently storing
-- them. Ids are UUIDv7 strings made by the app, not database counters.

-- The children the father writes about, in sidebar order.
CREATE TABLE child (
    id         TEXT PRIMARY KEY NOT NULL,
    name       TEXT NOT NULL CHECK (length(trim(name)) > 0),
    -- 0xRRGGBB
    color      INTEGER NOT NULL CHECK (color BETWEEN 0 AND 16777215),
    sort_order INTEGER NOT NULL
) STRICT;

-- The father's private labels. NOCASE makes "Funny" and "funny" the same
-- name, both for the UNIQUE constraint and for sorting.
CREATE TABLE label (
    id   TEXT PRIMARY KEY NOT NULL,
    name TEXT NOT NULL UNIQUE COLLATE NOCASE CHECK (length(trim(name)) > 0)
) STRICT;
