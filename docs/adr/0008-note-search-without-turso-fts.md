# 0008. Search notes in Rust; Turso's full-text search waits

- **Status:** Accepted
- **Date:** 2026-10-12

## Context

Phase 3 adds search to the note list. PLAN.md scheduled Turso's experimental full-text search (FTS) for this phase, with a plain fallback. Before building on FTS, a scratch crate tried it on `turso` 0.8.2 (`fts` feature, `Builder::experimental_index_method(true)`, `CREATE INDEX … USING fts (title, body_md)`). It works: matches, `fts_score` ranking, updates, deletes and rollbacks all behave. But:

- **The vault stops being a plain SQLite file.** Once an FTS index exists, `sqlite3` refuses the whole file (`malformed database schema … near "USING"`). Today any SQLite tool can open a vault, which is a cheap escape hatch while Turso is pre-1.0 (ADR 0003).
- **Whole words only.** The default tokenizer has no prefix queries: `bik` and `bik*` find nothing, and phrase prefixes need two words. Search-as-you-type would show nothing until a word is complete.
- **No accent folding.** `cafe` doesn't find `café`.
- **User input is query syntax.** A stray `'` is a parse error, so input would need escaping.
- **Build cost.** The feature pulls in `tantivy`.

A journal is small: thousands of notes, a few megabytes of text. A plain scan is fast enough at that size.

## Decision

`Store::notes(NoteQuery)` filters in Rust on the database thread:

- The search is split into words. A note matches when every word appears in its title or body, as a substring, ignoring case.
- Case folding uses Rust's Unicode `to_lowercase`. SQL `lower()` and `LIKE` fold only ASCII, so `émilie` wouldn't find `Émilie`.
- The app reruns the search 150 ms after typing pauses, so a quickly typed phrase costs one query.

Turso's FTS stays out of the build for now (`default-features = false`).

## Consequences

- Prefix and substring matches work as you type, with Unicode case folding and no syntax to escape.
- The vault stays readable by any SQLite tool.
- Cost grows linearly with the vault. Measured on 5,000 notes of about 800 characters each: 30 ms per search in a release build, 300–550 ms in a debug build.
- There is no relevance ranking: results stay newest first.
- Revisit FTS when vaults outgrow the scan or Turso's FTS stabilises. The `Store::notes` API wouldn't change, and this scan remains the fallback PLAN.md asks for.
