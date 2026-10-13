//! Writes `main.typ`: the document as calls to the layout in `book.typ`.
//!
//! For a book of two notes it looks like this (bodies shortened):
//!
//! ```typst
//! #import "/book.typ": *
//! #show: setup.with(paper: "a4", title: "Emma")
//! #cover([Emma], [2 notes])
//! #contents()
//! #note([First bike ride], [Mar 3 2026], [She refused the training wheels…], photos: (("/photos/0.jpg", 1600, 1200),))
//! #note([School play], none, […], first: false)
//! ```

use crate::Document;
use crate::markdown::{escape, string, write_body};
use crate::world::photo_path;

/// The Typst source of `document`, and the paths it gives the notes'
/// photos, in order.
pub fn write(document: &Document) -> (String, Vec<String>) {
    let mut out = String::new();
    let mut paths = Vec::new();

    let title = match (&document.cover, document.notes.as_slice()) {
        (Some(cover), _) => cover.title.as_str(),
        (None, [note, ..]) => note.title.as_str(),
        (None, []) => "",
    };
    out.push_str("#import \"/book.typ\": *\n");
    out.push_str(&format!(
        "#show: setup.with(paper: \"{}\", title: ",
        document.paper.typst_name()
    ));
    string(title, &mut out);
    out.push_str(")\n");

    if let Some(cover) = &document.cover {
        out.push_str("#cover(");
        content(&cover.title, &mut out);
        out.push_str(", ");
        content(&cover.subtitle, &mut out);
        out.push_str(")\n#contents()\n");
    }

    for (ix, note) in document.notes.iter().enumerate() {
        out.push_str("#note(");
        content(&note.title, &mut out);
        out.push_str(", ");
        match &note.meta {
            Some(meta) => content(meta, &mut out),
            None => out.push_str("none"),
        }
        out.push_str(", [\n");
        write_body(&note.body, &mut out);
        out.push_str("\n]");
        if !note.photos.is_empty() {
            out.push_str(", photos: (");
            for photo in &note.photos {
                let path = photo_path(paths.len(), photo.format.extension());
                out.push('(');
                string(&path, &mut out);
                out.push_str(&format!(", {}, {}), ", photo.width, photo.height));
                paths.push(path);
            }
            out.push(')');
        }
        if ix > 0 {
            out.push_str(", first: false");
        }
        out.push_str(")\n");
    }
    (out, paths)
}

/// Plain text as a content block: `[…]`, escaped.
fn content(text: &str, out: &mut String) {
    out.push('[');
    escape(text, out);
    out.push(']');
}
