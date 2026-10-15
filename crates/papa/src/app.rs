//! The root view: owns the app's state and lays out the window.
//!
//! `PapaApp`'s methods are split by the pane they serve: `sidebar.rs`
//! (children and labels) and `notes.rs` (the note list and the editor),
//! and by feature: `export.rs` (PDF), `lock.rs` (the password, locking)
//! and `backup.rs`. Rust allows several `impl` blocks for one type, in
//! different modules.

mod backup;
mod export;
mod lock;
mod notes;
mod palette;
mod sharing;
mod sidebar;

pub use lock::VaultStatus;
pub use sharing::AccountStatus;

use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, Instant};

use anyhow::Context as _;
use gpui_kit::component::{
    ActiveTheme as _, Icon, IconName, StyledExt as _, WindowExt as _, h_flex, h_resizable,
    input::{InputEvent, InputState},
    notification::Notification,
    resizable_panel,
    status_bar::StatusBar,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    Action, AnyWindowHandle, App, AppContext as _, Context, Entity, FocusHandle, Global,
    InteractiveElement as _, IntoElement, KeyBinding, KeyContext, ParentElement as _, Render,
    SharedString, Styled as _, Subscription, Task, UniformListScrollHandle, Window, div, px,
};
use papa_core::lock::{Password, VaultKind};
use papa_core::{
    Account, Child, ChildShare, Label, NoteFilter, NoteSummary, Sharing, Store, paths,
};

use crate::settings::AppSettings;
use crate::state::Selection;
use crate::ui::lightbox::{CloseLightbox, LIGHTBOX_CONTEXT, NextImage, PreviousImage};
use crate::ui::{
    AppSidebar, AppTitleBar, COMMAND_PALETTE_CONTEXT, CommandPalette, EmptyNotes,
    NOTE_SEARCH_CONTEXT, NoteEditor, NoteList, NoteListEvent, NoteView, UnlockScreen, sentence,
};
use crate::{settings_window, theme, vim};

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
        /// Choose images to add to the open note.
        AddImages,
        /// Open the Settings window.
        OpenSettings,
        /// Open the command palette, or close it if it's open.
        ToggleCommandPalette,
        /// Move keyboard focus to the sidebar.
        FocusSidebar,
        /// Move keyboard focus to the note list.
        FocusNoteList,
        /// Move keyboard focus to the open note.
        FocusEditor,
        /// Save the open note as a PDF.
        ExportNote,
        /// Save the notes in the list as a PDF: a child's book, or the
        /// notes a label or search picked out.
        ExportNotes,
        /// Lock a vault that has a password: close it and ask for the
        /// password again.
        LockVault,
        /// Give the vault a password, or change the one it has.
        ChangePassword,
        /// Take the password off the vault.
        RemovePassword,
        /// Save a backup of the vault.
        ExportBackup,
        /// Replace the vault with a backup.
        RestoreBackup,
        /// Sign in to a Papa account, to share notes with children.
        SignIn,
        /// Create a Papa account with an invite code.
        CreateAccount,
        /// Sign out of the Papa account.
        SignOut,
        /// Sync shared notes now, rather than after the next change.
        SyncNow,
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
/// Key context on the note editor, title, text and preview included.
pub const NOTE_EDITOR_CONTEXT: &str = "NoteEditor";
/// Added to the sidebar's and the note list's key contexts while Vim mode
/// is on, so Vim's keys move through them.
pub const VIM_CONTEXT: &str = "vim";

