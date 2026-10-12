//! The vault's data types and the rules they follow.
//!
//! These are plain values: the UI receives copies from the [`Store`](crate::Store)
//! and sends back drafts. Nothing here touches the database.

use std::fmt;
use std::str::FromStr;

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
