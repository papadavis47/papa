//! The Typst compiler's view of the outside world.
//!
//! Typst never touches the file system or the clock itself: it asks a
//! [`World`] for its sources, files, fonts and today's date. That's what
//! lets it run in-process, as a library, on files that exist only in
//! memory. Ours serves three kinds of file:
//!
//! - `/main.typ`: the document, written by [`crate::typst`] for this export;
//! - `/book.typ`: the layout, the same every time (`src/book.typ`);
//! - `/photos/<n>.<ext>`: the notes' photos, numbered in document order.
//!
//! and the fonts Typst bundles plus the ones installed on the machine.

use std::sync::LazyLock;

use typst::diag::{FileError, FileResult};
use typst::foundations::{Bytes, Datetime, Duration};
use typst::syntax::{FileId, RootedPath, Source, VirtualPath, VirtualRoot};
use typst::text::{Font, FontBook};
use typst::utils::LazyHash;
use typst::{Library, LibraryExt as _, World};
use typst_kit::fonts::{self, FontStore};

/// The layout every export uses.
const BOOK: &str = include_str!("book.typ");

/// Typst's standard library. Built once: it's the same for every export.
static LIBRARY: LazyLock<LazyHash<Library>> = LazyLock::new(|| LazyHash::new(Library::default()));

/// The fonts Typst bundles (Libertinus Serif, New Computer Modern, DejaVu
/// Sans Mono) first, so they win over an installed font of the same name.
/// Then the machine's, for the glyphs the bundled ones lack: emoji, Greek,
/// Chinese… Scanning the system takes a moment, so it happens on first use,
/// and only the font files a document needs are ever read.
static FONTS: LazyLock<FontStore> = LazyLock::new(|| {
    let mut fonts = FontStore::new();
    fonts.extend(fonts::embedded());
    fonts.extend(fonts::system());
    fonts
});

/// One export's files.
pub struct BookWorld {
    main: Source,
    book: Source,
    /// Indexed by the number in their path.
    photos: Vec<(FileId, Bytes)>,
}

impl BookWorld {
    /// A world whose `/main.typ` is `main`, with `photos` at the paths
    /// [`photo_path`] gives them.
    pub fn new(main: String, photos: impl IntoIterator<Item = (String, Vec<u8>)>) -> Self {
        Self {
            main: Source::new(file_id("/main.typ"), main),
            book: Source::new(file_id("/book.typ"), BOOK.to_owned()),
            photos: photos
                .into_iter()
                .map(|(path, bytes)| (file_id(&path), Bytes::new(bytes)))
                .collect(),
        }
    }
}

/// Where the `index`th photo of a document lives, with the extension that
/// tells Typst its format.
///
/// Typst interns every path it sees for the life of the process, and can
/// hold 65 535 of them. Numbering photos by position, rather than naming
/// them by id, keeps that count down to the most photos one export holds.
pub fn photo_path(index: usize, extension: &str) -> String {
    format!("/photos/{index}.{extension}")
}

fn file_id(path: &str) -> FileId {
    let path = VirtualPath::new(path).expect("our paths are valid");
    RootedPath::new(VirtualRoot::Project, path).intern()
}

impl World for BookWorld {
    fn library(&self) -> &LazyHash<Library> {
        &LIBRARY
    }

    fn book(&self) -> &LazyHash<FontBook> {
        FONTS.book()
    }

    fn main(&self) -> FileId {
        self.main.id()
    }

    fn source(&self, id: FileId) -> FileResult<Source> {
        [&self.main, &self.book]
            .into_iter()
            .find(|source| source.id() == id)
            .cloned()
            .ok_or_else(|| not_found(id))
    }

    fn file(&self, id: FileId) -> FileResult<Bytes> {
        if let Ok(source) = self.source(id) {
            return Ok(Bytes::from_string(source));
        }
        self.photos
            .iter()
            .find(|(photo, _)| *photo == id)
            .map(|(_, bytes)| bytes.clone())
            .ok_or_else(|| not_found(id))
    }

    fn font(&self, index: usize) -> Option<Font> {
        FONTS.font(index)
    }

    /// Exports don't show today's date, so Typst doesn't get one.
    fn today(&self, _offset: Option<Duration>) -> Option<Datetime> {
        None
    }
}

fn not_found(id: FileId) -> FileError {
    FileError::NotFound(id.vpath().get_without_slash().into())
}
