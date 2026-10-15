//! The "Sharing with Emma" dialog: start sharing with a child, show and
//! copy their access key, make a new one, or stop.
//!
//! An entity, because the dialog outlives the click that opened it: it
//! shows the share as it changes (created, rekeyed, synced) and a busy line
//! while the server works. It only reports what was clicked;
//! `PapaApp` does the work and sends back the new state.

use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, IconName, Sizable as _, StyledExt as _,
    button::{Button, ButtonVariants as _},
    h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    App, ClipboardItem, Context, EventEmitter, FocusHandle, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, SharedString, Styled as _, Window, div,
};
use jiff::Timestamp;
use papa_core::{Child, ChildShare};

use super::dates;

/// What there is to show about sharing with the child.
#[derive(Clone, Debug)]
pub enum ShareState {
    /// Loading it from the vault.
    Loading,
    /// The father isn't signed in to a Papa account.
    SignedOut,
    Off,
    On {
        /// Boxed: much larger than the other variants.
        share: Box<ChildShare>,
        /// The father's email: what the child signs in with, beside the key.
        email: String,
    },
}

/// What the father clicked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShareDialogEvent {
    SignIn,
    Start,
    NewKey,
    Stop,
    SyncNow,
}

/// A step that needs a second click.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Confirm {
    NewKey,
    Stop,
}

pub struct ShareDialog {
    child: Child,
    state: ShareState,
    /// What the server is doing, while it does.
    busy: Option<SharedString>,
    error: Option<SharedString>,
    confirm: Option<Confirm>,
    /// The key was just copied: the button says so.
    copied: bool,
    /// Keeps keyboard focus in the dialog when the button that had it goes
    /// away (Start sharing, once sharing has started), so Esc and Enter
    /// still reach it.
    focus_handle: FocusHandle,
}

impl EventEmitter<ShareDialogEvent> for ShareDialog {}

impl ShareDialog {
    pub fn new(child: Child, cx: &mut Context<Self>) -> Self {
        Self {
            child,
            state: ShareState::Loading,
            busy: None,
            error: None,
            confirm: None,
            copied: false,
            focus_handle: cx.focus_handle(),
        }
    }

    /// Focuses the dialog, unless something in it has focus already.
    pub fn keep_focus(&self, window: &mut Window, cx: &mut App) {
        if !self.focus_handle.contains_focused(window, cx) {
            self.focus_handle.focus(window, cx);
        }
    }

    pub fn child(&self) -> &Child {
        &self.child
    }

    pub fn is_busy(&self) -> bool {
        self.busy.is_some()
    }

    /// Shows `state`, and ends the busy line.
    pub fn set_state(&mut self, state: ShareState, cx: &mut Context<Self>) {
        self.state = state;
        self.busy = None;
        cx.notify();
    }

    pub fn set_busy(&mut self, what: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.busy = Some(what.into());
        self.error = None;
        self.confirm = None;
        cx.notify();
    }

    pub fn set_error(&mut self, message: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.error = Some(message.into());
        self.busy = None;
        cx.notify();
    }

    /// Enter: the step the dialog is waiting for. Starts sharing, signs in,
    /// or confirms a new key or stopping; otherwise nothing.
    pub fn primary(&mut self, cx: &mut Context<Self>) {
        if self.busy.is_some() {
            return;
        }
        let event = match (&self.state, self.confirm) {
            (ShareState::SignedOut, _) => ShareDialogEvent::SignIn,
            (ShareState::Off, _) => ShareDialogEvent::Start,
            (ShareState::On { .. }, Some(Confirm::NewKey)) => ShareDialogEvent::NewKey,
            (ShareState::On { .. }, Some(Confirm::Stop)) => ShareDialogEvent::Stop,
            _ => return,
        };
        self.click(event, cx);
    }

    fn click(&mut self, event: ShareDialogEvent, cx: &mut Context<Self>) {
        self.confirm = None;
        self.error = None;
        cx.emit(event);
    }

    /// Shows or hides a question. The button clicked goes away with it, so
    /// focus moves to the dialog, where Enter answers and Esc closes.
    fn ask(&mut self, confirm: Option<Confirm>, window: &mut Window, cx: &mut Context<Self>) {
        self.confirm = confirm;
        self.focus_handle.focus(window, cx);
        cx.notify();
    }

