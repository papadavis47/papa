//! The open note's images: the strip of thumbnails under the body, adding
//! images from the file picker, a drop or a paste, and deleting them.
//!
//! Adding an image has two slow steps, and each runs where it belongs:
//!
//! ```text
//!  UI thread            GPUI background pool          "papa-store" thread
//!  ─────────            ────────────────────          ───────────────────
//!  add(sources) ──▶ images::prepare_file(path)
//!                     decode, rotate, shrink,
//!                     encode (CPU-bound)  ──────────▶ store.add_image(…)
//!  added(result) ◀──────────────────────────────────── INSERT (I/O)
//! ```

use std::path::PathBuf;
use std::sync::Arc;

use gpui_kit::component::{
    ActiveTheme as _, Icon, WindowExt as _,
    attachment::{Attachment, AttachmentMedia, AttachmentStatus},
    h_flex,
    notification::Notification,
    scroll::ScrollableElement as _,
};
use gpui_kit::{
    App, AppContext as _, Axis, ClickEvent, ClipboardEntry, ClipboardItem, Context, Entity,
    EventEmitter, Image, InteractiveElement as _, IntoElement, ParentElement as _,
    PathPromptOptions, Render, ScrollHandle, SharedString, StatefulInteractiveElement as _,
    Styled as _, Subscription, Task, Window, div, point, px,
};
use papa_core::{
    EncodedImage, ImageFormat, ImageId, NoteId, NoteImage, PreparedImage, Store, images,
};

use super::lightbox::{Lightbox, LightboxEvent};
use super::{dialogs, sentence};

/// Side of a thumbnail tile in the strip.
const TILE: f32 = 88.;

/// An image as the gallery and lightbox hold it: the thumbnail ready for
/// `img()`, the rest left in the vault.
#[derive(Clone)]
pub struct GalleryImage {
    pub id: ImageId,
    pub width: u32,
    pub height: u32,
    pub thumb: Arc<Image>,
}

impl From<NoteImage> for GalleryImage {
    fn from(image: NoteImage) -> Self {
        Self {
            id: image.id,
            width: image.width,
            height: image.height,
            thumb: gpui_image(image.thumb),
        }
    }
}

/// Wraps encoded bytes for `img()`. GPUI decodes them on a background
/// thread the first time they're drawn, and caches the result.
pub fn gpui_image(image: EncodedImage) -> Arc<Image> {
    let format = match image.format {
        ImageFormat::Jpeg => gpui_kit::ImageFormat::Jpeg,
        ImageFormat::Png => gpui_kit::ImageFormat::Png,
        ImageFormat::Gif => gpui_kit::ImageFormat::Gif,
        ImageFormat::Webp => gpui_kit::ImageFormat::Webp,
    };
    Arc::new(Image::from_bytes(format, image.bytes))
}

/// Frees what GPUI keeps for an image that won't be drawn again: the
/// decoded pixels in its asset cache, and their texture on the GPU. GPUI
/// keeps both for as long as the app runs unless told otherwise, and a
/// full-size image takes 16 MB.
pub fn release(image: Arc<Image>, window: &mut Window, cx: &mut App) {
    if let Some(pixels) = image.clone().get_render_image(window, cx) {
        cx.drop_image(pixels, Some(window));
    }
    image.remove_asset(cx);
}

/// What the gallery tells the editor.
#[derive(Clone, Copy, Debug)]
pub enum GalleryEvent {
    /// An image was added to or removed from this note.
    Changed(NoteId),
}

/// Where an image to add comes from.
enum Source {
    File(PathBuf),
    /// Encoded bytes from the clipboard.
    Pasted(Vec<u8>),
}

impl Source {
    /// How a notification refers to it.
    fn name(&self) -> String {
        match self {
            Source::File(path) => match path.file_name() {
                Some(name) => format!("“{}”", name.to_string_lossy()),
                None => "the image".into(),
            },
            Source::Pasted(_) => "the pasted image".into(),
        }
    }

    /// The slow part: reading, decoding and resizing. Runs on a background
    /// thread, which is why `Source` holds only owned, `Send` data.
    fn prepare(self) -> papa_core::Result<PreparedImage> {
        match self {
            Source::File(path) => images::prepare_file(&path),
            Source::Pasted(bytes) => images::prepare(bytes),
        }
    }
}

