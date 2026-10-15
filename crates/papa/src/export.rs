//! What a PDF export holds and what it's called: plain Rust, no GPUI, so it
//! is unit-tested without opening a window. `app/export.rs` runs it.
//!
//! The father exports either the open note, or the notes the list shows:
//! a child's whole book when the list shows that child, otherwise whichever
//! notes his label or search picked out. A book reads oldest first, with a
//! title page and contents; a single note starts straight away.

use std::path::{Path, PathBuf};

use jiff::Timestamp;
use jiff::tz::TimeZone;
use papa_core::settings::PaperSize;
use papa_core::{Child, ChildId, EncodedImage, ImageFormat, Note, NoteFilter, NoteId, NoteImage};
use papa_pdf::{Cover, Document, Paper, Photo, PhotoFormat};

/// Which notes to export.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Scope {
    /// The open note.
    Note(NoteId),
    /// Notes from the list, newest first, as it shows them.
    List {
        notes: Vec<NoteId>,
        /// The child whose book this is: the list shows that child and
        /// isn't narrowed by a search.
        child: Option<ChildId>,
    },
}

impl Scope {
    /// The list's notes, as a book of `child` if the list shows all of a
    /// child's notes.
    pub fn list(notes: Vec<NoteId>, filter: NoteFilter, search: &str) -> Self {
        let child = match filter {
            NoteFilter::Child(id) if search.trim().is_empty() => Some(id),
            _ => None,
        };
        Self::List { notes, child }
    }

    /// The notes to read from the vault, in the order they'll be printed.
    pub fn note_ids(&self) -> Vec<NoteId> {
        match self {
            Self::Note(id) => vec![*id],
            Self::List { notes, .. } => notes.iter().rev().copied().collect(),
        }
    }

    fn child(&self) -> Option<ChildId> {
        match self {
            Self::Note(_) => None,
            Self::List { child, .. } => *child,
        }
    }
}

/// How the export is named in the UI, after "Export": "Emma's book",
/// "3 notes", "this note".
pub fn describe(scope: &Scope, children: &[Child]) -> String {
    match scope {
        Scope::Note(_) => "this note".to_owned(),
        Scope::List {
            child: Some(id), ..
        } => match find(children, *id) {
            Some(child) => format!("{}’s book", child.name),
            None => "this book".to_owned(),
        },
        Scope::List { notes, .. } => match notes.len() {
            1 => "1 note".to_owned(),
            n => format!("{n} notes"),
        },
    }
}

/// The file name the save dialog suggests: the note's title (`title`, for
/// a single note) or the child's name, made safe for any file system.
pub fn file_name(scope: &Scope, title: &str, children: &[Child]) -> String {
    let stem = match scope {
        Scope::Note(_) => title,
        Scope::List {
            child: Some(id), ..
        } => find(children, *id).map_or("", |c| c.name.as_str()),
        Scope::List { .. } => "Notes",
    };
    let mut name: String = stem
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '-',
            c if c.is_control() => ' ',
            c => c,
        })
        .take(80)
        .collect();
    // A leading dot would hide the file on Linux and macOS.
    name = name.trim().trim_start_matches('.').trim().to_owned();
    if name.is_empty() {
        name = "Note".to_owned();
    }
    name.push_str(".pdf");
    name
}

/// `dir/name`, or `dir/name (2)`, `(3)`… if that's taken. For saving
/// without a dialog, where nothing may be overwritten.
pub fn unused_path(dir: &Path, name: &str, exists: impl Fn(&Path) -> bool) -> PathBuf {
    let path = dir.join(name);
    if !exists(&path) {
        return path;
    }
    let (stem, extension) = name.rsplit_once('.').unwrap_or((name, ""));
    (2..)
        .map(|n| dir.join(format!("{stem} ({n}).{extension}")))
        .find(|path| !exists(path))
        .expect("some number is free")
}

