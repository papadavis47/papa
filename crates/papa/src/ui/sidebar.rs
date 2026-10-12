use std::rc::Rc;

use gpui_kit::component::{
    Icon, IconName,
    menu::{PopupMenu, PopupMenuItem},
    sidebar::{
        Sidebar, SidebarCollapsible, SidebarGroup, SidebarItem as _, SidebarMenu, SidebarMenuItem,
    },
};
use gpui_kit::{App, ClickEvent, IntoElement, RenderOnce, Styled as _, Window, rgb};
use papa_core::{Child, ChildId, Label, LabelId};

/// Everything the sidebar can ask its parent to do.
///
/// One enum and one callback, instead of a callback per button: the parent
/// handles them all in a single `match`, and adding an event is a compile
/// error until the parent handles it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SidebarEvent {
    SelectChild(ChildId),
    AddChild,
    EditChild(ChildId),
    DeleteChild(ChildId),
    AddLabel,
    RenameLabel(LabelId),
    DeleteLabel(LabelId),
}

type OnEvent = Rc<dyn Fn(&SidebarEvent, &mut Window, &mut App)>;

/// Children, labels, and the Messages / Settings entries. Right-click a child
/// or label to edit or delete it.
#[derive(IntoElement)]
pub struct AppSidebar {
    children: Vec<Child>,
    labels: Vec<Label>,
    selected: Option<ChildId>,
    /// `false` until the vault is open, so nothing can be added too early.
    enabled: bool,
    /// Shared by every item's click handler and context menu.
    on_event: OnEvent,
}

impl AppSidebar {
    pub fn new(
        children: &[Child],
        labels: &[Label],
        selected: Option<ChildId>,
        enabled: bool,
        on_event: impl Fn(&SidebarEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        Self {
            children: children.to_vec(),
            labels: labels.to_vec(),
            selected,
            enabled,
            on_event: Rc::new(on_event),
        }
    }
}

/// A context menu with "Edit…"/"Rename…" and "Delete…" entries that send
/// `edit` and `delete` through `on_event`.
fn edit_menu(
    on_event: &OnEvent,
    edit_label: &'static str,
    edit: SidebarEvent,
    delete: SidebarEvent,
) -> impl Fn(PopupMenu, &mut Window, &mut App) -> PopupMenu + 'static {
    let on_event = on_event.clone();
    move |menu, _, _| {
        let on_edit = on_event.clone();
        let on_delete = on_event.clone();
        menu.item(
            PopupMenuItem::new(edit_label)
                .on_click(move |_, window, cx| on_edit(&edit, window, cx)),
        )
        .item(
            PopupMenuItem::new("Delete…")
                .on_click(move |_, window, cx| on_delete(&delete, window, cx)),
        )
    }
}

impl RenderOnce for AppSidebar {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let on_event = self.on_event;
        let send = |event: SidebarEvent| {
            let on_event = on_event.clone();
            move |_: &ClickEvent, window: &mut Window, cx: &mut App| on_event(&event, window, cx)
        };

        let children = self.children.into_iter().map(|child| {
            SidebarMenuItem::new(child.name)
                .icon(Icon::new(IconName::CircleUser).text_color(rgb(child.color)))
                .active(self.selected == Some(child.id))
                .on_click(send(SidebarEvent::SelectChild(child.id)))
                .context_menu(edit_menu(
                    &on_event,
                    "Edit…",
                    SidebarEvent::EditChild(child.id),
                    SidebarEvent::DeleteChild(child.id),
                ))
        });
        let add_child = SidebarMenuItem::new("Add child")
            .icon(IconName::Plus)
            .disable(!self.enabled)
            .on_click(send(SidebarEvent::AddChild));

        // Filtering notes by label arrives with notes in Phase 3.
        let labels = self.labels.into_iter().map(|label| {
            SidebarMenuItem::new(format!("# {}", label.name)).context_menu(edit_menu(
                &on_event,
                "Rename…",
                SidebarEvent::RenameLabel(label.id),
                SidebarEvent::DeleteLabel(label.id),
            ))
        });
        let add_label = SidebarMenuItem::new("Add label")
            .icon(IconName::Plus)
            .disable(!self.enabled)
            .on_click(send(SidebarEvent::AddLabel));

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
            .child(
                SidebarGroup::new("Labels")
                    .child(SidebarMenu::new().children(labels.chain([add_label]))),
            )
            .footer(footer.render("sidebar-footer", window, cx))
    }
}
