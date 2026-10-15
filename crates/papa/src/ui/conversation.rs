//! A conversation between Dad and one child: the messages, oldest at the
//! top, and a box to write the next one. Both modes use it, each from its
//! own side ([`Sender`]).
//!
//! **Optimistic UI.** Pressing Enter shows the message at once, as
//! "Sending…", before anything is stored: the store, or the child's
//! thread, may be busy for a moment, or a while (a pull waiting on the
//! network). The message is made here, with its id, and handed to the
//! parent in [`ConversationEvent::Send`]. When the parent hears back it
//! calls [`Conversation::sent`] (the stored message replaces the draft,
//! matched by id) or [`Conversation::not_sent`] (the draft goes, and its
//! text returns to the box).

use std::collections::HashSet;
use std::rc::Rc;

use gpui_kit::component::{
    ActiveTheme as _, IconName, StyledExt as _,
    bubble::{Bubble, BubbleVariant},
    button::{Button, ButtonVariants as _},
    h_flex,
    input::{InputEvent, Textarea, TextareaState},
    message::{
        Message as MessageRow, MessageAlignment, MessageContent, MessageFooter, MessageGroup,
    },
    message_scroller::{MessageScroller, MessageScrollerState},
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Entity, EventEmitter, FocusHandle,
    InteractiveElement as _, IntoElement, ParentElement as _, Render, SharedString,
    StyleRefinement, Styled as _, Subscription, Window, div,
};
use jiff::tz::TimeZone;
use jiff::{Timestamp, Zoned};
use papa_core::model::clean_message;
use papa_core::{Message, MessageId, Sender};

use super::empty_state::EmptyState;

/// What the conversation asks its parent to do.
pub enum ConversationEvent {
    /// Store and send this message. It's on screen already.
    Send(Message),
}

pub struct Conversation {
    /// Whose side this is.
    me: Sender,
    /// Which conversation is shown, so another one starts at its end.
    shown: Option<SharedString>,
    /// Who's on the other side: "Emma", "Dad".
    other: SharedString,
    /// As stored, oldest first.
    stored: Vec<Message>,
    /// Shown, but not stored yet.
    drafts: Vec<Message>,
    /// `stored` and `drafts` together, in order: what the rows show.
    rows: Rc<Vec<Message>>,
    /// Why nothing can be written, if nothing can.
    blocked: Option<SharedString>,
    input: Entity<TextareaState>,
    scroller: Entity<MessageScrollerState>,
    focus_handle: FocusHandle,
    _subscription: Subscription,
}

impl EventEmitter<ConversationEvent> for Conversation {}

