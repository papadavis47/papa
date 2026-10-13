//! The vault's data types and the rules they follow.
//!
//! These are plain values: the UI receives copies from the [`Store`](crate::Store)
//! and sends back drafts. Nothing here touches the database.

use std::fmt;
use std::str::FromStr;

use jiff::Timestamp;
use uuid::Uuid;

use crate::{Error, Result};

/// Longest allowed name for a child or label, in characters.
pub const MAX_NAME_CHARS: usize = 60;

/// Largest valid `0xRRGGBB` colour.
const MAX_COLOR: u32 = 0xff_ff_ff;

/// Defines an id newtype around a UUIDv7.
///
/// Each kind of row gets its own type, so the compiler rejects a `LabelId`
/// where a `ChildId` is expected. UUIDv7 ids start with a timestamp, so they
/// sort by creation time and can be made on any device without a central
/// counter (which sharing will need later).
macro_rules! id_type {
    ($(#[$doc:meta])* $name:ident) => {
        $(#[$doc])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub struct $name(Uuid);

        impl $name {
            /// A new, unique id.
            #[allow(clippy::new_without_default)] // a "default" id would be misleading
            pub fn new() -> Self {
                Self(Uuid::now_v7())
            }
        }

        /// The hyphenated form stored in the database.
        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(f)
            }
        }

        impl FromStr for $name {
            type Err = uuid::Error;

            fn from_str(s: &str) -> Result<Self, Self::Err> {
                Uuid::parse_str(s).map(Self)
            }
        }
    };
}

id_type!(
    /// Identifies a child.
    ChildId
);
id_type!(
    /// Identifies a label.
    LabelId
);
id_type!(
    /// Identifies a note.
    NoteId
);
id_type!(
    /// Identifies an image attached to a note.
    ImageId
);

/// A child the father writes about.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Child {
    pub id: ChildId,
    pub name: String,
    /// Accent colour as `0xRRGGBB`, shown next to the name.
    pub color: u32,
}

/// What the father enters when adding or editing a child. The store checks
/// it with [`clean_child_name`] and assigns the id.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChildDraft {
    pub name: String,
    pub color: u32,
}

/// A private label the father puts on notes. Labels never leave his machine.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Label {
    pub id: LabelId,
    /// Stored without the `#` the UI shows in front of it.
    pub name: String,
}

/// A note, in full: what the editor loads.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Note {
    pub id: NoteId,
    /// May be empty; the UI shows "Untitled".
    pub title: String,
    /// Markdown.
    pub body: String,
    /// The children the note is about, in sidebar order.
    pub children: Vec<ChildId>,
    /// Its labels, alphabetically.
    pub labels: Vec<LabelId>,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
}

impl Note {
    /// The note as the list shows it, with `images` attached.
    pub fn summary(&self, images: usize) -> NoteSummary {
        NoteSummary {
            id: self.id,
            title: self.title.clone(),
            preview: preview(&self.body),
            children: self.children.clone(),
            labels: self.labels.clone(),
            images,
            created_at: self.created_at,
        }
    }
}

/// One row of the note list: the note without its full body.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NoteSummary {
    pub id: NoteId,
    pub title: String,
    /// The start of the body as one line of plain text. See [`preview`].
    pub preview: String,
    pub children: Vec<ChildId>,
    pub labels: Vec<LabelId>,
    /// How many images are attached.
    pub images: usize,
    pub created_at: Timestamp,
}

/// What the father enters when creating a note. The store assigns the id and
/// timestamps.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NoteDraft {
    pub title: String,
    pub body: String,
    pub children: Vec<ChildId>,
    pub labels: Vec<LabelId>,
}

/// The image formats Papa reads and stores.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ImageFormat {
    Jpeg,
    Png,
    Gif,
    Webp,
}

