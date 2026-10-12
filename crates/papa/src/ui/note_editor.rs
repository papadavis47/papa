//! The right pane: one note's title and Markdown body, saved as you type.

use std::rc::Rc;
use std::time::Duration;

use gpui_kit::component::{
    ActiveTheme as _, IconName, Sizable as _, StyledExt as _, WindowExt as _,
    button::{Button, ButtonVariants as _},
    checkbox::Checkbox,
    h_flex,
    input::{Input, InputEvent, InputState, RopeExt as _, Textarea, TextareaState},
    notification::Notification,
    popover::Popover,
    tab::{Tab, TabBar},
    tag::Tag,
    text::TextView,
    v_flex,
};
use gpui_kit::{
    AnyElement, App, AppContext as _, ClickEvent, Context, Entity, EventEmitter, FocusHandle,
    InteractiveElement as _, IntoElement, ParentElement as _, Render, SharedString, Styled as _,
    Subscription, Task, WeakEntity, Window, div, rgb,
};
use jiff::tz::TimeZone;
use papa_core::{Child, ChildId, Label, LabelId, Note, NoteId, Store};

use super::{NoteView, dates, dialogs, sentence};

/// How long typing must pause before the note is saved.
pub const AUTOSAVE_DELAY: Duration = Duration::from_millis(500);

/// What the editor tells its parent. `PapaApp` subscribes to these to keep
/// the note list in step.
#[derive(Clone, Debug)]
pub enum NoteEditorEvent {
    /// The note was stored: its text, children or labels changed.
    Saved(Note),
    Deleted(NoteId),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    Edit,
    Preview,
}

/// Edits one note at a time.
///
/// An entity, because it holds state that outlives a frame: the text
/// fields, which note is open and whether it has unsaved changes.
pub struct NoteEditor {
    store: Store,
    /// The open note as last stored. Its title and body may lag behind the
    /// text fields until the next save; its children and labels are current.
    note: Option<Note>,
    /// For the children and labels the note can be assigned to.
    children: Rc<Vec<Child>>,
    labels: Rc<Vec<Label>>,
    title: Entity<InputState>,
    body: Entity<TextareaState>,
    mode: Mode,
    /// The body shown in preview. Copied from the text field on entering
    /// preview, rather than on every frame.
    preview: SharedString,
    /// The text changed since the last save started.
    dirty: bool,
    /// Set when a note opens: the first switch to editing puts the cursor
    /// at the end of the body, ready to add to it.
    cursor_to_end: bool,
    /// The pending autosave: a timer, then a save. Replacing it drops the
    /// old task, and dropping a GPUI task cancels it, so only the timer
    /// started by the latest keystroke ever fires.
    autosave: Option<Task<()>>,
    /// Focused in preview, so keys don't go to the hidden text field.
    focus_handle: FocusHandle,
    /// Dropping a `Subscription` unsubscribes, so they live as long as we do.
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<NoteEditorEvent> for NoteEditor {}

impl NoteEditor {
    pub fn new(store: Store, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let title = cx.new(|cx| InputState::new(window, cx).placeholder("Untitled"));
        let body = cx.new(|cx| {
            TextareaState::new(window, cx)
                .placeholder("Write about the day… Markdown works: **bold**, # Heading, - list")
                .soft_wrap(true)
        });
        let subscriptions = vec![
            cx.subscribe_in(&title, window, Self::on_title_event),
            cx.subscribe_in(&body, window, Self::on_body_event),
            // GPUI awaits the returned future (briefly) before exiting.
            cx.on_app_quit(Self::save_before_quit),
        ];
        Self {
            store,
            note: None,
            children: Rc::default(),
            labels: Rc::default(),
            title,
            body,
            mode: Mode::Preview,
            preview: SharedString::default(),
            dirty: false,
            cursor_to_end: false,
            autosave: None,
            focus_handle: cx.focus_handle(),
            _subscriptions: subscriptions,
        }
    }

    pub fn note_id(&self) -> Option<NoteId> {
        self.note.as_ref().map(|note| note.id)
    }

    /// Shows `note`, saving the previous one first if it changed.
    pub fn open(&mut self, note: Note, window: &mut Window, cx: &mut Context<Self>) {
        if self.note_id() == Some(note.id) {
            // Already open, maybe with unsaved typing: keep it.
            return;
        }
        self.flush(window, cx);
        // `set_value` doesn't emit `InputEvent::Change`, so loading a note
        // doesn't count as an edit.
        self.title.update(cx, |input, cx| {
            input.set_value(note.title.clone(), window, cx)
        });
        self.body.update(cx, |input, cx| {
            input.set_value(note.body.clone(), window, cx)
        });
        self.preview = note.body.clone().into();
        self.cursor_to_end = true;
        if note.title.is_empty() && note.body.is_empty() {
            self.mode = Mode::Edit;
        }
        self.note = Some(note);
        cx.notify();
    }