/// The PDF's contents: `notes` in order, each with its photos.
pub fn document(
    scope: &Scope,
    notes: Vec<(Note, Vec<Photo>)>,
    children: &[Child],
    paper: PaperSize,
    tz: &TimeZone,
) -> Document {
    let book_child = scope.child();
    let cover = match scope {
        Scope::Note(_) => None,
        Scope::List { .. } => Some(Cover {
            title: book_child
                .and_then(|id| find(children, id))
                .map_or("Notes".to_owned(), |child| child.name.clone()),
            subtitle: subtitle(&notes, tz),
        }),
    };
    let notes = notes
        .into_iter()
        .map(|(note, photos)| {
            let date = long_date(note.created_at, tz);
            // Everyone the note is about, except the child whose book it is.
            let about: Vec<&str> = children
                .iter()
                .filter(|child| note.children.contains(&child.id) && Some(child.id) != book_child)
                .map(|child| child.name.as_str())
                .collect();
            // An untitled note goes by its date.
            let (title, mut meta) = match note.title.trim() {
                "" => (date, Vec::new()),
                title => (title.to_owned(), vec![date]),
            };
            if !about.is_empty() {
                meta.push(about.join(", "));
            }
            papa_pdf::Note {
                title,
                meta: (!meta.is_empty()).then(|| meta.join(" · ")),
                body: note.body,
                photos,
            }
        })
        .collect();
    Document {
        cover,
        notes,
        paper: match paper {
            PaperSize::A4 => Paper::A4,
            PaperSize::Letter => Paper::Letter,
        },
    }
}

/// A photo for the PDF: the image's display version, with its size.
pub fn photo(image: &NoteImage, display: EncodedImage) -> Photo {
    Photo {
        format: match display.format {
            ImageFormat::Jpeg => PhotoFormat::Jpeg,
            ImageFormat::Png => PhotoFormat::Png,
            ImageFormat::Gif => PhotoFormat::Gif,
            ImageFormat::Webp => PhotoFormat::Webp,
        },
        bytes: display.bytes,
        width: image.width,
        height: image.height,
    }
}

/// "12 notes · March 2024 to October 2026", for a book's title page.
/// `notes` are oldest first.
fn subtitle(notes: &[(Note, Vec<Photo>)], tz: &TimeZone) -> String {
    let count = match notes.len() {
        1 => "1 note".to_owned(),
        n => format!("{n} notes"),
    };
    let (Some((first, _)), Some((last, _))) = (notes.first(), notes.last()) else {
        return count;
    };
    let first = first.created_at.to_zoned(tz.clone());
    let last = last.created_at.to_zoned(tz.clone());
    let span = if (first.year(), first.month()) == (last.year(), last.month()) {
        first.strftime("%B %Y").to_string()
    } else if first.year() == last.year() {
        format!("{} to {}", first.strftime("%B"), last.strftime("%B %Y"))
    } else {
        format!("{} to {}", first.strftime("%B %Y"), last.strftime("%B %Y"))
    };
    format!("{count} · {span}")
}

/// "March 3, 2026": spelled out, for print.
fn long_date(at: Timestamp, tz: &TimeZone) -> String {
    at.to_zoned(tz.clone()).strftime("%B %-d, %Y").to_string()
}

