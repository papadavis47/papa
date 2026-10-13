//! One image at a time, as large as the window allows.
//!
//! An entity, because it keeps state between frames: which image is shown,
//! and the full-size versions loaded so far. `PapaApp` renders it as the
//! last child of the window, so it covers every pane; gpui-kit's dialogs and
//! notifications are drawn above the app and stay visible over it.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use gpui_kit::component::{
    IconName, WindowExt as _,
    button::{Button, ButtonCustomVariant, ButtonVariants as _},
    h_flex,
    notification::Notification,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    App, Context, EventEmitter, FocusHandle, Image, InteractiveElement as _, IntoElement,
    ObjectFit, ParentElement as _, Render, SharedString, Styled as _, StyledImage as _, Window,
    div, hsla, img,
};
use papa_core::{ImageId, Store};

use super::gallery::{GalleryImage, gpui_image, release};
use super::sentence;

gpui_kit::actions!(
    lightbox,
    [
        /// Close the lightbox.
        CloseLightbox,
        /// Show the next image.
        NextImage,
        /// Show the previous image.
        PreviousImage,
    ]
);

/// Key context of the open lightbox: Esc and ←/→ work only inside it.
pub const LIGHTBOX_CONTEXT: &str = "Lightbox";

/// What the lightbox asks of the gallery that opened it.
#[derive(Clone, Copy, Debug)]
pub enum LightboxEvent {
    /// The father asked to delete this image. The gallery confirms first.
    Delete(ImageId),
}

pub struct Lightbox {
    store: Store,
    images: Vec<GalleryImage>,
    /// The image shown, or `None` while closed.
    index: Option<usize>,
    /// Full-size versions loaded so far, kept so going back is instant.
    /// Released when the lightbox closes.
    display: HashMap<ImageId, Arc<Image>>,
    /// Requested from the store, not here yet.
    loading: HashSet<ImageId>,
    focus_handle: FocusHandle,
    /// What had focus before opening, to give it back on close.
    previous_focus: Option<FocusHandle>,
}

impl EventEmitter<LightboxEvent> for Lightbox {}

impl Lightbox {
    pub fn new(store: Store, cx: &mut Context<Self>) -> Self {
        Self {
            store,
            images: Vec::new(),
            index: None,
            display: HashMap::new(),
            loading: HashSet::new(),
            focus_handle: cx.focus_handle(),
            previous_focus: None,
        }
    }

    pub fn is_open(&self) -> bool {
        self.index.is_some()
    }

    /// Shows `images[index]`, taking keyboard focus so Esc and the arrows
    /// reach it.
    pub fn open(
        &mut self,
        images: Vec<GalleryImage>,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.is_open() {
            self.previous_focus = window.focused(cx);
        }
        self.images = images;
        self.focus_handle.focus(window, cx);
        self.show(index, window, cx);
    }