impl Conversation {
    pub fn new(me: Sender, window: &mut Window, cx: &mut Context<Self>) -> Self {
        // Enter sends; Shift-Enter starts a new line.
        let input = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(1, 6)
                .submit_on_enter(true)
        });
        let subscription = cx.subscribe_in(&input, window, |this, _, event, window, cx| {
            if let InputEvent::PressEnter { shift: false, .. } = event {
                this.send(window, cx);
            }
        });
        Self {
            me,
            shown: None,
            other: SharedString::default(),
            stored: Vec::new(),
            drafts: Vec::new(),
            rows: Rc::default(),
            blocked: None,
            input,
            scroller: cx.new(|cx| MessageScrollerState::new(0, cx)),
            focus_handle: cx.focus_handle(),
            _subscription: subscription,
        }
    }

    /// Shows the conversation `id` with `other`, as stored. The same one
    /// again keeps what's typed and where it's scrolled to.
    pub fn show(
        &mut self,
        id: impl Into<SharedString>,
        other: impl Into<SharedString>,
        messages: Vec<Message>,
        blocked: Option<SharedString>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let id = id.into();
        let other = other.into();
        if self.shown.as_ref() != Some(&id) {
            self.shown = Some(id);
            self.drafts.clear();
            self.stored.clear();
            self.rows = Rc::default();
            self.scroller
                .update(cx, |scroller, cx| scroller.reset(0, cx));
            self.input.update(cx, |input, cx| {
                input.set_value("", window, cx);
            });
        }
        if self.other != other {
            let placeholder = format!("Write to {other}…");
            self.input.update(cx, |input, cx| {
                input.set_placeholder(placeholder, window, cx)
            });
            self.other = other;
        }
        self.blocked = blocked;
        self.set_messages(messages, cx);
    }

    /// The conversation as stored now. Drafts it has caught up with go.
    pub fn set_messages(&mut self, messages: Vec<Message>, cx: &mut Context<Self>) {
        self.stored = messages;
        let stored: HashSet<MessageId> = self.stored.iter().map(|m| m.id).collect();
        self.drafts.retain(|draft| !stored.contains(&draft.id));
        self.refresh_rows(cx);
    }

    /// The parent stored `message`, a draft until now.
    pub fn sent(&mut self, message: Message, cx: &mut Context<Self>) {
        self.drafts.retain(|draft| draft.id != message.id);
        match self.stored.iter_mut().find(|m| m.id == message.id) {
            Some(stored) => *stored = message,
            None => self.stored.push(message),
        }
        self.refresh_rows(cx);
    }

    /// The parent couldn't store draft `id`: it goes, and its text goes
    /// back in the box, unless something new was typed there since.
    pub fn not_sent(&mut self, id: MessageId, window: &mut Window, cx: &mut Context<Self>) {
        let Some(index) = self.drafts.iter().position(|draft| draft.id == id) else {
            return;
        };
        let draft = self.drafts.remove(index);
        if self.input.read(cx).value().trim().is_empty() {
            self.input
                .update(cx, |input, cx| input.set_value(draft.body, window, cx));
        }
        self.refresh_rows(cx);
    }

    pub fn focus(&self, window: &mut Window, cx: &mut Context<Self>) {
        self.input.update(cx, |input, cx| input.focus(window, cx));
    }

    fn send(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.blocked.is_some() {
            return;
        }
        // Empty, or only spaces: nothing to send, and nothing to say.
        let Ok(body) = clean_message(&self.input.read(cx).value()) else {
            return;
        };
        let draft = Message::draft(MessageId::new(), self.me, body);
        self.drafts.push(draft.clone());
        self.input
            .update(cx, |input, cx| input.set_value("", window, cx));
        self.refresh_rows(cx);
        self.scroller
            .update(cx, |scroller, cx| scroller.scroll_to_end(cx));
        cx.emit(ConversationEvent::Send(draft));
    }

    /// Rebuilds the rows, and tells the list what changed: new rows at
    /// the end are appended, so a reader scrolled up stays where they are.
    fn refresh_rows(&mut self, cx: &mut Context<Self>) {
        let mut rows: Vec<Message> = self.stored.iter().chain(&self.drafts).cloned().collect();
        rows.sort_by_key(|message| (message.created_at, message.id));
        let before = self.rows.len();
        let prefix_kept = self
            .rows
            .iter()
            .zip(&rows)
            .all(|(old, new)| old.id == new.id);
        let count = rows.len();
        self.rows = Rc::new(rows);
        self.scroller.update(cx, |scroller, cx| {
            if prefix_kept && count >= before {
                scroller.append(count - before, cx);
                // A status under an old row may have changed ("Read").
                scroller.remeasure(cx);
            } else {
                scroller.reset(count, cx);
            }
        });
        cx.notify();
    }
}

impl Render for Conversation {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let body = if self.rows.is_empty() {
            EmptyState::new(
                "no-messages",
                IconName::Inbox,
                format!("No messages with {} yet", self.other),
            )
            .description(match self.me {
                Sender::Father => "What you write here reaches their Papa with the next sync.",
                Sender::Child => "What you write here reaches Dad's Papa with the next sync.",
            })
            .into_any_element()
        } else {
            let rows = self.rows.clone();
            let me = self.me;
            MessageScroller::new("messages", self.scroller.clone(), move |ix, _, cx| {
                row(&rows, ix, me, cx)
            })
            .with_bottom_fade(cx.theme().background)
            // The rows space themselves, closer within a run (see `row`).
            .with_row_style(StyleRefinement::default().pb_0())
            .size_full()
            .into_any_element()
        };

        let composer = match &self.blocked {
            Some(reason) => div()
                .px_4()
                .py_3()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child(reason.clone())
                .into_any_element(),
            None => h_flex()
                .gap_2()
                .p_3()
                .items_end()
                .child(div().flex_1().min_w_0().child(Textarea::new(&self.input)))
                .child(
                    Button::new("send-message")
                        .primary()
                        .icon(gpui_kit::assets::IconName::SendHorizontal)
                        .tooltip("Send (Enter)")
                        .on_click(cx.listener(|this, _, window, cx| this.send(window, cx))),
                )
                .into_any_element(),
        };

        v_flex()
            .size_full()
            .track_focus(&self.focus_handle)
            .child(
                h_flex()
                    .px_6()
                    .pt_4()
                    .pb_3()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .child(div().text_xl().font_semibold().child(self.other.clone())),
            )
            .child(div().flex_1().min_h_0().child(body))
            .child(
                div()
                    .border_t_1()
                    .border_color(cx.theme().border)
                    .child(composer),
            )
    }
}

