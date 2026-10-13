//! The note list's and editor's side of `PapaApp`: loading the list,
//! opening notes, and reacting to what the editor reports.

use std::rc::Rc;

use gpui_kit::{App, Context, Entity, ScrollStrategy, Window};
use papa_core::{NoteDraft, NoteFilter, NoteId, NoteQuery, NoteSummary};

use super::{
    AddImages, FocusSearch, NewNote, PapaApp, SEARCH_DELAY, SelectNextNote, SelectPreviousNote,
    TogglePreview,
};
use crate::ui::{NoteEditor, NoteEditorEvent, NoteListEvent};

impl PapaApp {
    fn note_query(&self, cx: &App) -> NoteQuery {
        NoteQuery {
            filter: self.selection.filter,
            search: self.search.read(cx).value().to_string(),
        }
    }

    fn note_ids(&self) -> Vec<NoteId> {
        self.notes.iter().map(|note| note.id).collect()
    }

    /// Reloads the list once typing in the search box pauses.
    pub(super) fn search_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.search_task = Some(cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(SEARCH_DELAY).await;
            this.update_in(cx, |this, window, cx| this.refresh_notes(window, cx))
                .ok();
        }));
    }

    /// Reloads the note list for the current filter and search.
    pub(super) fn refresh_notes(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let query = self.note_query(cx);
        self.with_store(
            "Couldn't load the notes",
            async move |store| store.notes(query).await,
            Self::set_notes,
            window,
            cx,
        );
    }

    fn set_notes(&mut self, notes: Vec<NoteSummary>, window: &mut Window, cx: &mut Context<Self>) {
        self.notes = Rc::new(notes);
        self.selection.reconcile_note(&self.note_ids());
        self.open_selected(window, cx);
    }

    /// Shows the selected note in the editor, loading it from the vault, and
    /// scrolls the list to it.
    fn open_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(editor) = self.editor.clone() else {
            return;
        };
        let selected = self.selection.note;
        if let Some(ix) = selected.and_then(|id| self.notes.iter().position(|n| n.id == id)) {
            self.note_scroll.scroll_to_item(ix, ScrollStrategy::Nearest);
        }
        match selected {
            None => editor.update(cx, |editor, cx| editor.close(window, cx)),
            Some(id) if editor.read(cx).note_id() == Some(id) => {}
            Some(id) => {
                // Save the note being left before asking for the next one:
                // the database thread answers requests in the order sent.
                editor.update(cx, |editor, cx| editor.flush(window, cx));
                self.with_store(
                    "Couldn't open the note",
                    async move |store| store.note(id).await,
                    move |this, note, window, cx| {
                        // The arrow keys may have moved on while it loaded.
                        if this.selection.note == Some(note.id) {
                            editor.update(cx, |editor, cx| editor.open(note, window, cx));
                        }
                    },
                    window,
                    cx,
                );
            }
        }
    }

    fn select_note(&mut self, id: NoteId, window: &mut Window, cx: &mut Context<Self>) {
        self.selection.note = Some(id);
        self.open_selected(window, cx);
        cx.notify();
    }

    pub(super) fn select_next_note(
        &mut self,
        _: &SelectNextNote,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.move_note(1, window, cx);
    }

    pub(super) fn select_previous_note(
        &mut self,
        _: &SelectPreviousNote,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.move_note(-1, window, cx);
    }

    fn move_note(&mut self, delta: isize, window: &mut Window, cx: &mut Context<Self>) {
        self.selection.move_note(delta, &self.note_ids());
        self.open_selected(window, cx);
        cx.notify();
    }

    /// Creates an empty note about the selected child, or with the selected
    /// label, and opens it with the cursor in the title.
    pub(super) fn new_note(&mut self, _: &NewNote, window: &mut Window, cx: &mut Context<Self>) {
        let Some(editor) = self.editor.clone() else {
            return;
        };
        let draft = match self.selection.filter {
            NoteFilter::All => NoteDraft::default(),
            NoteFilter::Child(id) => NoteDraft {
                children: vec![id],
                ..NoteDraft::default()
            },
            NoteFilter::Label(id) => NoteDraft {
                labels: vec![id],
                ..NoteDraft::default()
            },
        };
        // An empty note matches no search, so clear it to show the note.
        // `set_value` emits no change event, so this doesn't reload the list.
        self.search
            .update(cx, |search, cx| search.set_value("", window, cx));
        let query = self.note_query(cx);
        self.with_store(
            "Couldn't create the note",
            async move |store| {
                let note = store.add_note(draft).await?;
                Ok((note, store.notes(query).await?))
            },
            move |this, (note, notes), window, cx| {
                this.selection.note = Some(note.id);
                this.notes = Rc::new(notes);
                this.note_scroll.scroll_to_item(0, ScrollStrategy::Top);
                editor.update(cx, |editor, cx| {
                    editor.open(note, window, cx);
                    editor.focus_title(window, cx);
                });
            },
            window,
            cx,
        );
    }

    pub(super) fn toggle_preview(
        &mut self,
        _: &TogglePreview,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(editor) = &self.editor {
            editor.update(cx, |editor, cx| editor.toggle_mode(window, cx));
        }
    }

    pub(super) fn add_images(
        &mut self,
        _: &AddImages,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(editor) = &self.editor {
            editor.update(cx, |editor, cx| editor.add_images(window, cx));
        }
    }

    pub(super) fn focus_search(
        &mut self,
        _: &FocusSearch,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.search
            .update(cx, |search, cx| search.focus(window, cx));
    }

    pub(super) fn on_note_list_event(
        &mut self,
        event: &NoteListEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match *event {
            NoteListEvent::Select(id) => {
                // Keep focus in the list so the arrow keys move from here.
                self.note_list_focus.focus(window, cx);
                self.select_note(id, window, cx);
            }
            NoteListEvent::NewNote => self.new_note(&NewNote, window, cx),
        }
    }

    /// Keeps the list in step with the editor. A saved note's row is updated
    /// in place rather than reloaded: a note shouldn't vanish from under the
    /// cursor because its new title no longer matches the search.
    pub(super) fn on_editor_event(
        &mut self,
        _: &Entity<NoteEditor>,
        event: &NoteEditorEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            NoteEditorEvent::Saved(note) => {
                // `make_mut` clones the list only if a render still holds it.
                let notes = Rc::make_mut(&mut self.notes);
                if let Some(row) = notes.iter_mut().find(|row| row.id == note.id) {
                    *row = note.summary(row.images);
                }
            }
            NoteEditorEvent::Deleted(id) => {
                self.selection.after_removing(*id, &self.note_ids());
                Rc::make_mut(&mut self.notes).retain(|row| row.id != *id);
                self.open_selected(window, cx);
            }
            // Reload for the row's image count. Adding an image doesn't
            // change the text, so the note stays in a filtered list.
            NoteEditorEvent::ImagesChanged(id) => {
                if self.notes.iter().any(|row| row.id == *id) {
                    self.refresh_notes(window, cx);
                }
            }
        }
        cx.notify();
    }
}
