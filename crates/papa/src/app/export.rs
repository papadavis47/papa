//! PDF export's side of `PapaApp`: asking where to save, reading the notes
//! and photos from the vault, and laying out the PDF in the background.
//!
//! ```text
//!  UI thread                     database thread        background thread
//!  ─────────                     ───────────────        ─────────────────
//!  save dialog → path
//!  "Exporting…" in status bar
//!  load(ids) ──────────────────▶ notes, photos
//!            ◀──────────────────
//!  export::document(…)
//!  render + write ──────────────────────────────────▶  papa_pdf::render
//!                 ◀──────────────────────────────────  fs::write
//!  "Exported" notification, with Open
//! ```

use std::path::{Path, PathBuf};

use anyhow::Context as _;
use gpui_kit::component::{
    WindowExt as _,
    button::{Button, ButtonVariants as _},
    notification::Notification,
};
use gpui_kit::{App, AsyncWindowContext, Context, SharedString, Window};
use jiff::tz::TimeZone;
use papa_core::{Note, NoteId, Store, paths};
use papa_pdf::Photo;

use super::{ExportNote, ExportNotes, PapaApp, Vault};
use crate::export::{self, Scope};
use crate::open;
use crate::settings::AppSettings;
use crate::ui::sentence;

/// Identifies the export notifications, so a new one replaces the last.
struct ExportNotification;

impl PapaApp {
    pub(super) fn export_note(
        &mut self,
        _: &ExportNote,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(editor) = self.editor.clone() else {
            return;
        };
        let Some(id) = editor.read(cx).note_id() else {
            return;
        };
        // Save what's been typed, so the export has it. The database
        // thread answers in order, so the export reads it back.
        editor.update(cx, |editor, cx| editor.flush(window, cx));
        let title = editor.read(cx).title(cx);
        self.export(Scope::Note(id), &title, window, cx);
    }

    pub(super) fn export_notes(
        &mut self,
        _: &ExportNotes,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.notes.is_empty() {
            return;
        }
        if let Some(editor) = &self.editor {
            editor.update(cx, |editor, cx| editor.flush(window, cx));
        }
        let search = self.search.read(cx).value();
        let scope = Scope::list(self.note_ids(), self.selection.filter, &search);
        self.export(scope, "", window, cx);
    }

    /// What exporting the list would be called: "Emma’s book", "3 notes".
    pub(super) fn list_export_name(&self, cx: &App) -> String {
        let search = self.search.read(cx).value();
        export::describe(
            &Scope::list(self.note_ids(), self.selection.filter, &search),
            &self.children,
        )
    }

    /// Asks where to save, then exports `scope` there. `title` is the open
    /// note's, for the suggested file name.
    fn export(&mut self, scope: Scope, title: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Vault::Open { store, .. } = &self.vault else {
            return;
        };
        if self.exporting.is_some() {
            window.push_notification(
                Notification::info("Wait for it to finish, then try again.")
                    .title("An export is already running"),
                cx,
            );
            return;
        }
        let store = store.clone();
        let children = self.children.clone();
        let what = export::describe(&scope, &children);
        let name = export::file_name(&scope, title, &children);
        let dir = paths::export_dir();
        let chosen = cx.prompt_for_new_path(&dir, Some(&name));

        cx.spawn_in(window, async move |this, cx| {
            // The outer `Result` fails if the dialog went away without an
            // answer; the inner one if it couldn't open. `None` is Cancel.
            let (path, no_dialog) = match chosen.await {
                Ok(Ok(Some(path))) => (path, None),
                Ok(Ok(None)) | Err(_) => return,
                // Some Linux desktops have no save dialog to offer (it comes
                // from a "portal" service). Rather than fail, save in the
                // Documents folder under a name that's free.
                Ok(Err(err)) => (export::unused_path(&dir, &name, Path::exists), Some(err)),
            };
            let status = SharedString::from(format!("Exporting {what}…"));
            this.update(cx, |this, cx| {
                this.exporting = Some(status);
                cx.notify();
            })
            .ok();

            let result = run(&store, &scope, &children, path, cx).await;

            this.update_in(cx, |this, window, cx| {
                this.exporting = None;
                cx.notify();
                exported(result, &what, no_dialog, window, cx);
            })
            .ok();
        })
        .detach();
    }
}

/// Reads the notes, lays the PDF out on a background thread and writes it
/// to `path`. Returns where it went.
async fn run(
    store: &Store,
    scope: &Scope,
    children: &[papa_core::Child],
    path: PathBuf,
    cx: &mut AsyncWindowContext,
) -> anyhow::Result<PathBuf> {
    let notes = load(store, scope.note_ids())
        .await
        .context("couldn't read the notes")?;
    let paper = cx.update(|_, cx| AppSettings::get(cx).export.paper)?;
    let document = export::document(scope, notes, children, paper, &TimeZone::system());
    // Laying out a long book takes seconds of CPU: off the UI thread, so
    // the window stays responsive meanwhile.
    cx.background_executor()
        .spawn(async move {
            let pdf = papa_pdf::render(document)?;
            std::fs::write(&path, pdf)
                .with_context(|| format!("couldn't write {}", paths::display(&path)))?;
            Ok(path)
        })
        .await
}

/// Each note with its photos, in the order of `ids`.
async fn load(store: &Store, ids: Vec<NoteId>) -> papa_core::Result<Vec<(Note, Vec<Photo>)>> {
    let mut notes = Vec::with_capacity(ids.len());
    for id in ids {
        let note = store.note(id).await?;
        let mut photos = Vec::new();
        for image in store.note_images(id).await? {
            // The display version: at most 2048 pixels on its longest
            // edge, sharp enough to print across the page.
            let display = store.display_image(image.id).await?;
            photos.push(export::photo(&image, display));
        }
        notes.push((note, photos));
    }
    Ok(notes)
}

/// Says how the export went. A success offers to open the PDF.
fn exported(
    result: anyhow::Result<PathBuf>,
    what: &str,
    no_dialog: Option<anyhow::Error>,
    window: &mut Window,
    cx: &mut App,
) {
    let notification = match result {
        Ok(path) => {
            let mut message = format!("Saved to {}.", paths::display(&path));
            if no_dialog.is_some() {
                message.push_str(" There was no save dialog to ask where.");
            }
            Notification::success(message)
                .title(sentence(format!("Exported {what}")))
                .action(move |_, _, _| {
                    let path = path.clone();
                    Button::new("open-pdf")
                        .primary()
                        .label("Open")
                        .on_click(move |_, _, cx| open::open(&path, cx))
                })
        }
        Err(err) => Notification::error(sentence(format!("{err:#}")))
            .title(sentence(format!("Couldn't export {what}"))),
    };
    window.push_notification(notification.id::<ExportNotification>(), cx);
}
