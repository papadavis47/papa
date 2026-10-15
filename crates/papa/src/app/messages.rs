//! Messages, in the main window: the list of conversations, the one open,
//! sending, and marking what the father read.
//!
//! The vault holds every message; [`Sharing`](papa_core::Sharing)'s sync
//! carries them both ways (see `sharing.rs`, which polls). After each
//! sync, and each change here, the conversations are read again from the
//! vault: what the window shows is always what's stored, plus the drafts
//! [`Conversation`] shows ahead of the store.

use std::rc::Rc;
use std::time::Duration;

use gpui_kit::component::{WindowExt as _, notification::Notification};
use gpui_kit::{AppContext as _, Context, Entity, SharedString, Window};
use papa_core::{ChildId, Sender};

use super::{OpenMessages, PapaApp};
use crate::state::Pane;
use crate::ui::{Conversation, ConversationEntry, ConversationEvent, sentence};

/// How soon the father's receipts go out after he reads: soon, but a few
/// conversations read in a row share one sync.
const RECEIPT_DELAY: Duration = Duration::from_secs(2);

impl PapaApp {
    /// Made with the editor, when the vault opens.
    pub(super) fn new_conversation(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let conversation = cx.new(|cx| Conversation::new(Sender::Father, window, cx));
        self.conversation_subscription =
            Some(cx.subscribe_in(&conversation, window, Self::on_conversation_event));
        self.conversation = Some(conversation);
    }

    /// Every child with sharing on, or with messages from before it was
    /// turned off, in sidebar order.
    pub(super) fn conversation_entries(&self) -> Vec<ConversationEntry> {
        let shared = self.shared_children();
        self.children
            .iter()
            .filter_map(|child| {
                let summary = self.conversations.get(&child.id).cloned();
                let shared = shared.contains(&child.id);
                (shared || summary.is_some()).then(|| ConversationEntry {
                    child: child.clone(),
                    shared,
                    summary,
                })
            })
            .collect()
    }

    /// All the unread messages, for the sidebar.
    pub(super) fn unread(&self) -> usize {
        self.conversations
            .values()
            .map(|summary| summary.unread)
            .sum()
    }

    /// ⇧⌘M: shows the messages, with the conversation that was open, or
    /// the first with something unread, or the first.
    pub(super) fn open_messages(
        &mut self,
        _: &OpenMessages,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.is_open() {
            return;
        }
        let entries = self.conversation_entries();
        let child = self
            .conversation_child
            .filter(|child| entries.iter().any(|entry| entry.child.id == *child))
            .or_else(|| {
                let unread = entries
                    .iter()
                    .find(|entry| entry.summary.as_ref().is_some_and(|s| s.unread > 0));
                unread.or(entries.first()).map(|entry| entry.child.id)
            });
        self.pane = Pane::Messages;
        self.select_conversation(child, window, cx);
        self.note_list_focus.focus(window, cx);
    }

    /// Opens the conversation with `child`, or none.
    pub(super) fn select_conversation(
        &mut self,
        child: Option<ChildId>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.pane = Pane::Messages;
        self.conversation_child = child;
        self.load_conversation(window, cx);
        cx.notify();
    }

    /// ↑/↓ in the list of conversations.
    pub(super) fn move_conversation(
        &mut self,
        delta: isize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let ids: Vec<ChildId> = self
            .conversation_entries()
            .iter()
            .map(|entry| entry.child.id)
            .collect();
        let next = crate::state::step(self.conversation_child, delta, &ids);
        if next != self.conversation_child {
            self.select_conversation(next, window, cx);
        }
    }

    /// Reads the conversations again, and the open one: after a sync, or
    /// a change.
    pub(super) fn reload_messages(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.with_store(
            "Couldn't load your messages",
            async move |store| store.conversations().await,
            |this, conversations, window, cx| {
                this.conversations = Rc::new(conversations);
                if this.pane == Pane::Messages {
                    this.load_conversation(window, cx);
                }
            },
            window,
            cx,
        );
    }