/// Registers keybindings and app-wide action handlers. Call once at startup,
/// after `gpui_kit::init`.
pub fn init(cx: &mut App) {
    // A focused text field handles ↑/↓ itself. Matching on the search box's
    // `Input` makes this binding as specific as the field's own, and the
    // later of two equally specific bindings wins, so here the arrows move
    // through the notes while the search keeps focus.
    let in_search = format!("{NOTE_SEARCH_CONTEXT} > Input");
    // Text fields bind ⌘F to a search of their own text. The palette's
    // search box has nothing to search, so there ⌘F does what it does
    // elsewhere, and the palette shows ⌘F next to "Search notes".
    let in_palette = format!("{COMMAND_PALETTE_CONTEXT} > Input");
    // With Vim mode on, the lists take Vim's keys: j and k move, h and l
    // move between panes, / searches. `!Input` keeps the search box, which
    // is inside the list's pane, typing them as letters.
    let vim_sidebar = format!("{SIDEBAR_CONTEXT} && {VIM_CONTEXT} && !Input");
    let vim_list = format!("{NOTE_LIST_CONTEXT} && {VIM_CONTEXT} && !Input");
    cx.bind_keys([
        // `secondary` is Cmd on macOS and Ctrl elsewhere.
        KeyBinding::new("secondary-q", Quit, None),
        KeyBinding::new("secondary-shift-l", ToggleTheme, None),
        // No context: works in the Settings window too, where it brings
        // that window forward.
        KeyBinding::new("secondary-,", OpenSettings, None),
        KeyBinding::new("secondary-b", ToggleSidebar, Some(APP_CONTEXT)),
        KeyBinding::new("secondary-n", NewNote, Some(APP_CONTEXT)),
        KeyBinding::new("secondary-e", TogglePreview, Some(APP_CONTEXT)),
        KeyBinding::new("secondary-f", FocusSearch, Some(APP_CONTEXT)),
        KeyBinding::new("secondary-f", FocusSearch, Some(&in_palette)),
        KeyBinding::new("secondary-shift-o", AddImages, Some(APP_CONTEXT)),
        KeyBinding::new("secondary-p", ExportNote, Some(APP_CONTEXT)),
        KeyBinding::new("secondary-shift-p", ExportNotes, Some(APP_CONTEXT)),
        KeyBinding::new("secondary-l", LockVault, Some(APP_CONTEXT)),
        KeyBinding::new("secondary-k", ToggleCommandPalette, Some(APP_CONTEXT)),
        KeyBinding::new("secondary-1", FocusSidebar, Some(APP_CONTEXT)),
        KeyBinding::new("secondary-2", FocusNoteList, Some(APP_CONTEXT)),
        KeyBinding::new("secondary-3", FocusEditor, Some(APP_CONTEXT)),
        KeyBinding::new("escape", CloseLightbox, Some(LIGHTBOX_CONTEXT)),
        KeyBinding::new("right", NextImage, Some(LIGHTBOX_CONTEXT)),
        KeyBinding::new("left", PreviousImage, Some(LIGHTBOX_CONTEXT)),
        KeyBinding::new("down", SelectNextFilter, Some(SIDEBAR_CONTEXT)),
        KeyBinding::new("up", SelectPreviousFilter, Some(SIDEBAR_CONTEXT)),
        KeyBinding::new("down", SelectNextNote, Some(NOTE_LIST_CONTEXT)),
        KeyBinding::new("up", SelectPreviousNote, Some(NOTE_LIST_CONTEXT)),
        KeyBinding::new("down", SelectNextNote, Some(&in_search)),
        KeyBinding::new("up", SelectPreviousNote, Some(&in_search)),
        // Moving between the panes, left to right: sidebar, list, note. In
        // the search box, ←/→ and Enter stay the text field's: its own
        // bindings are more specific than these. Enter there opens the
        // note through the field's `PressEnter` event instead.
        KeyBinding::new("right", FocusNoteList, Some(SIDEBAR_CONTEXT)),
        KeyBinding::new("enter", FocusNoteList, Some(SIDEBAR_CONTEXT)),
        KeyBinding::new("left", FocusSidebar, Some(NOTE_LIST_CONTEXT)),
        KeyBinding::new("right", FocusEditor, Some(NOTE_LIST_CONTEXT)),
        KeyBinding::new("enter", FocusEditor, Some(NOTE_LIST_CONTEXT)),
        // A text field handles Esc first (closing its menu, say) and lets
        // it through when it has nothing to do with it. GPUI then tries the
        // next binding for the key: this one.
        KeyBinding::new("escape", FocusNoteList, Some(NOTE_EDITOR_CONTEXT)),
        KeyBinding::new("j", SelectNextFilter, Some(&vim_sidebar)),
        KeyBinding::new("k", SelectPreviousFilter, Some(&vim_sidebar)),
        KeyBinding::new("l", FocusNoteList, Some(&vim_sidebar)),
        KeyBinding::new("j", SelectNextNote, Some(&vim_list)),
        KeyBinding::new("k", SelectPreviousNote, Some(&vim_list)),
        KeyBinding::new("h", FocusSidebar, Some(&vim_list)),
        KeyBinding::new("l", FocusEditor, Some(&vim_list)),
        KeyBinding::new("/", FocusSearch, Some(&vim_list)),
    ]);

    // App-level handlers: quitting, the theme and the Settings window are
    // global, not owned by a view.
    cx.on_action(|_: &Quit, cx| cx.quit());
    cx.on_action(|_: &ToggleTheme, cx| theme::toggle(cx));
    cx.on_action(|_: &OpenSettings, cx| settings_window::open(cx));
}

