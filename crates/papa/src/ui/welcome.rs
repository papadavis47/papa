//! The first thing a new install shows: start a vault of your own, or open
//! the notes Dad shared with you. Choosing the second asks for Dad's email
//! and the key he handed over, and signs in.

use std::path::PathBuf;

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Sizable as _,
    button::{Button, ButtonVariants as _},
    input::{Input, InputEvent, InputState},
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AppContext as _, Context, Entity, EventEmitter, FocusHandle, InteractiveElement as _,
    IntoElement, ParentElement as _, Render, SharedString, Styled as _, Subscription, Window, div,
    px,
};
use papa_core::share::KeyCard;
use papa_core::{ChildSession, Error, ShareContents};
use papa_proto::ErrorCode;

use super::empty_state::EmptyState;
use super::{AppTitleBar, sentence};

pub enum WelcomeEvent {
    /// "Start my own vault": father mode, with a new vault.
    StartVault,
    /// A child signed in: child mode, with what they can read.
    SignedIn(ChildSession, ShareContents),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Step {
    Choose,
    SignIn,
}

pub struct WelcomeScreen {
    step: Step,
    /// Where the child's files go: `child/` next to where the vault would
    /// be. `None` if the system has no data folder, which the sign-in says.
    child_dir: Option<PathBuf>,
    email: Entity<InputState>,
    key: Entity<InputState>,
    /// Started on the first sign-in, and kept for the next try.
    session: Option<ChildSession>,
    error: Option<SharedString>,
    /// Signing in takes a moment: the key is stretched on purpose, then
    /// the share is downloaded.
    busy: bool,
    focus_handle: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<WelcomeEvent> for WelcomeScreen {}

impl WelcomeScreen {
    pub fn new(child_dir: Option<PathBuf>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let email = cx.new(|cx| InputState::new(window, cx).placeholder("Dad's email"));
        let key = cx.new(|cx| InputState::new(window, cx).placeholder("PAPA-XXXX-XXXX-…"));
        let submit_on_enter = |this: &mut Self,
                               _: &Entity<InputState>,
                               event: &InputEvent,
                               window: &mut Window,
                               cx: &mut Context<Self>| {
            if let InputEvent::PressEnter { .. } = event {
                this.submit(window, cx);
            }
        };
        let subscriptions = vec![
            cx.subscribe_in(&email, window, submit_on_enter),
            cx.subscribe_in(&key, window, submit_on_enter),
        ];
        Self {
            step: Step::Choose,
            child_dir,
            email,
            key,
            session: None,
            error: None,
            busy: false,
            focus_handle: cx.focus_handle(),
            _subscriptions: subscriptions,
        }
    }

    /// Goes straight to signing in, with `email` filled in and `message`
    /// above the form: after signing out because a key stopped working,
    /// say.
    pub fn sign_in_again(
        &mut self,
        email: &str,
        message: Option<SharedString>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.email.update(cx, |input, cx| {
            input.set_value(email.to_owned(), window, cx)
        });
        self.error = message;
        self.show_sign_in(window, cx);
    }

    pub fn focus(&self, window: &mut Window, cx: &mut Context<Self>) {
        match self.step {
            Step::Choose => self.focus_handle.focus(window, cx),
            Step::SignIn => {
                let empty = |input: &Entity<InputState>| input.read(cx).value().trim().is_empty();
                let field = if empty(&self.email) {
                    &self.email
                } else {
                    &self.key
                };
                field.update(cx, |input, cx| input.focus(window, cx));
            }
        }
    }

    fn show_sign_in(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.step = Step::SignIn;
        self.focus(window, cx);
        cx.notify();
    }

