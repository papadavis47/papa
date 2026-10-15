//! Child mode: the notes Dad shared, read-only.
//!
//! The same window as the father's, with less in it: the note list, the
//! note, and a status bar. No sidebar (there's one child, and labels are
//! Dad's), no editor, no vault. What it holds is a [`ChildSession`], the
//! handle to `papa-core`'s child thread, and the notes decrypted from it.
//!
//! Pulls happen when the app opens, every [`PULL_EVERY`], and on ⌘R. In
//! between, and offline, the list shows what the last pull brought.

use std::rc::Rc;
use std::time::Duration;

use anyhow::Context as _;
use gpui_kit::component::{
    ActiveTheme as _, WindowExt as _, h_resizable, input::InputEvent, input::InputState,
    notification::Notification, resizable_panel, status_bar::StatusBar, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AppContext as _, AsyncWindowContext, ClickEvent, Context, Entity, EventEmitter, FocusHandle,
    InteractiveElement as _, IntoElement, ParentElement as _, Render, SharedString, Styled as _,
    Subscription, Task, UniformListScrollHandle, Window, div, px,
};
use jiff::Timestamp;
use jiff::tz::TimeZone;
use papa_core::model::SearchTerms;
use papa_core::share::projection::BlobKind;
use papa_core::{ChildSession, Error, NoteFilter, NoteId, NoteSummary, SharedNote, SharedNotes};
use papa_pdf::Document;
use papa_proto::ErrorCode;

use crate::app::export::save_pdf;
use crate::app::{
    APP_CONTEXT, ExportNote, ExportNotes, FocusEditor, FocusNoteList, FocusSearch,
    NOTE_LIST_CONTEXT, SelectNextNote, SelectPreviousNote, SignOut, SyncNow, pane_context,
};
use crate::export::{self, Scope};
use crate::settings::AppSettings;
use crate::state::Selection;
use crate::ui::{
    AppTitleBar, EmptyNotes, NoteList, NoteListEvent, NoteReader, dates, dialogs, sentence,
};

/// How often the app checks for new notes while it's open.
pub const PULL_EVERY: Duration = Duration::from_secs(60);

/// What a child's book is called: its cover and its file.
const BOOK_TITLE: &str = "Notes from Dad";

/// What the child app tells the window.
pub enum ChildAppEvent {
    /// The child signed out, or their key stopped working: back to the
    /// sign-in, with `email` filled in and `reason` above it.
    SignedOut {
        email: String,
        reason: Option<SharedString>,
    },
}

/// Where checking for new notes stands, for the status bar.
#[derive(Clone, Debug, PartialEq, Eq)]
enum PullStatus {
    /// Showing what the last pull brought; not checked since opening.
    Saved,
    Pulling,
    Pulled(Timestamp),
    /// The network or the server: try again later. Offline is normal.
    Failed(String),
    /// The relay refused the key: Dad made a new one, or stopped sharing.
    KeyRefused,
}

