// The look of an exported PDF: page, type, cover, notes and photos.
//
// `main.typ`, which `papa-pdf` writes for each export, imports this file and
// calls its functions with the notes' content. Everything about layout lives
// here; the Rust side only says what goes where.

#let ink = luma(25%)
#let faint = luma(55%)
#let hairline = 0.5pt + luma(78%)

// The whole document: page size, fonts and spacing. Used as a show rule,
// `#show: setup.with(..)`, so it applies to everything after it.
#let setup(paper: "a4", title: none, body) = {
  set document(title: title)
  set page(
    paper: paper,
    margin: (x: 2.5cm, top: 2.5cm, bottom: 2.8cm),
    numbering: "1",
    footer: context align(center, text(size: 9pt, fill: faint, counter(page).display())),
  )
  // Libertinus Serif is Typst's own book face, bundled with the app. Glyphs
  // it lacks (emoji, other scripts) fall back to any other font Papa found.
  set text(font: "Libertinus Serif", size: 11.5pt, fill: ink, lang: "en")
  set par(justify: true, leading: 0.72em, spacing: 1.15em)
  show raw: set text(font: "DejaVu Sans Mono", size: 0.85em)
  show raw.where(block: true): block.with(
    width: 100%,
    inset: (x: 10pt, y: 8pt),
    radius: 3pt,
    fill: luma(96%),
  )
  show quote.where(block: true): it => block(
    inset: (left: 12pt, y: 2pt),
    stroke: (left: 1.5pt + luma(80%)),
    text(style: "italic", it.body),
  )
  show link: it => underline(stroke: hairline, offset: 2pt, it)
  set list(indent: 0.4em, body-indent: 0.6em)
  set enum(indent: 0.4em, body-indent: 0.6em)
  set table(stroke: hairline, inset: 6pt)

  // Level 1 is a note's title; the note's own headings start at level 2.
  // The general rules come first: a later rule wins over an earlier one.
  show heading: set text(size: 11.5pt, weight: "semibold")
  show heading: set block(above: 1.4em, below: 0.8em)
  show heading.where(level: 1): set text(size: 20pt, weight: "regular")
  show heading.where(level: 1): set block(above: 0pt, below: 0.5em)
  show heading.where(level: 2): set text(size: 15pt)
  show heading.where(level: 3): set text(size: 12.5pt)

  body
}

// A title page, for books of several notes. No page number on it.
#let cover(title, subtitle) = page(numbering: none, footer: none, {
  v(1fr)
  align(center, {
    text(size: 34pt, title)
    v(0.8em)
    text(size: 12pt, fill: faint, subtitle)
  })
  v(1.6fr)
})

// A list of the notes, with page numbers.
#let contents() = page(footer: none, outline(title: "Contents", depth: 1, indent: 0pt))

// A note's photos, under its text. Each is `(path, width, height)`. One
// photo gets the full width, or a fixed height if it's tall; several share
// a two-column grid. A photo never splits across pages.
#let gallery(photos) = {
  if photos.len() == 0 { return }
  v(0.8em)
  if photos.len() == 1 {
    let (path, w, h) = photos.first()
    let photo = if w >= h {
      image(path, width: 100%)
    } else {
      image(path, height: 11cm)
    }
    align(center, block(breakable: false, photo))
  } else {
    grid(
      columns: (1fr, 1fr),
      gutter: 6pt,
      ..photos.map(((path, w, h)) => block(breakable: false, image(path, width: 100%))),
    )
  }
}

// One note: its title, the line under it (date, who it's about), the text,
// then its photos. `first` is false for every note after the first, which
// gets a little ornament above it to set it apart.
#let note(title, meta, body, photos: (), first: true) = {
  if not first {
    v(2.2em, weak: true)
    align(center, text(fill: luma(70%), size: 10pt, [∗ #h(0.8em) ∗ #h(0.8em) ∗]))
    v(1.6em)
  }
  heading(level: 1, title)
  if meta != none {
    block(below: 1.4em, text(size: 10pt, fill: faint, meta))
  }
  body
  gallery(photos)
}

// A task list's box: `- [ ]` and `- [x]` in Markdown.
#let checkbox(done) = box(
  width: 0.75em,
  height: 0.75em,
  baseline: 0.1em,
  radius: 1.5pt,
  stroke: 0.6pt + ink,
  if done { align(center + horizon, text(size: 0.7em, sym.checkmark)) },
)

// `---` in Markdown.
#let rule() = align(center, line(length: 30%, stroke: hairline))
