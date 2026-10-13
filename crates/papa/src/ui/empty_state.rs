//! What a pane shows when it has nothing to show: an icon, a title, a line
//! about why, and usually a button for the obvious next step.

use std::time::Duration;

use gpui_kit::component::{
    Icon, Sizable as _,
    button::Button,
    empty::{
        Empty, EmptyContent, EmptyDescription, EmptyHeader, EmptyMedia, EmptyMediaVariant,
        EmptyTitle,
    },
    h_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    Action, Animation, AnimationExt as _, AnyElement, App, ClickEvent, ElementId, IntoElement,
    ParentElement as _, RenderOnce, SharedString, Styled as _, Window, ease_out_quint, px,
};

use super::shortcut;

/// How long an empty state takes to fade in.
const APPEAR: Duration = Duration::from_millis(240);

type OnClick = Box<dyn Fn(&ClickEvent, &mut Window, &mut App)>;

#[derive(IntoElement)]
pub struct EmptyState {
    /// Keys the fade-in. It plays again whenever an element with this id
    /// appears where none was in the frame before.
    id: ElementId,
    icon: Icon,
    title: SharedString,
    description: Option<SharedString>,
    button: Option<(SharedString, OnClick)>,
    /// Shown next to the button: the shortcut that does the same.
    shortcut: Option<Box<dyn Action>>,
    children: Vec<AnyElement>,
}

impl EmptyState {
    pub fn new(
        id: impl Into<ElementId>,
        icon: impl Into<Icon>,
        title: impl Into<SharedString>,
    ) -> Self {
        Self {
            id: id.into(),
            icon: icon.into(),
            title: title.into(),
            description: None,
            button: None,
            shortcut: None,
            children: Vec::new(),
        }
    }

    pub fn description(mut self, description: impl Into<SharedString>) -> Self {
        self.description = Some(description.into());
        self
    }

    pub fn button(
        mut self,
        label: impl Into<SharedString>,
        on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.button = Some((label.into(), Box::new(on_click)));
        self
    }

    /// Shows the key bound to `action` next to the button.
    pub fn shortcut(mut self, action: impl Action) -> Self {
        self.shortcut = Some(Box::new(action));
        self
    }

    /// Content under the description, in place of or besides the button.
    pub fn child(mut self, child: impl IntoElement) -> Self {
        self.children.push(child.into_any_element());
        self
    }
}

impl RenderOnce for EmptyState {
    fn render(self, window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let shortcut = self
            .shortcut
            .and_then(|action| shortcut(action.as_ref(), window));
        let button = self.button.map(|(label, on_click)| {
            h_flex()
                .gap_2()
                .child(
                    Button::new("empty-state-action")
                        .outline()
                        .small()
                        .label(label)
                        .on_click(on_click),
                )
                .children(shortcut)
        });

        let header = EmptyHeader::new()
            .media(
                EmptyMedia::new()
                    .with_variant(EmptyMediaVariant::Icon)
                    .child(self.icon),
            )
            .title(EmptyTitle::new().child(self.title))
            .when_some(self.description, |header, description| {
                header.description(EmptyDescription::new().child(description))
            });
        let has_content = button.is_some() || !self.children.is_empty();

        Empty::new()
            .size_full()
            .header(header)
            .when(has_content, |empty| {
                empty.content(EmptyContent::new().children(button).children(self.children))
            })
            // Rises a few pixels as it fades in. `relative` + `top` moves
            // the element without moving anything around it.
            .relative()
            .with_animation(
                self.id,
                Animation::new(APPEAR).with_easing(ease_out_quint()),
                |empty, t| empty.opacity(t).top(px(6. * (1. - t))),
            )
    }
}
