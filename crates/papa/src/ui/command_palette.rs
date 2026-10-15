//! The command palette (⌘K): a search box over every command, child, label
//! and note, built on gpui-kit's `Command`.
//!
//! `PapaApp` renders it as the last child of its root, over the panes,
//! rather than in one of gpui-kit's dialogs. Actions start at the focused
//! element and bubble up through its parents. Dialogs are drawn beside
//! `PapaApp`, not inside it, so ⌘N chosen in a dialog would never reach
//! `PapaApp`'s handler, and the palette couldn't find the shortcuts bound
//! in its key context to show them. Inside the root, both work: choosing
//! "New note" here is the same as pressing ⌘N.

use std::rc::Rc;
use std::time::Duration;

use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, IconName,
    command::{Command, CommandGroup, CommandItem, CommandState},
    v_flex,
};
use gpui_kit::{
    Action, Animation, AnimationExt as _, AppContext as _, Context, Entity, EventEmitter,
    FocusHandle, InteractiveElement as _, IntoElement, ParentElement as _, Render, Styled as _,
    Subscription, Window, div, ease_out_quint, px,
};
use papa_core::NoteFilter;

use crate::app;
use crate::palette::{PaletteCommand, PaletteGroup, PaletteItem, Shortcut};

/// Key context of the open palette.
pub const COMMAND_PALETTE_CONTEXT: &str = "CommandPalette";

/// How long the palette takes to appear. Short: it's in the way of typing.
const APPEAR: Duration = Duration::from_millis(160);

/// What the palette asks of `PapaApp`, once it has closed and handed focus
/// back. Commands with a shortcut don't come through here: the palette
/// dispatches their action instead.
#[derive(Clone, Copy, Debug)]
pub enum CommandPaletteEvent {
    Run(PaletteCommand),
}

pub struct CommandPalette {
    /// The search box, the highlighted row and the list's scroll position.
    state: Entity<CommandState>,
    /// What to show, built by `PapaApp` from its state when it opens.
    groups: Rc<Vec<PaletteGroup>>,
    open: bool,
    /// How many times the palette has opened. Part of the animation's
    /// element id: GPUI keeps an animation's progress under its id, so a new
    /// id each time makes every opening play it from the start.
    openings: usize,
    /// On the panel around the search box and list, so it contains their
    /// focus. When focus moves anywhere outside, the palette closes. Not on
    /// the backdrop: clicking an element that tracks a focus handle focuses
    /// it, and a click on the backdrop should only close the palette.
    focus_handle: FocusHandle,
    /// What had focus before opening, to give it back on close.
    previous_focus: Option<FocusHandle>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<CommandPaletteEvent> for CommandPalette {}

impl CommandPalette {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let state = cx.new(|cx| CommandState::new(window, cx));
        let focus_handle = cx.focus_handle();
        // A command that moves focus, such as "Search notes" or "New note",
        // closes the palette by doing so. Clicking outside it does too.
        // Focus is already where it should be then, so it isn't given back.
        let subscriptions = vec![cx.on_focus_out(&focus_handle, window, |this, _, _, cx| {
            this.previous_focus = None;
            this.hide(cx);
        })];
        Self {
            state,
            groups: Rc::default(),
            open: false,
            openings: 0,
            focus_handle,
            previous_focus: None,
            _subscriptions: subscriptions,
        }
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Opens the palette with an empty search, showing `groups`. `loading`
    /// spins the search box until [`Self::set_groups`] brings the rest.
    pub fn open(
        &mut self,
        groups: Vec<PaletteGroup>,
        loading: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.open {
            self.previous_focus = window.focused(cx);
            self.openings += 1;
        }
        self.open = true;
        self.groups = Rc::new(groups);
        self.state.update(cx, |state, cx| {
            state.set_query("", window, cx);
            state.set_loading(loading, window, cx);
            state.focus(window, cx);
        });
        cx.notify();
    }

    /// Replaces what the palette shows, keeping the search and the
    /// highlighted row.
    pub fn set_groups(
        &mut self,
        groups: Vec<PaletteGroup>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.open {
            return;
        }
        self.groups = Rc::new(groups);
        self.state
            .update(cx, |state, cx| state.set_loading(false, window, cx));
        cx.notify();
    }

    /// Closes the palette and gives focus back to what had it before.
    pub fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.open {
            return;
        }
        // Before moving focus, so the focus-out listener finds it closed.
        self.hide(cx);
        if let Some(previous) = self.previous_focus.take() {
            previous.focus(window, cx);
        }
    }

