//! Turns a photo the father attaches into what the vault stores.
//!
//! Decoding and resizing are CPU work that takes a while for a phone photo,
//! so nothing here touches the database or any UI. The app runs
//! [`prepare_file`] or [`prepare`] on a background thread, then hands the
//! result to [`Store::add_image`](crate::Store::add_image).
//!
//! Each image is kept in up to three versions:
//!
//! - the **original**, byte for byte, as a keepsake (and later for PDFs);
//! - a **display** copy no larger than [`DISPLAY_EDGE`], for the lightbox
//!   and, in Phase 11, for sharing. Skipped when the original is already
//!   small and upright;
//! - a **thumbnail** no larger than [`THUMB_EDGE`], for the gallery.

use std::fs::File;
use std::io::{Cursor, Read as _};
use std::path::Path;

use image::metadata::Orientation;
use image::{DynamicImage, ImageDecoder as _, ImageError, ImageReader};
use sha2::{Digest as _, Sha256};

use crate::model::{EncodedImage, ImageFormat};
use crate::{Error, Result};

/// Largest image file Papa accepts.
pub const MAX_IMAGE_BYTES: u64 = 50 * 1024 * 1024;

/// Most pixels an image may have: 100 megapixels, more than any common
/// camera. Decoding takes 3 to 4 bytes per pixel, plus a copy to rotate it.
pub const MAX_PIXELS: u64 = 100_000_000;

/// Longest edge of the display copy, in pixels. Also what PLAN.md picks for
/// shared images.
pub const DISPLAY_EDGE: u32 = 2048;

/// Longest edge of a thumbnail, in pixels: sharp on a 2× screen at the
/// gallery's tile size.
pub const THUMB_EDGE: u32 = 400;

const DISPLAY_JPEG_QUALITY: u8 = 85;
const THUMB_JPEG_QUALITY: u8 = 80;

/// An image ready to be stored: every version encoded, plus what the vault
/// records about it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreparedImage {
    /// The bytes as they came in, untouched.
    pub original: EncodedImage,
    /// `None` when the original can be shown as it is.
    pub display: Option<EncodedImage>,
    pub thumb: EncodedImage,
    /// Size as shown, after applying the camera's rotation.
    pub width: u32,
    pub height: u32,
    /// SHA-256 of the original, in hex.
    pub sha256: String,
}