/// The main window, so the Settings window can send it commands.
#[derive(Clone, Copy)]
struct MainWindow(AnyWindowHandle);

impl Global for MainWindow {}

/// Runs `action` in the main window, as if chosen there, and brings that
/// window forward. The Settings window's vault buttons use it: the vault
/// and its dialogs belong to the main window.
pub fn dispatch_to_main(action: &dyn Action, cx: &mut App) {
    let Some(&MainWindow(window)) = cx.try_global::<MainWindow>() else {
        return;
    };
    let action = action.boxed_clone();
    window
        .update(cx, |_, window, cx| {
            window.activate_window();
            window.dispatch_action(action, cx);
        })
        .ok();
}

/// Ends the app when `main_window` closes, even if the Settings window is
/// still open.
pub fn quit_with(main_window: AnyWindowHandle, cx: &mut App) {
    cx.set_global(MainWindow(main_window));
    let main_window = main_window.window_id();
    cx.on_window_closed(move |cx, closed| {
        if closed == main_window || cx.windows().is_empty() {
            cx.quit();
        }
    })
    .detach();
}

/// Where the vault is in its life.
enum Vault {
    /// The database thread is starting up and migrating, or the vault is
    /// being closed or replaced.
    Opening,
    /// Encrypted, and waiting for its password.
    Locked {
        path: PathBuf,
        screen: Entity<UnlockScreen>,
    },
    Open {
        /// Cheap to clone: each background task takes its own copy.
        store: Store,
        path: PathBuf,
        /// It has a password.
        encrypted: bool,
        /// The father's side of sharing, on its own thread. Dropped with
        /// the vault, which stops that thread.
        sharing: Sharing,
    },
    /// Opening failed; the error was shown in a notification.
    Failed,
}

/// How far opening the vault got.
enum Opened {
    Open(OpenedVault),
    /// It's encrypted, and no password was given.
    Locked(PathBuf),
}

/// What opening hands back once the vault is open.
struct OpenedVault {
    store: Store,
    path: PathBuf,
    encrypted: bool,
    sharing: Sharing,
    children: Vec<Child>,
    labels: Vec<Label>,
    account: Option<Account>,
    shares: Vec<ChildShare>,
}

