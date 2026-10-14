//! The icons the app embeds.
//!
//! gpui-kit's default `Assets` holds only the icons its components use.
//! Other Lucide icons are named in `gpui_kit::assets::IconName`, but their
//! SVGs are embedded only when listed here, so the binary carries just the
//! ones the app draws.

use std::borrow::Cow;

use gpui_kit::assets::Assets;
use gpui_kit::{AssetSource, SharedString};

gpui_kit::assets::icon_assets!(
    ExtraIcons,
    [
        ArchiveRestore,
        BookDown,
        DatabaseBackup,
        FileDown,
        Hash,
        Image,
        ImagePlus,
        Keyboard,
        KeyRound,
        List,
        Lock,
        LockKeyhole,
        LogOut,
        NotebookPen,
        SearchX,
        Shield,
        Trash,
        UserPlus
    ]
);

/// The components' icons plus [`ExtraIcons`].
pub struct PapaAssets;

impl AssetSource for PapaAssets {
    fn load(&self, path: &str) -> gpui_kit::Result<Option<Cow<'static, [u8]>>> {
        // Ours first: `ExtraIcons` answers `Ok(None)` for a path it doesn't
        // have, but `Assets` answers with an error.
        match ExtraIcons.load(path)? {
            Some(bytes) => Ok(Some(bytes)),
            None => Assets.load(path),
        }
    }

    fn list(&self, path: &str) -> gpui_kit::Result<Vec<SharedString>> {
        let mut paths = Assets.list(path)?;
        paths.extend(ExtraIcons.list(path)?);
        Ok(paths)
    }
}
