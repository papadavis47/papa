//! The right pane: one note's title and Markdown body, saved as you type.

use std::rc::Rc;
use std::time::Duration;

use gpui_kit::component::{
    ActiveTheme as _, Icon, IconName, Sizable as _, StyledExt as _, WindowExt as _,
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
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    Animation, AnimationExt as _, AnyElement, App, AppContext as _, ClickEvent, ClipboardItem,
    ContentMask, Context, Entity, EventEmitter, ExternalPaths, FocusHandle, Focusable as _,
    InteractiveElement as _, IntoElement, Keystroke, ParentElement as _, Render, SharedString,
    Styled as _, Subscription, Task, WeakEntity, Window, canvas, div, ease_out_quint, fill,
    relative, rgb,
};
use jiff::tz::TimeZone;
use papa_core::settings::{Density, NoteMode};
use papa_core::{Child, ChildId, Label, LabelId, Note, NoteId, Store};

use super::gallery::{Gallery, GalleryEvent, ImageSource};
use super::{NoteView, dates, dialogs, sentence};
use crate::app::{APP_CONTEXT, ExportNote, FocusNoteList, NOTE_EDITOR_CONTEXT};
use crate::settings::AppSettings;
use crate::theme;
use crate::vim::{self, Effect, VimLayer};

/// What the editor tells its parent. `PapaApp` subscribes to these to keep
/// the note list in step.
#[derive(Clone, Debug)]
pub enum NoteEditorEvent {
    /// The note was stored: its text, children or labels changed.
    Saved(Note),
    Deleted(NoteId),
    /// An image was added to or removed from the note.
    ImagesChanged(NoteId),
}