    fn hide(&mut self, cx: &mut Context<Self>) {
        if self.open {
            self.open = false;
            cx.notify();
        }
    }

    /// A row was chosen. Its action, if it has one, has been dispatched
    /// already; if that moved focus, the palette is closed already too.
    fn confirmed(&mut self, command: PaletteCommand, window: &mut Window, cx: &mut Context<Self>) {
        self.close(window, cx);
        if !matches!(command, PaletteCommand::Shortcut(_)) {
            cx.emit(CommandPaletteEvent::Run(command));
        }
    }

    /// The groups as `Command` entries. Rebuilt on every render, as
    /// gpui-kit expects; `CommandState` keeps what must last between frames.
    fn command(&self) -> Command {
        self.groups
            .iter()
            .fold(Command::new(&self.state), |command, group| {
                command.group(
                    CommandGroup::new()
                        .label(group.heading)
                        .items(group.items.iter().map(command_item)),
                )
            })
    }
}

impl Render for CommandPalette {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.open {
            return div().into_any_element();
        }
        let groups = self.groups.clone();
        let on_confirm = cx.weak_entity();
        let on_cancel = cx.weak_entity();
        let on_click_out = cx.weak_entity();
        let command = self
            .command()
            .bordered(false)
            .max_h(px(380.))
            .placeholder("Search notes, children, labels and commands…")
            .on_confirm(move |index, window, cx| {
                let Some(item) = groups
                    .get(index.section)
                    .and_then(|group| group.items.get(index.row))
                else {
                    return;
                };
                let command = item.command;
                on_confirm
                    .update(cx, |this, cx| this.confirmed(command, window, cx))
                    .ok();
            })
            // Esc with an empty search; the first Esc clears the search.
            .on_cancel(move |window, cx| {
                on_cancel.update(cx, |this, cx| this.close(window, cx)).ok();
            });

        let overlay = cx.theme().overlay;
        let animation = Animation::new(APPEAR).with_easing(ease_out_quint());
        let panel = v_flex()
            .w(px(560.))
            .max_w_full()
            .p_1()
            .bg(cx.theme().popover)
            .text_color(cx.theme().popover_foreground)
            .border_1()
            .border_color(cx.theme().border)
            .rounded(cx.theme().radius_lg)
            .shadow_lg()
            .key_context(COMMAND_PALETTE_CONTEXT)
            .track_focus(&self.focus_handle)
            .on_mouse_down_out(move |_, window, cx| {
                on_click_out
                    .update(cx, |this, cx| this.close(window, cx))
                    .ok();
            })
            .child(command)
            // Drops in from a little above as it fades in.
            .with_animation(
                ("command-palette", self.openings),
                animation.clone(),
                |panel, t| panel.mt(px(-12. * (1. - t))).opacity(t),
            );

        div()
            .absolute()
            .inset_0()
            // Swallows clicks and scrolls, so nothing underneath reacts.
            .occlude()
            .flex()
            .flex_col()
            .items_center()
            .pt(px(72.))
            .px_4()
            .child(panel)
            .with_animation(
                ("command-palette-overlay", self.openings),
                animation,
                move |this, t| this.bg(overlay.opacity(t)),
            )
            .into_any_element()
    }
}

/// One row: its label, icon and search words. A row with a shortcut
/// carries the shortcut's action, which `Command` dispatches when it's
/// chosen and looks up the key binding of, to show it on the right.
fn command_item(item: &PaletteItem) -> CommandItem {
    let mut row = CommandItem::new()
        .label(item.label.clone())
        .keywords(item.keywords.iter().cloned())
        .icon(icon(item.command))
        .checked(item.checked)
        .disabled(!item.enabled);
    if let PaletteCommand::Shortcut(shortcut) = item.command {
        row = row.action(action(shortcut));
    }
    row
}

/// The action a shortcut dispatches.
fn action(shortcut: Shortcut) -> Box<dyn Action> {
    match shortcut {
        Shortcut::NewNote => Box::new(app::NewNote),
        Shortcut::FocusSearch => Box::new(app::FocusSearch),
        Shortcut::TogglePreview => Box::new(app::TogglePreview),
        Shortcut::AddImages => Box::new(app::AddImages),
        Shortcut::FocusSidebar => Box::new(app::FocusSidebar),
        Shortcut::FocusNoteList => Box::new(app::FocusNoteList),
        Shortcut::FocusEditor => Box::new(app::FocusEditor),
        Shortcut::ExportNote => Box::new(app::ExportNote),
        Shortcut::ExportNotes => Box::new(app::ExportNotes),
        Shortcut::LockVault => Box::new(app::LockVault),
        Shortcut::ToggleSidebar => Box::new(app::ToggleSidebar),
        Shortcut::ToggleTheme => Box::new(app::ToggleTheme),
        Shortcut::OpenSettings => Box::new(app::OpenSettings),
        Shortcut::Quit => Box::new(app::Quit),
    }
}

