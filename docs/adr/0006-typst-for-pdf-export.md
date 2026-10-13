# 0006. Typst for PDF export

- **Status:** Accepted (built in Phase 8)
- **Date:** 2026-10-11, accepted 2026-10-13

## Context

Children (and fathers) can export notes to PDF, possibly to print as a keepsake. The output should look typeset, not like a screenshot. Options: `typst` (a full typesetting engine), `krilla` (a lower-level PDF writer), and `printpdf`.

## Decision

Convert a note's Markdown into Typst markup and compile it in-process with `typst` + `typst-pdf`, using bundled fonts. The code lives in its own crate, `papa-pdf`.

How it's built (Phase 8):

- **Markdown is parsed with the `markdown` crate**, the same parser the app's preview uses, and its tree is walked into Typst markup. Structure is written as function calls and every ASCII punctuation character in the text is escaped, so what a father types can never turn into Typst code.
- **The layout is a Typst file** (`book.typ`) compiled into the binary. Rust only writes calls to it.
- **Our own `World`** serves the sources, the photos and the fonts from memory. Fonts are Typst's bundled ones (via `typst-kit`'s `embedded-fonts`) followed by the system's, for emoji and scripts the bundled ones lack.
- **`papa-pdf` takes plain values**, not vault types, so the child's app (Phase 12) can feed it decrypted notes.

## Consequences

- Good-looking output with headings, lists, tables, footnotes, code and photos, with little layout code of our own. Changing the look means editing `book.typ`, not Rust.
- Typst is heavy: about 120 more crates, and roughly a minute more on a cold build. Isolating it in `papa-pdf` keeps it out of `papa-core` and the relay, though the app still builds it.
- A debug build lays out a 120-note book with 240 photos in about 9 seconds; it runs on the background pool, so the window stays responsive.
- Typst assumes a compiler run per process in places. A long-running app has to give photos positional paths (Typst interns every path forever, up to 65,535) and evict Typst's memoization cache after each export.
- System fonts make output depend slightly on the machine (an emoji font may be missing). The bundled fonts cover all Latin text.
- `krilla` stays the fallback, and is already in the build: `typst-pdf` uses it to write the PDF.
