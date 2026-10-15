//! The main window's root: which of Papa's two modes it shows.
//!
//! One binary, two apps. A father keeps a vault and writes in it
//! ([`PapaApp`]); a child reads what he shared ([`ChildApp`]). Each mode
//! is its own type with its own state, and the window holds exactly one
//! of them in an enum, [`Mode`]. Nothing of the father's (the vault, the
//! editor, labels) exists while a child's notes are open, and the
//! compiler, not a flag checked at run time, is what keeps the two apart.
//!
//! Which one opens ([`launch`]): a vault on this computer means father
//! mode, as before; a child's saved sign-in means child mode; neither,
//! and the window asks ([`WelcomeScreen`]).

use std::path::{Path, PathBuf};

use gpui_kit::component::{ActiveTheme as _, WindowExt as _, notification::Notification, v_flex};
use gpui_kit::{
    AppContext as _, Context, Entity, FocusHandle, Global, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, Styled as _, Subscription, Window, div,
};
use papa_core::{ChildSession, SharedNotes, paths};

use crate::app::PapaApp;
use crate::child_app::{ChildApp, ChildAppEvent};
use crate::settings::AppSettings;
use crate::theme;
use crate::ui::{AppTitleBar, WelcomeEvent, WelcomeScreen, sentence};

/// Which mode the window is in, for the Settings window: it shows the
/// pages that apply. A global, like [`VaultStatus`](crate::app::VaultStatus).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum ModeStatus {
    /// Starting, or asking which mode.
    #[default]
    Welcome,
    Father,
    /// Signed in to what Dad shared, by him at this email.
    Child {
        father_email: String,
    },
}

impl Global for ModeStatus {}

/// Which mode to open at launch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Launch {
    Father,
    Child,
    Welcome,
}

/// Picks the mode: a father's vault wins, then a child's sign-in.
///
/// The vault comes first so that nothing could ever hide it: if both are
/// somehow there, the father's notes open, and the child's copy, which
/// can be downloaded again, waits.
pub fn launch(vault_exists: bool, child_signed_in: bool) -> Launch {
    match (vault_exists, child_signed_in) {
        (true, _) => Launch::Father,
        (false, true) => Launch::Child,
        (false, false) => Launch::Welcome,
    }
}

/// What the window shows. Exactly one mode's state exists at a time.
enum Mode {
    /// Reading the child's saved sign-in: a moment, offline too.
    Opening,
    Welcome(Entity<WelcomeScreen>),
    Father(Entity<PapaApp>),
    Child(Entity<ChildApp>),
}

pub struct PapaWindow {
    mode: Mode,
    /// Where the vault is, or would be. `None` without a data folder: then
    /// father mode opens, and says so.
    vault: Option<PathBuf>,
    /// Focused when nothing else is, so the window's shortcuts keep working.
    focus_handle: FocusHandle,
    /// The current mode's events. Replaced with the mode.
    mode_subscription: Option<Subscription>,
    _subscriptions: Vec<Subscription>,
}