    fn back(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        self.step = Step::Choose;
        self.error = None;
        self.focus(window, cx);
        cx.notify();
    }

    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        let email = self.email.read(cx).value().trim().to_owned();
        let key = self.key.read(cx).value().to_string();
        let checked = match (email.is_empty(), key.trim().is_empty()) {
            (true, _) => Err("Type Dad's email.".to_owned()),
            (_, true) => Err("Paste the key Dad gave you.".to_owned()),
            // The key's checksum catches a typo here, before the network.
            _ => key.parse::<KeyCard>().map_err(|err| message(&err)),
        };
        let card = match checked {
            Ok(card) => card,
            Err(error) => {
                self.error = Some(error.into());
                cx.notify();
                return;
            }
        };
        let session = match self.session() {
            Ok(session) => session,
            Err(err) => {
                self.error = Some(message(&err).into());
                cx.notify();
                return;
            }
        };
        self.busy = true;
        self.error = None;
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            let result = session.sign_in(email, card).await;
            this.update_in(cx, |this, window, cx| {
                this.busy = false;
                match result {
                    Ok(notes) => cx.emit(WelcomeEvent::SignedIn(session, notes)),
                    Err(err) => {
                        this.error = Some(message(&err).into());
                        this.focus(window, cx);
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn session(&mut self) -> papa_core::Result<ChildSession> {
        if let Some(session) = &self.session {
            return Ok(session.clone());
        }
        let dir = self.child_dir.clone().ok_or(Error::NoDataDir)?;
        Ok(self.session.insert(ChildSession::start(dir)?).clone())
    }

    fn render_choice(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        let choice = |button: Button, text: &'static str| {
            v_flex()
                .w(px(220.))
                .gap_2()
                .child(button.w_full())
                .child(div().text_sm().text_color(muted).child(text))
        };
        EmptyState::new("welcome", Lucide::BookHeart, "Welcome to Papa")
            .description(
                "A private journal for fathers, and the notes they share with their children.",
            )
            .child(
                gpui_kit::component::h_flex()
                    .items_start()
                    .gap_6()
                    .child(choice(
                        Button::new("start-vault")
                            .primary()
                            .label("Start my own vault")
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(WelcomeEvent::StartVault))),
                        "Write about your children. Your notes stay on this computer.",
                    ))
                    .child(choice(
                        Button::new("open-shared")
                            .outline()
                            .label("Open notes shared with me")
                            .on_click(
                                cx.listener(|this, _, window, cx| this.show_sign_in(window, cx)),
                            ),
                        "Read what Dad shared with you, with his email and your key.",
                    )),
            )
    }

    fn render_sign_in(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let form = v_flex()
            .w(px(320.))
            .gap_2()
            .child(Input::new(&self.email).disabled(self.busy))
            .child(Input::new(&self.key).disabled(self.busy))
            .child(
                Button::new("sign-in")
                    .primary()
                    .label(if self.busy {
                        "Opening…"
                    } else {
                        "Open the notes"
                    })
                    .loading(self.busy)
                    .on_click(cx.listener(|this, _, window, cx| this.submit(window, cx))),
            )
            .when_some(self.error.clone(), |form, error| {
                form.child(div().text_sm().text_color(cx.theme().danger).child(error))
            })
            .child(
                Button::new("back")
                    .ghost()
                    .small()
                    .label("Back")
                    .disabled(self.busy)
                    .on_click(cx.listener(|this, _, window, cx| this.back(window, cx))),
            );
        EmptyState::new(
            "child-sign-in",
            Lucide::KeyRound,
            "Open notes shared with you",
        )
        .description(
            "Type Dad's email and paste the key he gave you. Your notes are \
                 downloaded and decrypted on this computer.",
        )
        .child(form)
    }
}

/// What to tell a child whose sign-in didn't work.
fn message(err: &Error) -> String {
    match err {
        Error::InvalidAccessKey => {
            "That key has a typo, or isn't all there. Check it against the one Dad sent.".to_owned()
        }
        Error::Relay {
            code: ErrorCode::InvalidCredentials,
            ..
        } => {
            "That email and key don't go together. Check both, or ask Dad for a new key.".to_owned()
        }
        Error::Network(reason) => format!("Couldn't reach the Papa server ({reason})."),
        err => sentence(err),
    }
}

impl Render for WelcomeScreen {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let content = match self.step {
            Step::Choose => self.render_choice(cx).into_any_element(),
            Step::SignIn => self.render_sign_in(cx).into_any_element(),
        };
        v_flex()
            .size_full()
            .track_focus(&self.focus_handle)
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .child(AppTitleBar::welcome())
            .child(div().flex_1().min_h_0().child(content))
    }
}
