//! The vault's storage, run as an actor on its own thread.
//!
//! ```text
//!  UI thread (GPUI)                      "papa-store" thread (tokio)
//!  ───────────────                       ───────────────────────────
//!  store.add_child(draft).await
//!    ├─ Request::AddChild(draft, reply) ──▶ inbox.recv()
//!    │                                      db.add_child(draft).await
//!    └─ reply.await  ◀──────────────────── reply.send(result)
//! ```
//!
//! [`Store`] is the handle: it only holds the sending end of a channel, so it
//! is cheap to clone and safe to move between threads. The thread owns the
//! database connection and handles one request at a time, in order. When the
//! last `Store` clone is dropped the channel closes and the thread exits.

mod db;
mod migrations;

use std::path::PathBuf;
use std::thread;

use tokio::sync::{mpsc, oneshot};

use crate::{
    Child, ChildDraft, ChildId, Error, Label, LabelId, Note, NoteDraft, NoteId, NoteQuery,
    NoteSummary, Result,
};
use db::{Db, Location};

/// A handle to the vault. Every method sends a request to the database thread
/// and waits for its reply, so they can be awaited from any async executor,
/// including GPUI's, without blocking it.
#[derive(Clone, Debug)]
pub struct Store {
    requests: mpsc::UnboundedSender<Request>,
}

/// Where the database thread sends a request's result.
type Reply<T> = oneshot::Sender<Result<T>>;

/// One message per [`Store`] method. Everything in it is owned (`String`, not
/// `&str`), because it moves to another thread and may outlive the caller's
/// borrows.
enum Request {
    Children(Reply<Vec<Child>>),
    AddChild(ChildDraft, Reply<Child>),
    UpdateChild(ChildId, ChildDraft, Reply<Child>),
    DeleteChild(ChildId, Reply<()>),
    Labels(Reply<Vec<Label>>),
    AddLabel(String, Reply<Label>),
    RenameLabel(LabelId, String, Reply<Label>),
    DeleteLabel(LabelId, Reply<()>),
    Notes(NoteQuery, Reply<Vec<NoteSummary>>),
    Note(NoteId, Reply<Note>),
    AddNote(NoteDraft, Reply<Note>),
    UpdateNoteText(NoteId, String, String, Reply<Note>),
    SetNoteChildren(NoteId, Vec<ChildId>, Reply<Note>),
    SetNoteLabels(NoteId, Vec<LabelId>, Reply<Note>),
    DeleteNote(NoteId, Reply<()>),
}

impl Store {
    /// Opens the vault file at `path`, creating it and its folder if needed,
    /// and migrates it to the current schema.
    pub async fn open(path: impl Into<PathBuf>) -> Result<Self> {
        Self::start(Location::File(path.into())).await
    }

    /// A throwaway vault in memory: for tests, and later for demo mode.
    pub async fn open_in_memory() -> Result<Self> {
        Self::start(Location::Memory).await
    }

    /// Spawns the database thread and waits until the vault is open.
    async fn start(location: Location) -> Result<Self> {
        let (requests, inbox) = mpsc::unbounded_channel();
        let (opened, open_result) = oneshot::channel();
        thread::Builder::new()
            .name("papa-store".into())
            .spawn(move || serve(location, inbox, opened))
            .map_err(Error::StartThread)?;
        // The outer `?` covers the thread dying before it replied; the inner
        // one is the result of opening the vault.
        open_result.await.map_err(|_| Error::StoreClosed)??;
        Ok(Self { requests })
    }

    /// All children, in sidebar order.
    pub async fn children(&self) -> Result<Vec<Child>> {
        // A tuple variant's name is also a function from its fields to the
        // enum, so `Request::Children` is an `fn(Reply<_>) -> Request`.
        self.call(Request::Children).await
    }

    /// Adds a child at the end of the list.
    pub async fn add_child(&self, draft: ChildDraft) -> Result<Child> {
        self.call(|reply| Request::AddChild(draft, reply)).await
    }

    /// Replaces a child's name and colour.
    pub async fn update_child(&self, id: ChildId, draft: ChildDraft) -> Result<Child> {
        self.call(|reply| Request::UpdateChild(id, draft, reply))
            .await
    }

    pub async fn delete_child(&self, id: ChildId) -> Result<()> {
        self.call(|reply| Request::DeleteChild(id, reply)).await
    }

    /// All labels, alphabetically, ignoring case.
    pub async fn labels(&self) -> Result<Vec<Label>> {
        self.call(Request::Labels).await
    }

    /// Adds a label. Fails with [`Error::DuplicateLabel`] if the name is
    /// taken, ignoring case.
    pub async fn add_label(&self, name: impl Into<String>) -> Result<Label> {
        let name = name.into();
        self.call(|reply| Request::AddLabel(name, reply)).await
    }

    pub async fn rename_label(&self, id: LabelId, name: impl Into<String>) -> Result<Label> {
        let name = name.into();
        self.call(|reply| Request::RenameLabel(id, name, reply))
            .await
    }

