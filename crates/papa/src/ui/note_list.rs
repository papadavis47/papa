use std::rc::Rc;

use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, IconName, Sizable as _, StyledExt as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    input::{Input, InputState},
    scroll::ScrollableElement as _,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    App, Div, Entity, InteractiveElement as _, IntoElement, ParentElement as _, RenderOnce,
    SharedString, Stateful, StatefulInteractiveElement as _, Styled as _, UniformListScrollHandle,
    Window, div, rems, rgb, uniform_list,
};
use jiff::Zoned;
use papa_core::settings::Density;
use papa_core::{Child, NoteId, NoteSummary};

use super::dates;
use crate::app::{APP_CONTEXT, NewNote};

/// Key context around the search box. Arrow keys bound to `"NoteSearch >
/// Input"` move through the list while the father types a search.
pub const NOTE_SEARCH_CONTEXT: &str = "NoteSearch";

/// What the note list can ask its parent to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NoteListEvent {
    Select(NoteId),
    NewNote,
}

type OnEvent = Rc<dyn Fn(&NoteListEvent, &mut Window, &mut App)>;

/// The middle pane: a heading with the note count, a search box and the
/// notes, newest first.
///
/// The list is virtualized: `uniform_list` asks for only the rows in view,
/// so a thousand notes cost no more to draw than ten.
#[derive(IntoElement)]
pub struct NoteList {
    heading: SharedString,
    /// `Rc`, because the row builder outlives this render and runs again
    /// whenever the list scrolls. Cloning it is a reference count bump.
    notes: Rc<Vec<NoteSummary>>,
    children: Rc<Vec<Child>>,
    selected: Option<NoteId>,
    search: Entity<InputState>,
    scroll: UniformListScrollHandle,
    /// Shown instead of the list when it's empty.
    empty_message: SharedString,
    /// Compact rows are one line: no preview of the text.
    density: Density,
    /// `false` until the vault is open.
    enabled: bool,
    on_event: OnEvent,
}

impl NoteList {
    #[allow(clippy::too_many_arguments)] // one per thing the pane shows
    pub fn new(
        heading: impl Into<SharedString>,
        notes: Rc<Vec<NoteSummary>>,
        children: Rc<Vec<Child>>,
        selected: Option<NoteId>,
        search: Entity<InputState>,
        scroll: UniformListScrollHandle,
        empty_message: impl Into<SharedString>,
        density: Density,
        enabled: bool,
        on_event: impl Fn(&NoteListEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        Self {
            heading: heading.into(),
            notes,
            children,
            selected,
            search,
            scroll,
            empty_message: empty_message.into(),
            density,
            enabled,
            on_event: Rc::new(on_event),
        }
    }
}

impl RenderOnce for NoteList {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let on_new = self.on_event.clone();
        let header = h_flex()
            .gap_2()
            .px_3()
            .pt_3()
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .font_semibold()
                    .child(self.heading),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(self.notes.len().to_string()),
            )
            .child(
                Button::new("new-note")
                    .ghost()
                    .small()
                    .icon(IconName::Plus)
                    .disabled(!self.enabled)
                    .tooltip_with_action("New note", &NewNote, Some(APP_CONTEXT))
                    .on_click(move |_, window, cx| on_new(&NoteListEvent::NewNote, window, cx)),
            );

        let search = div().key_context(NOTE_SEARCH_CONTEXT).px_3().py_2().child(
            Input::new(&self.search)
                .small()
                .prefix(Icon::new(IconName::Search).text_color(cx.theme().muted_foreground))
                .cleanable(true)
                .disabled(!self.enabled),
        );