    /// Closes and frees the full-size images.
    pub fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.is_open() {
            return;
        }
        self.index = None;
        self.images.clear();
        for (_, image) in self.display.drain() {
            release(image, window, cx);
        }
        if let Some(previous) = self.previous_focus.take() {
            previous.focus(window, cx);
        }
        cx.notify();
    }

    /// Follows a change to the gallery while open: stays on the same image
    /// if it's still there, otherwise on the one that took its place.
    pub fn set_images(
        &mut self,
        images: Vec<GalleryImage>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(index) = self.index else { return };
        let current = self.images.get(index).map(|image| image.id);
        self.images = images;
        let ids: Vec<ImageId> = self.images.iter().map(|image| image.id).collect();
        match index_after_change(current, index, &ids) {
            Some(index) => self.show(index, window, cx),
            None => self.close(window, cx),
        }
    }

    fn show(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.index = Some(index);
        // The neighbours too, so the arrow keys don't wait for the database.
        for ix in [Some(index), index.checked_sub(1), Some(index + 1)] {
            if let Some(ix) = ix.filter(|ix| *ix < self.images.len()) {
                self.load(self.images[ix].id, window, cx);
            }
        }
        cx.notify();
    }

    /// Fetches an image's full-size version from the vault, unless it's
    /// here or on its way.
    fn load(&mut self, id: ImageId, window: &mut Window, cx: &mut Context<Self>) {
        if self.display.contains_key(&id) || !self.loading.insert(id) {
            return;
        }
        let store = self.store.clone();
        cx.spawn_in(window, async move |this, cx| {
            let result = store.display_image(id).await;
            this.update_in(cx, |this, window, cx| {
                this.loading.remove(&id);
                match result {
                    // Closed meanwhile: nothing to show it in.
                    Ok(_) if !this.is_open() => {}
                    Ok(image) => {
                        this.display.insert(id, gpui_image(image));
                        cx.notify();
                    }
                    Err(err) => window.push_notification(
                        Notification::error(sentence(err)).title("Couldn't open the image"),
                        cx,
                    ),
                }
            })
            .ok();
        })
        .detach();
    }

    fn step(&mut self, delta: isize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(index) = self.index else { return };
        let last = self.images.len().saturating_sub(1);
        let next = index.saturating_add_signed(delta).min(last);
        if next != index {
            self.show(next, window, cx);
        }
    }

    fn close_action(&mut self, _: &CloseLightbox, window: &mut Window, cx: &mut Context<Self>) {
        self.close(window, cx);
    }

    fn next(&mut self, _: &NextImage, window: &mut Window, cx: &mut Context<Self>) {
        self.step(1, window, cx);
    }

    fn previous(&mut self, _: &PreviousImage, window: &mut Window, cx: &mut Context<Self>) {
        self.step(-1, window, cx);
    }
}

impl Render for Lightbox {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some((index, image)) = self
            .index
            .and_then(|index| Some((index, self.images.get(index)?.clone())))
        else {
            return div().into_any_element();
        };
        let count = self.images.len();
        // Until the full-size version is decoded, the thumbnail stands in,
        // scaled up: blurry for a moment rather than blank. It's drawn
        // underneath, as GPUI shows an `img`'s loading placeholder only
        // after 200 ms. Not for images with transparency, which would show
        // their own thumbnail through them; those get the placeholder.
        // (Thumbnails are JPEG exactly when the image is opaque.)
        let underlay = (image.thumb.format == gpui_kit::ImageFormat::Jpeg)
            .then(|| fill(img(image.thumb.clone())));
        let thumb = image.thumb.clone();
        let picture = match self.display.get(&image.id) {
            Some(display) => img(display.clone())
                .with_loading(move || fill(img(thumb.clone())).into_any_element()),
            None => img(thumb),
        }
        // An id gives the image element state between frames, which is
        // where GPUI keeps an animated GIF's current frame. Without one, a
        // GIF stays on its first frame.
        .id(SharedString::from(format!("lightbox-{}", image.id)));
        // White on near-black in both themes, like any photo viewer.
        let muted = hsla(0., 0., 1., 0.7);

