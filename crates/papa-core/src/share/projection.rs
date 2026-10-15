//! The share projection: which rows a child's share database should hold,
//! compared with what it holds now.
//!
//! The projection is idempotent. Its input is the vault and a [`Manifest`]
//! of the share database (ids, versions and key epochs, no content); its
//! output is the [`ShareChanges`] that make the database match the vault.
//! Run it on a database that already matches, and the change set is empty.
//! A sync that stopped halfway (the network dropped, the app quit) needs no
//! recovery: the next run sees what's missing and sends only that.

use std::collections::HashMap;
use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use super::Sealed;
use crate::{EncodedImage, Error, ImageFormat, ImageId, NoteId, Result};

/// Which version of an image a blob row holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum BlobKind {
    /// At most 400 px: for the gallery.
    Thumb,
    /// At most 2048 px: for the lightbox. Originals never leave the vault.
    Display,
}

impl BlobKind {
    pub const ALL: [Self; 2] = [Self::Thumb, Self::Display];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Thumb => "thumb",
            Self::Display => "display",
        }
    }
}

impl FromStr for BlobKind {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self> {
        Self::ALL
            .into_iter()
            .find(|kind| kind.as_str() == s)
            .ok_or_else(|| Error::Corrupt {
                what: "blob kind",
                value: s.to_owned(),
            })
    }
}

/// A note as the child gets it, before sealing: JSON inside the
/// ciphertext. Labels and the other children it's about stay behind.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NotePayload {
    pub title: String,
    /// Markdown.
    pub body: String,
    /// Unix milliseconds.
    pub created_at: i64,
    pub updated_at: i64,
    /// The note's images, in order. Their bytes are in `shared_blob`.
    pub images: Vec<String>,
}

/// An image as sealed in a blob row: its MIME type, a newline, its bytes.
pub fn encode_image(image: &EncodedImage) -> Vec<u8> {
    let mime = image.format.mime().as_bytes();
    let mut bytes = Vec::with_capacity(mime.len() + 1 + image.bytes.len());
    bytes.extend_from_slice(mime);
    bytes.push(b'\n');
    bytes.extend_from_slice(&image.bytes);
    bytes
}

pub fn decode_image(mut bytes: Vec<u8>) -> Result<EncodedImage> {
    let corrupt = || Error::Corrupt {
        what: "shared image",
        value: "no image type".into(),
    };
    let newline = bytes.iter().position(|b| *b == b'\n').ok_or_else(corrupt)?;
    let mime = std::str::from_utf8(&bytes[..newline]).map_err(|_| corrupt())?;
    let format = ImageFormat::from_mime(mime).ok_or_else(corrupt)?;
    bytes.drain(..=newline);
    Ok(EncodedImage { format, bytes })
}

/// What a share database holds, without the content.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Manifest {
    pub notes: HashMap<NoteId, NoteVersion>,
    /// Each image version's key epoch.
    pub blobs: HashMap<(ImageId, BlobKind), i64>,
}

/// Which version of a note a row holds: the note's `updated_at` in the
/// vault, and the key epoch it's sealed under.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NoteVersion {
    pub updated_at: i64,
    pub epoch: i64,
}

/// What to write to a share database to make it match the vault.
#[derive(Debug, Default)]
pub struct ShareChanges {
    pub notes: Vec<SealedNote>,
    pub removed_notes: Vec<NoteId>,
    pub blobs: Vec<SealedBlob>,
    pub removed_blobs: Vec<(ImageId, BlobKind)>,
    /// Some images didn't fit in this batch: apply these changes, then ask
    /// again.
    pub more: bool,
}

impl ShareChanges {
    pub fn is_empty(&self) -> bool {
        self.notes.is_empty()
            && self.removed_notes.is_empty()
            && self.blobs.is_empty()
            && self.removed_blobs.is_empty()
    }
}

pub struct SealedNote {
    pub id: NoteId,
    pub version: NoteVersion,
    pub sealed: Sealed,
}

pub struct SealedBlob {
    pub image: ImageId,
    pub note: NoteId,
    pub kind: BlobKind,
    pub epoch: i64,
    pub sealed: Sealed,
}

impl fmt::Debug for SealedNote {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "SealedNote({}, {:?})", self.id, self.version)
    }
}

impl fmt::Debug for SealedBlob {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "SealedBlob({}, {:?})", self.image, self.kind)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_image_survives_encoding() {
        let image = EncodedImage {
            format: ImageFormat::Webp,
            // Starts with a newline, to catch a decoder that splits on the
            // last one instead of the first.
            bytes: b"\nRIFF\n\xff".to_vec(),
        };
        assert_eq!(decode_image(encode_image(&image)).unwrap(), image);
        assert!(decode_image(b"image/bmp\nxx".to_vec()).is_err());
        assert!(decode_image(b"no newline".to_vec()).is_err());
    }
}