impl PapaWindow {
    /// `settings_problem` is why the settings file couldn't be read, if it
    /// couldn't; it's shown once the window is up.
    pub fn new(
        settings_problem: Option<anyhow::Error>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let subscriptions = vec![
            // "Match system" follows the OS switching between light and dark
            // while the app runs, in every mode.
            cx.observe_window_appearance(window, |_, window, cx| {
                theme::set_system_appearance(window, cx)
            }),
            cx.observe_global::<AppSettings>(|_, cx| cx.notify()),
            // A closing dialog hands focus back to whatever had it, which
            // may be gone. With nothing focused, no shortcut works.
            cx.on_focus_lost(window, |this, window, cx| this.focus(window, cx)),
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

        let vault = paths::vault_path().ok();
        let child_dir = vault.as_deref().map(paths::child_dir);
        let mut this = Self {
            mode: Mode::Opening,
            vault,
            focus_handle: cx.focus_handle(),
            mode_subscription: None,
            _subscriptions: subscriptions,
        };
        let exists = |path: Option<&Path>| path.is_some_and(Path::exists);
        let signed_in = child_dir.as_deref().is_some_and(ChildSession::is_signed_in);
        match (
            this.vault.is_none(),
            launch(exists(this.vault.as_deref()), signed_in),
        ) {
            // No data folder at all: father mode says what's wrong.
            (true, _) | (_, Launch::Father) => this.open_father(window, cx),
            (_, Launch::Child) => this.open_child(window, cx),
            (_, Launch::Welcome) => {
                this.show_welcome(window, cx);
            }
        }
        this
    }

    fn child_dir(&self) -> Option<PathBuf> {
        self.vault.as_deref().map(paths::child_dir)
    }

    /// Switches modes. What the last one had to say goes with it: a "your
    /// key no longer works" mustn't linger over the sign-in that fixes it.
    fn set_mode(
        &mut self,
        mode: Mode,
        status: ModeStatus,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Not while starting: the window's `Root`, which holds the
        // notifications, doesn't exist until `new` has returned.
        if !matches!(self.mode, Mode::Opening) {
            window.clear_notifications(cx);
        }
        self.mode = mode;
        cx.set_global(status);
        cx.notify();
    }

    fn open_father(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let app = cx.new(|cx| PapaApp::new(window, cx));
        self.mode_subscription = None;
        self.set_mode(Mode::Father(app), ModeStatus::Father, window, cx);
    }

    fn show_welcome(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<WelcomeScreen> {
        let child_dir = self.child_dir();
        let screen = cx.new(|cx| WelcomeScreen::new(child_dir, window, cx));
        self.mode_subscription = Some(cx.subscribe_in(&screen, window, Self::on_welcome_event));
        screen.update(cx, |screen, cx| screen.focus(window, cx));
        self.set_mode(
            Mode::Welcome(screen.clone()),
            ModeStatus::Welcome,
            window,
            cx,
        );
        screen
    }

    /// Child mode, from the saved sign-in: reads the replica, which works
    /// offline, then shows the notes.
    fn open_child(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(dir) = self.child_dir() else {
            self.show_welcome(window, cx);
            return;
        };
        self.set_mode(Mode::Opening, ModeStatus::Welcome, window, cx);
        let session = match ChildSession::start(dir) {
            Ok(session) => session,
            Err(err) => return self.child_failed(err, window, cx),
        };
        cx.spawn_in(window, async move |this, cx| {
            let opened = session.open().await;
            this.update_in(cx, |this, window, cx| match opened {
                Ok(Some(contents)) => this.show_child(session, contents, window, cx),
                // The sign-in went away meanwhile.
                Ok(None) => {
                    this.show_welcome(window, cx);
                }
                Err(err) => this.child_failed(err, window, cx),
            })
            .ok();
        })
        .detach();
    }

    /// The saved notes couldn't be opened: say why, and offer to sign in,
    /// which downloads them anew.
    fn child_failed(&mut self, err: papa_core::Error, window: &mut Window, cx: &mut Context<Self>) {
        let screen = self.show_welcome(window, cx);
        screen.update(cx, |screen, cx| screen.sign_in_again("", None, window, cx));
        window.push_notification(
            Notification::error(sentence(err))
                .title("Papa couldn't open the notes shared with you")
                .autohide(false),
            cx,
        );
    }

    fn show_child(
        &mut self,
        session: ChildSession,
        contents: SharedNotes,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let status = ModeStatus::Child {
            father_email: contents.father_email.clone(),
        };
        let app = cx.new(|cx| ChildApp::new(session, contents, window, cx));
        self.mode_subscription = Some(cx.subscribe_in(&app, window, Self::on_child_event));
        self.set_mode(Mode::Child(app), status, window, cx);
    }

    fn on_welcome_event(
        &mut self,
        _: &Entity<WelcomeScreen>,
        event: &WelcomeEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            WelcomeEvent::StartVault => self.open_father(window, cx),
            WelcomeEvent::SignedIn(session, contents) => {
                self.show_child(session.clone(), contents.clone(), window, cx)
            }
        }
    }

    fn on_child_event(
        &mut self,
        _: &Entity<ChildApp>,
        event: &ChildAppEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            ChildAppEvent::SignedOut { email, reason } => {
                let screen = self.show_welcome(window, cx);
                if let Some(reason) = reason {
                    screen.update(cx, |screen, cx| {
                        screen.sign_in_again(email, Some(reason.clone()), window, cx)
                    });
                }
            }
        }
    }

    fn focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match &self.mode {
            Mode::Welcome(screen) => screen.update(cx, |screen, cx| screen.focus(window, cx)),
            Mode::Opening => self.focus_handle.focus(window, cx),
            // Each app has a fallback of its own, inside its key context.
            Mode::Father(_) | Mode::Child(_) => {}
        }
    }
}

impl Render for PapaWindow {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let root = div().size_full().track_focus(&self.focus_handle);
        match &self.mode {
            Mode::Father(app) => root.child(app.clone()),
            Mode::Child(app) => root.child(app.clone()),
            Mode::Welcome(screen) => root.child(screen.clone()),
            Mode::Opening => root.child(
                v_flex()
                    .size_full()
                    .bg(cx.theme().background)
                    .text_color(cx.theme().foreground)
                    .child(AppTitleBar::welcome())
                    .child(div().flex_1()),
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_vault_opens_father_mode_whatever_else_is_there() {
        assert_eq!(launch(true, false), Launch::Father);
        assert_eq!(launch(true, true), Launch::Father);
    }

    #[test]
    fn a_childs_sign_in_opens_child_mode() {
        assert_eq!(launch(false, true), Launch::Child);
    }

    #[test]
    fn with_neither_the_window_asks() {
        assert_eq!(launch(false, false), Launch::Welcome);
    }
}