        v_flex()
            // Fills the window and swallows clicks and scrolls, so nothing
            // underneath reacts.
            .absolute()
            .inset_0()
            .occlude()
            .key_context(LIGHTBOX_CONTEXT)
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::close_action))
            .on_action(cx.listener(Self::next))
            .on_action(cx.listener(Self::previous))
            .bg(hsla(0., 0., 0.05, 0.96))
            .text_color(muted)
            .child(
                h_flex()
                    .flex_none()
                    .gap_2()
                    .px_4()
                    .py_2()
                    .text_sm()
                    .child(format!("{} of {count}", index + 1))
                    .child(format!("{} × {}", image.width, image.height))
                    .child(div().flex_1())
                    .child(
                        overlay_button("lightbox-delete", cx)
                            .icon(gpui_kit::assets::IconName::Trash)
                            .tooltip("Delete image")
                            .on_click(cx.listener(move |_, _, _, cx| {
                                cx.emit(LightboxEvent::Delete(image.id));
                            })),
                    )
                    .child(
                        overlay_button("lightbox-close", cx)
                            .icon(IconName::Close)
                            .tooltip("Close (Esc)")
                            .on_click(cx.listener(|this, _, window, cx| this.close(window, cx))),
                    ),
            )
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .gap_2()
                    .px_2()
                    .pb_4()
                    .child(arrow(
                        overlay_button("lightbox-previous", cx)
                            .icon(IconName::ChevronLeft)
                            .tooltip("Previous (←)")
                            .on_click(cx.listener(|this, _, window, cx| this.step(-1, window, cx))),
                        index > 0,
                    ))
                    .child(
                        div()
                            .relative()
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .children(underlay)
                            .child(fill(picture)),
                    )
                    .child(arrow(
                        overlay_button("lightbox-next", cx)
                            .icon(IconName::ChevronRight)
                            .tooltip("Next (→)")
                            .on_click(cx.listener(|this, _, window, cx| this.step(1, window, cx))),
                        index + 1 < count,
                    )),
            )
            .into_any_element()
    }
}

/// Which image to show after the list changed from under the one at
/// `index` (whose id was `current`): the same image if it's still there,
/// otherwise the one that took its place, or the new last one. `None` once
/// the list is empty.
fn index_after_change(current: Option<ImageId>, index: usize, ids: &[ImageId]) -> Option<usize> {
    let last = ids.len().checked_sub(1)?;
    let same = current.and_then(|current| ids.iter().position(|id| *id == current));
    Some(same.unwrap_or(index.min(last)))
}

/// Stretches an image over its parent, shrunk to fit and centred.
///
/// The image is taken out of the layout (`absolute`) and given the parent's
/// size. Laid out normally, an image asks for its own size, and a large
/// photo would push past the window instead of shrinking to fit.
fn fill<E: gpui_kit::Styled + gpui_kit::StyledImage>(image: E) -> E {
    image
        .absolute()
        .inset_0()
        .size_full()
        .object_fit(ObjectFit::Contain)
}

/// A previous/next button. Hidden (but still taking its space, so the
/// image doesn't jump) at either end.
fn arrow(button: Button, visible: bool) -> impl IntoElement {
    div()
        .flex_none()
        .w_10()
        .when(visible, |this| this.child(button))
}

/// A borderless button in light colours: the theme's own buttons are
/// meant for its background, and would vanish on the dark backdrop in the
/// light theme.
fn overlay_button(id: &'static str, cx: &App) -> Button {
    let light = |alpha| hsla(0., 0., 1., alpha);
    Button::new(id).custom(
        ButtonCustomVariant::new(cx)
            .color(light(0.))
            .foreground(light(0.85))
            .hover(light(0.12))
            .active(light(0.2))
            .shadow(false),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stays_on_the_same_image_when_others_change() {
        let [a, b, c] = [ImageId::new(), ImageId::new(), ImageId::new()];

        assert_eq!(index_after_change(Some(b), 1, &[a, b, c]), Some(1));
        assert_eq!(
            index_after_change(Some(b), 1, &[b, c]),
            Some(0),
            "a deleted"
        );
        assert_eq!(
            index_after_change(Some(b), 1, &[a, b, c, ImageId::new()]),
            Some(1)
        );
    }

    #[test]
    fn a_deleted_image_is_replaced_by_its_neighbour() {
        let [a, b, c] = [ImageId::new(), ImageId::new(), ImageId::new()];

        assert_eq!(index_after_change(Some(b), 1, &[a, c]), Some(1), "the next");
        assert_eq!(index_after_change(Some(c), 2, &[a, b]), Some(1), "the last");
        assert_eq!(index_after_change(Some(a), 0, &[]), None, "nothing left");
    }
}