impl ImageFormat {
    /// The MIME type, as stored in the vault.
    pub fn mime(self) -> &'static str {
        match self {
            Self::Jpeg => "image/jpeg",
            Self::Png => "image/png",
            Self::Gif => "image/gif",
            Self::Webp => "image/webp",
        }
    }

    pub fn from_mime(mime: &str) -> Option<Self> {
        [Self::Jpeg, Self::Png, Self::Gif, Self::Webp]
            .into_iter()
            .find(|format| format.mime() == mime)
    }
}

/// An image's bytes and the format they are in.
#[derive(Clone, PartialEq, Eq)]
pub struct EncodedImage {
    pub format: ImageFormat,
    pub bytes: Vec<u8>,
}

/// Bytes would flood the output of `{:?}`, so only their length is shown.
impl fmt::Debug for EncodedImage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EncodedImage")
            .field("format", &self.format)
            .field("bytes", &format_args!("<{} bytes>", self.bytes.len()))
            .finish()
    }
}

/// An image attached to a note, as the gallery shows it: its thumbnail, but
/// not the full-size versions.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NoteImage {
    pub id: ImageId,
    pub note_id: NoteId,
    /// The original's format.
    pub format: ImageFormat,
    /// Size as shown, upright.
    pub width: u32,
    pub height: u32,
    pub thumb: EncodedImage,
}

/// Which notes the list shows, newest first.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NoteQuery {
    pub filter: NoteFilter,
    /// Words to look for in the title and body. Every word must appear,
    /// ignoring case; an empty search matches every note. See [`SearchTerms`].
    pub search: String,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum NoteFilter {
    #[default]
    All,
    /// Notes about this child.
    Child(ChildId),
    /// Notes with this label.
    Label(LabelId),
}

/// Longest [`NoteSummary::preview`], in characters.
pub const PREVIEW_CHARS: usize = 120;

/// The start of a Markdown body as one line of plain text, for the note
/// list: blank lines dropped, heading, quote and list markers removed,
/// emphasis and code markers removed, cut to [`PREVIEW_CHARS`].
pub fn preview(body: &str) -> String {
    let mut text = String::new();
    for line in body.lines() {
        let line = strip_block_marker(line.trim());
        if line.is_empty() {
            continue;
        }
        if !text.is_empty() {
            text.push(' ');
        }
        text.extend(line.chars().filter(|c| !matches!(c, '*' | '`')));
        if text.chars().count() > PREVIEW_CHARS {
            break;
        }
    }
    match text.char_indices().nth(PREVIEW_CHARS) {
        Some((cut, _)) => format!("{}…", text[..cut].trim_end()),
        None => text,
    }
}

/// Drops a leading `#`, `>`, `-`, `*`, `+` or `1.` and the space after it.
fn strip_block_marker(line: &str) -> &str {
    let rest = line.trim_start_matches(['#', '>']);
    let rest = match rest.strip_prefix(['-', '*', '+']) {
        Some(after) if after.starts_with(' ') => after,
        _ => rest,
    };
    let digits = rest.len() - rest.trim_start_matches(|c: char| c.is_ascii_digit()).len();
    let rest = match rest[digits..].strip_prefix(". ") {
        Some(after) if digits > 0 => after,
        _ => rest,
    };
    rest.trim_start()
}

/// A search split into lowercase words. A note matches when every word
/// appears somewhere in its title or body, ignoring case.
///
/// Lowercasing happens in Rust rather than SQL: SQLite's `lower()` and
/// `LIKE` (and so Turso's) only fold ASCII, so "émilie" wouldn't find
/// "Émilie".
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchTerms(Vec<String>);