/// How long a note takes to fade in when it opens. Short enough that
/// moving through the list with ↑/↓ never waits for it.
const NOTE_FADE: Duration = Duration::from_millis(140);

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
    /// Children whose sharing is on: the note can be shared with those it's
    /// about.
    shared_children: Rc<Vec<ChildId>>,
    title: Entity<InputState>,
    body: Entity<TextareaState>,
    /// The note's images, under the body.
    gallery: Entity<Gallery>,
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
    /// Vim mode, while the setting is on. `None` when it's off: no hook on
    /// the keyboard, nothing to pay for.
    vim: Option<VimLayer>,
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
        let gallery = cx.new(|cx| Gallery::new(ImageSource::Vault(store.clone()), window, cx));
        let subscriptions = vec![
            cx.subscribe_in(&title, window, Self::on_title_event),
            cx.subscribe_in(&body, window, Self::on_body_event),
            cx.subscribe(&gallery, |_, _, event: &GalleryEvent, cx| match *event {
                GalleryEvent::Changed(note) => cx.emit(NoteEditorEvent::ImagesChanged(note)),
            }),
            // GPUI awaits the returned future (briefly) before exiting.
            cx.on_app_quit(Self::save_before_quit),
            // Vim mode can be switched on and off while the app runs.
            cx.observe_global_in::<AppSettings>(window, Self::sync_vim),
        ];
        let vim = AppSettings::get(cx)
            .editor
            .vim_mode
            .then(|| VimLayer::attach(cx, Self::vim_keystroke));
        Self {
            store,
            note: None,
            children: Rc::default(),
            labels: Rc::default(),
            shared_children: Rc::default(),
            title,
            body,
            gallery,
            mode: Mode::Preview,
            preview: SharedString::default(),
            dirty: false,
            cursor_to_end: false,
            autosave: None,
            focus_handle: cx.focus_handle(),
            vim,
            _subscriptions: subscriptions,
        }
    }

    pub fn note_id(&self) -> Option<NoteId> {
        self.note.as_ref().map(|note| note.id)
    }

    /// The open note as last stored; its children and labels are current.
    pub fn note(&self) -> Option<&Note> {
        self.note.as_ref()
    }

    /// The title as typed, which may not be saved yet.
    pub fn title(&self, cx: &App) -> String {
        self.title.read(cx).value().to_string()
    }

    pub fn gallery(&self) -> &Entity<Gallery> {
        &self.gallery
    }

    /// Opens the file picker to add images to the note.
    pub fn add_images(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.gallery
            .update(cx, |gallery, cx| gallery.pick_files(window, cx));
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
        if let Some(layer) = &mut self.vim {
            layer.vim.reset();
        }
        self.mode = if note.title.is_empty() && note.body.is_empty() {
            Mode::Edit
        } else {
            match AppSettings::get(cx).editor.open_notes_in {
                NoteMode::Preview => Mode::Preview,
                NoteMode::Edit => Mode::Edit,
            }
        };
        self.show_images(Some(note.id), window, cx);
        self.note = Some(note);
        cx.notify();
    }

    fn show_images(&self, note: Option<NoteId>, window: &mut Window, cx: &mut Context<Self>) {
        self.gallery
            .update(cx, |gallery, cx| gallery.show(note, window, cx));
    }

    /// Saves and clears the editor.
    pub fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.flush(window, cx);
        self.note = None;
        self.show_images(None, window, cx);
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

    /// Takes keyboard focus: the text while editing, the preview otherwise.
    pub fn focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.note.is_none() {
            return;
        }
        match self.mode {
            Mode::Edit => self.edit_body(window, cx),
            Mode::Preview => self.focus_handle.focus(window, cx),
        }
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

    /// Updates which children the note can be shared with.
    pub fn set_shared_children(&mut self, shared: Rc<Vec<ChildId>>, cx: &mut Context<Self>) {
        self.shared_children = shared;
        cx.notify();
    }

    /// Sharing with `child` stopped: the vault no longer shares any note
    /// with them, this one included.
    pub fn forget_share(&mut self, child: ChildId, cx: &mut Context<Self>) {
        if let Some(note) = &mut self.note {
            note.shared_with.retain(|id| *id != child);
        }
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
        // Vim starts every visit to the text in Normal mode.
        if let Some(layer) = &mut self.vim {
            layer.vim.reset();
            layer.show_cursor(&self.body, cx);
        }
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
            // Undo restores a selection, and a paste leaves the caret after
            // the text: Normal mode puts the cursor back on a character.
            if let Some(layer) = &self.vim {
                layer.show_cursor(&self.body, cx);
            }
        }
    }

    /// The text changed: save once typing pauses for the autosave delay
    /// in the settings.
    fn changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.note.is_none() {
            return;
        }
        self.dirty = true;
        let delay = Duration::from_millis(AppSettings::get(cx).editor.autosave_delay_ms);
        self.autosave = Some(cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(delay).await;
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

    /// Starts saving what's unsaved, and returns a future that's done once
    /// it's stored. Locking the vault waits for it before closing the vault.
    pub fn save_before_closing(
        &mut self,
        cx: &mut Context<Self>,
    ) -> impl Future<Output = ()> + use<> {
        self.save_before_quit(cx)
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

    // --- Vim ---

    /// Attaches Vim when the setting is turned on, and detaches it when it's
    /// turned off.
    fn sync_vim(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let enabled = AppSettings::get(cx).editor.vim_mode;
        if enabled == self.vim.is_some() {
            return;
        }
        if enabled {
            let layer = VimLayer::attach(cx, Self::vim_keystroke);
            if self.body_focused(window, cx) {
                layer.show_cursor(&self.body, cx);
            }
            self.vim = Some(layer);
        } else {
            self.vim = None;
        }
        cx.notify();
    }

    fn body_focused(&self, window: &Window, cx: &App) -> bool {
        self.mode == Mode::Edit
            && self.note.is_some()
            && self.body.focus_handle(cx).is_focused(window)
    }

    /// Every keystroke in the app passes through here first while Vim is
    /// on. Only those for the note's text are Vim's; the title, the search
    /// box and the rest of the app get theirs as usual.
    fn vim_keystroke(
        &mut self,
        keystroke: &Keystroke,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.body_focused(window, cx) {
            return false;
        }
        let Some(layer) = &mut self.vim else {
            return false;
        };
        let (handled, requests) = layer.keystroke(keystroke, &self.body, window, cx);
        for request in requests {
            match request {
                Effect::Save => self.flush(window, cx),
                // Like Esc without Vim: back to the note list. Dispatched,
                // so `PapaApp` moves focus the same way the key does.
                Effect::Leave => window.dispatch_action(Box::new(FocusNoteList), cx),
                _ => {}
            }
        }
        if handled {
            // The status bar shows the mode and the keys typed so far.
            cx.notify();
        }
        handled
    }

    /// What the status bar shows for Vim, while the note's text has focus.
    pub fn vim_status(&self, window: &Window, cx: &App) -> Option<vim::Status> {
        let layer = self.vim.as_ref()?;
        self.body_focused(window, cx).then(|| layer.vim.status())
    }

    // --- Images ---

    /// The paste hook of both text fields: images and image files on the
    /// clipboard go to the gallery; anything else pastes as text.
    fn paste_images(
        editor: WeakEntity<Self>,
    ) -> impl Fn(&ClipboardItem, &mut Window, &mut App) -> bool + 'static {
        move |item, window, cx| {
            editor
                .update(cx, |this, cx| {
                    this.gallery
                        .update(cx, |gallery, cx| gallery.paste(item, window, cx))
                })
                .unwrap_or(false)
        }
    }

    fn drop_files(&mut self, paths: &ExternalPaths, window: &mut Window, cx: &mut Context<Self>) {
        self.gallery.update(cx, |gallery, cx| {
            gallery.add_files(paths.paths(), window, cx)
        });
    }

    // --- Children and labels ---

    /// Assigns the note to the child, or unassigns it.
    pub fn toggle_child(&mut self, id: ChildId, window: &mut Window, cx: &mut Context<Self>) {
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

    /// Adds the label to the note, or removes it.
    pub fn toggle_label(&mut self, id: LabelId, window: &mut Window, cx: &mut Context<Self>) {
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

    /// Lets the child read the note, or stops.
    pub fn toggle_shared(&mut self, id: ChildId, window: &mut Window, cx: &mut Context<Self>) {
        let Some(note) = &mut self.note else { return };
        toggle(&mut note.shared_with, id);
        let (note_id, shared) = (note.id, note.shared_with.contains(&id));
        let store = self.store.clone();
        self.relink(
            async move { store.set_note_shared(note_id, id, shared).await },
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
                        // Unassigning a child unshares the note too.
                        note.shared_with = stored.shared_with.clone();
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

    /// Asks before deleting the open note.
    pub fn confirm_delete(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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
                        this.show_images(None, window, cx);
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
                let shared = note.shared_with.contains(&child.id);
                h_flex()
                    .gap_1()
                    .child(div().size_2().rounded_full().bg(rgb(child.color)))
                    .child(child.name.clone())
                    // Shared with them: they can read it.
                    .when(shared, |chip| {
                        chip.child(Icon::new(gpui_kit::assets::IconName::Share2).xsmall())
                    })
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
                h_flex()
                    .gap_1()
                    .child(mode_tabs)
                    .child(
                        Button::new("add-images")
                            .ghost()
                            .small()
                            .icon(gpui_kit::assets::IconName::ImagePlus)
                            .tooltip("Add images")
                            .on_click(
                                cx.listener(|this, _, window, cx| this.add_images(window, cx)),
                            ),
                    )
                    .child(
                        Button::new("export-note")
                            .ghost()
                            .small()
                            .icon(gpui_kit::assets::IconName::FileDown)
                            .tooltip_with_action("Export to PDF", &ExportNote, Some(APP_CONTEXT))
                            // Handled by `PapaApp`, which owns exports.
                            .on_click(|_, window, cx| {
                                window.dispatch_action(Box::new(ExportNote), cx)
                            }),
                    )
                    .child(
                        Button::new("delete-note")
                            .ghost()
                            .small()
                            // Not in the components' `IconName`; see
                            // `assets.rs`.
                            .icon(gpui_kit::assets::IconName::Trash)
                            .tooltip("Delete note")
                            .on_click(
                                cx.listener(|this, _, window, cx| this.confirm_delete(window, cx)),
                            ),
                    ),
            )
    }

    /// The Markdown in the writing font, or rendered in the reading font.
    fn render_body(&self, note: &Note, cx: &mut Context<Self>) -> AnyElement {
        let line_height = relative(AppSettings::get(cx).appearance.line_height);
        match self.mode {
            // On the field itself: it sets its own line height, which an
            // inherited one can't override.
            Mode::Edit => div()
                .relative()
                .size_full()
                .child(
                    Textarea::new(&self.body)
                        .appearance(false)
                        .h_full()
                        .font_family(theme::writing_font(cx))
                        .line_height(line_height)
                        .on_paste(Self::paste_images(cx.entity().downgrade())),
                )
                // After the field, so it's painted over the text.
                .children(self.vim.is_some().then(|| vim_block(cx)))
                .into_any_element(),
            Mode::Preview if self.preview.trim().is_empty() => div()
                .text_color(cx.theme().muted_foreground)
                .child("Nothing written yet. Switch to Edit to write.")
                .into_any_element(),
            // Text style is inherited, the way CSS inherits it, so the
            // rendered Markdown takes the font and line height from here.
            Mode::Preview => div()
                .size_full()
                .font_family(theme::reading_font(cx))
                .line_height(line_height)
                .child(
                    // The id keys the preview's cached layout and scroll
                    // position: one per note.
                    TextView::markdown(
                        SharedString::from(format!("preview-{}", note.id)),
                        self.preview.clone(),
                    )
                    .scrollable(true)
                    .size_full(),
                )
                .into_any_element(),
        }
    }
}

/// Vim's block cursor in Normal mode: a translucent box over the character
/// the cursor is on, the way a terminal draws it.
///
/// The text field only draws a thin caret, so this paints the block
/// itself. A `canvas` paints with plain code rather than child elements; it
/// runs after the field has painted this frame, so the field's layout, and
/// the character's position in it, are already current.
fn vim_block(cx: &mut Context<NoteEditor>) -> impl IntoElement {
    let editor = cx.entity().downgrade();
    let color = cx.theme().caret.opacity(0.35);
    canvas(
        |_, _, _| {},
        move |area, _, window, cx| {
            let Some(editor) = editor.upgrade() else {
                return;
            };
            let editor = editor.read(cx);
            let Some(layer) = &editor.vim else { return };
            if !editor.body_focused(window, cx) {
                return;
            }
            if let Some(block) = layer.block(editor.body.read(cx)) {
                // Clipped to the text's area, for a line half scrolled away.
                window.with_content_mask(Some(ContentMask { bounds: area }), |window| {
                    window.paint_quad(fill(block, color));
                });
            }
        },
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full()
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

            list = list.child(heading("Shared with", cx));
            let shareable: Vec<&Child> = this
                .children
                .iter()
                .filter(|child| note.children.contains(&child.id))
                .filter(|child| this.shared_children.contains(&child.id))
                .collect();
            if shareable.is_empty() {
                list = list.child(hint("Right-click a child, then Sharing….", cx));
            }
            for (ix, child) in shareable.into_iter().enumerate() {
                let (editor, id) = (editor.clone(), child.id);
                list = list.child(
                    Checkbox::new(("shared", ix))
                        .label(format!("{} can read it", child.name))
                        .checked(note.shared_with.contains(&child.id))
                        .on_click(move |_, window, cx| {
                            editor
                                .update(cx, |this, cx| this.toggle_shared(id, window, cx))
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
        let root = v_flex()
            .size_full()
            .key_context(NOTE_EDITOR_CONTEXT)
            .track_focus(&self.focus_handle);
        let Some(note) = &self.note else {
            return root.child(NoteView::new(true));
        };
        let drop_target = cx.theme().drop_target;
        let compact = AppSettings::get(cx).appearance.density == Density::Compact;

        let content = v_flex()
            .size_full()
            .child(
                v_flex()
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
                    // The title gets a line of its own, so a narrow pane can't
                    // squeeze it away.
                    .child(
                        div().text_xl().font_semibold().child(
                            Input::new(&self.title)
                                .appearance(false)
                                .large()
                                .on_paste(Self::paste_images(cx.entity().downgrade())),
                        ),
                    )
                    .child(self.render_toolbar(note, cx)),
            )
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
                    .child(self.render_body(note, cx)),
            )
            .child(self.gallery.clone());

        root.child(
            // Each note fades in as it opens. The id names the note, so
            // switching to another starts the animation again, while
            // switching between Edit and Preview doesn't.
            content.with_animation(
                SharedString::from(format!("note-{}", note.id)),
                Animation::new(NOTE_FADE).with_easing(ease_out_quint()),
                |content, t| content.opacity(t),
            ),
        )
        // Files dragged from the file manager can land anywhere on the note.
        // While they hover, the pane is tinted to show it takes them.
        .drag_over::<ExternalPaths>(move |style, _, _, _| style.bg(drop_target))
        .on_drop(cx.listener(Self::drop_files))
    }
}