    /// Saves and clears the editor.
    pub fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.flush(window, cx);
        self.note = None;
        cx.notify();
    }

    /// Saves now rather than when the timer fires.
    pub fn flush(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.autosave = None;
        self.save(window, cx);
    }

    pub fn focus_title(&self, window: &mut Window, cx: &mut App) {
        self.title.update(cx, |input, cx| input.focus(window, cx));
    }

    /// Switches between editing the Markdown and reading it rendered.
    pub fn toggle_mode(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.mode {
            Mode::Edit => self.show_preview(window, cx),
            Mode::Preview => self.edit_body(window, cx),
        }
    }

    /// Updates the children and labels the note can be assigned to. Links to
    /// deleted ones are dropped: the vault already removed them.
    pub fn set_catalog(
        &mut self,
        children: Rc<Vec<Child>>,
        labels: Rc<Vec<Label>>,
        cx: &mut Context<Self>,
    ) {
        if let Some(note) = &mut self.note {
            note.children
                .retain(|id| children.iter().any(|child| child.id == *id));
            note.labels
                .retain(|id| labels.iter().any(|label| label.id == *id));
        }
        self.children = children;
        self.labels = labels;
        cx.notify();
    }

    fn edit_body(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.mode = Mode::Edit;
        let to_end = std::mem::take(&mut self.cursor_to_end);
        self.body.update(cx, |input, cx| {
            if to_end {
                let end = input.text().offset_to_position(input.text().len());
                // Also focuses the field.
                input.set_cursor_position(end, window, cx);
            } else {
                input.focus(window, cx);
            }
        });
        cx.notify();
    }

    fn show_preview(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.mode = Mode::Preview;
        self.preview = self.body.read(cx).value();
        self.focus_handle.focus(window, cx);
        cx.notify();
    }

    // --- Saving ---

