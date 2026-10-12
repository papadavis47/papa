//! The root view: owns the app's state and lays out the window.

use std::path::PathBuf;

use anyhow::Context as _;
use gpui_kit::component::{
    ActiveTheme as _, WindowExt as _, h_resizable, notification::Notification, resizable_panel,
    status_bar::StatusBar, v_flex,
};
use gpui_kit::{
    App, ClickEvent, Context, FocusHandle, InteractiveElement as _, IntoElement, KeyBinding,
    ParentElement as _, Render, SharedString, Styled as _, Window, div, px,
};
use papa_core::{Child, ChildDraft, ChildId, Label, LabelId, Store, paths};

use crate::state::Selection;
use crate::theme;
use crate::ui::{
    AppSidebar, AppTitleBar, NoteList, NoteView, SidebarEvent, dialogs, sentence, suggested_color,
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
        /// Select the child below the current one.
        SelectNextChild,
        /// Select the child above the current one.
        SelectPreviousChild,
    ]
);

/// Key context on the root element: its bindings work anywhere in the window.
pub const APP_CONTEXT: &str = "PapaApp";
/// Key context on the sidebar: its bindings work only while it has focus.
pub const CHILD_LIST_CONTEXT: &str = "ChildList";

/// Registers keybindings and app-wide action handlers. Call once at startup.
pub fn init(cx: &mut App) {
    cx.bind_keys([
        // `secondary` is Cmd on macOS and Ctrl elsewhere.
        KeyBinding::new("secondary-q", Quit, None),
        KeyBinding::new("secondary-shift-l", ToggleTheme, None),
        KeyBinding::new("secondary-b", ToggleSidebar, Some(APP_CONTEXT)),
        KeyBinding::new("down", SelectNextChild, Some(CHILD_LIST_CONTEXT)),
        KeyBinding::new("up", SelectPreviousChild, Some(CHILD_LIST_CONTEXT)),
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
    /// Copies of the vault's rows, refreshed after every change.
    children: Vec<Child>,
    labels: Vec<Label>,
    selection: Selection,
    sidebar_visible: bool,
    /// Clicking an element that tracks a focus handle focuses it. This one is
    /// on the root, so a click anywhere outside the sidebar still leaves
    /// focus inside `APP_CONTEXT` and the window-wide bindings keep working.
    focus_handle: FocusHandle,
    /// Focused while the sidebar is active; enables the `CHILD_LIST_CONTEXT` bindings.
    child_list_focus: FocusHandle,
}

impl PapaApp {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let child_list_focus = cx.focus_handle();
        // Start in the child list so the arrow keys work straight away.
        child_list_focus.focus(window, cx);

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
            children: Vec::new(),
            labels: Vec::new(),
            selection: Selection::default(),
            sidebar_visible: true,
            focus_handle: cx.focus_handle(),
            child_list_focus,
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
                self.vault = Vault::Open {
                    store: opened.store,
                    path: opened.path,
                };
                self.labels = opened.labels;
                self.set_children(opened.children);
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
        apply: impl FnOnce(&mut Self, T) + 'static,
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
                    Ok(value) => apply(this, value),
                    Err(err) => window
                        .push_notification(Notification::error(sentence(err)).title(failure), cx),
                }
                cx.notify();
            })
        })
        .detach();
    }

    fn set_children(&mut self, children: Vec<Child>) {
        self.children = children;
        let ids: Vec<ChildId> = self.children.iter().map(|child| child.id).collect();
        self.selection.reconcile(&ids);
    }

    fn child(&self, id: ChildId) -> Option<&Child> {
        self.children.iter().find(|child| child.id == id)
    }

    fn label(&self, id: LabelId) -> Option<&Label> {
        self.labels.iter().find(|label| label.id == id)
    }

    // --- Changes to the vault. Each reloads the affected list afterwards, so
    // the UI always shows exactly what's stored. ---

    fn add_child(&mut self, draft: ChildDraft, window: &mut Window, cx: &mut Context<Self>) {
        self.with_store(
            "Couldn't add the child",
            async move |store| {
                let child = store.add_child(draft).await?;
                Ok((child.id, store.children().await?))
            },
            |this, (id, children)| {
                this.selection.select_child(id);
                this.set_children(children);
            },
            window,
            cx,
        );
    }

    fn update_child(
        &mut self,
        id: ChildId,
        draft: ChildDraft,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.with_store(
            "Couldn't save the child",
            async move |store| {
                store.update_child(id, draft).await?;
                store.children().await
            },
            Self::set_children,
            window,
            cx,
        );
    }

    fn delete_child(&mut self, id: ChildId, window: &mut Window, cx: &mut Context<Self>) {
        self.with_store(
            "Couldn't delete the child",
            async move |store| {
                store.delete_child(id).await?;
                store.children().await
            },
            Self::set_children,
            window,
            cx,
        );
    }

    fn add_label(&mut self, name: String, window: &mut Window, cx: &mut Context<Self>) {
        self.with_store(
            "Couldn't add the label",
            async move |store| {
                store.add_label(name).await?;
                store.labels().await
            },
            |this, labels| this.labels = labels,
            window,
            cx,
        );
    }

    fn rename_label(
        &mut self,
        id: LabelId,
        name: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.with_store(
            "Couldn't rename the label",
            async move |store| {
                store.rename_label(id, name).await?;
                store.labels().await
            },
            |this, labels| this.labels = labels,
            window,
            cx,
        );
    }

    fn delete_label(&mut self, id: LabelId, window: &mut Window, cx: &mut Context<Self>) {
        self.with_store(
            "Couldn't delete the label",
            async move |store| {
                store.delete_label(id).await?;
                store.labels().await
            },
            |this, labels| this.labels = labels,
            window,
            cx,
        );
    }

    // --- Input ---

    fn on_sidebar_event(
        &mut self,
        event: &SidebarEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match *event {
            SidebarEvent::SelectChild(id) => {
                self.selection.select_child(id);
                // The click landed in the sidebar; keep focus on the list so
                // the arrow keys move through the children.
                self.child_list_focus.focus(window, cx);
                cx.notify();
            }
            SidebarEvent::AddChild => {
                let used: Vec<u32> = self.children.iter().map(|child| child.color).collect();
                let color = suggested_color(&used);
                dialogs::child_dialog(
                    None,
                    color,
                    cx.listener(|this, draft: &ChildDraft, window, cx| {
                        this.add_child(draft.clone(), window, cx)
                    }),
                    window,
                    cx,
                );
            }
            SidebarEvent::EditChild(id) => {
                let Some(child) = self.child(id) else { return };
                dialogs::child_dialog(
                    Some(child),
                    child.color,
                    cx.listener(move |this, draft: &ChildDraft, window, cx| {
                        this.update_child(id, draft.clone(), window, cx)
                    }),
                    window,
                    cx,
                );
            }
            SidebarEvent::DeleteChild(id) => {
                let Some(child) = self.child(id) else { return };
                dialogs::confirm_delete(
                    format!("Delete {}?", child.name),
                    "This removes them from your vault and can't be undone.",
                    cx.listener(move |this, _: &ClickEvent, window, cx| {
                        this.delete_child(id, window, cx)
                    }),
                    window,
                    cx,
                );
            }
            SidebarEvent::AddLabel => {
                let taken = self.labels.iter().map(|l| l.name.clone()).collect();
                dialogs::label_dialog(
                    None,
                    taken,
                    cx.listener(|this, name: &String, window, cx| {
                        this.add_label(name.clone(), window, cx)
                    }),
                    window,
                    cx,
                );
            }
            SidebarEvent::RenameLabel(id) => {
                let Some(label) = self.label(id) else { return };
                let taken = self
                    .labels
                    .iter()
                    .filter(|l| l.id != id)
                    .map(|l| l.name.clone())
                    .collect();
                dialogs::label_dialog(
                    Some(label),
                    taken,
                    cx.listener(move |this, name: &String, window, cx| {
                        this.rename_label(id, name.clone(), window, cx)
                    }),
                    window,
                    cx,
                );
            }
            SidebarEvent::DeleteLabel(id) => {
                let Some(label) = self.label(id) else { return };
                dialogs::confirm_delete(
                    format!("Delete #{}?", label.name),
                    "This can't be undone.",
                    cx.listener(move |this, _: &ClickEvent, window, cx| {
                        this.delete_label(id, window, cx)
                    }),
                    window,
                    cx,
                );
            }
        }
    }

    fn toggle_sidebar(&mut self, _: &ToggleSidebar, _: &mut Window, cx: &mut Context<Self>) {
        self.sidebar_visible = !self.sidebar_visible;
        cx.notify();
    }

    fn select_next_child(&mut self, _: &SelectNextChild, _: &mut Window, cx: &mut Context<Self>) {
        self.move_child(1, cx);
    }

    fn select_previous_child(
        &mut self,
        _: &SelectPreviousChild,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.move_child(-1, cx);
    }

    fn move_child(&mut self, delta: isize, cx: &mut Context<Self>) {
        let ids: Vec<ChildId> = self.children.iter().map(|child| child.id).collect();
        self.selection.move_child(delta, &ids);
        cx.notify();
    }
}

