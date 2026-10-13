//! PDF export for Papa notes.
//!
//! A [`Document`] (some notes, maybe a cover) becomes a PDF in three steps,
//! all in memory:
//!
//! 1. **Markup:** each note's Markdown is turned into Typst markup by
//!    walking its syntax tree ([`markdown`]), and wrapped in calls to the
//!    layout in `book.typ` ([`markup`]).
//! 2. **Layout:** the Typst compiler lays that out into pages, reading the
//!    layout, the photos and the fonts through a [`World`](world::BookWorld)
//!    we provide.
//! 3. **PDF:** `typst-pdf` writes the pages out.
//!
//! The crate knows nothing of the vault or the app: it takes plain values,
//! so the father's app and (later) the child's can both feed it. It's on
//! its own because Typst is big; only this crate pays to compile it.

mod markdown;
mod markup;
mod world;

use typst::diag::{SourceDiagnostic, Warned};
use typst::ecow::EcoVec;
use typst::foundations::Smart;
use typst_layout::PagedDocument;
use typst_pdf::PdfOptions;

use crate::world::BookWorld;

/// What to export.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Document {
    /// A title page, and a table of contents after it. For books of
    /// several notes; a single note starts straight away.
    pub cover: Option<Cover>,
    /// In the order they appear.
    pub notes: Vec<Note>,
    pub paper: Paper,
}

/// A book's title page.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cover {
    /// Large, in the middle: "Emma".
    pub title: String,
    /// Under it: "24 notes · March 2024 to October 2026".
    pub subtitle: String,
}

/// One note, ready to print.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Note {
    pub title: String,
    /// The small line under the title: its date, who it's about. Plain
    /// text, already formatted by the caller.
    pub meta: Option<String>,
    /// Markdown.
    pub body: String,
    /// Shown after the text, in this order.
    pub photos: Vec<Photo>,
}

/// An image attached to a note.
#[derive(Clone, PartialEq, Eq)]
pub struct Photo {
    pub format: PhotoFormat,
    pub bytes: Vec<u8>,
    /// Size in pixels, upright. Tall photos are laid out differently.
    pub width: u32,
    pub height: u32,
}

/// Bytes would flood the output of `{:?}`, so only their length is shown.
impl std::fmt::Debug for Photo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Photo")
            .field("format", &self.format)
            .field("bytes", &format_args!("<{} bytes>", self.bytes.len()))
            .field("width", &self.width)
            .field("height", &self.height)
            .finish()
    }
}

/// The image formats Papa stores, all of which Typst reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PhotoFormat {
    Jpeg,
    Png,
    Gif,
    Webp,
}

impl PhotoFormat {
    /// The file extension, which is how Typst tells formats apart.
    fn extension(self) -> &'static str {
        match self {
            Self::Jpeg => "jpg",
            Self::Png => "png",
            Self::Gif => "gif",
            Self::Webp => "webp",
        }
    }
}

/// Paper size.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Paper {
    /// 210 × 297 mm: most of the world.
    #[default]
    A4,
    /// 8.5 × 11 in: North America.
    Letter,
}

impl Paper {
    /// The name Typst knows it by.
    fn typst_name(self) -> &'static str {
        match self {
            Self::A4 => "a4",
            Self::Letter => "us-letter",
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("there are no notes to export")]
    Empty,
    /// Typst rejected the markup we wrote: a bug in this crate, not
    /// something the user did. The messages say where.
    #[error("couldn't lay out the PDF: {0}")]
    Layout(String),
    #[error("couldn't write the PDF: {0}")]
    Pdf(String),
}

/// Lays out `document` and returns the PDF's bytes.
///
/// Takes a while for a long book (a few seconds for hundreds of notes), and
/// blocks while it works: run it off the UI thread.
/// Takes the document by value so its photos move into the compiler
/// rather than being copied.
pub fn render(document: Document) -> Result<Vec<u8>, Error> {
    let pages = layout(document)?;
    let options = PdfOptions {
        creator: Smart::Custom(Some("Papa".into())),
        ..PdfOptions::default()
    };
    let pdf = typst_pdf::pdf(&pages, &options).map_err(|errors| Error::Pdf(messages(&errors)));
    // Typst memoizes its work, so editing and recompiling the same document
    // is fast. Each export is a one-off, so free that memory now rather
    // than keep it for the life of the app.
    typst::comemo::evict(0);
    pdf
}

/// Lays `document` out into pages.
fn layout(document: Document) -> Result<PagedDocument, Error> {
    if document.notes.is_empty() {
        return Err(Error::Empty);
    }
    let (main, paths) = markup::write(&document);
    let photos = document.notes.into_iter().flat_map(|note| note.photos);
    let world = BookWorld::new(main, paths.into_iter().zip(photos.map(|p| p.bytes)));
    // Warnings (say, a font that lacks a glyph) don't stop the export.
    let Warned { output, .. } = typst::compile::<PagedDocument>(&world);
    output.map_err(|errors| Error::Layout(messages(&errors)))
}

fn messages(diagnostics: &EcoVec<SourceDiagnostic>) -> String {
    diagnostics
        .iter()
        .map(|diagnostic| diagnostic.message.as_str())
        .collect::<Vec<_>>()
        .join("; ")
}

#[cfg(test)]
mod tests;