fn icon(command: PaletteCommand) -> Icon {
    use gpui_kit::assets::IconName as Lucide;

    match command {
        PaletteCommand::Shortcut(shortcut) => match shortcut {
            Shortcut::NewNote => IconName::Plus.into(),
            Shortcut::FocusSearch => IconName::Search.into(),
            Shortcut::TogglePreview => IconName::Eye.into(),
            Shortcut::AddImages => Lucide::ImagePlus.into(),
            Shortcut::FocusSidebar => IconName::PanelLeft.into(),
            Shortcut::FocusNoteList => Lucide::List.into(),
            Shortcut::FocusEditor => IconName::FileText.into(),
            Shortcut::ExportNote => Lucide::FileDown.into(),
            Shortcut::ExportNotes => Lucide::BookDown.into(),
            Shortcut::LockVault => Lucide::Lock.into(),
            Shortcut::ToggleSidebar => IconName::PanelLeftClose.into(),
            Shortcut::ToggleTheme => IconName::Moon.into(),
            Shortcut::OpenSettings => IconName::Settings.into(),
            Shortcut::Quit => Lucide::LogOut.into(),
        },
        PaletteCommand::AddChild => Lucide::UserPlus.into(),
        PaletteCommand::AddLabel => Lucide::Hash.into(),
        PaletteCommand::DeleteNote => Lucide::Trash.into(),
        PaletteCommand::Show(NoteFilter::All) => IconName::BookOpen.into(),
        PaletteCommand::Show(NoteFilter::Child(_)) | PaletteCommand::ToggleChild(_) => {
            IconName::CircleUser.into()
        }
        PaletteCommand::Show(NoteFilter::Label(_)) | PaletteCommand::ToggleLabel(_) => {
            Lucide::Hash.into()
        }
        PaletteCommand::Open(_) => IconName::FileText.into(),
        PaletteCommand::ToggleVimMode => Lucide::Keyboard.into(),
        PaletteCommand::ChangePassword | PaletteCommand::RemovePassword => Lucide::KeyRound.into(),
        PaletteCommand::ExportBackup => Lucide::DatabaseBackup.into(),
        PaletteCommand::RestoreBackup => Lucide::ArchiveRestore.into(),
        PaletteCommand::ToggleShared(_) => Lucide::Share2.into(),
        PaletteCommand::ShareChild(_) => Lucide::Cloud.into(),
        PaletteCommand::SignIn => Lucide::LogIn.into(),
        PaletteCommand::SignOut => Lucide::LogOut.into(),
        PaletteCommand::SyncNow => Lucide::RefreshCw.into(),
    }
}

/// UI tests: the palette in a headless window, driven by key presses.
#[cfg(test)]
mod tests {
    use gpui_kit::test::TestWindowExt as _;
    use gpui_kit::{
        AnyWindowHandle, Bounds, TestAppContext, WindowBounds, WindowOptions, point, size,
    };
    use jiff::Timestamp;
    use papa_core::{NoteId, NoteSummary};

    use super::*;
    use crate::palette::{PaletteData, groups};

    /// Stands in for `PapaApp`: hosts the palette, handles `NewNote`, and
    /// records what the palette asks for.
    struct Host {
        palette: Entity<CommandPalette>,
        focus_handle: FocusHandle,
        new_notes: usize,
        runs: Vec<PaletteCommand>,
        _subscription: Subscription,
    }