    fn copy_key(&mut self, key: String, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(key));
        self.copied = true;
        cx.notify();
    }

    fn render_on(
        &self,
        share: &ChildShare,
        email: &str,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let name = self.child.name.clone();
        let key = share.access_key.to_string();
        let busy = self.busy.is_some();
        let muted = cx.theme().muted_foreground;
        let synced = match share.last_synced_at {
            Some(at) => format!("Last synced {}.", dates::ago(at, Timestamp::now())),
            None => "Not synced yet.".to_owned(),
        };

        let actions = match self.confirm {
            None => h_flex()
                .gap_2()
                .child(
                    Button::new("sync-now")
                        .outline()
                        .small()
                        .icon(gpui_kit::assets::IconName::RefreshCw)
                        .label("Sync now")
                        .disabled(busy)
                        .on_click(
                            cx.listener(|this, _, _, cx| this.click(ShareDialogEvent::SyncNow, cx)),
                        ),
                )
                .child(
                    Button::new("new-key")
                        .outline()
                        .small()
                        .label("New key…")
                        .disabled(busy)
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.ask(Some(Confirm::NewKey), window, cx)
                        })),
                )
                .child(
                    Button::new("stop")
                        .outline()
                        .small()
                        .label("Stop sharing…")
                        .disabled(busy)
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.ask(Some(Confirm::Stop), window, cx)
                        })),
                )
                .into_any_element(),
            Some(confirm) => {
                let (question, ok, event) = match confirm {
                    Confirm::NewKey => (
                        format!(
                            "{name}'s current key stops working at once. {name} will need the \
                             new one to read the shared notes."
                        ),
                        "Make a new key",
                        ShareDialogEvent::NewKey,
                    ),
                    Confirm::Stop => (
                        format!(
                            "{name} can no longer read any note, and the copy on the server \
                             is deleted. Your notes stay here."
                        ),
                        "Stop sharing",
                        ShareDialogEvent::Stop,
                    ),
                };
                v_flex()
                    .gap_2()
                    .child(div().text_sm().child(question))
                    .child(
                        h_flex()
                            .gap_2()
                            .child(
                                Button::new("cancel")
                                    .outline()
                                    .small()
                                    .label("Cancel")
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.ask(None, window, cx)
                                    })),
                            )
                            .child(Button::new("confirm").danger().small().label(ok).on_click(
                                cx.listener(move |this, _, _, cx| this.click(event, cx)),
                            )),
                    )
                    .into_any_element()
            }
        };

        v_flex()
            .gap_3()
            .child(
                v_flex()
                    .gap_1()
                    .child(
                        div()
                            .text_xs()
                            .font_semibold()
                            .text_color(muted)
                            .child(format!("{name}'s access key")),
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .child(
                                div()
                                    .flex_1()
                                    .px_2()
                                    .py_1()
                                    .rounded(cx.theme().radius)
                                    .bg(cx.theme().muted)
                                    .font_family(cx.theme().mono_font_family.clone())
                                    .text_sm()
                                    .child(key.clone()),
                            )
                            .child(
                                Button::new("copy-key")
                                    .outline()
                                    .small()
                                    .icon(IconName::Copy)
                                    .label(if self.copied { "Copied" } else { "Copy" })
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.copy_key(key.clone(), cx)
                                    })),
                            ),
                    ),
            )
            .child(div().text_sm().child(format!(
                "{name} signs in to Papa with your email, {email}, and this key. Hand it \
                 over in person, or send it in a message only {name} reads."
            )))
            .child(
                div()
                    .text_xs()
                    .text_color(muted)
                    .child(format!("{synced} Server: {}", share.relay_url)),
            )
            .when(share.auth_key_stale, |this| {
                this.child(div().text_sm().text_color(cx.theme().warning).child(
                    "The server doesn't know this key yet, so it won't work until the next \
                     sync reaches the server.",
                ))
            })
            .child(actions)
    }
}

impl Render for ShareDialog {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let name = self.child.name.clone();
        let busy = self.busy.is_some();
        let body = match &self.state {
            ShareState::Loading => div().into_any_element(),
            ShareState::SignedOut => v_flex()
                .gap_3()
                .child(div().text_sm().child(format!(
                    "To share notes with {name}, sign in to your Papa account first. \
                     Papa encrypts every shared note on this computer: the server only \
                     ever sees ciphertext."
                )))
                .child(
                    h_flex().child(
                        Button::new("sign-in")
                            .primary()
                            .small()
                            .label("Sign in…")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.click(ShareDialogEvent::SignIn, cx)
                            })),
                    ),
                )
                .into_any_element(),
            ShareState::Off => v_flex()
                .gap_3()
                .child(div().text_sm().child(format!(
                    "{name} will be able to read the notes you share, in Papa on their own \
                     computer. Nothing is shared until you choose a note: open it and tick \
                     “{name} can read it” under the + next to its date."
                )))
                .child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(
                            "Notes are encrypted here before they leave. Labels and \
                             unshared notes never leave this computer.",
                        ),
                )
                .child(
                    h_flex().child(
                        Button::new("start")
                            .primary()
                            .small()
                            .label("Start sharing")
                            .loading(busy)
                            .disabled(busy)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.click(ShareDialogEvent::Start, cx)
                            })),
                    ),
                )
                .into_any_element(),
            ShareState::On { share, email } => {
                let (share, email) = (share.clone(), email.clone());
                self.render_on(&share, &email, cx).into_any_element()
            }
        };
        v_flex()
            .track_focus(&self.focus_handle)
            .gap_3()
            .child(body)
            .when_some(self.busy.clone(), |this, busy| {
                this.child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child(busy),
                )
            })
            .when_some(self.error.clone(), |this, error| {
                this.child(div().text_sm().text_color(cx.theme().danger).child(error))
            })
    }
}
