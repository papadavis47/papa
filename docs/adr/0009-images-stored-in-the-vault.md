# 0009. Images live in the vault, in three versions, one row each

- **Status:** Accepted
- **Date:** 2026-10-13

## Context

Phase 4 lets the father attach photos to notes: phone photos of 3–8 MB each, screenshots, a child's drawing. They need to be stored, shown as a strip of thumbnails under the note, and opened full size. Forces at play:

- **The vault should stay one file.** Backups (Phase 9) copy one file, and Phase 9's encryption at rest only covers what's inside the database. Photos stored as files next to the vault would sit unencrypted on disk.
- **Originals are keepsakes.** The owner chose to keep every original byte for byte rather than downscale on import, at the cost of a larger vault.
- **Full-size photos are too big to draw.** A 12 MP photo decodes to 48 MB of pixels, and a 48 MP one won't fit in a GPU texture on every machine. The lightbox needs a smaller copy. Sharing (Phase 11) needs one too: PLAN.md §2.2 sends images at most 2048 px on their longest edge.
- **Phone photos are stored sideways.** The camera writes the sensor's pixels plus an EXIF tag saying how to turn them upright.
- **Turso reads a row's whole record.** Measured on `turso` 0.8.2: with all versions as columns of one `image` row, reading just the thumbnails of a note with eight 7 MB photos took 1.3 s in a debug build. Moving the big columns to the end of the row made stock SQLite 55× faster but changed nothing in Turso, which loads every overflow page of a record whichever column a query asks for.
- **Turso's planner can pick a full scan.** Joining images to their thumbnails, it scanned the table of bytes, originals included, unless an index on `image (note_id, id)` gave it a better path.
- **The `image` crate can't read HEIC**, the iPhone's default format. Every HEIC decoder needs the C library libheif.

## Decision

- **Images are stored in the vault**, in two tables:
  - `image` holds what an image is: its note, the original's MIME type, upright width and height, SHA-256 and creation time. No bytes.
  - `image_data` holds the bytes, one row per version: `thumb` (at most 400 px), `display` (at most 2048 px) and `original` (untouched). The display row is skipped when the original is already small and upright.
- **Every query reads only the rows it needs.** The gallery reads thumbnails, the lightbox reads display copies, and nothing reads originals yet (PDF export will). A test checks the query plans for full scans.
- **`papa_core::images::prepare`** does the CPU work: detect the format from the bytes, decode, apply the EXIF rotation, shrink, and encode the copies. Copies are JPEG, or PNG when the image has transparency. It is a pure function. The app runs it on GPUI's background thread pool, never on the UI or database thread, one image at a time.
- **Accepted:** JPEG, PNG, GIF and WebP, up to 50 MB and 100 megapixels. HEIC gets its own error message telling the father to export as JPEG; supporting it is left for later.
- **Attaching the same file twice to one note** returns the image already there (by SHA-256).
- **Deleting an image removes its bytes for good** and touches the note's `updated_at`. Unlike notes, images leave no tombstone: sharing can see the note changed and re-project it.

## Consequences

- The vault grows by the size of each original plus roughly 10–20% for its copies (measured: 10% on 7 MB photos). A hundred phone photos make a vault of about half a gigabyte.
- Showing a note's thumbnails takes about 5 ms in a debug build (from 1.3 s), and loading eight display copies 70 ms (from 680 ms).
- SQLite doesn't shrink its file when rows are deleted; freed pages are reused. Reclaiming space (`VACUUM`) belongs with backup in Phase 9.
- Sharing (Phase 11) can send the stored display copies as they are, after encrypting them.
- Turso's record reading may improve. The two-table layout is right for stock SQLite too, so nothing would need to change back.
- iPhone users must export HEIC photos, or set the camera to "Most Compatible", until HEIC is supported.