/// Opens the vault at `path` and reads everything the sidebar shows. An
/// encrypted vault is opened with `password`, or comes back
/// [`Opened::Locked`] without one.
///
/// Returns `anyhow::Result`: the app only displays these errors, so it adds
/// context to them rather than matching on `papa_core::Error`'s variants.
async fn open_vault(path: PathBuf, password: Option<Password>) -> anyhow::Result<Opened> {
    let encrypted = papa_core::lock::inspect(&path)? == VaultKind::Encrypted;
    let store = match password {
        Some(password) if encrypted => Store::unlock(&path, password).await,
        _ if encrypted => return Ok(Opened::Locked(path)),
        _ => Store::open(&path).await,
    }
    .with_context(|| format!("couldn't open the vault at {}", paths::display(&path)))?;
    let children = store.children().await.context("couldn't load children")?;
    let labels = store.labels().await.context("couldn't load labels")?;
    let account = store
        .account()
        .await
        .context("couldn't load your account")?;
    let shares = store.shares().await.context("couldn't load your sharing")?;
    let sharing = Sharing::start(store.clone(), paths::shares_dir(&path))?;
    Ok(Opened::Open(OpenedVault {
        store,
        path,
        encrypted,
        sharing,
        children,
        labels,
        account,
        shares,
    }))
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
    /// Created once the vault is open: it needs the store. Dropped when
    /// it closes, with everything it shows.
    editor: Option<Entity<NoteEditor>>,
    editor_subscription: Option<Subscription>,
    /// Counts the vaults opened. A store request answered after its vault
    /// was closed (locked, say) is ignored: its results belong to a vault
    /// that's gone.
    generation: u64,
    /// The last key press, click or mouse move: auto-lock counts from here.
    last_activity: Instant,
    /// Checks every few seconds whether it's time to auto-lock.
    _auto_lock: Task<()>,
    unlock_subscription: Option<Subscription>,
    /// ⌘K. Drawn over everything else while open.
    palette: Entity<CommandPalette>,
    sidebar_visible: bool,
    /// The father's Papa account, if he's signed in.
    account: Option<Account>,
    /// Each shared child's share. `Rc`, like the lists above.
    shares: Rc<Vec<ChildShare>>,
    sync_status: sharing::SyncStatus,
    /// The next or running sync of shared notes. Replacing it cancels one
    /// still waiting for its delay.
    sync_task: Option<Task<()>>,
    /// The open sharing dialog, so the results of its buttons reach it.
    share_dialog: Option<(Entity<crate::ui::ShareDialog>, Subscription)>,
    /// What's being exported to PDF, for the status bar: "Exporting
    /// Emma’s book…". One export at a time.
    exporting: Option<SharedString>,
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
    /// `settings_problem` is why the settings file couldn't be read, if it
    /// couldn't; it's shown once the window is up.
    pub fn new(
        settings_problem: Option<anyhow::Error>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let sidebar_focus = cx.focus_handle();
        // Start in the sidebar so the arrow keys work straight away.
        sidebar_focus.focus(window, cx);

        let search = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Search")
                .clean_on_escape()
        });
        let palette = cx.new(|cx| CommandPalette::new(window, cx));
        let subscriptions = vec![
            cx.subscribe_in(
                &search,
                window,
                |this, _, event: &InputEvent, window, cx| match event {
                    InputEvent::Change => this.search_changed(window, cx),
                    // Search, pick a note with ↑/↓, then Enter to read it.
                    InputEvent::PressEnter { .. } => this.focus_editor(&FocusEditor, window, cx),
                    _ => {}
                },
            ),
            cx.subscribe_in(&palette, window, Self::on_palette_event),
            // A closing dialog hands focus back to whatever had it before,
            // and that may be gone (a right-click menu, say). With nothing
            // focused, no key context matches and the shortcuts stop working,
            // so fall back to the root.
            cx.on_focus_lost(window, |this, window, cx| {
                this.focus_handle.focus(window, cx);
            }),
            // "Match system" follows the OS switching between light and dark
            // while the app runs.
            cx.observe_window_appearance(window, |_, window, cx| {
                theme::set_system_appearance(window, cx)
            }),
            // Density, fonts and line height are read while rendering, so a
            // change only needs a redraw. It redraws the whole window, the
            // editor included.
            cx.observe_global::<AppSettings>(|_, cx| cx.notify()),
            // Typing anywhere, the Settings window included, counts as use.
            cx.observe_keystrokes(|this, _, _, _| this.last_activity = Instant::now()),
        ];
        theme::set_system_appearance(window, cx);
        if let Some(err) = settings_problem {
            // Deferred: the window's `Root`, which shows notifications, is
            // built around this view after `new` returns.
            cx.defer_in(window, move |_, window, cx| {
                window.push_notification(
                    Notification::warning(sentence(format!("{err:#}")))
                        .title("Papa couldn't read your settings")
                        .autohide(false),
                    cx,
                );
            });
        }

        // The window shows up immediately; this task waits for the database
        // thread without blocking the UI, then fills in the state. `this` is a
        // weak handle, so the task doesn't keep a closed window's view alive.
        cx.spawn_in(window, async move |this, cx| {
            let opened = async { open_vault(paths::vault_path()?, None).await }.await;
            this.update_in(cx, |this, window, cx| this.vault_opened(opened, window, cx))
        })
        .detach();
        cx.set_global(VaultStatus::Unavailable);
        cx.set_global(AccountStatus::default());

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
            editor_subscription: None,
            generation: 0,
            last_activity: Instant::now(),
            _auto_lock: Self::watch_idle(window, cx),
            unlock_subscription: None,
            palette,
            sidebar_visible: true,
            account: None,
            shares: Rc::default(),
            sync_status: sharing::SyncStatus::default(),
            sync_task: None,
            share_dialog: None,
            exporting: None,
            focus_handle: cx.focus_handle(),
            sidebar_focus,
            note_list_focus: cx.focus_handle(),
            _subscriptions: subscriptions,
        }
    }

    fn vault_opened(
        &mut self,
        opened: anyhow::Result<Opened>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match opened {
            Ok(Opened::Open(opened)) => {
                let editor = cx.new(|cx| NoteEditor::new(opened.store.clone(), window, cx));
                self.editor_subscription =
                    Some(cx.subscribe_in(&editor, window, Self::on_editor_event));
                self.editor = Some(editor);
                self.unlock_subscription = None;
                self.set_vault(
                    Vault::Open {
                        store: opened.store,
                        path: opened.path,
                        encrypted: opened.encrypted,
                        sharing: opened.sharing,
                    },
                    cx,
                );
                self.last_activity = Instant::now();
                self.children = Rc::new(opened.children);
                self.labels = Rc::new(opened.labels);
                self.account = opened.account;
                self.shares = Rc::new(opened.shares);
                self.catalog_changed(window, cx);
                self.sharing_changed(cx);
                self.sidebar_focus.focus(window, cx);
                // Whatever the last session didn't get to: syncing is
                // always safe to repeat.
                self.schedule_sync(Duration::ZERO, window, cx);
            }
            Ok(Opened::Locked(path)) => self.show_locked(path, window, cx),
            Err(err) => {
                self.set_vault(Vault::Failed, cx);
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
        let generation = self.generation;
        cx.spawn_in(window, async move |this, cx| {
            let result = job(store).await;
            this.update_in(cx, |this, window, cx| {
                if this.generation != generation {
                    return;
                }
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

    // --- Moving focus between the panes. Focusing a pane is what makes its
    // key bindings apply, and what draws its focus ring. ---

    fn focus_sidebar(&mut self, _: &FocusSidebar, window: &mut Window, cx: &mut Context<Self>) {
        // A hidden sidebar can't take focus: show it.
        self.sidebar_visible = true;
        self.sidebar_focus.focus(window, cx);
        cx.notify();
    }

    fn focus_note_list(&mut self, _: &FocusNoteList, window: &mut Window, cx: &mut Context<Self>) {
        self.note_list_focus.focus(window, cx);
    }

    fn focus_editor(&mut self, _: &FocusEditor, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(editor) = &self.editor {
            editor.update(cx, |editor, cx| editor.focus(window, cx));
        }
    }
}

impl Render for PapaApp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // GPUI redraws the window whenever focus moves, so these are
        // always current. `contains_focused`: the search box counts as
        // the list, since ↑/↓ move through the notes from there too.
        let sidebar_focused = self.sidebar_focus.contains_focused(window, cx);
        let note_list_focused = self.note_list_focus.contains_focused(window, cx);
        let status: SharedString = match &self.vault {
            Vault::Opening => "Opening vault…".into(),
            Vault::Locked { .. } => "Locked".into(),
            Vault::Open {
                path,
                encrypted: true,
                ..
            } => format!("{} · encrypted", paths::display(path)).into(),
            Vault::Open { path, .. } => paths::display(path).into(),
            Vault::Failed => "Vault unavailable".into(),
        };
        let locked = match &self.vault {
            Vault::Locked { screen, .. } => Some(screen.clone()),
            _ => None,
        };
        let encrypted = matches!(
            self.vault,
            Vault::Open {
                encrypted: true,
                ..
            }
        );
        let vim = AppSettings::get(cx).editor.vim_mode;
        let vim_status = self
            .editor
            .as_ref()
            .and_then(|editor| editor.read(cx).vim_status(window, cx));
        let editor = match &self.editor {
            Some(editor) => editor.clone().into_any_element(),
            None => NoteView::new(false).into_any_element(),
        };
        // Last child, so it's drawn over the panes. Renders nothing while
        // closed.
        let lightbox = self
            .editor
            .as_ref()
            .map(|editor| editor.read(cx).gallery().read(cx).lightbox().clone());

        v_flex()
            .key_context(APP_CONTEXT)
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::toggle_sidebar))
            .on_action(cx.listener(Self::new_note))
            .on_action(cx.listener(Self::toggle_preview))
            .on_action(cx.listener(Self::focus_search))
            .on_action(cx.listener(Self::add_images))
            .on_action(cx.listener(Self::toggle_command_palette))
            .on_action(cx.listener(Self::focus_sidebar))
            .on_action(cx.listener(Self::focus_note_list))
            .on_action(cx.listener(Self::focus_editor))
            .on_action(cx.listener(Self::export_note))
            .on_action(cx.listener(Self::export_notes))
            .on_action(cx.listener(Self::lock_vault))
            .on_action(cx.listener(Self::change_password))
            .on_action(cx.listener(Self::remove_password))
            .on_action(cx.listener(Self::export_backup))
            .on_action(cx.listener(Self::restore_backup))
            .on_action(cx.listener(Self::sign_in))
            .on_action(cx.listener(Self::create_account))
            .on_action(cx.listener(Self::sign_out))
            .on_action(cx.listener(Self::sync_now))
            // Using the window keeps an encrypted vault from auto-locking.
            .on_mouse_move(cx.listener(|this, _, _, _| this.last_activity = Instant::now()))
            .capture_any_mouse_down(
                cx.listener(|this, _, _, _| this.last_activity = Instant::now()),
            )
            .on_scroll_wheel(cx.listener(|this, _, _, _| this.last_activity = Instant::now()))
            .size_full()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .child(AppTitleBar::new(
                self.sidebar_visible,
                locked.is_some(),
                encrypted,
            ))
            .when_some(locked.clone(), |this, screen| {
                this.child(div().flex_1().min_h_0().child(screen))
            })
            .when(locked.is_none(), |this| {
                this.child(
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
                                            .key_context(pane_context(SIDEBAR_CONTEXT, vim))
                                            .track_focus(&self.sidebar_focus)
                                            .on_action(cx.listener(Self::select_next_filter))
                                            .on_action(cx.listener(Self::select_previous_filter))
                                            .child(
                                                AppSidebar::new(
                                                    &self.children,
                                                    &self.shared_children(),
                                                    &self.labels,
                                                    self.selection.filter,
                                                    self.is_open(),
                                                    cx.listener(Self::on_sidebar_event),
                                                )
                                                .focused(sidebar_focused),
                                            ),
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
                                            .key_context(pane_context(NOTE_LIST_CONTEXT, vim))
                                            .track_focus(&self.note_list_focus)
                                            .on_action(cx.listener(Self::select_next_note))
                                            .on_action(cx.listener(Self::select_previous_note))
                                            .child(
                                                NoteList::new(
                                                    self.list_heading(),
                                                    self.notes.clone(),
                                                    self.children.clone(),
                                                    self.selection.note,
                                                    self.search.clone(),
                                                    self.note_scroll.clone(),
                                                    self.empty_notes(cx),
                                                    AppSettings::get(cx).appearance.density,
                                                    self.is_open(),
                                                    cx.listener(Self::on_note_list_event),
                                                )
                                                .export(self.list_export_name(cx))
                                                .focused(note_list_focused),
                                            ),
                                    ),
                            )
                            .child(resizable_panel().child(editor)),
                    ),
                )
            })
            .child(
                StatusBar::new()
                    .when_some(vim_status, |bar, status| {
                        bar.left(vim_indicator(status, cx))
                    })
                    .left(status)
                    .when_some(self.exporting.clone(), |bar, exporting| bar.left(exporting))
                    .when_some(self.sync_label(), |bar, label| bar.right(label))
                    .right(cx.theme().theme_name().clone()),
            )
            .children(lightbox)
            .child(self.palette.clone())
    }
}