    fn on_title_event(
        &mut self,
        _: &Entity<InputState>,
        event: &InputEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            InputEvent::Change => self.changed(window, cx),
            // Enter in the title moves on to the body.
            InputEvent::PressEnter { .. } => self.edit_body(window, cx),
            _ => {}
        }
    }

    fn on_body_event(
        &mut self,
        _: &Entity<TextareaState>,
        event: &InputEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let InputEvent::Change = event {
            self.changed(window, cx);
        }
    }

    /// The text changed: save once typing pauses for [`AUTOSAVE_DELAY`].
    fn changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.note.is_none() {
            return;
        }
        self.dirty = true;
        self.autosave = Some(cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(AUTOSAVE_DELAY).await;
            this.update_in(cx, |this, window, cx| this.save(window, cx))
                .ok();
        }));
    }

    /// Starts saving the text if it changed since the last save.
    fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(save) = self.start_save(cx) else {
            return;
        };
        // Detached: the save finishes even if the editor moves on to another
        // note (or goes away) meanwhile.
        cx.spawn_in(window, async move |this, cx| {
            let (id, result) = save.await;
            this.update_in(cx, |this, window, cx| this.saved(id, result, window, cx))
                .ok();
        })
        .detach();
    }

    /// The unsaved text as a store request, or `None` if nothing changed.
    ///
    /// Reads the text now, so the request is independent of the editor: it
    /// owns its strings and its own `Store` handle. That's what lets it run
    /// detached, or while the app quits. `use<>` says so to the compiler:
    /// the future borrows nothing, neither `self` nor `cx`.
    fn start_save(
        &mut self,
        cx: &App,
    ) -> Option<impl Future<Output = (NoteId, papa_core::Result<Note>)> + use<>> {
        let id = self.note_id().filter(|_| self.dirty)?;
        self.dirty = false;
        let title = self.title.read(cx).value().to_string();
        let body = self.body.read(cx).value().to_string();
        let store = self.store.clone();
        Some(async move { (id, store.update_note_text(id, title, body).await) })
    }

    fn saved(
        &mut self,
        id: NoteId,
        result: papa_core::Result<Note>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match result {
            Ok(saved) => {
                if let Some(note) = self.note.as_mut().filter(|note| note.id == id) {
                    note.title = saved.title.clone();
                    note.body = saved.body.clone();
                    note.updated_at = saved.updated_at;
                }
                cx.emit(NoteEditorEvent::Saved(saved));
            }
            Err(err) => {
                if self.note_id() == Some(id) {
                    // Try again with the next keystroke.
                    self.dirty = true;
                }
                window.push_notification(
                    Notification::error(sentence(err)).title("Couldn't save the note"),
                    cx,
                );
            }
        }
    }

    fn save_before_quit(&mut self, cx: &mut Context<Self>) -> impl Future<Output = ()> + use<> {
        let save = self.start_save(cx);
        async move {
            if let Some(save) = save {
                // Nowhere left to report an error.
                let _ = save.await;
            }
        }
    }

    // --- Children and labels ---

    fn toggle_child(&mut self, id: ChildId, window: &mut Window, cx: &mut Context<Self>) {
        let Some(note) = &mut self.note else { return };
        toggle(&mut note.children, id);
        let (note_id, children) = (note.id, note.children.clone());
        let store = self.store.clone();
        self.relink(
            async move { store.set_note_children(note_id, children).await },
            window,
            cx,
        );
    }

    fn toggle_label(&mut self, id: LabelId, window: &mut Window, cx: &mut Context<Self>) {
        let Some(note) = &mut self.note else { return };
        toggle(&mut note.labels, id);
        let (note_id, labels) = (note.id, note.labels.clone());
        let store = self.store.clone();
        self.relink(
            async move { store.set_note_labels(note_id, labels).await },
            window,
            cx,
        );
    }

    /// Runs `request`, a change of children or labels. The checkbox already
    /// shows the change (an optimistic update); the stored note replaces it
    /// when the reply arrives.
    fn relink(
        &mut self,
        request: impl Future<Output = papa_core::Result<Note>> + 'static,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            let result = request.await;
            this.update_in(cx, |this, window, cx| match result {
                Ok(stored) => {
                    if let Some(note) = this.note.as_mut().filter(|note| note.id == stored.id) {
                        note.children = stored.children.clone();
                        note.labels = stored.labels.clone();
                    }
                    cx.emit(NoteEditorEvent::Saved(stored));
                    cx.notify();
                }
                Err(err) => window.push_notification(
                    Notification::error(sentence(err)).title("Couldn't change the note"),
                    cx,
                ),
            })
            .ok();
        })
        .detach();
    }

    // --- Deleting ---

    fn confirm_delete(&mut self, _: &ClickEvent, window: &mut Window, cx: &mut Context<Self>) {
        let Some(note) = &self.note else { return };
        let id = note.id;
        let title = self.title.read(cx).value();
        let question = if title.trim().is_empty() {
            "Delete this note?".to_owned()
        } else {
            format!("Delete “{}”?", title.trim())
        };
        dialogs::confirm_delete(
            question,
            "This can't be undone.",
            cx.listener(move |this, _: &ClickEvent, window, cx| this.delete(id, window, cx)),
            window,
            cx,
        );
    }

    fn delete(&mut self, id: NoteId, window: &mut Window, cx: &mut Context<Self>) {
        if self.note_id() == Some(id) {
            // Nothing left to save.
            self.autosave = None;
            self.dirty = false;
        }
        let store = self.store.clone();
        cx.spawn_in(window, async move |this, cx| {
            let result = store.delete_note(id).await;
            this.update_in(cx, |this, window, cx| match result {
                Ok(()) => {
                    if this.note_id() == Some(id) {
                        this.note = None;
                    }
                    cx.emit(NoteEditorEvent::Deleted(id));
                    cx.notify();
                }
                Err(err) => window.push_notification(
                    Notification::error(sentence(err)).title("Couldn't delete the note"),
                    cx,
                ),
            })
            .ok();
        })
        .detach();
    }

    // --- Rendering ---

    /// Date, children and labels, the button to change them, and the mode
    /// and delete controls. One wrapping row: in a narrow pane the controls
    /// move to a second line instead of squeezing the rest.
    fn render_toolbar(&self, note: &Note, cx: &mut Context<Self>) -> impl IntoElement {
        let children = self
            .children
            .iter()
            .filter(|child| note.children.contains(&child.id))
            .map(|child| {
                h_flex()
                    .gap_1()
                    .child(div().size_2().rounded_full().bg(rgb(child.color)))
                    .child(child.name.clone())
            });
        let labels = self
            .labels
            .iter()
            .filter(|label| note.labels.contains(&label.id))
            .map(|label| Tag::secondary().small().child(format!("#{}", label.name)));
        let mode_tabs = TabBar::new("mode")
            .segmented()
            .small()
            .selected_index(match self.mode {
                Mode::Edit => 0,
                Mode::Preview => 1,
            })
            .on_click(cx.listener(|this, ix: &usize, window, cx| match ix {
                0 => this.edit_body(window, cx),
                _ => this.show_preview(window, cx),
            }))
            .child(Tab::new().label("Edit"))
            .child(Tab::new().label("Preview"));

        h_flex()
            .flex_wrap()
            .gap_x_3()
            .gap_y_2()
            .text_sm()
            .text_color(cx.theme().muted_foreground)
            .child(dates::long(note.created_at, &TimeZone::system()))
            // Before the chips, so it stays put while they come and go.
            .child(assign_popover(cx.entity().downgrade()))
            .children(children)
            .children(labels)
            .child(div().flex_1())
            .child(
                h_flex().gap_1().child(mode_tabs).child(
                    Button::new("delete-note")
                        .ghost()
                        .small()
                        // Not in the components' `IconName`; see
                        // `assets.rs`.
                        .icon(gpui_kit::assets::IconName::Trash)
                        .tooltip("Delete note")
                        .on_click(cx.listener(Self::confirm_delete)),
                ),
            )
    }

    fn render_body(&self, note: &Note, cx: &mut Context<Self>) -> AnyElement {
        match self.mode {
            Mode::Edit => Textarea::new(&self.body)
                .appearance(false)
                .h_full()
                .into_any_element(),
            Mode::Preview if self.preview.trim().is_empty() => div()
                .text_color(cx.theme().muted_foreground)
                .child("Nothing written yet. Switch to Edit to write.")
                .into_any_element(),
            // The id keys the preview's cached layout and scroll position:
            // one per note.
            Mode::Preview => TextView::markdown(
                SharedString::from(format!("preview-{}", note.id)),
                self.preview.clone(),
            )
            .scrollable(true)
            .size_full()
            .into_any_element(),
        }
    }
}