pub struct ChildApp {
    session: ChildSession,
    father_email: String,
    /// Every note, newest first, decrypted. `Rc`, like the father's lists.
    notes: Rc<Vec<SharedNote>>,
    /// The ones the search matches, as the list shows them.
    list: Rc<Vec<NoteSummary>>,
    /// Dad's new key hasn't reached the share yet (see
    /// [`SharedNotes::waiting`]).
    waiting: bool,
    selection: Selection,
    search: Entity<InputState>,
    note_scroll: UniformListScrollHandle,
    reader: Entity<NoteReader>,
    status: PullStatus,
    /// The running pull. One at a time.
    pulling: Option<Task<()>>,
    /// Pulls every [`PULL_EVERY`].
    _poll: Task<()>,
    /// "Exporting 3 notes…", for the status bar.
    exporting: Option<SharedString>,
    focus_handle: FocusHandle,
    note_list_focus: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<ChildAppEvent> for ChildApp {}

impl ChildApp {
    /// Shows `contents`, from signing in or from the saved replica, and
    /// checks for new notes straight away.
    pub fn new(
        session: ChildSession,
        contents: SharedNotes,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let search = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Search")
                .clean_on_escape()
        });
        let reader = cx.new(|cx| NoteReader::new(session.clone(), window, cx));
        let subscriptions = vec![
            cx.subscribe_in(
                &search,
                window,
                |this, _, event: &InputEvent, window, cx| match event {
                    InputEvent::Change => this.refresh_list(window, cx),
                    InputEvent::PressEnter { .. } => this.focus_reader(&FocusEditor, window, cx),
                    _ => {}
                },
            ),
            // A closing dialog hands focus back to whatever had it, which
            // may be gone. Keep it inside, where the shortcuts work.
            cx.on_focus_lost(window, |this, window, cx| {
                this.focus_handle.focus(window, cx);
            }),
        ];
        let note_list_focus = cx.focus_handle();
        note_list_focus.focus(window, cx);
        let poll = cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor().timer(PULL_EVERY).await;
                let pulled = this.update_in(cx, |this, window, cx| this.pull(window, cx));
                if pulled.is_err() {
                    break;
                }
            }
        });
        let mut app = Self {
            session,
            father_email: contents.father_email.clone(),
            notes: Rc::default(),
            list: Rc::default(),
            waiting: false,
            selection: Selection::default(),
            search,
            note_scroll: UniformListScrollHandle::new(),
            reader,
            status: PullStatus::Saved,
            pulling: None,
            _poll: poll,
            exporting: None,
            focus_handle: cx.focus_handle(),
            note_list_focus,
            _subscriptions: subscriptions,
        };
        app.set_contents(contents, window, cx);
        app.pull(window, cx);
        app
    }

    /// The share's notes changed: keep the open note if it's still there.
    fn set_contents(&mut self, contents: SharedNotes, window: &mut Window, cx: &mut Context<Self>) {
        self.notes = Rc::new(contents.notes);
        self.waiting = contents.waiting;
        self.refresh_list(window, cx);
    }

    /// Filters the notes by the search, and keeps the selection valid.
    fn refresh_list(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let search = self.search.read(cx).value();
        self.list = Rc::new(list(&self.notes, &search));
        self.selection.reconcile_note(&self.note_ids());
        self.open_selected(window, cx);
        cx.notify();
    }

    fn note_ids(&self) -> Vec<NoteId> {
        self.list.iter().map(|note| note.id).collect()
    }

    fn note(&self, id: NoteId) -> Option<&SharedNote> {
        self.notes.iter().find(|note| note.id == id)
    }

    fn open_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let note = self.selection.note.and_then(|id| self.note(id)).cloned();
        self.reader
            .update(cx, |reader, cx| reader.show(note, window, cx));
    }

    // --- Pulling ---

    /// Checks for new notes, unless a check is running.
    fn pull(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.pulling.is_some() || self.status == PullStatus::KeyRefused {
            return;
        }
        self.status = PullStatus::Pulling;
        cx.notify();
        let session = self.session.clone();
        self.pulling = Some(cx.spawn_in(window, async move |this, cx| {
            let result = session.pull().await;
            this.update_in(cx, |this, window, cx| {
                this.pulling = None;
                this.pulled(result, window, cx);
            })
            .ok();
        }));
    }

    fn pulled(
        &mut self,
        result: papa_core::Result<Option<SharedNotes>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match result {
            Ok(contents) => {
                self.status = PullStatus::Pulled(Timestamp::now());
                if let Some(contents) = contents {
                    self.set_contents(contents, window, cx);
                }
            }
            Err(Error::Relay {
                code: ErrorCode::InvalidCredentials,
                ..
            }) => self.key_refused(window, cx),
            Err(err) => self.status = PullStatus::Failed(sentence(err)),
        }
        cx.notify();
    }

    /// The key stopped working. The notes already here stay readable until
    /// the child signs in again; the notification offers to do that.
    fn key_refused(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.status = PullStatus::KeyRefused;
        let this = cx.entity().downgrade();
        window.push_notification(
            Notification::warning(
                "Dad may have made a new key, or stopped sharing. Ask him for your key, \
                 then sign in again.",
            )
            .title("Your key no longer works")
            .autohide(false)
            .action(move |_, _, cx| {
                let this = this.clone();
                gpui_kit::component::button::Button::new("sign-in-again")
                    .label("Sign in again")
                    .on_click(cx.listener(move |notification, _, window, cx| {
                        notification.dismiss(window, cx);
                        this.update(cx, |this, cx| {
                            this.sign_out_now(
                                Some(
                                    "Your old key no longer works. Paste the new one from Dad."
                                        .into(),
                                ),
                                window,
                                cx,
                            )
                        })
                        .ok();
                    }))
            }),
            cx,
        );
    }

    fn sync_now(&mut self, _: &SyncNow, window: &mut Window, cx: &mut Context<Self>) {
        if self.status == PullStatus::KeyRefused {
            // Asking again can't help; signing in again can.
            self.key_refused(window, cx);
            return;
        }
        self.pull(window, cx);
    }

    /// The status bar's right side: when the notes were last checked.
    fn status_label(&self) -> SharedString {
        match &self.status {
            PullStatus::Saved => "Showing saved notes".into(),
            PullStatus::Pulling => "Checking for new notes…".into(),
            PullStatus::Pulled(at) => {
                format!("Checked {}", dates::ago(*at, Timestamp::now())).into()
            }
            PullStatus::Failed(_) => "Offline: showing saved notes".into(),
            PullStatus::KeyRefused => "Your key no longer works".into(),
        }
    }

    // --- Signing out ---

    fn sign_out(&mut self, _: &SignOut, window: &mut Window, cx: &mut Context<Self>) {
        let this = cx.entity().downgrade();
        dialogs::confirm_danger(
            "Sign out?".into(),
            "The notes are removed from this computer. Signing in again with Dad's email \
             and your key brings them back.",
            "Sign out",
            move |_: &ClickEvent, window, cx| {
                this.update(cx, |this, cx| this.sign_out_now(None, window, cx))
                    .ok();
            },
            window,
            cx,
        );
    }

    fn sign_out_now(
        &mut self,
        reason: Option<SharedString>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let session = self.session.clone();
        let email = self.father_email.clone();
        cx.spawn_in(window, async move |this, cx| {
            let result = session.sign_out().await;
            this.update_in(cx, |_, window, cx| match result {
                Ok(()) => cx.emit(ChildAppEvent::SignedOut { email, reason }),
                Err(err) => window.push_notification(
                    Notification::error(sentence(err)).title("Couldn't sign out"),
                    cx,
                ),
            })
            .ok();
        })
        .detach();
    }

    // --- The list and the keyboard ---

    fn on_note_list_event(
        &mut self,
        event: &NoteListEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match *event {
            NoteListEvent::Select(id) => {
                self.note_list_focus.focus(window, cx);
                self.selection.note = Some(id);
                self.open_selected(window, cx);
                cx.notify();
            }
            NoteListEvent::ClearSearch => {
                self.search
                    .update(cx, |search, cx| search.set_value("", window, cx));
                self.note_list_focus.focus(window, cx);
                self.refresh_list(window, cx);
            }
            NoteListEvent::NewNote => {}
        }
    }

    fn move_note(&mut self, delta: isize, window: &mut Window, cx: &mut Context<Self>) {
        self.selection.move_note(delta, &self.note_ids());
        self.open_selected(window, cx);
        cx.notify();
    }

    fn select_next(&mut self, _: &SelectNextNote, window: &mut Window, cx: &mut Context<Self>) {
        self.move_note(1, window, cx);
    }

    fn select_previous(
        &mut self,
        _: &SelectPreviousNote,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.move_note(-1, window, cx);
    }

    fn focus_search(&mut self, _: &FocusSearch, window: &mut Window, cx: &mut Context<Self>) {
        self.search
            .update(cx, |search, cx| search.focus(window, cx));
    }

    fn focus_list(&mut self, _: &FocusNoteList, window: &mut Window, cx: &mut Context<Self>) {
        self.note_list_focus.focus(window, cx);
    }

    fn focus_reader(&mut self, _: &FocusEditor, window: &mut Window, cx: &mut Context<Self>) {
        let reader = self.reader.clone();
        reader.read(cx).focus_handle().focus(window, cx);
    }

    /// Why the list is empty.
    fn empty_notes(&self, cx: &Context<Self>) -> EmptyNotes {
        use gpui_kit::assets::IconName as Lucide;

        let search = self.search.read(cx).value();
        if !search.trim().is_empty() {
            return EmptyNotes {
                id: "no-match",
                icon: Lucide::SearchX.into(),
                title: "No matches".into(),
                description: Some(format!("No note here mentions “{}”.", search.trim()).into()),
                action: Some(("Clear search".into(), NoteListEvent::ClearSearch)),
            };
        }
        if self.waiting {
            return EmptyNotes {
                id: "waiting",
                icon: Lucide::KeyRound.into(),
                title: "Waiting for Dad's new key".into(),
                description: Some(
                    "The notes are locked with a key Papa doesn't have yet. They show up \
                     once Dad's Papa syncs, or when you sign in with the key he gave you."
                        .into(),
                ),
                action: None,
            };
        }
        EmptyNotes {
            id: "nothing-shared",
            icon: Lucide::BookHeart.into(),
            title: "Nothing shared yet".into(),
            description: Some("When Dad shares a note with you, it shows up here.".into()),
            action: None,
        }
    }

    // --- PDF export ---

    fn export_note(&mut self, _: &ExportNote, window: &mut Window, cx: &mut Context<Self>) {
        let Some(note) = self.reader.read(cx).note().cloned() else {
            return;
        };
        let scope = Scope::Note(note.id);
        let name = export::file_name(&scope, &note.title, &[]);
        let what = export::describe(&scope, &[]);
        self.export(scope, what, name, None, window, cx);
    }

    fn export_notes(&mut self, _: &ExportNotes, window: &mut Window, cx: &mut Context<Self>) {
        if self.list.is_empty() {
            return;
        }
        let search = self.search.read(cx).value();
        let scope = Scope::list(self.note_ids(), NoteFilter::All, &search);
        // Without a search, the list is everything: Dad's book.
        let (what, name, cover) = if search.trim().is_empty() {
            (
                "all the notes".to_owned(),
                format!("{BOOK_TITLE}.pdf"),
                Some(BOOK_TITLE),
            )
        } else {
            (
                export::describe(&scope, &[]),
                export::file_name(&scope, "", &[]),
                None,
            )
        };
        self.export(scope, what, name, cover, window, cx);
    }

    /// What exporting the list would be called, for its button.
    fn list_export_name(&self, cx: &Context<Self>) -> String {
        if self.search.read(cx).value().trim().is_empty() {
            return "all the notes".to_owned();
        }
        export::describe(&Scope::list(self.note_ids(), NoteFilter::All, ""), &[])
    }

    fn export(
        &mut self,
        scope: Scope,
        what: String,
        name: String,
        cover: Option<&'static str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let session = self.session.clone();
        let notes: Vec<SharedNote> = scope
            .note_ids()
            .into_iter()
            .filter_map(|id| self.note(id).cloned())
            .collect();
        let document = async move |cx: &mut AsyncWindowContext| {
            build_document(&session, &scope, notes, cover, cx).await
        };
        save_pdf(
            self,
            |this| &mut this.exporting,
            what,
            name,
            document,
            window,
            cx,
        );
    }
}

