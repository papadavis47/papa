//! The root view: owns the app's state and lays out the window.
//!
//! `PapaApp`'s methods are split by the pane they serve: `sidebar.rs`
//! (children and labels) and `notes.rs` (the note list and the editor).
//! Rust allows several `impl` blocks for one type, in different modules.

mod notes;
mod sidebar;

use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

use anyhow::Context as _;
use gpui_kit::component::{
    ActiveTheme as _, WindowExt as _, h_resizable,
    input::{InputEvent, InputState},
    notification::Notification,
    resizable_panel,
    status_bar::StatusBar,
    v_flex,
};
use gpui_kit::{
    App, AppContext as _, Context, Entity, FocusHandle, InteractiveElement as _, IntoElement,
    KeyBinding, ParentElement as _, Render, SharedString, Styled as _, Subscription, Task,
    UniformListScrollHandle, Window, div, px,
};
use papa_core::{Child, Label, NoteFilter, NoteSummary, Store, paths};

use crate::state::Selection;
use crate::theme;
use crate::ui::{
    AppSidebar, AppTitleBar, NOTE_SEARCH_CONTEXT, NoteEditor, NoteList, NoteView, sentence,
};

gpui_kit::actions!(
    papa,
    [
        /// Quit the application.
        Quit,
        /// Switch between the light and dark theme.
        ToggleTheme,
        /// Show or hide the sidebar.
        ToggleSidebar,
        /// Select the sidebar entry below the current one.
        SelectNextFilter,
        /// Select the sidebar entry above the current one.
        SelectPreviousFilter,
        /// Open the note below the current one.
        SelectNextNote,
        /// Open the note above the current one.
        SelectPreviousNote,
        /// Create a note for the selected child or label.
        NewNote,
        /// Switch the open note between editing and preview.
        TogglePreview,
        /// Jump to the search box.
        FocusSearch,
    ]
);

/// How long typing in the search box must pause before the list reloads.
/// Each search reads every note of the filter, so one per keystroke would
/// queue up behind each other on the database thread.
const SEARCH_DELAY: Duration = Duration::from_millis(150);

/// Key context on the root element: its bindings work anywhere in the window.
pub const APP_CONTEXT: &str = "PapaApp";
/// Key context on the sidebar: its bindings work only while it has focus.
pub const SIDEBAR_CONTEXT: &str = "Sidebar";
/// Key context on the note list pane, search box included.
pub const NOTE_LIST_CONTEXT: &str = "NoteList";