fn find(children: &[Child], id: ChildId) -> Option<&Child> {
    children.iter().find(|child| child.id == id)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn child(name: &str) -> Child {
        Child {
            id: ChildId::new(),
            name: name.to_owned(),
            color: 0,
        }
    }

    fn note(title: &str, at: &str, children: &[&Child]) -> (Note, Vec<Photo>) {
        let at: Timestamp = at.parse().unwrap();
        let note = Note {
            id: NoteId::new(),
            title: title.to_owned(),
            body: "Text".to_owned(),
            children: children.iter().map(|child| child.id).collect(),
            labels: Vec::new(),
            shared_with: Vec::new(),
            created_at: at,
            updated_at: at,
        };
        (note, Vec::new())
    }

    #[test]
    fn a_childs_list_is_their_book_unless_searched() {
        let emma = child("Emma");
        let notes = vec![NoteId::new()];
        let filter = NoteFilter::Child(emma.id);

        let children = [emma];

        let book = Scope::list(notes.clone(), filter, " ");
        assert_eq!(describe(&book, &children), "Emma’s book");

        let searched = Scope::list(notes, filter, "bike");
        assert_eq!(describe(&searched, &children), "1 note");
        let all = Scope::list(vec![NoteId::new(), NoteId::new()], NoteFilter::All, "");
        assert_eq!(describe(&all, &children), "2 notes");
    }

    #[test]
    fn books_print_oldest_first() {
        let ids = [NoteId::new(), NoteId::new(), NoteId::new()];
        let scope = Scope::list(ids.to_vec(), NoteFilter::All, "");
        assert_eq!(scope.note_ids(), [ids[2], ids[1], ids[0]]);
    }

    #[test]
    fn a_book_names_its_child_once_on_the_cover() {
        let emma = child("Emma");
        let leo = child("Leo");
        let children = [emma.clone(), leo.clone()];
        let scope = Scope::List {
            notes: Vec::new(),
            child: Some(emma.id),
        };
        let notes = vec![
            note("First bike ride", "2025-03-03T12:00:00Z", &[&emma]),
            note("", "2026-10-01T12:00:00Z", &[&emma, &leo]),
        ];

        let document = document(&scope, notes, &children, PaperSize::Letter, &TimeZone::UTC);

        let cover = document.cover.unwrap();
        assert_eq!(cover.title, "Emma");
        assert_eq!(cover.subtitle, "2 notes · March 2025 to October 2026");
        assert_eq!(document.notes[0].title, "First bike ride");
        assert_eq!(document.notes[0].meta.as_deref(), Some("March 3, 2025"));
        // Untitled: the date becomes the title. Leo is named, Emma isn't.
        assert_eq!(document.notes[1].title, "October 1, 2026");
        assert_eq!(document.notes[1].meta.as_deref(), Some("Leo"));
        assert_eq!(document.paper, Paper::Letter);
    }

    #[test]
    fn a_single_note_has_no_cover_and_names_everyone() {
        let emma = child("Emma");
        let leo = child("Leo");
        let (first, photos) = note("Beach", "2026-07-14T23:30:00Z", &[&emma, &leo]);
        let scope = Scope::Note(first.id);
        // 23:30 UTC is already the next day in Paris.
        let paris = TimeZone::get("Europe/Paris").unwrap();

        let document = document(
            &scope,
            vec![(first, photos)],
            &[emma, leo],
            PaperSize::A4,
            &paris,
        );

        assert_eq!(document.cover, None);
        assert_eq!(
            document.notes[0].meta.as_deref(),
            Some("July 15, 2026 · Emma, Leo")
        );
    }

    #[test]
    fn subtitles_shorten_ranges_within_a_year_or_month() {
        let tz = TimeZone::UTC;
        let span = |a, b| subtitle(&[note("", a, &[]), note("", b, &[])], &tz);
        assert_eq!(
            span("2026-03-01T00:00:00Z", "2026-04-30T00:00:00Z"),
            "2 notes · March to April 2026"
        );
        assert_eq!(
            span("2026-03-01T00:00:00Z", "2026-03-30T00:00:00Z"),
            "2 notes · March 2026"
        );
        assert_eq!(
            subtitle(&[note("", "2026-03-01T00:00:00Z", &[])], &tz),
            "1 note · March 2026"
        );
    }

    #[test]
    fn file_names_are_safe() {
        let emma = child("Emma");
        let titled = |title: &str| file_name(&Scope::Note(NoteId::new()), title, &[]);
        assert_eq!(titled("First bike ride"), "First bike ride.pdf");
        assert_eq!(titled("1/2 term: \"best\"?"), "1-2 term- -best--.pdf");
        assert_eq!(titled(" ..hidden"), "hidden.pdf");
        assert_eq!(titled("   "), "Note.pdf");
        assert_eq!(titled(&"é".repeat(100)).chars().count(), 84);

        let book = Scope::List {
            notes: Vec::new(),
            child: Some(emma.id),
        };
        assert_eq!(file_name(&book, "", &[emma]), "Emma.pdf");
    }

    #[test]
    fn saving_without_a_dialog_never_overwrites() {
        let dir = Path::new("/docs");
        let taken = [dir.join("Emma.pdf"), dir.join("Emma (2).pdf")];
        let exists = |path: &Path| taken.iter().any(|taken| taken == path);

        assert_eq!(
            unused_path(dir, "Emma.pdf", exists),
            dir.join("Emma (3).pdf")
        );
        assert_eq!(unused_path(dir, "Leo.pdf", exists), dir.join("Leo.pdf"));
    }
}