impl SearchTerms {
    pub fn new(search: &str) -> Self {
        Self(search.split_whitespace().map(str::to_lowercase).collect())
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn matches(&self, title: &str, body: &str) -> bool {
        if self.is_empty() {
            return true;
        }
        let title = title.to_lowercase();
        let body = body.to_lowercase();
        self.0
            .iter()
            .all(|term| title.contains(term.as_str()) || body.contains(term.as_str()))
    }
}

/// Trims a child's name and checks it isn't empty or too long.
///
/// Public so the UI can check a form with the same rules the store enforces.
pub fn clean_child_name(raw: &str) -> Result<String> {
    let name = raw.trim();
    if name.is_empty() {
        return Err(Error::EmptyName);
    }
    if name.chars().count() > MAX_NAME_CHARS {
        return Err(Error::NameTooLong);
    }
    Ok(name.to_owned())
}

/// Like [`clean_child_name`], and also drops leading `#`s, so typing `#funny`
/// and `funny` both make the label `funny`.
pub fn clean_label_name(raw: &str) -> Result<String> {
    clean_child_name(raw.trim().trim_start_matches('#'))
}

pub(crate) fn check_color(color: u32) -> Result<u32> {
    if color > MAX_COLOR {
        return Err(Error::InvalidColor(color));
    }
    Ok(color)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_trimmed() {
        assert_eq!(clean_child_name("  Emma \n").unwrap(), "Emma");
    }

    #[test]
    fn blank_names_are_rejected() {
        assert!(matches!(clean_child_name(" \t "), Err(Error::EmptyName)));
        assert!(matches!(clean_label_name(" # "), Err(Error::EmptyName)));
    }

    #[test]
    fn long_names_are_rejected_by_characters_not_bytes() {
        // 60 two-byte characters is 120 bytes but still allowed.
        assert!(clean_child_name(&"é".repeat(MAX_NAME_CHARS)).is_ok());
        assert!(matches!(
            clean_child_name(&"a".repeat(MAX_NAME_CHARS + 1)),
            Err(Error::NameTooLong)
        ));
    }

    #[test]
    fn label_names_drop_leading_hashes() {
        assert_eq!(clean_label_name(" #funny").unwrap(), "funny");
        assert_eq!(clean_label_name("##a#b").unwrap(), "a#b");
    }

    #[test]
    fn colors_must_fit_in_rgb() {
        assert_eq!(check_color(0xe0795b).unwrap(), 0xe0795b);
        assert!(matches!(
            check_color(0x1_00_00_00),
            Err(Error::InvalidColor(_))
        ));
    }

    #[test]
    fn previews_are_one_line_of_plain_text() {
        let body = "# First bike ride\n\n> She **refused** the `training` wheels!\n- one\n2. two";
        assert_eq!(
            preview(body),
            "First bike ride She refused the training wheels! one two"
        );
    }

    #[test]
    fn previews_keep_markers_that_are_part_of_the_text() {
        assert_eq!(preview("-5 degrees, 3.5 km"), "-5 degrees, 3.5 km");
        assert_eq!(preview("   "), "");
    }

    #[test]
    fn long_previews_are_cut_by_characters() {
        let preview = preview(&"é".repeat(PREVIEW_CHARS + 10));
        assert_eq!(preview.chars().count(), PREVIEW_CHARS + 1);
        assert!(preview.ends_with('…'));
    }

    #[test]
    fn search_needs_every_word_in_title_or_body() {
        let terms = SearchTerms::new("  BIKE  émilie ");
        assert!(terms.matches("First bike ride", "Émilie came along"));
        assert!(!terms.matches("First bike ride", "Leo came along"));
        assert!(SearchTerms::new("ike rid").matches("First bike ride", ""));
    }

    #[test]
    fn an_empty_search_matches_everything() {
        let terms = SearchTerms::new(" \t ");
        assert!(terms.is_empty());
        assert!(terms.matches("", ""));
    }

    #[test]
    fn image_formats_round_trip_through_mime_types() {
        for format in [
            ImageFormat::Jpeg,
            ImageFormat::Png,
            ImageFormat::Gif,
            ImageFormat::Webp,
        ] {
            assert_eq!(ImageFormat::from_mime(format.mime()), Some(format));
        }
        assert_eq!(ImageFormat::from_mime("image/heic"), None);
    }

    #[test]
    fn ids_round_trip_through_text() {
        let id = ChildId::new();
        assert_eq!(id.to_string().parse::<ChildId>().unwrap(), id);
    }

    #[test]
    fn ids_sort_by_creation_time() {
        let first = LabelId::new();
        let second = LabelId::new();
        assert!(first < second);
    }
}
