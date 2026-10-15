//! The right pane in child mode: one note Dad shared, read-only. Its title
//! and date, the Markdown rendered in the reading font, and its photos.
//!
//! The father's [`NoteEditor`](super::NoteEditor) edits, saves, assigns
//! and labels; none of that applies here, so this is a view of its own
//! rather than the editor with half of it switched off.

use std::time::Duration;

use gpui_kit::component::{
    ActiveTheme as _, IconName, Sizable as _, StyledExt as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    text::TextView,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    Action, Animation, AnimationExt as _, App, AppContext as _, Context, Entity, FocusHandle,
    InteractiveElement as _, IntoElement, ParentElement as _, Render, SharedString, Styled as _,
    Window, div, ease_out_quint, px, relative,
};
use jiff::tz::TimeZone;
use papa_core::settings::Density;
use papa_core::{ChildSession, SharedNote};

use super::empty_state::EmptyState;
use super::gallery::{Gallery, ImageSource};
use super::{dates, shortcut};
use crate::app::{
    APP_CONTEXT, ExportNote, ExportNotes, FocusNoteList, FocusSearch, NOTE_EDITOR_CONTEXT, SyncNow,
};
use crate::settings::AppSettings;
use crate::theme;

/// How long a note takes to fade in, as in the editor.
const NOTE_FADE: Duration = Duration::from_millis(140);

pub struct NoteReader {
    note: Option<SharedNote>,
    gallery: Entity<Gallery>,
    /// Focused while the pane is active, so Esc goes back to the list.
    focus_handle: FocusHandle,
}

impl NoteReader {
    pub fn new(session: ChildSession, window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self {
            note: None,
            gallery: cx.new(|cx| Gallery::new(ImageSource::Shared(session), window, cx)),
            focus_handle: cx.focus_handle(),
        }
    }

    pub fn gallery(&self) -> &Entity<Gallery> {
        &self.gallery
    }

    pub fn note(&self) -> Option<&SharedNote> {
        self.note.as_ref()
    }

    /// Shows `note`, or nothing. A newer version of the open note (after a
    /// pull) replaces it in place, and reloads its photos if they changed.
    pub fn show(&mut self, note: Option<SharedNote>, window: &mut Window, cx: &mut Context<Self>) {
        if self.note == note {
            return;
        }
        let id = note.as_ref().map(|note| note.id);
        let images_changed = match (&self.note, &note) {
            (Some(old), Some(new)) => old.id == new.id && old.images != new.images,
            _ => false,
        };
        self.gallery.update(cx, |gallery, cx| {
            if images_changed {
                gallery.show(None, window, cx);
            }
            gallery.show(id, window, cx);
        });
        self.note = note;
        cx.notify();
    }

    /// Focused while the pane is active: Esc goes back to the list.
    pub fn focus_handle(&self) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for NoteReader {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let root = v_flex()
            .size_full()
            .key_context(NOTE_EDITOR_CONTEXT)
            .track_focus(&self.focus_handle);
        let Some(note) = &self.note else {
            return root.child(hints(window, cx));
        };
        let settings = AppSettings::get(cx);
        let compact = settings.appearance.density == Density::Compact;
        let line_height = relative(settings.appearance.line_height);
        let title: SharedString = match note.title.trim() {
            "" => "Untitled".into(),
            title => title.to_owned().into(),
        };

        let header = v_flex()
            .gap_2()
            .map(|header| {
                if compact {
                    header.px_4().pt_2().pb_2()
                } else {
                    header.px_6().pt_4().pb_3()
                }
            })
            .border_b_1()
            .border_color(cx.theme().border)
            .child(div().text_xl().font_semibold().child(title))
            .child(
                h_flex()
                    .gap_3()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(dates::long(note.created_at, &TimeZone::system()))
                    .child(div().flex_1())
                    .child(
                        Button::new("export-note")
                            .ghost()
                            .small()
                            .icon(gpui_kit::assets::IconName::FileDown)
                            .tooltip_with_action("Export to PDF", &ExportNote, Some(APP_CONTEXT))
                            // Handled by `ChildApp`, which owns exports.
                            .on_click(|_, window, cx| {
                                window.dispatch_action(Box::new(ExportNote), cx)
                            }),
                    ),
            );

        let body = if note.body.trim().is_empty() {
            div()
                .text_color(cx.theme().muted_foreground)
                .child("No text, just photos.")
                .into_any_element()
        } else {
            // Text style is inherited, so the rendered Markdown takes the
            // reading font and line height from here.
            div()
                .size_full()
                .font_family(theme::reading_font(cx))
                .line_height(line_height)
                .child(
                    TextView::markdown(
                        SharedString::from(format!("shared-{}", note.id)),
                        note.body.clone(),
                    )
                    .scrollable(true)
                    .size_full(),
                )
                .into_any_element()
        };

        let content = v_flex()
            .size_full()
            .child(header)
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .map(|body| {
                        if compact {
                            body.px_4().py_2()
                        } else {
                            body.px_6().py_4()
                        }
                    })
                    .child(body),
            )
            .child(self.gallery.clone());

        root.child(content.with_animation(
            SharedString::from(format!("shared-note-{}", note.id)),
            Animation::new(NOTE_FADE).with_easing(ease_out_quint()),
            |content, t| content.opacity(t),
        ))
    }
}

/// With no note open: the keys that get somewhere from here.
fn hints(window: &Window, cx: &App) -> impl IntoElement {
    let muted = cx.theme().muted_foreground;
    let hint = |label: &'static str, actions: &[&dyn Action]| {
        h_flex()
            .justify_between()
            .gap_4()
            .text_sm()
            .child(SharedString::from(label))
            .child(
                h_flex().gap_1().text_color(muted).children(
                    actions
                        .iter()
                        .filter_map(|action| shortcut(*action, window)),
                ),
            )
    };
    let hints = v_flex()
        .w(px(260.))
        .gap_2()
        .child(hint("Search the notes", &[&FocusSearch]))
        .child(hint("Go to the list", &[&FocusNoteList]))
        .child(hint("Check for new notes", &[&SyncNow]))
        .child(hint("Save them all as a PDF", &[&ExportNotes]));
    v_flex().size_full().child(
        EmptyState::new("no-shared-note", IconName::FileText, "No note open")
            .description("Pick one from the list.")
            .child(hints),
    )
}