/// Reads and prepares the image file at `path`. Refuses files over
/// [`MAX_IMAGE_BYTES`] before reading them.
pub fn prepare_file(path: &Path) -> Result<PreparedImage> {
    let read_error = |reason| Error::ReadFile {
        path: path.to_owned(),
        reason,
    };
    let file = File::open(path).map_err(read_error)?;
    let len = file.metadata().map_err(read_error)?.len();
    if len > MAX_IMAGE_BYTES {
        return Err(Error::ImageTooLarge);
    }
    let mut bytes = Vec::with_capacity(len as usize);
    // `take` guards against a file that grew after the size check.
    file.take(MAX_IMAGE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(read_error)?;
    prepare(bytes)
}

/// Prepares an image from its encoded bytes, e.g. from the clipboard.
pub fn prepare(bytes: Vec<u8>) -> Result<PreparedImage> {
    if bytes.len() as u64 > MAX_IMAGE_BYTES {
        return Err(Error::ImageTooLarge);
    }
    let format = detect_format(&bytes)?;

    let mut decoder = ImageReader::with_format(Cursor::new(&bytes), format.into())
        .into_decoder()
        .map_err(image_error)?;
    // A small file can claim enormous dimensions, and decoding allocates
    // for all of them up front: an allocation that fails aborts the whole
    // app. The `image` crate's default limits don't catch this, so check.
    let (width, height) = decoder.dimensions();
    if u64::from(width) * u64::from(height) > MAX_PIXELS {
        return Err(Error::ImageTooManyPixels);
    }
    // Phone cameras store pixels as the sensor saw them, plus an EXIF tag
    // saying how to turn them upright. Viewers that ignore the tag show
    // the photo sideways, so the copies we make have it applied.
    let orientation = decoder.orientation().map_err(image_error)?;
    let mut pixels = DynamicImage::from_decoder(decoder).map_err(image_error)?;
    pixels.apply_orientation(orientation);

    let (width, height) = (pixels.width(), pixels.height());
    let display = if width.max(height) > DISPLAY_EDGE || orientation != Orientation::NoTransforms {
        Some(encode(
            &shrink(&pixels, DISPLAY_EDGE),
            DISPLAY_JPEG_QUALITY,
        )?)
    } else {
        None
    };
    let thumb = encode(&shrink(&pixels, THUMB_EDGE), THUMB_JPEG_QUALITY)?;

    Ok(PreparedImage {
        sha256: format!("{:x}", Sha256::digest(&bytes)),
        original: EncodedImage { format, bytes },
        display,
        thumb,
        width,
        height,
    })
}

/// The width and height an encoded image says it has, read from its
/// header without decoding the pixels. Papa's stored copies are already
/// upright, so this is their size as shown.
pub fn dimensions(image: &EncodedImage) -> Result<(u32, u32)> {
    ImageReader::with_format(Cursor::new(&image.bytes), image.format.into())
        .into_dimensions()
        .map_err(image_error)
}

/// Recognises the formats Papa accepts by their first bytes, ignoring the
/// file name: a pasted image has none, and extensions lie.
fn detect_format(bytes: &[u8]) -> Result<ImageFormat> {
    if is_heif(bytes) {
        return Err(Error::HeicImage);
    }
    match image::guess_format(bytes) {
        Ok(image::ImageFormat::Jpeg) => Ok(ImageFormat::Jpeg),
        Ok(image::ImageFormat::Png) => Ok(ImageFormat::Png),
        Ok(image::ImageFormat::Gif) => Ok(ImageFormat::Gif),
        Ok(image::ImageFormat::WebP) => Ok(ImageFormat::Webp),
        _ => Err(Error::UnsupportedImage),
    }
}

/// HEIC/HEIF, the iPhone's default photo format. The `image` crate can't
/// read it, and every decoder needs the C library libheif, so Papa says so
/// clearly instead of "unsupported".
///
/// HEIF files are ISO media files: bytes 4..8 are `ftyp`, followed by a
/// four-letter brand.
fn is_heif(bytes: &[u8]) -> bool {
    const BRANDS: &[&[u8; 4]] = &[
        b"heic", b"heix", b"heim", b"heis", b"hevc", b"hevx", b"hevm", b"hevs", b"mif1", b"msf1",
    ];
    bytes.len() >= 12 && &bytes[4..8] == b"ftyp" && BRANDS.iter().any(|b| &bytes[8..12] == *b)
}

/// Scales `pixels` down to fit in `edge`×`edge`, keeping the aspect ratio.
/// Never scales up.
fn shrink(pixels: &DynamicImage, edge: u32) -> DynamicImage {
    if pixels.width() <= edge && pixels.height() <= edge {
        return pixels.clone();
    }
    // `thumbnail` averages every source pixel into the target: fast, and
    // sharp enough when shrinking by a large factor.
    pixels.thumbnail(edge, edge)
}

/// JPEG for photos; PNG when there's transparency, which JPEG can't hold.
fn encode(pixels: &DynamicImage, jpeg_quality: u8) -> Result<EncodedImage> {
    let mut bytes = Vec::new();
    let format = if pixels.color().has_alpha() {
        let encoder = image::codecs::png::PngEncoder::new(&mut bytes);
        DynamicImage::from(pixels.to_rgba8())
            .write_with_encoder(encoder)
            .map_err(image_error)?;
        ImageFormat::Png
    } else {
        let encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut bytes, jpeg_quality);
        DynamicImage::from(pixels.to_rgb8())
            .write_with_encoder(encoder)
            .map_err(image_error)?;
        ImageFormat::Jpeg
    };
    Ok(EncodedImage { format, bytes })
}

fn image_error(err: ImageError) -> Error {
    match err {
        ImageError::Limits(_) => Error::ImageTooManyPixels,
        err => Error::InvalidImage(err.to_string()),
    }
}

