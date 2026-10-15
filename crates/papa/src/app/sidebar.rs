//! The sidebar's side of `PapaApp`: choosing a filter, and adding, editing
//! and deleting children and labels.

use std::rc::Rc;

use gpui_kit::{ClickEvent, Context, Window};
use papa_core::{Child, ChildDraft, ChildId, Label, LabelId, NoteFilter};

use super::{OpenMessages, PapaApp, SelectNextFilter, SelectPreviousFilter};
use crate::state::Pane;
use crate::ui::{SidebarEvent, dialogs, suggested_color};

impl PapaApp {
    pub(super) fn child(&self, id: ChildId) -> Option<&Child> {
        self.children.iter().find(|child| child.id == id)
    }

    pub(super) fn label(&self, id: LabelId) -> Option<&Label> {
        self.labels.iter().find(|label| label.id == id)
    }

    /// Every sidebar entry that filters notes, top to bottom.
    fn filters(&self) -> Vec<NoteFilter> {
        let children = self.children.iter().map(|c| NoteFilter::Child(c.id));
        let labels = self.labels.iter().map(|l| NoteFilter::Label(l.id));
        [NoteFilter::All]
            .into_iter()
            .chain(children)
            .chain(labels)
            .collect()
    }

    /// After children or labels changed: keeps the filter valid, tells the
    /// editor, and reloads the notes (deleting a child unassigns its notes).
    pub(super) fn catalog_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let children: Vec<ChildId> = self.children.iter().map(|child| child.id).collect();
        let labels: Vec<LabelId> = self.labels.iter().map(|label| label.id).collect();
        self.selection.reconcile_filter(&children, &labels);
        if let Some(editor) = &self.editor {
            let (children, labels) = (self.children.clone(), self.labels.clone());
            editor.update(cx, |editor, cx| editor.set_catalog(children, labels, cx));
        }
        self.refresh_notes(window, cx);
    }

    fn set_children(&mut self, children: Vec<Child>, window: &mut Window, cx: &mut Context<Self>) {
        self.children = Rc::new(children);
        self.catalog_changed(window, cx);
    }

    fn set_labels(&mut self, labels: Vec<Label>, window: &mut Window, cx: &mut Context<Self>) {
        self.labels = Rc::new(labels);
        self.catalog_changed(window, cx);
    }

    pub(super) fn select_filter(
        &mut self,
        filter: NoteFilter,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.pane = Pane::Notes;
        self.selection.filter = filter;
        self.refresh_notes(window, cx);
        cx.notify();
    }

    pub(super) fn select_next_filter(
        &mut self,
        _: &SelectNextFilter,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.move_filter(1, window, cx);
    }

    pub(super) fn select_previous_filter(
        &mut self,
        _: &SelectPreviousFilter,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.move_filter(-1, window, cx);
    }

    fn move_filter(&mut self, delta: isize, window: &mut Window, cx: &mut Context<Self>) {
        // From Messages, the arrows go back to the notes they left.
        if self.pane == Pane::Messages {
            self.select_filter(self.selection.filter, window, cx);
            return;
        }
        let before = self.selection.filter;
        self.selection.move_filter(delta, &self.filters());
        if self.selection.filter != before {
            self.refresh_notes(window, cx);
        }
        cx.notify();
    }

    // --- Changes to the vault. Each reloads the affected list afterwards, so
    // the UI always shows exactly what's stored. ---

    fn add_child(&mut self, draft: ChildDraft, window: &mut Window, cx: &mut Context<Self>) {
        self.with_store(
            "Couldn't add the child",
            async move |store| {
                let child = store.add_child(draft).await?;
                Ok((child.id, store.children().await?))
            },
            |this, (id, children), window, cx| {
                this.selection.filter = NoteFilter::Child(id);
                this.set_children(children, window, cx);
            },
            window,
            cx,
        );
    }

    fn update_child(
        &mut self,
        id: ChildId,
        draft: ChildDraft,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.with_store(
            "Couldn't save the child",
            async move |store| {
                store.update_child(id, draft).await?;
                store.children().await
            },
            Self::set_children,
            window,
            cx,
        );
    }

    fn delete_child(&mut self, id: ChildId, window: &mut Window, cx: &mut Context<Self>) {
        self.with_store(
            "Couldn't delete the child",
            async move |store| {
                store.delete_child(id).await?;
                store.children().await
            },
            Self::set_children,
            window,
            cx,
        );
    }

    fn add_label(&mut self, name: String, window: &mut Window, cx: &mut Context<Self>) {
        self.with_store(
            "Couldn't add the label",
            async move |store| {
                store.add_label(name).await?;
                store.labels().await
            },
            Self::set_labels,
            window,
            cx,
        );
    }

    fn rename_label(
        &mut self,
        id: LabelId,
        name: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.with_store(
            "Couldn't rename the label",
            async move |store| {
                store.rename_label(id, name).await?;
                store.labels().await
            },
            Self::set_labels,
            window,
            cx,
        );
    }

    fn delete_label(&mut self, id: LabelId, window: &mut Window, cx: &mut Context<Self>) {
        self.with_store(
            "Couldn't delete the label",
            async move |store| {
                store.delete_label(id).await?;
                store.labels().await
            },
            Self::set_labels,
            window,
            cx,
        );
    }

    pub(super) fn on_sidebar_event(
        &mut self,
        event: &SidebarEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match *event {
            SidebarEvent::Select(filter) => {
                // The click landed in the sidebar; keep focus there so the
                // arrow keys move through its entries.
                self.sidebar_focus.focus(window, cx);
                self.select_filter(filter, window, cx);
            }
            SidebarEvent::AddChild => {
                let used: Vec<u32> = self.children.iter().map(|child| child.color).collect();
                let color = suggested_color(&used);
                dialogs::child_dialog(
                    None,
                    color,
                    cx.listener(|this, draft: &ChildDraft, window, cx| {
                        this.add_child(draft.clone(), window, cx)
                    }),
                    window,
                    cx,
                );
            }
            SidebarEvent::EditChild(id) => {
                let Some(child) = self.child(id) else { return };
                dialogs::child_dialog(
                    Some(child),
                    child.color,
                    cx.listener(move |this, draft: &ChildDraft, window, cx| {
                        this.update_child(id, draft.clone(), window, cx)
                    }),
                    window,
                    cx,
                );
            }
            SidebarEvent::ShareChild(id) => self.open_share_dialog(id, window, cx),
            SidebarEvent::Messages => {
                self.sidebar_focus.focus(window, cx);
                self.open_messages(&OpenMessages, window, cx);
            }
            SidebarEvent::DeleteChild(id) => {
                let Some(child) = self.child(id) else { return };
                dialogs::confirm_delete(
                    format!("Delete {}?", child.name),
                    "This removes them from your vault and can't be undone. \
                     Notes about them are kept.",
                    cx.listener(move |this, _: &ClickEvent, window, cx| {
                        this.delete_child(id, window, cx)
                    }),
                    window,
                    cx,
                );
            }
            SidebarEvent::AddLabel => {
                let taken = self.labels.iter().map(|l| l.name.clone()).collect();
                dialogs::label_dialog(
                    None,
                    taken,
                    cx.listener(|this, name: &String, window, cx| {
                        this.add_label(name.clone(), window, cx)
                    }),
                    window,
                    cx,
                );
            }
            SidebarEvent::RenameLabel(id) => {
                let Some(label) = self.label(id) else { return };
                let taken = self
                    .labels
                    .iter()
                    .filter(|l| l.id != id)
                    .map(|l| l.name.clone())
                    .collect();
                dialogs::label_dialog(
                    Some(label),
                    taken,
                    cx.listener(move |this, name: &String, window, cx| {
                        this.rename_label(id, name.clone(), window, cx)
                    }),
                    window,
                    cx,
                );
            }
            SidebarEvent::DeleteLabel(id) => {
                let Some(label) = self.label(id) else { return };
                dialogs::confirm_delete(
                    format!("Delete #{}?", label.name),
                    "This can't be undone. Notes with the label are kept.",
                    cx.listener(move |this, _: &ClickEvent, window, cx| {
                        this.delete_label(id, window, cx)
                    }),
                    window,
                    cx,
                );
            }
        }
    }
}