/// Registers keybindings and app-wide action handlers. Call once at startup,
/// after `gpui_kit::init`.
pub fn init(cx: &mut App) {
    // A focused text field handles ↑/↓ itself. Matching on the search box's
    // `Input` makes this binding as specific as the field's own, and the
    // later of two equally specific bindings wins, so here the arrows move
    // through the notes while the search keeps focus.
    let in_search = format!("{NOTE_SEARCH_CONTEXT} > Input");
    cx.bind_keys([
        // `secondary` is Cmd on macOS and Ctrl elsewhere.
        KeyBinding::new("secondary-q", Quit, None),
        KeyBinding::new("secondary-shift-l", ToggleTheme, None),
        KeyBinding::new("secondary-b", ToggleSidebar, Some(APP_CONTEXT)),
        KeyBinding::new("secondary-n", NewNote, Some(APP_CONTEXT)),
        KeyBinding::new("secondary-e", TogglePreview, Some(APP_CONTEXT)),
        KeyBinding::new("secondary-f", FocusSearch, Some(APP_CONTEXT)),
        KeyBinding::new("down", SelectNextFilter, Some(SIDEBAR_CONTEXT)),
        KeyBinding::new("up", SelectPreviousFilter, Some(SIDEBAR_CONTEXT)),
        KeyBinding::new("down", SelectNextNote, Some(NOTE_LIST_CONTEXT)),
        KeyBinding::new("up", SelectPreviousNote, Some(NOTE_LIST_CONTEXT)),
        KeyBinding::new("down", SelectNextNote, Some(&in_search)),
        KeyBinding::new("up", SelectPreviousNote, Some(&in_search)),
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

/// Where the vault is in its life.
enum Vault {
    /// The database thread is starting up and migrating.
    Opening,
    Open {
        /// Cheap to clone: each background task takes its own copy.
        store: Store,
        path: PathBuf,
    },
    /// Opening failed; the error was shown in a notification.
    Failed,
}

/// What the startup task hands back once the vault is open.
struct OpenedVault {
    store: Store,
    path: PathBuf,
    children: Vec<Child>,
    labels: Vec<Label>,
}

/// Opens the vault and reads everything the sidebar shows.
///
/// Returns `anyhow::Result`: the app only displays these errors, so it adds
/// context to them rather than matching on `papa_core::Error`'s variants.
async fn open_vault() -> anyhow::Result<OpenedVault> {
    let path = paths::vault_path()?;
    let store = Store::open(&path)
        .await
        .with_context(|| format!("couldn't open the vault at {}", paths::display(&path)))?;
    let children = store.children().await.context("couldn't load children")?;
    let labels = store.labels().await.context("couldn't load labels")?;
    Ok(OpenedVault {
        store,
        path,
        children,
        labels,
    })
}

pub struct PapaApp {
    vault: Vault,
    /// Copies of the vault's rows, refreshed after every change. `Rc`, so
    /// the components and the editor share them instead of copying.
    children: Rc<Vec<Child>>,
    labels: Rc<Vec<Label>>,
    /// The note list for the selected filter and search, newest first.
    notes: Rc<Vec<NoteSummary>>,
    selection: Selection,
    /// The search box above the note list.
    search: Entity<InputState>,
    /// Re-runs the search once typing pauses. Like the editor's autosave,
    /// replacing the task cancels the previous one.
    search_task: Option<Task<()>>,
    note_scroll: UniformListScrollHandle,
    /// Created once the vault is open: it needs the store.
    editor: Option<Entity<NoteEditor>>,
    sidebar_visible: bool,
    /// Clicking an element that tracks a focus handle focuses it. This one is
    /// on the root, so a click anywhere outside the panes still leaves focus
    /// inside `APP_CONTEXT` and the window-wide bindings keep working.
    focus_handle: FocusHandle,
    /// Focused while the sidebar is active; enables its ↑/↓ bindings.
    sidebar_focus: FocusHandle,
    /// Focused while the note list is active; enables its ↑/↓ bindings.
    note_list_focus: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

impl PapaApp {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let sidebar_focus = cx.focus_handle();
        // Start in the sidebar so the arrow keys work straight away.
        sidebar_focus.focus(window, cx);

        let search = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Search")
                .clean_on_escape()
        });
        let subscriptions = vec![
            cx.subscribe_in(
                &search,
                window,
                |this, _, event: &InputEvent, window, cx| {
                    if let InputEvent::Change = event {
                        this.search_changed(window, cx);
                    }
                },
            ),
            // A closing dialog hands focus back to whatever had it before,
            // and that may be gone (a right-click menu, say). With nothing
            // focused, no key context matches and the shortcuts stop working,
            // so fall back to the root.
            cx.on_focus_lost(window, |this, window, cx| {
                this.focus_handle.focus(window, cx);
            }),
        ];

        // The window shows up immediately; this task waits for the database
        // thread without blocking the UI, then fills in the state. `this` is a
        // weak handle, so the task doesn't keep a closed window's view alive.
        cx.spawn_in(window, async move |this, cx| {
            let opened = open_vault().await;
            this.update_in(cx, |this, window, cx| this.vault_opened(opened, window, cx))
        })
        .detach();

        Self {
            vault: Vault::Opening,
            children: Rc::default(),
            labels: Rc::default(),
            notes: Rc::default(),
            selection: Selection::default(),
            search,
            search_task: None,
            note_scroll: UniformListScrollHandle::new(),
            editor: None,
            sidebar_visible: true,
            focus_handle: cx.focus_handle(),
            sidebar_focus,
            note_list_focus: cx.focus_handle(),
            _subscriptions: subscriptions,
        }
    }

    fn vault_opened(
        &mut self,
        opened: anyhow::Result<OpenedVault>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match opened {
            Ok(opened) => {
                let editor = cx.new(|cx| NoteEditor::new(opened.store.clone(), window, cx));
                self._subscriptions
                    .push(cx.subscribe_in(&editor, window, Self::on_editor_event));
                self.editor = Some(editor);
                self.vault = Vault::Open {
                    store: opened.store,
                    path: opened.path,
                };
                self.children = Rc::new(opened.children);
                self.labels = Rc::new(opened.labels);
                self.catalog_changed(window, cx);
            }
            Err(err) => {
                self.vault = Vault::Failed;
                // `{:#}` prints the whole chain: "couldn't open the vault at
                // …: <the reason from papa-core>".
                window.push_notification(
                    Notification::error(sentence(format!("{err:#}")))
                        .title("Papa can't open your vault")
                        .autohide(false),
                    cx,
                );
            }
        }
        cx.notify();
    }

    fn is_open(&self) -> bool {
        matches!(self.vault, Vault::Open { .. })
    }

    /// Runs `job` against the store in the background, then hands its result
    /// to `apply` back on the UI thread. A failure becomes a notification
    /// titled `failure`, and the state is left as it was.
    ///
    /// The `job` runs as a GPUI task: awaiting the store parks the task until
    /// the database thread replies, and the UI keeps rendering meanwhile.
    fn with_store<T: 'static>(
        &self,
        failure: &'static str,
        job: impl AsyncFnOnce(Store) -> papa_core::Result<T> + 'static,
        apply: impl FnOnce(&mut Self, T, &mut Window, &mut Context<Self>) + 'static,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Vault::Open { store, .. } = &self.vault else {
            return;
        };
        let store = store.clone();
        cx.spawn_in(window, async move |this, cx| {
            let result = job(store).await;
            this.update_in(cx, |this, window, cx| {
                match result {
                    Ok(value) => apply(this, value, window, cx),
                    Err(err) => window
                        .push_notification(Notification::error(sentence(err)).title(failure), cx),
                }
                cx.notify();
            })
        })
        .detach();
    }

    fn toggle_sidebar(&mut self, _: &ToggleSidebar, _: &mut Window, cx: &mut Context<Self>) {
        self.sidebar_visible = !self.sidebar_visible;
        cx.notify();
    }
}