/// The PDF of `notes`, in order, with their photos from the share.
async fn build_document(
    session: &ChildSession,
    scope: &Scope,
    notes: Vec<SharedNote>,
    cover: Option<&str>,
    cx: &mut AsyncWindowContext,
) -> anyhow::Result<Document> {
    let mut printed = Vec::with_capacity(notes.len());
    for note in notes {
        let mut photos = Vec::new();
        for image in session
            .note_images(note.id)
            .await
            .context("couldn't read the photos")?
        {
            let display = session
                .image(image.id, BlobKind::Display)
                .await
                .context("couldn't read a photo")?;
            photos.push(export::photo(&image, display));
        }
        printed.push((note.to_note(), photos));
    }
    let paper = cx.update(|_, cx| AppSettings::get(cx).export.paper)?;
    let mut document = export::document(scope, printed, &[], paper, &TimeZone::system());
    if let (Some(title), Some(book)) = (cover, document.cover.as_mut()) {
        book.title = title.to_owned();
    }
    Ok(document)
}

/// The notes the search matches, as the list shows them. Newest first, as
/// `notes` are.
fn list(notes: &[SharedNote], search: &str) -> Vec<NoteSummary> {
    let terms = SearchTerms::new(search);
    notes
        .iter()
        .filter(|note| terms.matches(&note.title, &note.body))
        .map(SharedNote::summary)
        .collect()
}