        let body = if self.notes.is_empty() {
            v_flex()
                .flex_1()
                .items_center()
                .justify_center()
                .px_6()
                .text_sm()
                .text_center()
                .text_color(cx.theme().muted_foreground)
                .child(self.empty_message)
                .into_any_element()
        } else {
            let notes = self.notes.clone();
            let rows = move |range: std::ops::Range<usize>, _: &mut Window, cx: &mut App| {
                let now = Zoned::now();
                range
                    .map(|ix| {
                        let note = &notes[ix];
                        let on_event = self.on_event.clone();
                        let select = NoteListEvent::Select(note.id);
                        let selected = self.selected == Some(note.id);
                        row(note, selected, self.density, &self.children, &now, cx)
                            .on_click(move |_, window, cx| on_event(&select, window, cx))
                    })
                    .collect::<Vec<_>>()
            };
            // `uniform_list` reports its rows' unwrapped width as its
            // minimum, which would stretch the pane; `min_w_0` and
            // `overflow_hidden` keep it to the pane's width so rows truncate.
            div()
                .flex_1()
                .min_h_0()
                .min_w_0()
                .overflow_hidden()
                .relative()
                .child(
                    uniform_list("notes", self.notes.len(), rows)
                        .track_scroll(&self.scroll)
                        .size_full()
                        .px_2()
                        .pb_2(),
                )
                .vertical_scrollbar(&self.scroll)
                .into_any_element()
        };

        v_flex()
            .size_full()
            .min_w_0()
            .child(header)
            .child(search)
            .child(body)
    }
}

/// One note: title and date, then the children's colours and the start of
/// the text. Compact rows leave out the second line, and show the children's
/// colours next to the title instead. Every row has the same height, as
/// `uniform_list` requires.
///
/// A plain `div` rather than gpui-kit's `ListItem`, whose inner wrapper
/// can't shrink below its text and so pushed the date out of view.
fn row(
    note: &NoteSummary,
    selected: bool,
    density: Density,
    children: &[Child],
    now: &Zoned,
    cx: &App,
) -> Stateful<Div> {
    let title: SharedString = if note.title.is_empty() {
        "Untitled".into()
    } else {
        note.title.clone().into()
    };
    let dots = note.children.iter().filter_map(|id| {
        let child = children.iter().find(|child| child.id == *id)?;
        Some(
            div()
                .flex_none()
                .size_2()
                .rounded_full()
                .bg(rgb(child.color)),
        )
    });
    let preview: SharedString = if note.preview.is_empty() {
        "No text".into()
    } else {
        note.preview.clone().into()
    };
    let list_hover = cx.theme().list_hover;
    let compact = density == Density::Compact;
    let images = (note.images > 0).then(|| {
        h_flex()
            .flex_none()
            .gap_0p5()
            .text_xs()
            .text_color(cx.theme().muted_foreground)
            .child(Icon::new(gpui_kit::assets::IconName::Image).xsmall())
            .child(note.images.to_string())
    });

    let row = v_flex()
        .id(SharedString::from(note.id.to_string()))
        .w_full()
        .gap_0p5()
        .px_3()
        .map(|row| if compact { row.py_1() } else { row.py_1p5() })
        .rounded(cx.theme().radius)
        .cursor_pointer()
        .map(|row| {
            if selected {
                row.bg(cx.theme().list_active)
            } else {
                row.hover(|row| row.bg(list_hover))
            }
        });
    let date = div()
        .flex_none()
        .text_xs()
        .text_color(cx.theme().muted_foreground)
        .child(dates::short(note.created_at, now));
    let title = div()
        .flex_1()
        .min_w_0()
        .truncate()
        .font_medium()
        .child(title);

    if compact {
        return row.child(
            h_flex()
                .gap_2()
                .child(title)
                .children(dots)
                .children(images)
                .child(date),
        );
    }
    row.child(h_flex().gap_2().child(title).child(date)).child(
        h_flex()
            // In rems, so the row grows with the text size.
            .h(rems(1.125))
            .gap_1()
            .text_sm()
            .text_color(cx.theme().muted_foreground)
            .children(dots)
            .child(div().flex_1().min_w_0().truncate().child(preview))
            .children(images),
    )
}
