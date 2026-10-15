//! The body of the account dialogs: signing in to a Papa server, or
//! creating an account on one with an invite code.

use gpui_kit::component::{
    ActiveTheme as _,
    input::{Input, InputState},
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    App, AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render, SharedString,
    Styled as _, Window, div,
};
use papa_proto::Secret;

/// Signing in, or creating an account first.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AccountPurpose {
    SignIn,
    SignUp,
}

impl AccountPurpose {
    pub fn title(self) -> &'static str {
        match self {
            Self::SignIn => "Sign in to Papa",
            Self::SignUp => "Create a Papa account",
        }
    }

    pub fn ok_label(self) -> &'static str {
        match self {
            Self::SignIn => "Sign in",
            Self::SignUp => "Create account",
        }
    }

    fn busy(self) -> &'static str {
        match self {
            Self::SignIn => "Signing in…",
            Self::SignUp => "Creating your account…",
        }
    }
}

/// What the father typed, checked as far as the dialog can.
pub struct AccountEntry {
    pub relay_url: String,
    /// Only when signing up.
    pub invite_code: Option<Secret>,
    pub email: String,
    pub password: Secret,
}

pub struct AccountForm {
    purpose: AccountPurpose,
    server: Entity<InputState>,
    invite: Option<Entity<InputState>>,
    email: Entity<InputState>,
    password: Entity<InputState>,
    error: Option<SharedString>,
    busy: bool,
}

impl AccountForm {
    /// `server` and `email` prefill their fields: the last ones used.
    pub fn new(
        purpose: AccountPurpose,
        server: String,
        email: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let field =
            |placeholder: &'static str, value: String, window: &mut Window, cx: &mut App| {
                cx.new(|cx| {
                    InputState::new(window, cx)
                        .placeholder(placeholder)
                        .default_value(value)
                })
            };
        Self {
            purpose,
            server: field("Server address", server, window, cx),
            invite: (purpose == AccountPurpose::SignUp)
                .then(|| field("Invite code", String::new(), window, cx)),
            email: field("Email", email, window, cx),
            password: cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("Password")
                    .masked(true)
            }),
            error: None,
            busy: false,
        }
    }

    /// The field to type in first: the first empty one.
    pub fn first_input(&self, cx: &App) -> Entity<InputState> {
        [Some(&self.server), self.invite.as_ref(), Some(&self.email)]
            .into_iter()
            .flatten()
            .find(|input| input.read(cx).value().trim().is_empty())
            .unwrap_or(&self.password)
            .clone()
    }

    pub fn is_busy(&self) -> bool {
        self.busy
    }

    /// Reads and checks the fields. `Err` is the message to show.
    pub fn entry(&self, cx: &App) -> Result<AccountEntry, String> {
        let value = |input: &Entity<InputState>| input.read(cx).value().trim().to_owned();
        let relay_url = value(&self.server);
        if relay_url.is_empty() {
            return Err("Type the server's address.".into());
        }
        let invite_code = match &self.invite {
            Some(invite) => {
                let code = value(invite);
                if code.is_empty() {
                    return Err("Type your invite code.".into());
                }
                Some(Secret::new(code))
            }
            None => None,
        };
        let email = value(&self.email);
        if !email.contains('@') {
            return Err("Type your email address.".into());
        }
        // Not trimmed: spaces can be part of a password.
        let password = self.password.read(cx).value().to_string();
        if password.is_empty() {
            return Err("Type your password.".into());
        }
        Ok(AccountEntry {
            relay_url,
            invite_code,
            email,
            password: Secret::new(password),
        })
    }

    pub fn set_error(&mut self, message: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.error = Some(message.into());
        self.busy = false;
        cx.notify();
    }

    pub fn set_busy(&mut self, cx: &mut Context<Self>) {
        self.error = None;
        self.busy = true;
        cx.notify();
    }
}

impl Render for AccountForm {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let note = match self.purpose {
            AccountPurpose::SignIn => {
                "Your account lets Papa share the notes you choose with your children. \
                 The server only ever sees them encrypted."
            }
            AccountPurpose::SignUp => {
                "Papa accounts are by invitation. The server only ever sees your notes \
                 encrypted, and only those you share."
            }
        };
        let hint = |text: &'static str| {
            div()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child(text)
        };
        v_flex()
            .gap_3()
            .child(div().text_sm().child(note))
            .child(Input::new(&self.server).disabled(self.busy))
            .children(
                self.invite
                    .as_ref()
                    .map(|invite| Input::new(invite).disabled(self.busy)),
            )
            .child(Input::new(&self.email).disabled(self.busy))
            .child(
                Input::new(&self.password)
                    .mask_toggle()
                    .disabled(self.busy),
            )
            .when(self.purpose == AccountPurpose::SignUp, |this| {
                this.child(hint(
                    "Your children sign in with this email and the key Papa makes for each of them.",
                ))
            })
            .when(self.busy, |this| {
                this.child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child(self.purpose.busy()),
                )
            })
            .when_some(self.error.clone(), |this, error| {
                this.child(div().text_sm().text_color(cx.theme().danger).child(error))
            })
    }
}
