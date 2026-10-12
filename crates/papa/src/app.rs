//! The root view: owns the app shell's state and lays out the window.

use gpui_kit::component::{
    ActiveTheme as _, h_resizable, resizable_panel, status_bar::StatusBar, v_flex,
};
use gpui_kit::{
    App, Context, FocusHandle, InteractiveElement as _, IntoElement, KeyBinding,
    ParentElement as _, Render, Styled as _, Window, div, px,
};

use crate::placeholder::{ChildId, Journal, NoteId};
use crate::state::Selection;
use crate::theme;
use crate::ui::{AppSidebar, AppTitleBar, NoteList, NoteView};

gpui_kit::actions!(
    papa,
    [
        /// Quit the application.
        Quit,
        /// Switch between the light and dark theme.
        ToggleTheme,
        /// Show or hide the sidebar.
        ToggleSidebar,
        /// Select the note below the current one.
        SelectNextNote,
        /// Select the note above the current one.
        SelectPreviousNote,
    ]
);

/// Key context on the root element: its bindings work anywhere in the window.
pub const APP_CONTEXT: &str = "PapaApp";
/// Key context on the note list: its bindings work only while the list has focus.
pub const NOTE_LIST_CONTEXT: &str = "NoteList";

/// Registers keybindings and app-wide action handlers. Call once at startup.
pub fn init(cx: &mut App) {
    cx.bind_keys([
        // `secondary` is Cmd on macOS and Ctrl elsewhere.
        KeyBinding::new("secondary-q", Quit, None),
        KeyBinding::new("secondary-shift-l", ToggleTheme, None),
        KeyBinding::new("secondary-b", ToggleSidebar, Some(APP_CONTEXT)),
        KeyBinding::new("down", SelectNextNote, Some(NOTE_LIST_CONTEXT)),
        KeyBinding::new("up", SelectPreviousNote, Some(NOTE_LIST_CONTEXT)),
    ]);

    // App-level handlers: quitting and the theme are global, not owned by a view.
    cx.on_action(|_: &Quit, cx| cx.quit());
    cx.on_action(|_: &ToggleTheme, cx| theme::toggle(cx));

    // Closing the last window ends the app.
    cx.on_window_closed(|cx, _| {
        if cx.windows().is_empty() {
            cx.quit();
        }
    })
    .detach();
}

pub struct PapaApp {
    journal: Journal,
    selection: Selection,
    sidebar_visible: bool,
    /// Clicking an element that tracks a focus handle focuses it. This one is
    /// on the root, so a click anywhere outside the note list still leaves
    /// focus inside `APP_CONTEXT` and the window-wide bindings keep working.
    focus_handle: FocusHandle,
    /// Focused while the note list is active; enables the `NOTE_LIST_CONTEXT` bindings.
    note_list_focus: FocusHandle,
}

impl PapaApp {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let journal = Journal::sample();
        let selection = Selection::first(&journal);
        let note_list_focus = cx.focus_handle();
        // Start in the note list so the arrow keys work straight away.
        note_list_focus.focus(window, cx);

        Self {
            journal,
            selection,
            sidebar_visible: true,
            focus_handle: cx.focus_handle(),
            note_list_focus,
        }
    }

    fn toggle_sidebar(&mut self, _: &ToggleSidebar, _: &mut Window, cx: &mut Context<Self>) {
        self.sidebar_visible = !self.sidebar_visible;
        cx.notify();
    }

    fn select_next_note(&mut self, _: &SelectNextNote, _: &mut Window, cx: &mut Context<Self>) {
        self.move_note(1, cx);
    }

    fn select_previous_note(
        &mut self,
        _: &SelectPreviousNote,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.move_note(-1, cx);
    }

    fn move_note(&mut self, delta: isize, cx: &mut Context<Self>) {
        let visible = self
            .selection
            .child
            .map(|child| self.journal.note_ids_for(child))
            .unwrap_or_default();
        self.selection.move_note(delta, &visible);
        cx.notify();
    }

    fn select_child(&mut self, child: &ChildId, window: &mut Window, cx: &mut Context<Self>) {
        self.selection.select_child(*child, &self.journal);
        // The click landed in the sidebar; hand focus to the list so the
        // arrow keys move through this child's notes.
        self.note_list_focus.focus(window, cx);
        cx.notify();
    }

    fn select_note(&mut self, note: &NoteId, _: &mut Window, cx: &mut Context<Self>) {
        // No need to focus the list here: the click was inside it.
        self.selection.select_note(*note);
        cx.notify();
    }
}

impl Render for PapaApp {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let child = self.selection.child.and_then(|id| self.journal.child(id));
        let notes = child
            .map(|child| self.journal.notes_for(child.id).cloned().collect())
            .unwrap_or_default();
        let note = self
            .selection
            .note
            .and_then(|id| self.journal.note(id))
            .cloned();
        let about = note
            .iter()
            .flat_map(|note| note.children)
            .filter_map(|id| self.journal.child(*id))
            .map(|child| child.name)
            .collect();

        v_flex()
            .key_context(APP_CONTEXT)
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::toggle_sidebar))
            .size_full()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .child(AppTitleBar::new(self.sidebar_visible))
            .child(
                div().flex_1().min_h_0().child(
                    h_resizable("panes")
                        .child(
                            resizable_panel()
                                .visible(self.sidebar_visible)
                                .size(px(220.))
                                .size_range(px(180.)..px(320.))
                                .flex_none()
                                .child(AppSidebar::new(
                                    &self.journal,
                                    self.selection.child,
                                    cx.listener(Self::select_child),
                                )),
                        )
                        .child(
                            resizable_panel()
                                .size(px(300.))
                                .size_range(px(220.)..px(480.))
                                .flex_none()
                                .child(
                                    div()
                                        .size_full()
                                        .key_context(NOTE_LIST_CONTEXT)
                                        .track_focus(&self.note_list_focus)
                                        .on_action(cx.listener(Self::select_next_note))
                                        .on_action(cx.listener(Self::select_previous_note))
                                        .child(NoteList::new(
                                            child.map_or("No child selected", |child| child.name),
                                            notes,
                                            self.selection.note,
                                            cx.listener(Self::select_note),
                                        )),
                                ),
                        )
                        .child(resizable_panel().child(NoteView::new(note, about))),
                ),
            )
            .child(
                StatusBar::new()
                    .left("Sample data")
                    .right(if cx.theme().mode.is_dark() {
                        "Dark"
                    } else {
                        "Light"
                    }),
            )
    }
}