pub struct Gallery {
    store: Store,
    note: Option<NoteId>,
    /// The open note's images, in the order they were added.
    images: Vec<GalleryImage>,
    /// Images on their way into the vault: one entry each, naming the note
    /// they're for. The open note's show as placeholder tiles.
    pending: Vec<NoteId>,
    /// Loads the open note's images. Replacing it cancels a load for a note
    /// that's no longer open.
    loading: Option<Task<()>>,
    lightbox: Entity<Lightbox>,
    /// Shared by the strip and its scrollbar.
    scroll: ScrollHandle,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<GalleryEvent> for Gallery {}

impl Gallery {
    pub fn new(store: Store, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let lightbox = cx.new(|cx| Lightbox::new(store.clone(), cx));
        let subscriptions = vec![cx.subscribe_in(&lightbox, window, Self::on_lightbox_event)];
        Self {
            store,
            note: None,
            images: Vec::new(),
            pending: Vec::new(),
            loading: None,
            lightbox,
            scroll: ScrollHandle::new(),
            _subscriptions: subscriptions,
        }
    }

    /// The full-size viewer. `PapaApp` draws it over the whole window.
    pub fn lightbox(&self) -> &Entity<Lightbox> {
        &self.lightbox
    }

    /// Switches to `note`'s images, freeing the previous note's.
    pub fn show(&mut self, note: Option<NoteId>, window: &mut Window, cx: &mut Context<Self>) {
        if self.note == note {
            return;
        }
        self.note = note;
        // Each note's strip starts at its first image.
        self.scroll.set_offset(point(px(0.), px(0.)));
        self.lightbox
            .update(cx, |lightbox, cx| lightbox.close(window, cx));
        for image in self.images.drain(..) {
            release(image.thumb, window, cx);
        }
        self.loading = note.map(|note| {
            let store = self.store.clone();
            cx.spawn_in(window, async move |this, cx| {
                let result = store.note_images(note).await;
                this.update_in(cx, |this, window, cx| match result {
                    Ok(images) => {
                        this.images = images.into_iter().map(GalleryImage::from).collect();
                        cx.notify();
                    }
                    Err(err) => window.push_notification(
                        Notification::error(sentence(err)).title("Couldn't load the images"),
                        cx,
                    ),
                })
                .ok();
            })
        });
        cx.notify();
    }

    /// Asks for image files with the system's file picker.
    pub fn pick_files(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // The note open now, even if the father moves on while choosing.
        let Some(note) = self.note else { return };
        let picked = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: Some("Add".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            // The outer `Result` fails if the picker went away without an
            // answer; the inner one if it couldn't open. `None` is Cancel.
            let picked = picked.await;
            this.update_in(cx, |this, window, cx| match picked {
                Ok(Ok(Some(paths))) => this.add(
                    note,
                    paths.into_iter().map(Source::File).collect(),
                    window,
                    cx,
                ),
                Ok(Ok(None)) | Err(_) => {}
                Ok(Err(err)) => window.push_notification(
                    Notification::error(sentence(err)).title("Couldn't open the file picker"),
                    cx,
                ),
            })
            .ok();
        })
        .detach();
    }

    /// Adds dropped files to the open note.
    pub fn add_files(&mut self, paths: &[PathBuf], window: &mut Window, cx: &mut Context<Self>) {
        let Some(note) = self.note else { return };
        let sources = paths.iter().cloned().map(Source::File).collect();
        self.add(note, sources, window, cx);
    }

    /// Adds the images or image files on the clipboard to the open note.
    /// Returns `false` if there are none, so the text field pastes text as
    /// usual.
    pub fn paste(
        &mut self,
        item: &ClipboardItem,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(note) = self.note else { return false };
        let mut sources = Vec::new();
        for entry in item.entries() {
            match entry {
                ClipboardEntry::Image(image) => sources.push(Source::Pasted(image.bytes.clone())),
                ClipboardEntry::ExternalPaths(paths) => {
                    sources.extend(paths.paths().iter().cloned().map(Source::File));
                }
                ClipboardEntry::String(_) => {}
            }
        }
        if sources.is_empty() {
            return false;
        }
        self.add(note, sources, window, cx);
        true
    }

    fn add(
        &mut self,
        note: NoteId,
        sources: Vec<Source>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.pending.extend(sources.iter().map(|_| note));
        cx.notify();
        let store = self.store.clone();
        cx.spawn_in(window, async move |this, cx| {
            // One at a time: decoding a photo takes a few hundred MB, and a
            // drop of fifty photos shouldn't take them all at once.
            for source in sources {
                let name = source.name();
                // `background_spawn` runs the closure on GPUI's thread pool
                // and hands back a `Task` to await, so the UI thread stays
                // free. Whatever moves in must be `Send`: owned bytes and
                // paths, no `Rc` and no entities.
                let prepared = cx.background_spawn(async move { source.prepare() }).await;
                let result = match prepared {
                    Ok(image) => store.add_image(note, image).await,
                    Err(err) => Err(err),
                };
                let delivered = this.update_in(cx, |this, window, cx| {
                    this.added(note, &name, result, window, cx)
                });
                if delivered.is_err() {
                    // The window closed; stop working for it.
                    break;
                }
            }
        })
        .detach();
    }

