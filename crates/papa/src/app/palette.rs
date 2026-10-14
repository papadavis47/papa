//! The command palette's side of `PapaApp`: opening it with a snapshot of
//! the app's state, and running what it can't run by dispatching an action.

use gpui_kit::{App, Context, Entity, Window};
use papa_core::{NoteFilter, NoteId, NoteQuery, NoteSummary};

use super::{
    ChangePassword, ExportBackup, PapaApp, RemovePassword, RestoreBackup, ToggleCommandPalette,
    Vault,
};
use crate::palette::{self, OpenNote, PaletteCommand, PaletteData, PaletteGroup};
use crate::settings::AppSettings;
use crate::ui::{CommandPalette, CommandPaletteEvent, SidebarEvent};

impl PapaApp {
    /// The palette's groups from the current state, with `notes` as the
    /// notes to jump to.
    fn palette_groups(&self, notes: &[NoteSummary], cx: &App) -> Vec<PaletteGroup> {
        let editor = self.editor.as_ref().map(|editor| editor.read(cx));
        let open_note = editor
            .and_then(|editor| editor.note())
            .map(|note| OpenNote {
                children: &note.children,
                labels: &note.labels,
            });
        let list_export = (!self.notes.is_empty()).then(|| self.list_export_name(cx));
        palette::groups(&PaletteData {
            vault_open: self.is_open(),
            children: &self.children,
            labels: &self.labels,
            notes,
            open_note,
            filter: self.selection.filter,
            vim_mode: AppSettings::get(cx).editor.vim_mode,
            encrypted: matches!(
                self.vault,
                Vault::Open {
                    encrypted: true,
                    ..
                }
            ),
            list_export: list_export.as_deref(),
        })
    }

    /// ⌘K: opens the palette, or closes it when it's open.
    ///
    /// It opens at once with the commands, children and labels, which are
    /// at hand. The note list may be filtered, so every note is read from
    /// the vault, and joins the palette when it arrives.
    pub(super) fn toggle_command_palette(
        &mut self,
        _: &ToggleCommandPalette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let palette = self.palette.clone();
        if palette.read(cx).is_open() {
            palette.update(cx, |palette, cx| palette.close(window, cx));
            return;
        }
        // Nothing to offer while the vault is locked: not even its notes'
        // titles.
        if matches!(self.vault, Vault::Locked { .. }) {
            return;
        }
        let groups = self.palette_groups(&[], cx);
        let loading = self.is_open();
        palette.update(cx, |palette, cx| palette.open(groups, loading, window, cx));
        self.with_store(
            "Couldn't load the notes",
            async move |store| store.notes(NoteQuery::default()).await,
            move |this, notes, window, cx| {
                let groups = this.palette_groups(&notes, cx);
                palette.update(cx, |palette, cx| palette.set_groups(groups, window, cx));
            },
            window,
            cx,
        );
    }

    /// Runs a command the palette doesn't run itself. The palette has
    /// closed and given focus back by now, so a dialog this opens returns
    /// focus to where it was before the palette.
    pub(super) fn on_palette_event(
        &mut self,
        _: &Entity<CommandPalette>,
        event: &CommandPaletteEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let CommandPaletteEvent::Run(command) = *event;
        match command {
            // Dispatched by the palette, as the shortcut would be.
            PaletteCommand::Shortcut(_) => {}
            PaletteCommand::AddChild => self.on_sidebar_event(&SidebarEvent::AddChild, window, cx),
            PaletteCommand::AddLabel => self.on_sidebar_event(&SidebarEvent::AddLabel, window, cx),
            PaletteCommand::DeleteNote => {
                if let Some(editor) = &self.editor {
                    editor.update(cx, |editor, cx| editor.confirm_delete(window, cx));
                }
            }
            PaletteCommand::Show(filter) => {
                self.sidebar_focus.focus(window, cx);
                self.select_filter(filter, window, cx);
            }
            PaletteCommand::Open(id) => self.open_note(id, window, cx),
            PaletteCommand::ToggleChild(id) => {
                if let Some(editor) = &self.editor {
                    editor.update(cx, |editor, cx| editor.toggle_child(id, window, cx));
                }
            }
            PaletteCommand::ToggleLabel(id) => {
                if let Some(editor) = &self.editor {
                    editor.update(cx, |editor, cx| editor.toggle_label(id, window, cx));
                }
            }
            PaletteCommand::ToggleVimMode => {
                AppSettings::update(cx, |s| s.editor.vim_mode = !s.editor.vim_mode);
            }
            PaletteCommand::ChangePassword => self.change_password(&ChangePassword, window, cx),
            PaletteCommand::RemovePassword => self.remove_password(&RemovePassword, window, cx),
            PaletteCommand::ExportBackup => self.export_backup(&ExportBackup, window, cx),
            PaletteCommand::RestoreBackup => self.restore_backup(&RestoreBackup, window, cx),
        }
    }

    /// Opens a note from anywhere in the vault. If the list doesn't show
    /// it, the list switches to all notes, with no search, first.
    fn open_note(&mut self, id: NoteId, window: &mut Window, cx: &mut Context<Self>) {
        self.note_list_focus.focus(window, cx);
        if self.notes.iter().any(|note| note.id == id) {
            self.select_note(id, window, cx);
            return;
        }
        self.selection.filter = NoteFilter::All;
        self.selection.note = Some(id);
        // `set_value` emits no change event, so this doesn't reload the
        // list a second time.
        self.search
            .update(cx, |search, cx| search.set_value("", window, cx));
        // The reload keeps the selection: the note is in every note.
        self.refresh_notes(window, cx);
        cx.notify();
    }
}
