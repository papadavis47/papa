//! The middle pane while Messages is open, in father mode: one row per
//! child he can write to, or has written with. The newest message under
//! each name, and how many he hasn't read.

use std::rc::Rc;

use gpui_kit::component::{ActiveTheme as _, Icon, IconName, StyledExt as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    App, InteractiveElement as _, IntoElement, ParentElement as _, RenderOnce, SharedString,
    StatefulInteractiveElement as _, Styled as _, Window, div, rgb, transparent_black,
};
use jiff::Zoned;
use papa_core::{Child, ChildId, ConversationSummary, Sender};

use super::conversation::unread_pill;
use super::dates;
use super::empty_state::EmptyState;

/// One child in the list.
#[derive(Clone, Debug)]
pub struct ConversationEntry {
    pub child: Child,
    /// Sharing is on, so messages can go out.
    pub shared: bool,
    /// `None` until the first message.
    pub summary: Option<ConversationSummary>,
}

type OnSelect = Rc<dyn Fn(&ChildId, &mut Window, &mut App)>;

#[derive(IntoElement)]
pub struct ConversationList {
    entries: Vec<ConversationEntry>,
    selected: Option<ChildId>,
    /// The list has keyboard focus: the selected row gets a focus ring.
    focused: bool,
    on_select: OnSelect,
}

impl ConversationList {
    pub fn new(
        entries: Vec<ConversationEntry>,
        selected: Option<ChildId>,
        on_select: impl Fn(&ChildId, &mut Window, &mut App) + 'static,
    ) -> Self {
        Self {
            entries,
            selected,
            focused: false,
            on_select: Rc::new(on_select),
        }
    }

    pub fn focused(mut self, focused: bool) -> Self {
        self.focused = focused;
        self
    }
}

impl RenderOnce for ConversationList {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let header = h_flex()
            .px_3()
            .pt_3()
            .pb_2()
            .child(div().flex_1().font_semibold().child("Messages"));
        let body = if self.entries.is_empty() {
            EmptyState::new(
                "no-conversations",
                IconName::Inbox,
                "No one to write to yet",
            )
            .description(
                "Turn sharing on for a child: right-click their name, then Sharing…. \
                 They write back from their own Papa.",
            )
            .into_any_element()
        } else {
            let now = Zoned::now();
            let rows = self.entries.iter().map(|entry| {
                let id = entry.child.id;
                let on_select = self.on_select.clone();
                row(entry, self.selected == Some(id), self.focused, &now, cx)
                    .on_click(move |_, window, cx| on_select(&id, window, cx))
            });
            div()
                .id("conversations")
                .flex_1()
                .min_h_0()
                .px_2()
                .pb_2()
                .overflow_y_scroll()
                .children(rows)
                .into_any_element()
        };
        v_flex()
            .size_full()
            .min_w_0()
            .child(header)
            .child(div().flex_1().min_h_0().child(body))
    }
}

fn row(
    entry: &ConversationEntry,
    selected: bool,
    focused: bool,
    now: &Zoned,
    cx: &App,
) -> gpui_kit::Stateful<gpui_kit::Div> {
    let summary = entry.summary.as_ref();
    let unread = summary.map_or(0, |summary| summary.unread);
    let preview: SharedString = match summary {
        Some(summary) if summary.last.sender == Sender::Father => {
            format!("You: {}", one_line(&summary.last.body)).into()
        }
        Some(summary) => one_line(&summary.last.body).into(),
        None => "No messages yet".into(),
    };
    let detail: SharedString = match summary {
        _ if !entry.shared => "Sharing off".into(),
        Some(summary) => dates::short(summary.last.created_at, now).into(),
        None => SharedString::default(),
    };
    let ring = if selected && focused {
        cx.theme().ring
    } else {
        transparent_black()
    };
    v_flex()
        .id(SharedString::from(format!(
            "conversation-{}",
            entry.child.id
        )))
        .w_full()
        .gap_0p5()
        .px_3()
        .py_1p5()
        .rounded(cx.theme().radius)
        .border_1()
        .border_color(ring)
        .cursor_pointer()
        .map(|row| {
            if selected {
                row.bg(cx.theme().list_active)
            } else {
                row.hover(|row| row.bg(cx.theme().list_hover))
            }
        })
        .child(
            h_flex()
                .gap_2()
                .child(
                    Icon::new(IconName::CircleUser)
                        .text_color(rgb(entry.child.color))
                        .flex_none(),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .when(unread > 0, |name| name.font_semibold())
                        .when(unread == 0, |name| name.font_medium())
                        .child(entry.child.name.clone()),
                )
                .child(
                    div()
                        .flex_none()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(detail),
                ),
        )
        .child(
            h_flex()
                .gap_2()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child(div().flex_1().min_w_0().truncate().child(preview))
                .children(unread_pill(unread, cx)),
        )
}

/// A message's text on one line, for a preview.
fn one_line(body: &str) -> String {
    body.split_whitespace().collect::<Vec<_>>().join(" ")
}