    /// Shows the open conversation as stored, and marks it read if the
    /// father can see it.
    fn load_conversation(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(conversation), Some(child)) =
            (self.conversation.clone(), self.conversation_child)
        else {
            return;
        };
        let Some(name) = self.child(child).map(|child| child.name.clone()) else {
            return;
        };
        let blocked: Option<SharedString> = (!self.shared_children().contains(&child)).then(|| {
            format!("Sharing with {name} is off. Turn it on to write: right-click their name, then Sharing….")
                .into()
        });
        self.with_store(
            "Couldn't load the messages",
            async move |store| store.messages(child).await,
            move |this, messages, window, cx| {
                if this.conversation_child != Some(child) {
                    return;
                }
                conversation.update(cx, |conversation, cx| {
                    conversation.show(child.to_string(), name, messages, blocked, window, cx)
                });
                this.mark_read_if_seen(window, cx);
            },
            window,
            cx,
        );
    }

    /// Marks the open conversation read, if the father is looking at it:
    /// Messages is showing, and the window is in front.
    pub(super) fn mark_read_if_seen(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(child) = self.conversation_child else {
            return;
        };
        let unread = self.conversations.get(&child).map_or(0, |s| s.unread);
        if self.pane != Pane::Messages || unread == 0 || !window.is_window_active() {
            return;
        }
        self.with_store(
            "Couldn't mark the messages read",
            async move |store| {
                let read = store.mark_read(child).await?;
                Ok((read, store.conversations().await?))
            },
            |this, (read, conversations), window, cx| {
                this.conversations = Rc::new(conversations);
                if read > 0 {
                    // The receipts go out with the next sync.
                    this.schedule_sync(RECEIPT_DELAY, window, cx);
                }
            },
            window,
            cx,
        );
    }

    fn on_conversation_event(
        &mut self,
        conversation: &Entity<Conversation>,
        event: &ConversationEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let ConversationEvent::Send(draft) = event;
        let Some(child) = self.conversation_child else {
            return;
        };
        let (id, body) = (draft.id, draft.body.clone());
        let conversation = conversation.clone();
        let failed = conversation.clone();
        let Some(store) = self.store() else {
            return;
        };
        let generation = self.generation;
        cx.spawn_in(window, async move |this, cx| {
            let result = store.add_message(child, id, body).await;
            this.update_in(cx, |this, window, cx| {
                if this.generation != generation {
                    return;
                }
                match result {
                    Ok(message) => {
                        conversation.update(cx, |conversation, cx| conversation.sent(message, cx));
                        this.reload_messages(window, cx);
                        // Sent now, not after a pause: someone's waiting.
                        this.schedule_sync(Duration::ZERO, window, cx);
                    }
                    Err(err) => {
                        failed.update(cx, |conversation, cx| conversation.not_sent(id, window, cx));
                        window.push_notification(
                            Notification::error(sentence(err)).title("Couldn't send the message"),
                            cx,
                        );
                    }
                }
            })
            .ok();
        })
        .detach();
    }

    /// After a sync brought messages: a notification for each child whose
    /// conversation the father isn't looking at.
    pub(super) fn announce_messages(
        &mut self,
        received: Vec<ChildId>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        for child in received {
            let seen = self.pane == Pane::Messages
                && self.conversation_child == Some(child)
                && window.is_window_active();
            let Some(name) = self.child(child).map(|child| child.name.clone()) else {
                continue;
            };
            if seen {
                continue;
            }
            let this = cx.entity().downgrade();
            window.push_notification(
                Notification::info(format!("{name} wrote to you."))
                    .title("New message")
                    .action(move |_, _, cx| {
                        let this = this.clone();
                        gpui_kit::component::button::Button::new("open-conversation")
                            .label("Read")
                            .on_click(cx.listener(move |notification, _, window, cx| {
                                notification.dismiss(window, cx);
                                this.update(cx, |this, cx| {
                                    this.select_conversation(Some(child), window, cx)
                                })
                                .ok();
                            }))
                    }),
                cx,
            );
        }
    }
}