/// A pane's key context, with [`VIM_CONTEXT`] while Vim mode is on.
fn pane_context(name: &'static str, vim: bool) -> KeyContext {
    let mut context = KeyContext::default();
    context.add(name);
    if vim {
        context.add(VIM_CONTEXT);
    }
    context
}

/// Vim's mode, then the keys typed so far or a message: `NORMAL  2d`.
fn vim_indicator(status: vim::Status, cx: &App) -> impl IntoElement {
    h_flex()
        .gap_2()
        .children(status.mode.map(|mode| {
            div()
                .font_semibold()
                .text_color(cx.theme().foreground)
                .child(mode)
        }))
        .child(status.detail)
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

    /// Why the note list is empty, and what to do about it.
    fn empty_notes(&self, cx: &App) -> EmptyNotes {
        use gpui_kit::assets::IconName as Lucide;

        let empty = |id, icon: Icon, title: SharedString| EmptyNotes {
            id,
            icon,
            title,
            description: None,
            action: None,
        };
        let new_note =
            |label: &str| Some((SharedString::from(label.to_owned()), NoteListEvent::NewNote));
        match self.vault {
            Vault::Opening | Vault::Locked { .. } => {
                return empty(
                    "opening",
                    IconName::BookOpen.into(),
                    "Opening your vault…".into(),
                );
            }
            Vault::Failed => {
                return EmptyNotes {
                    description: Some("The message in the corner says why.".into()),
                    ..empty(
                        "failed",
                        IconName::CircleAlert.into(),
                        "Your vault couldn't be opened".into(),
                    )
                };
            }
            Vault::Open { .. } => {}
        }
        let search = self.search.read(cx).value();
        if !search.trim().is_empty() {
            return EmptyNotes {
                description: Some(format!("No note here mentions “{}”.", search.trim()).into()),
                action: Some(("Clear search".into(), NoteListEvent::ClearSearch)),
                ..empty("no-match", Lucide::SearchX.into(), "No matches".into())
            };
        }
        match self.selection.filter {
            NoteFilter::All => EmptyNotes {
                description: Some("Write down a moment worth remembering.".into()),
                action: new_note("New note"),
                ..empty(
                    "no-notes",
                    Lucide::NotebookPen.into(),
                    "No notes yet".into(),
                )
            },
            NoteFilter::Child(id) => {
                let name = self.child(id).map_or("", |child| child.name.as_str());
                EmptyNotes {
                    description: Some(format!("Notes you write about {name} gather here.").into()),
                    action: new_note(&format!("New note about {name}")),
                    ..empty(
                        "no-child-notes",
                        IconName::CircleUser.into(),
                        format!("Nothing about {name} yet").into(),
                    )
                }
            }
            NoteFilter::Label(id) => {
                let name = self.label(id).map_or("", |label| label.name.as_str());
                EmptyNotes {
                    description: Some(
                        "Add the label to a note with the + next to its date, or write a new one."
                            .into(),
                    ),
                    action: new_note("New note"),
                    ..empty(
                        "no-label-notes",
                        Lucide::Hash.into(),
                        format!("Nothing labelled #{name}").into(),
                    )
                }
            }
        }
    }
}
