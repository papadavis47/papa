//! What the window shows while the vault is locked: a password field and
//! nothing else of the father's.

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::{
    ActiveTheme as _, Sizable as _,
    button::{Button, ButtonVariants as _},
    input::{Input, InputEvent, InputState},
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AppContext as _, Context, Entity, EventEmitter, IntoElement, ParentElement as _, Render,
    SharedString, Styled as _, Subscription, Window, div, px,
};
use papa_core::lock::Password;

use super::empty_state::EmptyState;

pub enum UnlockEvent {
    /// The father asks to unlock with this password.
    Submit(Password),
}

/// The unlock screen. An entity: it keeps the field's text, the last
/// error and whether a password is being checked.
pub struct UnlockScreen {
    password: Entity<InputState>,
    error: Option<SharedString>,
    /// Checking a password takes a moment (Argon2id, on purpose).
    busy: bool,
    _subscription: Subscription,
}

impl EventEmitter<UnlockEvent> for UnlockScreen {}

impl UnlockScreen {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let password = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Password")
                .masked(true)
        });
        let subscription =
            cx.subscribe_in(&password, window, |this, _, event: &InputEvent, _, cx| {
                if let InputEvent::PressEnter { .. } = event {
                    this.submit(cx);
                }
            });
        Self {
            password,
            error: None,
            busy: false,
            _subscription: subscription,
        }
    }

    pub fn focus(&self, window: &mut Window, cx: &mut Context<Self>) {
        self.password
            .update(cx, |input, cx| input.focus(window, cx));
    }

    /// The password didn't open the vault: say why, empty the field, and
    /// let the father try again.
    pub fn failed(
        &mut self,
        message: impl Into<SharedString>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.busy = false;
        self.error = Some(message.into());
        self.password
            .update(cx, |input, cx| input.set_value("", window, cx));
        self.focus(window, cx);
        cx.notify();
    }

    fn submit(&mut self, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        let password = Password::new(self.password.read(cx).text().to_string());
        if password.is_empty() {
            return;
        }
        self.busy = true;
        self.error = None;
        cx.emit(UnlockEvent::Submit(password));
        cx.notify();
    }
}

impl Render for UnlockScreen {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let form = v_flex()
            .w(px(280.))
            .gap_2()
            .child(Input::new(&self.password).mask_toggle().disabled(self.busy))
            .child(
                Button::new("unlock")
                    .primary()
                    .label(if self.busy { "Unlocking…" } else { "Unlock" })
                    .loading(self.busy)
                    .on_click(cx.listener(|this, _, _, cx| this.submit(cx))),
            )
            .when_some(self.error.clone(), |form, error| {
                form.child(div().text_sm().text_color(cx.theme().danger).child(error))
            })
            .child(
                // A forgotten password can't be recovered, but a backup can
                // still be put back.
                Button::new("restore")
                    .ghost()
                    .small()
                    .label("Restore a backup…")
                    .on_click(|_, window, cx| {
                        window.dispatch_action(Box::new(crate::app::RestoreBackup), cx)
                    }),
            );

        div().size_full().child(
            EmptyState::new("locked", Lucide::LockKeyhole, "Papa is locked")
                .description("Type your password to open your vault.")
                .child(form),
        )
    }
}