impl From<ImageFormat> for image::ImageFormat {
    fn from(format: ImageFormat) -> Self {
        match format {
            ImageFormat::Jpeg => Self::Jpeg,
            ImageFormat::Png => Self::Png,
            ImageFormat::Gif => Self::Gif,
            ImageFormat::Webp => Self::WebP,
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use image::{Rgb, RgbImage, Rgba, RgbaImage};

    use super::*;

    /// A `width`×`height` photo-like image encoded as `format`.
    pub(crate) fn sample(format: image::ImageFormat, width: u32, height: u32) -> Vec<u8> {
        let pixels = RgbImage::from_fn(width, height, |x, y| Rgb([x as u8, y as u8, 128]));
        let mut bytes = Cursor::new(Vec::new());
        pixels.write_to(&mut bytes, format).unwrap();
        bytes.into_inner()
    }

    /// A JPEG of `width`×`height` pixels whose EXIF says "rotate 90°
    /// clockwise", as a phone held upright writes it.
    fn rotated_jpeg(width: u32, height: u32) -> Vec<u8> {
        let jpeg = sample(image::ImageFormat::Jpeg, width, height);
        // A minimal APP1 segment: "Exif", a little-endian TIFF header and
        // one IFD entry, Orientation (0x0112) = 6.
        let mut exif = b"Exif\0\0II*\0\x08\0\0\0\x01\0".to_vec();
        exif.extend([0x12, 0x01, 3, 0, 1, 0, 0, 0, 6, 0, 0, 0]);
        exif.extend([0, 0, 0, 0]);
        let mut out = jpeg[..2].to_vec(); // SOI
        out.extend([0xff, 0xe1]);
        out.extend(((exif.len() + 2) as u16).to_be_bytes());
        out.extend(exif);
        out.extend(&jpeg[2..]);
        out
    }

    fn decoded_size(image: &EncodedImage) -> (u32, u32) {
        let pixels = image::load_from_memory(&image.bytes).unwrap();
        let size = (pixels.width(), pixels.height());
        // The header says the same, without decoding.
        assert_eq!(dimensions(image).unwrap(), size);
        size
    }

    #[test]
    fn a_small_upright_image_needs_no_display_copy() {
        let bytes = sample(image::ImageFormat::Png, 300, 200);

        let prepared = prepare(bytes.clone()).unwrap();

        assert_eq!(prepared.original.bytes, bytes, "kept byte for byte");
        assert_eq!(prepared.original.format, ImageFormat::Png);
        assert_eq!((prepared.width, prepared.height), (300, 200));
        assert_eq!(prepared.display, None);
        assert_eq!(decoded_size(&prepared.thumb), (300, 200), "never scaled up");
        assert_eq!(prepared.thumb.format, ImageFormat::Jpeg);
    }

    #[test]
    fn a_large_image_gets_smaller_copies_with_its_aspect_ratio() {
        let prepared = prepare(sample(image::ImageFormat::Jpeg, 3000, 1500)).unwrap();

        assert_eq!((prepared.width, prepared.height), (3000, 1500));
        let display = prepared.display.as_ref().unwrap();
        assert_eq!(display.format, ImageFormat::Jpeg);
        assert_eq!(decoded_size(display), (DISPLAY_EDGE, DISPLAY_EDGE / 2));
        assert_eq!(decoded_size(&prepared.thumb), (THUMB_EDGE, THUMB_EDGE / 2));
    }

    #[test]
    fn the_camera_rotation_is_applied_to_the_copies() {
        let bytes = rotated_jpeg(60, 40);

        let prepared = prepare(bytes.clone()).unwrap();

        assert_eq!(prepared.original.bytes, bytes);
        assert_eq!((prepared.width, prepared.height), (40, 60), "upright");
        let display = prepared.display.as_ref().expect("the original is sideways");
        assert_eq!(decoded_size(display), (40, 60));
        assert_eq!(decoded_size(&prepared.thumb), (40, 60));
    }

    #[test]
    fn transparency_is_kept_as_png() {
        let pixels = RgbaImage::from_pixel(10, 10, Rgba([255, 0, 0, 100]));
        let mut bytes = Cursor::new(Vec::new());
        pixels
            .write_to(&mut bytes, image::ImageFormat::WebP)
            .unwrap();

        let prepared = prepare(bytes.into_inner()).unwrap();

        assert_eq!(prepared.original.format, ImageFormat::Webp);
        assert_eq!(prepared.thumb.format, ImageFormat::Png);
    }

    #[test]
    fn gifs_are_accepted() {
        let prepared = prepare(sample(image::ImageFormat::Gif, 20, 10)).unwrap();
        assert_eq!(prepared.original.format, ImageFormat::Gif);
    }

    #[test]
    fn the_hash_identifies_the_original() {
        let a = prepare(sample(image::ImageFormat::Png, 10, 10)).unwrap();
        let b = prepare(sample(image::ImageFormat::Png, 10, 11)).unwrap();

        assert_eq!(a.sha256.len(), 64);
        assert_eq!(a.sha256, prepare(a.original.bytes.clone()).unwrap().sha256);
        assert_ne!(a.sha256, b.sha256);
    }

    #[test]
    fn heic_photos_get_their_own_error() {
        let mut heic = vec![0, 0, 0, 24];
        heic.extend(b"ftypheic\0\0\0\0mif1heic");

        assert!(matches!(prepare(heic), Err(Error::HeicImage)));
    }

    #[test]
    fn other_files_are_rejected() {
        let text = b"Dear Emma, ...".to_vec();
        assert!(matches!(prepare(text), Err(Error::UnsupportedImage)));
        let tiff = b"II*\0\x08\0\0\0".to_vec();
        assert!(matches!(prepare(tiff), Err(Error::UnsupportedImage)));
    }

    #[test]
    fn a_damaged_image_is_reported() {
        let mut png = sample(image::ImageFormat::Png, 50, 50);
        png.truncate(60);

        assert!(matches!(prepare(png), Err(Error::InvalidImage(_))));
    }

    #[test]
    fn huge_dimensions_hit_the_decoder_limit() {
        // A 1×1 PNG whose header is changed to claim 100,000 × 100,000
        // pixels: 10 billion, far past what decoding may allocate.
        let mut png = Cursor::new(Vec::new());
        let encoder = image::codecs::png::PngEncoder::new(&mut png);
        image::GrayImage::new(1, 1)
            .write_with_encoder(encoder)
            .unwrap();
        let mut png = png.into_inner();
        // The IHDR chunk: type at 12..16, width and height at 16..24, and a
        // CRC of type and data at 29..33.
        png[16..20].copy_from_slice(&100_000u32.to_be_bytes());
        png[20..24].copy_from_slice(&100_000u32.to_be_bytes());
        let crc = crc32(&png[12..29]);
        png[29..33].copy_from_slice(&crc.to_be_bytes());

        let result = prepare(png);

        assert!(
            matches!(result, Err(Error::ImageTooManyPixels)),
            "{result:?}"
        );
    }

    /// The CRC-32 PNG chunks end with.
    fn crc32(bytes: &[u8]) -> u32 {
        let mut crc = !0u32;
        for &byte in bytes {
            crc ^= u32::from(byte);
            for _ in 0..8 {
                crc = if crc & 1 == 1 {
                    (crc >> 1) ^ 0xedb8_8320
                } else {
                    crc >> 1
                };
            }
        }
        !crc
    }

    #[test]
    fn files_over_the_limit_are_refused_before_reading() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("big.jpg");
        let file = File::create(&path).unwrap();
        file.set_len(MAX_IMAGE_BYTES + 1).unwrap();

        assert!(matches!(prepare_file(&path), Err(Error::ImageTooLarge)));
    }

    #[test]
    fn a_missing_file_names_its_path() {
        let err = prepare_file(Path::new("/nonexistent/emma.jpg")).unwrap_err();
        assert!(err.to_string().contains("/nonexistent/emma.jpg"), "{err}");
    }
}