    pub async fn delete_label(&self, id: LabelId) -> Result<()> {
        self.call(|reply| Request::DeleteLabel(id, reply)).await
    }

    /// The notes `query` asks for, newest first.
    pub async fn notes(&self, query: NoteQuery) -> Result<Vec<NoteSummary>> {
        self.call(|reply| Request::Notes(query, reply)).await
    }

    /// One note, in full. Fails with [`Error::NotFound`] if it was deleted.
    pub async fn note(&self, id: NoteId) -> Result<Note> {
        self.call(|reply| Request::Note(id, reply)).await
    }

    pub async fn add_note(&self, draft: NoteDraft) -> Result<Note> {
        self.call(|reply| Request::AddNote(draft, reply)).await
    }

    /// Saves a note's title and body, and returns the note as stored.
    pub async fn update_note_text(
        &self,
        id: NoteId,
        title: impl Into<String>,
        body: impl Into<String>,
    ) -> Result<Note> {
        let (title, body) = (title.into(), body.into());
        self.call(|reply| Request::UpdateNoteText(id, title, body, reply))
            .await
    }

    /// Replaces the children a note is about.
    pub async fn set_note_children(&self, id: NoteId, children: Vec<ChildId>) -> Result<Note> {
        self.call(|reply| Request::SetNoteChildren(id, children, reply))
            .await
    }

    /// Replaces a note's labels.
    pub async fn set_note_labels(&self, id: NoteId, labels: Vec<LabelId>) -> Result<Note> {
        self.call(|reply| Request::SetNoteLabels(id, labels, reply))
            .await
    }

    pub async fn delete_note(&self, id: NoteId) -> Result<()> {
        self.call(|reply| Request::DeleteNote(id, reply)).await
    }

    /// Sends the request built by `request` and waits for the reply.
    async fn call<T>(&self, request: impl FnOnce(Reply<T>) -> Request) -> Result<T> {
        let (reply, response) = oneshot::channel();
        self.requests
            .send(request(reply))
            .map_err(|_| Error::StoreClosed)?;
        response.await.map_err(|_| Error::StoreClosed)?
    }
}

/// The body of the database thread.
fn serve(
    location: Location,
    mut inbox: mpsc::UnboundedReceiver<Request>,
    opened: oneshot::Sender<Result<()>>,
) {
    // A single-threaded runtime: this thread is the only one that touches the
    // database. tokio is here for its channels now, and for turso's sync
    // (which needs it) in later phases.
    let runtime = match tokio::runtime::Builder::new_current_thread().build() {
        Ok(runtime) => runtime,
        Err(err) => {
            let _ = opened.send(Err(Error::StartThread(err)));
            return;
        }
    };

    runtime.block_on(async move {
        let mut db = match Db::open(&location).await {
            Ok(db) => db,
            Err(err) => {
                let _ = opened.send(Err(err));
                return;
            }
        };
        let _ = opened.send(Ok(()));

        // `recv` returns `None` once every `Store` clone has been dropped.
        while let Some(request) = inbox.recv().await {
            handle(&mut db, request).await;
        }
    });
}

async fn handle(db: &mut Db, request: Request) {
    // `send` fails only if the caller stopped waiting (say, its window
    // closed). The work is done either way, so the error is ignored.
    match request {
        Request::Children(reply) => {
            let _ = reply.send(db.children().await);
        }
        Request::AddChild(draft, reply) => {
            let _ = reply.send(db.add_child(draft).await);
        }
        Request::UpdateChild(id, draft, reply) => {
            let _ = reply.send(db.update_child(id, draft).await);
        }
        Request::DeleteChild(id, reply) => {
            let _ = reply.send(db.delete_child(id).await);
        }
        Request::Labels(reply) => {
            let _ = reply.send(db.labels().await);
        }
        Request::AddLabel(name, reply) => {
            let _ = reply.send(db.add_label(&name).await);
        }
        Request::RenameLabel(id, name, reply) => {
            let _ = reply.send(db.rename_label(id, &name).await);
        }
        Request::DeleteLabel(id, reply) => {
            let _ = reply.send(db.delete_label(id).await);
        }
        Request::Notes(query, reply) => {
            let _ = reply.send(db.notes(&query).await);
        }
        Request::Note(id, reply) => {
            let _ = reply.send(db.note(id).await);
        }
        Request::AddNote(draft, reply) => {
            let _ = reply.send(db.add_note(draft).await);
        }
        Request::UpdateNoteText(id, title, body, reply) => {
            let _ = reply.send(db.update_note_text(id, &title, &body).await);
        }
        Request::SetNoteChildren(id, children, reply) => {
            let _ = reply.send(db.set_note_children(id, &children).await);
        }
        Request::SetNoteLabels(id, labels, reply) => {
            let _ = reply.send(db.set_note_labels(id, &labels).await);
        }
        Request::DeleteNote(id, reply) => {
            let _ = reply.send(db.delete_note(id).await);
        }
    }
}

#[cfg(test)]
mod tests;