/// Messages from one sender less than this apart read as one run: the
/// time shows once, under the last.
const RUN_GAP_MS: i64 = 5 * 60 * 1000;

/// Row `ix`: the day, if it's the first of its day, then the message. Mine
/// on the right, theirs on the left. Under the last of a run: the time,
/// and for mine whether it's there or read; only the newest says "Sent"
/// or "Read", so the conversation isn't a column of ticks. One on its way
/// says so wherever it is.
fn row(rows: &[Message], ix: usize, me: Sender, cx: &App) -> AnyElement {
    let message = &rows[ix];
    let tz = TimeZone::system();
    let day = |at: Timestamp| at.to_zoned(tz.clone()).date();
    let new_day = ix == 0 || day(rows[ix - 1].created_at) != day(message.created_at);
    let mine = message.sender == me;
    let newest_mine = mine && !rows[ix + 1..].iter().any(|later| later.sender == me);
    let end_of_run = rows.get(ix + 1).is_none_or(|next| {
        next.sender != message.sender
            || day(next.created_at) != day(message.created_at)
            || next.created_at.as_millisecond() - message.created_at.as_millisecond() > RUN_GAP_MS
    });

    let time = message
        .created_at
        .to_zoned(tz.clone())
        .strftime("%H:%M")
        .to_string();
    let status = match (mine, message.sent, message.read_at) {
        (false, ..) => None,
        (true, false, _) => Some("Sending…".to_owned()),
        (true, true, Some(_)) if newest_mine => Some("Read".to_owned()),
        (true, true, None) if newest_mine => Some("Sent".to_owned()),
        _ => None,
    };
    let alignment = if mine {
        MessageAlignment::End
    } else {
        MessageAlignment::Start
    };
    let bubble = Bubble::new()
        .with_variant(if mine {
            BubbleVariant::Filled
        } else {
            BubbleVariant::Secondary
        })
        .when(!message.sent, |bubble| bubble.opacity(0.7))
        .child(message.body.clone());
    let footer = (end_of_run || status.is_some()).then(|| {
        MessageFooter::new()
            .child(time)
            .when_some(status, |footer, status| footer.child("·").child(status))
    });
    let mut message_row = MessageRow::new()
        .alignment(alignment)
        .content(MessageContent::new().bubble(bubble));
    if let Some(footer) = footer {
        message_row = message_row.footer(footer);
    }

    v_flex()
        .w_full()
        .px_2()
        .pb_1()
        .when(end_of_run, |row| row.pb_4())
        .when(new_day, |row| {
            row.child(
                div()
                    .w_full()
                    .py_2()
                    .flex()
                    .justify_center()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(day_label(message.created_at, &Zoned::now())),
            )
        })
        .child(MessageGroup::new().child(message_row))
        .into_any_element()
}

/// "Today", "Yesterday", or the date: above the first message of a day.
fn day_label(at: Timestamp, now: &Zoned) -> String {
    let date = at.to_zoned(now.time_zone().clone()).date();
    if date == now.date() {
        "Today".to_owned()
    } else if now
        .date()
        .yesterday()
        .is_ok_and(|yesterday| yesterday == date)
    {
        "Yesterday".to_owned()
    } else if date.year() == now.year() {
        date.strftime("%A, %b %-d").to_string()
    } else {
        date.strftime("%b %-d %Y").to_string()
    }
}

/// How many unread messages there are, as a small pill. Nothing for none.
pub fn unread_pill(count: usize, cx: &App) -> Option<impl IntoElement + use<>> {
    (count > 0).then(|| {
        div()
            .flex_none()
            .px_1p5()
            .rounded_full()
            .bg(cx.theme().primary)
            .text_color(cx.theme().primary_foreground)
            .text_xs()
            .font_semibold()
            .child(if count > 99 {
                "99+".to_owned()
            } else {
                count.to_string()
            })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn days_read_as_today_yesterday_or_the_date() {
        let now: Zoned = "2026-10-15T09:00:00+02:00[Europe/Paris]".parse().unwrap();
        let at = |time: &str| time.parse::<Timestamp>().unwrap();
        // 22:30 UTC on the 14th is already 00:30 on the 15th in Paris.
        assert_eq!(day_label(at("2026-10-14T22:30:00Z"), &now), "Today");
        assert_eq!(day_label(at("2026-10-14T21:30:00Z"), &now), "Yesterday");
        assert_eq!(
            day_label(at("2026-10-12T12:00:00Z"), &now),
            "Monday, Oct 12"
        );
        assert_eq!(day_label(at("2025-12-31T12:00:00Z"), &now), "Dec 31 2025");
    }
}