    impl Render for Host {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            div()
                .size_full()
                .track_focus(&self.focus_handle)
                .on_action(cx.listener(|this, _: &app::NewNote, _, _| this.new_notes += 1))
                .child(self.palette.clone())
        }
    }

    fn note(title: &str) -> NoteSummary {
        NoteSummary {
            id: NoteId::new(),
            title: title.to_owned(),
            preview: String::new(),
            children: Vec::new(),
            labels: Vec::new(),
            images: 0,
            created_at: Timestamp::now(),
        }
    }

    /// Opens a window with the host focused, and the palette closed.
    fn host(cx: &mut TestAppContext) -> (AnyWindowHandle, Entity<Host>) {
        cx.update(gpui_kit::init);
        let (window, host) = cx.update(|cx| {
            let options = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds {
                    origin: point(px(0.), px(0.)),
                    size: size(px(900.), px(700.)),
                })),
                ..Default::default()
            };
            gpui_kit::open_window(options, cx, |window, cx| {
                cx.new(|cx| {
                    let palette = cx.new(|cx| CommandPalette::new(window, cx));
                    let subscription = cx.subscribe(&palette, |this: &mut Host, _, event, _| {
                        let CommandPaletteEvent::Run(command) = *event;
                        this.runs.push(command);
                    });
                    let focus_handle = cx.focus_handle();
                    focus_handle.focus(window, cx);
                    Host {
                        palette,
                        focus_handle,
                        new_notes: 0,
                        runs: Vec::new(),
                        _subscription: subscription,
                    }
                })
            })
            .expect("open the test window")
        });
        (window, host)
    }

    fn open(
        window: AnyWindowHandle,
        host: &Entity<Host>,
        notes: &[NoteSummary],
        cx: &mut TestAppContext,
    ) {
        let host = host.clone();
        cx.update_window(window, |_, window, cx| {
            let groups = groups(&PaletteData {
                vault_open: true,
                children: &[],
                labels: &[],
                notes,
                open_note: None,
                filter: NoteFilter::All,
                vim_mode: false,
                encrypted: false,
                list_export: None,
                signed_in: false,
                shared_children: &[],
            });
            let palette = host.read(cx).palette.clone();
            palette.update(cx, |palette, cx| palette.open(groups, false, window, cx));
            window.render_frame(cx);
        })
        .unwrap();
    }

    fn is_open(host: &Entity<Host>, cx: &mut TestAppContext) -> bool {
        cx.update(|cx| host.read(cx).palette.read(cx).is_open())
    }

    fn host_focused(window: AnyWindowHandle, host: &Entity<Host>, cx: &mut TestAppContext) -> bool {
        cx.update_window(window, |_, window, cx| {
            host.read(cx).focus_handle.is_focused(window)
        })
        .unwrap()
    }

    #[gpui_kit::test]
    fn choosing_a_note_closes_the_palette_and_asks_to_open_it(cx: &mut TestAppContext) {
        let (window, host) = host(cx);
        let notes = [note("First bike ride"), note("School play")];
        open(window, &host, &notes, cx);
        assert!(is_open(&host, cx));

        // The list filters once the search box reports the change.
        cx.update_window(window, |_, window, cx| window.input("school", cx))
            .unwrap();
        cx.run_until_parked();
        cx.update_window(window, |_, window, cx| window.press("enter", cx))
            .unwrap();
        cx.run_until_parked();

        assert!(!is_open(&host, cx));
        assert!(host_focused(window, &host, cx), "focus goes back");
        let runs = cx.update(|cx| host.read(cx).runs.clone());
        assert_eq!(runs, [PaletteCommand::Open(notes[1].id)]);
    }

    #[gpui_kit::test]
    fn a_command_with_a_shortcut_dispatches_its_action(cx: &mut TestAppContext) {
        let (window, host) = host(cx);
        open(window, &host, &[], cx);

        // "New note" is the first row, highlighted on opening.
        cx.update_window(window, |_, window, cx| window.press("enter", cx))
            .unwrap();
        cx.run_until_parked();

        assert!(!is_open(&host, cx));
        cx.update(|cx| {
            assert_eq!(host.read(cx).new_notes, 1, "the host's handler ran");
            assert!(host.read(cx).runs.is_empty(), "and nothing else");
        });
    }

    #[gpui_kit::test]
    fn escape_clears_the_search_then_closes(cx: &mut TestAppContext) {
        let (window, host) = host(cx);
        open(window, &host, &[note("Beach day")], cx);

        cx.update_window(window, |_, window, cx| {
            window.input("beach", cx);
            window.press("escape", cx);
        })
        .unwrap();
        cx.run_until_parked();
        assert!(is_open(&host, cx), "the first Esc only clears the search");

        cx.update_window(window, |_, window, cx| window.press("escape", cx))
            .unwrap();
        cx.run_until_parked();
        assert!(!is_open(&host, cx));
        assert!(host_focused(window, &host, cx), "focus goes back");
        assert!(cx.update(|cx| host.read(cx).runs.is_empty()));
    }
}