/// Adds `id` if missing, removes it if present.
fn toggle<T: PartialEq>(ids: &mut Vec<T>, id: T) {
    match ids.iter().position(|existing| *existing == id) {
        Some(index) => {
            ids.remove(index);
        }
        None => ids.push(id),
    }
}

/// A "+" button opening checkboxes for every child and label.
///
/// The popover's content is built by a closure that runs on every render
/// of the popover, outside our `Context`. It holds a weak handle to the
/// editor, reads the current note through it, and updates the editor when a
/// box is clicked.
fn assign_popover(editor: WeakEntity<NoteEditor>) -> impl IntoElement {
    Popover::new("assign")
        .trigger(
            Button::new("assign-button")
                .ghost()
                .xsmall()
                .icon(IconName::Plus)
                .tooltip("Children and labels"),
        )
        .content(move |_, _, cx| {
            let mut list = v_flex().gap_2().min_w_40();
            let Some(this) = editor.upgrade() else {
                return list;
            };
            let this = this.read(cx);
            let Some(note) = &this.note else { return list };

            list = list.child(heading("Children", cx));
            if this.children.is_empty() {
                list = list.child(hint("Add children in the sidebar.", cx));
            }
            for (ix, child) in this.children.iter().enumerate() {
                let (editor, id) = (editor.clone(), child.id);
                list = list.child(
                    Checkbox::new(("child", ix))
                        .label(child.name.clone())
                        .checked(note.children.contains(&child.id))
                        .on_click(move |_, window, cx| {
                            editor
                                .update(cx, |this, cx| this.toggle_child(id, window, cx))
                                .ok();
                        }),
                );
            }

            list = list.child(heading("Labels", cx));
            if this.labels.is_empty() {
                list = list.child(hint("Add labels in the sidebar.", cx));
            }
            for (ix, label) in this.labels.iter().enumerate() {
                let (editor, id) = (editor.clone(), label.id);
                list = list.child(
                    Checkbox::new(("label", ix))
                        .label(format!("#{}", label.name))
                        .checked(note.labels.contains(&label.id))
                        .on_click(move |_, window, cx| {
                            editor
                                .update(cx, |this, cx| this.toggle_label(id, window, cx))
                                .ok();
                        }),
                );
            }
            list
        })
}

fn heading(text: &'static str, cx: &App) -> impl IntoElement {
    div()
        .text_xs()
        .font_semibold()
        .text_color(cx.theme().muted_foreground)
        .child(text)
}

fn hint(text: &'static str, cx: &App) -> impl IntoElement {
    div()
        .text_sm()
        .text_color(cx.theme().muted_foreground)
        .child(text)
}

impl Render for NoteEditor {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let root = v_flex().size_full().track_focus(&self.focus_handle);
        let Some(note) = &self.note else {
            return root.child(NoteView);
        };

        root.child(
            v_flex()
                .gap_2()
                .px_6()
                .pt_4()
                .pb_3()
                .border_b_1()
                .border_color(cx.theme().border)
                // The title gets a line of its own, so a narrow pane can't
                // squeeze it away.
                .child(
                    div()
                        .text_xl()
                        .font_semibold()
                        .child(Input::new(&self.title).appearance(false).large()),
                )
                .child(self.render_toolbar(note, cx)),
        )
        .child(
            div()
                .flex_1()
                .min_h_0()
                .px_6()
                .py_4()
                .child(self.render_body(note, cx)),
        )
    }
}
