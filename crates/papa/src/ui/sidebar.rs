use std::rc::Rc;

use gpui_kit::component::{
    ActiveTheme as _, Icon, IconName,
    sidebar::{
        Sidebar, SidebarCollapsible, SidebarGroup, SidebarItem as _, SidebarMenu, SidebarMenuItem,
    },
};
use gpui_kit::{App, IntoElement, ParentElement as _, RenderOnce, Styled as _, Window, div, rgb};

use crate::placeholder::{Child, ChildId, Journal};

type OnSelect = Rc<dyn Fn(&ChildId, &mut Window, &mut App)>;

/// Children, labels, and the Messages / Settings entries.
#[derive(IntoElement)]
pub struct AppSidebar {
    children: Vec<(Child, usize)>,
    labels: Vec<&'static str>,
    selected: Option<ChildId>,
    /// Shared by every child's menu item, so it lives behind an `Rc`.
    on_select: OnSelect,
}

impl AppSidebar {
    pub fn new(
        journal: &Journal,
        selected: Option<ChildId>,
        on_select: impl Fn(&ChildId, &mut Window, &mut App) + 'static,
    ) -> Self {
        Self {
            children: journal
                .children
                .iter()
                .map(|child| (child.clone(), journal.notes_for(child.id).count()))
                .collect(),
            labels: journal.labels.clone(),
            selected,
            on_select: Rc::new(on_select),
        }
    }
}

impl RenderOnce for AppSidebar {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let children = self.children.into_iter().map(|(child, note_count)| {
            let on_select = self.on_select.clone();
            SidebarMenuItem::new(child.name)
                .icon(Icon::new(IconName::CircleUser).text_color(rgb(child.color)))
                .active(self.selected == Some(child.id))
                .suffix(move |_, cx| {
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(note_count.to_string())
                })
                .on_click(move |_, window, cx| on_select(&child.id, window, cx))
        });
        // Placeholders: adding children arrives in Phase 2, filtering by label in Phase 3.
        let add_child = SidebarMenuItem::new("Add child")
            .icon(IconName::Plus)
            .disable(true);
        let labels = self
            .labels
            .into_iter()
            .map(|label| SidebarMenuItem::new(format!("# {label}")));

        // Messages and Settings arrive in later phases.
        let footer = SidebarMenu::new().children([
            SidebarMenuItem::new("Messages")
                .icon(IconName::Inbox)
                .disable(true),
            SidebarMenuItem::new("Settings")
                .icon(IconName::Settings)
                .disable(true),
        ]);

        Sidebar::new("sidebar")
            // The resizable panel sets the width and the title bar button hides
            // the whole panel, so the sidebar itself never collapses.
            .collapsible(SidebarCollapsible::None)
            .w_full()
            .child(
                SidebarGroup::new("Children")
                    .child(SidebarMenu::new().children(children.chain([add_child]))),
            )
            .child(SidebarGroup::new("Labels").child(SidebarMenu::new().children(labels)))
            .footer(footer.render("sidebar-footer", window, cx))
    }
}