impl Render for PapaApp {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let status: SharedString = match &self.vault {
            Vault::Opening => "Opening vault…".into(),
            Vault::Open { path, .. } => paths::display(path).into(),
            Vault::Failed => "Vault unavailable".into(),
        };
        let editor = match &self.editor {
            Some(editor) => editor.clone().into_any_element(),
            None => NoteView.into_any_element(),
        };

        v_flex()
            .key_context(APP_CONTEXT)
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::toggle_sidebar))
            .on_action(cx.listener(Self::new_note))
            .on_action(cx.listener(Self::toggle_preview))
            .on_action(cx.listener(Self::focus_search))
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
                                .child(
                                    div()
                                        .size_full()
                                        .key_context(SIDEBAR_CONTEXT)
                                        .track_focus(&self.sidebar_focus)
                                        .on_action(cx.listener(Self::select_next_filter))
                                        .on_action(cx.listener(Self::select_previous_filter))
                                        .child(AppSidebar::new(
                                            &self.children,
                                            &self.labels,
                                            self.selection.filter,
                                            self.is_open(),
                                            cx.listener(Self::on_sidebar_event),
                                        )),
                                ),
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
                                            self.list_heading(),
                                            self.notes.clone(),
                                            self.children.clone(),
                                            self.selection.note,
                                            self.search.clone(),
                                            self.note_scroll.clone(),
                                            self.empty_message(cx),
                                            self.is_open(),
                                            cx.listener(Self::on_note_list_event),
                                        )),
                                ),
                        )
                        .child(resizable_panel().child(editor)),
                ),
            )
            .child(
                StatusBar::new()
                    .left(status)
                    .right(if cx.theme().mode.is_dark() {
                        "Dark"
                    } else {
                        "Light"
                    }),
            )
    }
}

impl PapaApp {
    /// The note list's title: what the sidebar selection shows.
    fn list_heading(&self) -> SharedString {
        match self.selection.filter {
            NoteFilter::All => "All notes".into(),
            NoteFilter::Child(id) => self
                .child(id)
                .map_or("Notes".into(), |child| child.name.clone().into()),
            NoteFilter::Label(id) => self
                .label(id)
                .map_or("Notes".into(), |label| format!("#{}", label.name).into()),
        }
    }

    /// Why the note list is empty.
    fn empty_message(&self, cx: &App) -> SharedString {
        match self.vault {
            Vault::Opening => return "Opening your vault…".into(),
            Vault::Failed => return "Your vault couldn't be opened.".into(),
            Vault::Open { .. } => {}
        }
        let search = self.search.read(cx).value();
        if !search.trim().is_empty() {
            return format!("No notes match “{}”.", search.trim()).into();
        }
        match self.selection.filter {
            NoteFilter::All => "No notes yet. Click + to write one.".into(),
            NoteFilter::Child(id) => self.child(id).map_or("".into(), |child| {
                format!("No notes about {} yet.", child.name).into()
            }),
            NoteFilter::Label(id) => self.label(id).map_or("".into(), |label| {
                format!("No notes labelled #{} yet.", label.name).into()
            }),
        }
    }
}
