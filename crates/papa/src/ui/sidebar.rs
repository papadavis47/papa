use std::rc::Rc;

use gpui_kit::component::{
    ActiveTheme as _, Icon, IconName, Sizable as _,
    menu::{PopupMenu, PopupMenuItem},
    sidebar::{
        Sidebar, SidebarCollapsible, SidebarGroup, SidebarItem as _, SidebarMenu, SidebarMenuItem,
    },
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    App, ClickEvent, Hsla, IntoElement, RenderOnce, Styled as _, Window, rgb, transparent_black,
};
use papa_core::{Child, ChildId, Label, LabelId, NoteFilter};

use crate::app::OpenSettings;

/// Everything the sidebar can ask its parent to do.
///
/// One enum and one callback, instead of a callback per button: the parent
/// handles them all in a single `match`, and adding an event is a compile
/// error until the parent handles it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SidebarEvent {
    /// Show these notes in the list.
    Select(NoteFilter),
    AddChild,
    EditChild(ChildId),
    /// Open the child's sharing dialog.
    ShareChild(ChildId),
    DeleteChild(ChildId),
    AddLabel,
    RenameLabel(LabelId),
    DeleteLabel(LabelId),
}

type OnEvent = Rc<dyn Fn(&SidebarEvent, &mut Window, &mut App)>;

/// "All notes", children, labels, and the Messages / Settings entries.
/// Clicking one filters the note list; right-click a child or label to edit
/// or delete it, or a child to share notes with them.
#[derive(IntoElement)]
pub struct AppSidebar {
    children: Vec<Child>,
    /// Children whose sharing is on: they get a cloud next to their name.
    shared: Vec<ChildId>,
    labels: Vec<Label>,
    selected: NoteFilter,
    /// `false` until the vault is open, so nothing can be added too early.
    enabled: bool,
    /// The sidebar has keyboard focus: the selected entry gets a focus ring.
    focused: bool,
    /// Shared by every item's click handler and context menu.
    on_event: OnEvent,
}

impl AppSidebar {
    pub fn new(
        children: &[Child],
        shared: &[ChildId],
        labels: &[Label],
        selected: NoteFilter,
        enabled: bool,
        on_event: impl Fn(&SidebarEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        Self {
            children: children.to_vec(),
            shared: shared.to_vec(),
            labels: labels.to_vec(),
            selected,
            enabled,
            focused: false,
            on_event: Rc::new(on_event),
        }
    }

    /// Draws the focus ring, for when the sidebar has keyboard focus.
    pub fn focused(mut self, focused: bool) -> Self {
        self.focused = focused;
        self
    }
}

/// An entry that filters the notes, highlighted when it's the selected one.
/// Every entry has the border, so the focus ring doesn't move the text.
/// `ring` is the colour of the focus ring, or `None` while the sidebar
/// doesn't have focus.
fn filter_item(item: SidebarMenuItem, active: bool, ring: Option<Hsla>) -> SidebarMenuItem {
    let border = ring.filter(|_| active).unwrap_or(transparent_black());
    item.active(active).border_1().border_color(border)
}

/// A context menu with "Edit…"/"Rename…" and "Delete…" entries that send
/// `edit` and `delete` through `on_event`, and for a child, "Sharing…"
/// between them.
fn edit_menu(
    on_event: &OnEvent,
    edit_label: &'static str,
    edit: SidebarEvent,
    share: Option<SidebarEvent>,
    delete: SidebarEvent,
) -> impl Fn(PopupMenu, &mut Window, &mut App) -> PopupMenu + 'static {
    let on_event = on_event.clone();
    move |menu, _, _| {
        let on_edit = on_event.clone();
        let on_share = on_event.clone();
        let on_delete = on_event.clone();
        menu.item(
            PopupMenuItem::new(edit_label)
                .on_click(move |_, window, cx| on_edit(&edit, window, cx)),
        )
        .when_some(share, |menu, share| {
            menu.item(
                PopupMenuItem::new("Sharing…")
                    .on_click(move |_, window, cx| on_share(&share, window, cx)),
            )
        })
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

        let ring = self.focused.then_some(cx.theme().ring);
        let shared = self.shared;
        let children = self.children.into_iter().map(|child| {
            let mut item = SidebarMenuItem::new(child.name)
                .icon(Icon::new(IconName::CircleUser).text_color(rgb(child.color)));
            if shared.contains(&child.id) {
                item = item.suffix(|_, cx| {
                    Icon::new(gpui_kit::assets::IconName::Cloud)
                        .xsmall()
                        .text_color(cx.theme().muted_foreground)
                });
            }
            let active = self.selected == NoteFilter::Child(child.id);
            filter_item(item, active, ring)
                .on_click(send(SidebarEvent::Select(NoteFilter::Child(child.id))))
                .context_menu(edit_menu(
                    &on_event,
                    "Edit…",
                    SidebarEvent::EditChild(child.id),
                    Some(SidebarEvent::ShareChild(child.id)),
                    SidebarEvent::DeleteChild(child.id),
                ))
        });
        let add_child = SidebarMenuItem::new("Add child")
            .icon(IconName::Plus)
            .disable(!self.enabled)
            .on_click(send(SidebarEvent::AddChild));

        let labels = self.labels.into_iter().map(|label| {
            let filter = NoteFilter::Label(label.id);
            let item = SidebarMenuItem::new(format!("# {}", label.name));
            filter_item(item, self.selected == filter, ring)
                .on_click(send(SidebarEvent::Select(filter)))
                .context_menu(edit_menu(
                    &on_event,
                    "Rename…",
                    SidebarEvent::RenameLabel(label.id),
                    None,
                    SidebarEvent::DeleteLabel(label.id),
                ))
        });
        let add_label = SidebarMenuItem::new("Add label")
            .icon(IconName::Plus)
            .disable(!self.enabled)
            .on_click(send(SidebarEvent::AddLabel));

        // `Sidebar` takes groups as children; a lone entry goes in the header.
        let all_notes = SidebarMenu::new().child(
            filter_item(
                SidebarMenuItem::new("All notes").icon(IconName::BookOpen),
                self.selected == NoteFilter::All,
                ring,
            )
            .on_click(send(SidebarEvent::Select(NoteFilter::All))),
        );

        // Messages arrive in a later phase. Settings dispatches the same
        // action as ⌘, so both open the one Settings window.
        let footer = SidebarMenu::new().children([
            SidebarMenuItem::new("Messages")
                .icon(IconName::Inbox)
                .disable(true),
            SidebarMenuItem::new("Settings")
                .icon(IconName::Settings)
                .on_click(|_, window, cx| window.dispatch_action(Box::new(OpenSettings), cx)),
        ]);

        Sidebar::new("sidebar")
            // The resizable panel sets the width and the title bar button hides
            // the whole panel, so the sidebar itself never collapses.
            .collapsible(SidebarCollapsible::None)
            .w_full()
            .header(all_notes.render("sidebar-header", window, cx))
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
