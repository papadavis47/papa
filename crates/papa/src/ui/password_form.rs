//! The body of the password dialogs: setting, changing and removing the
//! vault's password, and typing a backup's password to restore it.

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
use papa_core::lock::{MIN_PASSWORD_CHARS, Password};

use super::sentence;

/// What the dialog is for. It decides which fields show.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PasswordPurpose {
    /// The vault has no password yet: a new one, twice.
    Set,
    /// The current one, then a new one, twice.
    Change,
    /// The current one.
    Remove,
    /// The password a backup was made with.
    Restore,
}

impl PasswordPurpose {
    pub fn title(self) -> &'static str {
        match self {
            Self::Set => "Set a password",
            Self::Change => "Change the password",
            Self::Remove => "Remove the password",
            Self::Restore => "Restore an encrypted backup",
        }
    }

    pub fn ok_label(self) -> &'static str {
        match self {
            Self::Set => "Encrypt vault",
            Self::Change => "Change password",
            Self::Remove => "Decrypt vault",
            Self::Restore => "Replace vault",
        }
    }

    /// What the dialog says while the vault does the work.
    fn busy(self) -> &'static str {
        match self {
            Self::Set => "Encrypting your vault…",
            Self::Change => "Changing the password…",
            Self::Remove => "Decrypting your vault…",
            Self::Restore => "Checking the backup…",
        }
    }

    fn asks_current(self) -> bool {
        matches!(self, Self::Change | Self::Remove | Self::Restore)
    }

    fn asks_new(self) -> bool {
        matches!(self, Self::Set | Self::Change)
    }
}

/// The passwords typed in the dialog, checked as far as the dialog can.
pub struct PasswordEntry {
    pub current: Option<Password>,
    pub new: Option<Password>,
}

/// An entity, like `NameForm`: it holds what's typed while the dialog is
/// open, and a message, and whether the vault is busy with it.
pub struct PasswordForm {
    purpose: PasswordPurpose,
    current: Option<Entity<InputState>>,
    new: Option<Entity<InputState>>,
    confirm: Option<Entity<InputState>>,
    note: Option<SharedString>,
    error: Option<SharedString>,
    busy: bool,
}

/// A text field that draws dots instead of what's typed.
fn masked(placeholder: &'static str, window: &mut Window, cx: &mut App) -> Entity<InputState> {
    cx.new(|cx| {
        InputState::new(window, cx)
            .placeholder(placeholder)
            .masked(true)
    })
}

impl PasswordForm {
    /// `note` is shown above the fields: a warning, or what the backup is.
    pub fn new(
        purpose: PasswordPurpose,
        note: Option<SharedString>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let current_placeholder = match purpose {
            PasswordPurpose::Restore => "The backup's password",
            _ => "Current password",
        };
        Self {
            purpose,
            current: purpose
                .asks_current()
                .then(|| masked(current_placeholder, window, cx)),
            new: purpose
                .asks_new()
                .then(|| masked("New password", window, cx)),
            confirm: purpose
                .asks_new()
                .then(|| masked("New password again", window, cx)),
            note,
            error: None,
            busy: false,
        }
    }

    /// The field to type in first.
    pub fn first_input(&self) -> Entity<InputState> {
        self.current
            .clone()
            .or_else(|| self.new.clone())
            .expect("every purpose asks for a password")
    }

    pub fn is_busy(&self) -> bool {
        self.busy
    }

    /// Reads and checks the fields: the new password long enough, and typed
    /// the same twice. `Err` is the message to show.
    pub fn entry(&self, cx: &App) -> Result<PasswordEntry, String> {
        let current = self.current.as_ref().map(|input| read(input, cx));
        if current.as_ref().is_some_and(Password::is_empty) {
            return Err("Type the password.".into());
        }
        let new = match (&self.new, &self.confirm) {
            (Some(new), Some(confirm)) => {
                let new = read(new, cx);
                new.check_new().map_err(sentence)?;
                if new != read(confirm, cx) {
                    return Err("The two new passwords aren't the same.".into());
                }
                Some(new)
            }
            _ => None,
        };
        Ok(PasswordEntry { current, new })
    }

    /// Shows `message` and lets the father try again.
    pub fn set_error(&mut self, message: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.error = Some(message.into());
        self.busy = false;
        cx.notify();
    }

    /// While busy, the dialog can't be closed or sent again.
    pub fn set_busy(&mut self, cx: &mut Context<Self>) {
        self.error = None;
        self.busy = true;
        cx.notify();
    }
}

/// The field's text as a [`Password`], which zeroes its copy when dropped.
/// The field keeps its own until the dialog closes: GPUI's text isn't ours
/// to zero.
fn read(input: &Entity<InputState>, cx: &App) -> Password {
    Password::new(input.read(cx).text().to_string())
}

impl Render for PasswordForm {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let hint = self.purpose.asks_new().then(|| {
            format!("At least {MIN_PASSWORD_CHARS} characters. A few unrelated words are easy to remember and hard to guess.")
        });
        v_flex()
            .gap_3()
            .when_some(self.note.clone(), |this, note| {
                this.child(div().text_sm().child(note))
            })
            .children(
                self.current
                    .as_ref()
                    .map(|input| Input::new(input).mask_toggle()),
            )
            .children(
                self.new
                    .as_ref()
                    .map(|input| Input::new(input).mask_toggle()),
            )
            .children(
                self.confirm
                    .as_ref()
                    .map(|input| Input::new(input).mask_toggle()),
            )
            .when_some(hint, |this, hint| {
                this.child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(hint),
                )
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
