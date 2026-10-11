# 0006. Typst for PDF export

- **Status:** Proposed (to be confirmed in Phase 8)
- **Date:** 2026-10-11

## Context

Children (and fathers) can export notes to PDF, possibly to print as a keepsake. The output should look typeset, not like a screenshot. Options: `typst` (a full typesetting engine), `krilla` (a lower-level PDF writer), and `printpdf`.

## Decision

Convert a note's Markdown into Typst markup and compile it in-process with `typst` + `typst-pdf`, using bundled fonts. The code lives in its own crate, `papa-pdf`.

## Consequences

- Good-looking output with headings, lists and images, with little layout code of our own.
- Typst is heavy to compile. Isolating it in `papa-pdf` contains the cost.
- If it proves too heavy, fall back to `krilla` and do more layout ourselves.