    fn added(
        &mut self,
        note: NoteId,
        name: &str,
        result: papa_core::Result<NoteImage>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(ix) = self.pending.iter().position(|pending| *pending == note) {
            self.pending.remove(ix);
        }
        match result {
            Ok(image) => {
                // Attaching an image the note already has returns that one.
                if self.note == Some(note) && !self.images.iter().any(|i| i.id == image.id) {
                    self.images.push(image.into());
                    self.sync_lightbox(window, cx);
                }
                cx.emit(GalleryEvent::Changed(note));
            }
            Err(err) => window.push_notification(
                Notification::error(sentence(err)).title(format!("Couldn't add {name}")),
                cx,
            ),
        }
        cx.notify();
    }

    fn open_lightbox(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let images = self.images.clone();
        self.lightbox
            .update(cx, |lightbox, cx| lightbox.open(images, index, window, cx));
    }

    /// Keeps an open lightbox in step with the strip.
    fn sync_lightbox(&self, window: &mut Window, cx: &mut Context<Self>) {
        let images = self.images.clone();
        self.lightbox
            .update(cx, |lightbox, cx| lightbox.set_images(images, window, cx));
    }

    fn on_lightbox_event(
        &mut self,
        _: &Entity<Lightbox>,
        event: &LightboxEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match *event {
            LightboxEvent::Delete(id) => self.confirm_delete(id, window, cx),
        }
    }

    fn confirm_delete(&mut self, id: ImageId, window: &mut Window, cx: &mut Context<Self>) {
        dialogs::confirm_delete(
            "Delete this image?".into(),
            "It is removed from the vault. This can't be undone.",
            cx.listener(move |this, _: &ClickEvent, window, cx| this.delete(id, window, cx)),
            window,
            cx,
        );
    }

    fn delete(&mut self, id: ImageId, window: &mut Window, cx: &mut Context<Self>) {
        let Some(note) = self.note else { return };
        let store = self.store.clone();
        cx.spawn_in(window, async move |this, cx| {
            let result = store.delete_image(id).await;
            this.update_in(cx, |this, window, cx| match result {
                Ok(()) => {
                    if let Some(ix) = this.images.iter().position(|image| image.id == id) {
                        let image = this.images.remove(ix);
                        this.sync_lightbox(window, cx);
                        release(image.thumb, window, cx);
                    }
                    cx.emit(GalleryEvent::Changed(note));
                    cx.notify();
                }
                Err(err) => window.push_notification(
                    Notification::error(sentence(err)).title("Couldn't delete the image"),
                    cx,
                ),
            })
            .ok();
        })
        .detach();
    }
}

impl Render for Gallery {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let pending = self.note.map_or(0, |note| {
            self.pending
                .iter()
                .filter(|pending| **pending == note)
                .count()
        });
        if self.images.is_empty() && pending == 0 {
            return div().into_any_element();
        }

        let tiles = self.images.iter().enumerate().map(|(ix, image)| {
            let id = image.id;
            Attachment::new()
                .id(SharedString::from(format!("image-{id}")))
                // Vertical without text: a square tile the image fills.
                .axis(Axis::Vertical)
                .size(px(TILE))
                .media(AttachmentMedia::new().src(image.thumb.clone()))
                .on_click(
                    cx.listener(move |this, _, window, cx| this.open_lightbox(ix, window, cx)),
                )
                .on_remove(cx.listener(move |this, _, window, cx| {
                    // The × sits on the tile: without this, the tile's own
                    // click would also open the lightbox.
                    cx.stop_propagation();
                    this.confirm_delete(id, window, cx)
                }))
        });
        let placeholders = (0..pending).map(|ix| {
            Attachment::new()
                .id(("adding-image", ix))
                .axis(Axis::Vertical)
                .size(px(TILE))
                .status(AttachmentStatus::Processing)
                .tooltip("Adding…")
                .media(AttachmentMedia::new().child(Icon::new(gpui_kit::assets::IconName::Image)))
        });

        div()
            .relative()
            .flex_none()
            .border_t_1()
            .border_color(cx.theme().border)
            .child(
                h_flex()
                    .id("gallery")
                    .gap_3()
                    .px_6()
                    .py_3()
                    .children(tiles)
                    .children(placeholders)
                    // Plain GPUI scrolling rather than gpui-kit's
                    // `overflow_x_scrollbar`: on an area that scrolls only
                    // sideways, GPUI turns a mouse wheel's vertical turns into
                    // sideways scrolling, which the wrapper switches off.
                    .overflow_x_scroll()
                    .track_scroll(&self.scroll),
            )
            // Drawn over the strip; shows while scrolling.
            .horizontal_scrollbar(&self.scroll)
            .into_any_element()
    }
}