impl Render for PapaApp {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let child = self.selection.child.and_then(|id| self.child(id));
        let (heading, message): (Option<SharedString>, SharedString) = match (&self.vault, child) {
            (Vault::Opening, _) => (None, "Opening your vault…".into()),
            (Vault::Failed, _) => (None, "Your vault couldn't be opened.".into()),
            (Vault::Open { .. }, Some(child)) => (
                Some(child.name.clone().into()),
                format!("No notes about {} yet.", child.name).into(),
            ),
            (Vault::Open { .. }, None) => (None, "Add a child in the sidebar to begin.".into()),
        };
        let status: SharedString = match &self.vault {
            Vault::Opening => "Opening vault…".into(),
            Vault::Open { path, .. } => paths::display(path).into(),
            Vault::Failed => "Vault unavailable".into(),
        };

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
                                .child(
                                    div()
                                        .size_full()
                                        .key_context(CHILD_LIST_CONTEXT)
                                        .track_focus(&self.child_list_focus)
                                        .on_action(cx.listener(Self::select_next_child))
                                        .on_action(cx.listener(Self::select_previous_child))
                                        .child(AppSidebar::new(
                                            &self.children,
                                            &self.labels,
                                            self.selection.child,
                                            matches!(self.vault, Vault::Open { .. }),
                                            cx.listener(Self::on_sidebar_event),
                                        )),
                                ),
                        )
                        .child(
                            resizable_panel()
                                .size(px(300.))
                                .size_range(px(220.)..px(480.))
                                .flex_none()
                                .child(NoteList::new(heading, message)),
                        )
                        .child(resizable_panel().child(NoteView)),
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