impl Render for ChildApp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let vim = AppSettings::get(cx).editor.vim_mode;
        let list_focused = self.note_list_focus.contains_focused(window, cx);
        let lightbox = self.reader.read(cx).gallery().read(cx).lightbox().clone();
        let density = AppSettings::get(cx).appearance.density;
        let note_list = NoteList::new(
            BOOK_TITLE,
            self.list.clone(),
            Rc::default(),
            self.selection.note,
            self.search.clone(),
            self.note_scroll.clone(),
            self.empty_notes(cx),
            density,
            true,
            cx.listener(Self::on_note_list_event),
        )
        .read_only()
        .export(self.list_export_name(cx))
        .focused(list_focused);

        v_flex()
            .key_context(APP_CONTEXT)
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::select_next))
            .on_action(cx.listener(Self::select_previous))
            .on_action(cx.listener(Self::focus_search))
            .on_action(cx.listener(Self::focus_list))
            .on_action(cx.listener(Self::focus_reader))
            .on_action(cx.listener(Self::export_note))
            .on_action(cx.listener(Self::export_notes))
            .on_action(cx.listener(Self::sync_now))
            .on_action(cx.listener(Self::sign_out))
            .size_full()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .child(AppTitleBar::child())
            .child(
                div().flex_1().min_h_0().child(
                    h_resizable("child-panes")
                        .child(
                            resizable_panel()
                                .size(px(320.))
                                .size_range(px(220.)..px(480.))
                                .flex_none()
                                .child(
                                    div()
                                        .size_full()
                                        .key_context(pane_context(NOTE_LIST_CONTEXT, vim))
                                        .track_focus(&self.note_list_focus)
                                        .child(note_list),
                                ),
                        )
                        .child(resizable_panel().child(self.reader.clone())),
                ),
            )
            .child(
                StatusBar::new()
                    .left(format!("Shared by {}", self.father_email))
                    .when_some(self.exporting.clone(), |bar, exporting| bar.left(exporting))
                    .right(self.status_label())
                    .right(cx.theme().theme_name().clone()),
            )
            .child(lightbox)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(title: &str, body: &str) -> SharedNote {
        SharedNote {
            id: NoteId::new(),
            title: title.into(),
            body: body.into(),
            images: Vec::new(),
            created_at: Timestamp::UNIX_EPOCH,
            updated_at: Timestamp::UNIX_EPOCH,
        }
    }

    #[test]
    fn the_search_narrows_the_list_and_keeps_its_order() {
        let notes = [
            note("Lost tooth", "At school, in an apple."),
            note("Beach day", "Sand castles."),
            note("First bike ride", "No training wheels at SCHOOL either."),
        ];

        let titles = |search| -> Vec<String> {
            list(&notes, search)
                .into_iter()
                .map(|note| note.title)
                .collect()
        };
        assert_eq!(titles(""), ["Lost tooth", "Beach day", "First bike ride"]);
        assert_eq!(titles("school"), ["Lost tooth", "First bike ride"]);
        assert_eq!(titles("school wheels"), ["First bike ride"]);
        assert_eq!(titles("castle"), ["Beach day"]);
        assert_eq!(titles("piano"), Vec::<String>::new());
    }
}
