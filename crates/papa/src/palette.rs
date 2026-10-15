//! What the command palette offers: plain Rust, no GPUI, so it is
//! unit-tested without opening a window. `ui::CommandPalette` draws it.
//!
//! The palette is built from a snapshot of the app's state each time it
//! opens. It is closed again before anything it runs changes that state, so
//! it never has to follow changes while open.

use papa_core::{Child, ChildId, Label, LabelId, NoteFilter, NoteId, NoteSummary};

/// Something the palette can do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PaletteCommand {
    /// What a keyboard shortcut does. The palette dispatches the shortcut's
    /// action, so both run the same handler, and shows the shortcut.
    Shortcut(Shortcut),
    AddChild,
    AddLabel,
    DeleteNote,
    /// Show these notes in the list.
    Show(NoteFilter),
    /// Open this note.
    Open(NoteId),
    /// Assign the open note to this child, or unassign it.
    ToggleChild(ChildId),
    /// Add this label to the open note, or remove it.
    ToggleLabel(LabelId),
    /// Let this child read the open note, or stop.
    ToggleShared(ChildId),
    /// Open this child's sharing dialog.
    ShareChild(ChildId),
    SignIn,
    SignOut,
    SyncNow,
    /// Turn the Vim mode setting on or off.
    ToggleVimMode,
    /// Set a password, or change it.
    ChangePassword,
    RemovePassword,
    ExportBackup,
    RestoreBackup,
}

/// The commands that have a keyboard shortcut. `ui::command_palette` maps
/// each to its action; a `match` there makes a new one a compile error
/// until it has one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shortcut {
    NewNote,
    FocusSearch,
    TogglePreview,
    AddImages,
    FocusSidebar,
    FocusNoteList,
    FocusEditor,
    ExportNote,
    ExportNotes,
    LockVault,
    ToggleSidebar,
    ToggleTheme,
    OpenSettings,
    Quit,
}

/// One row of the palette.
#[derive(Clone, Debug, PartialEq)]
pub struct PaletteItem {
    pub label: String,
    /// Other words the search matches, besides the label.
    pub keywords: Vec<String>,
    pub command: PaletteCommand,
    /// Drawn with a check: the current filter, or a child or label the open
    /// note has.
    pub checked: bool,
    /// Shown greyed out and skipped by the arrow keys when `false`.
    pub enabled: bool,
}

impl PaletteItem {
    fn new(label: impl Into<String>, command: PaletteCommand) -> Self {
        Self {
            label: label.into(),
            keywords: Vec::new(),
            command,
            checked: false,
            enabled: true,
        }
    }

    fn keywords<const N: usize>(mut self, keywords: [&str; N]) -> Self {
        self.keywords.extend(keywords.map(str::to_owned));
        self
    }

    fn checked(mut self, checked: bool) -> Self {
        self.checked = checked;
        self
    }

    fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }
}

/// A titled section of the palette.
#[derive(Clone, Debug, PartialEq)]
pub struct PaletteGroup {
    pub heading: &'static str,
    pub items: Vec<PaletteItem>,
}

/// The open note's links, for the "Assign" group.
#[derive(Clone, Copy, Debug)]
pub struct OpenNote<'a> {
    pub children: &'a [ChildId],
    pub labels: &'a [LabelId],
    /// The children who can read it.
    pub shared_with: &'a [ChildId],
}

/// The app's state the palette is built from.
#[derive(Clone, Copy, Debug)]
pub struct PaletteData<'a> {
    pub vault_open: bool,
    pub children: &'a [Child],
    pub labels: &'a [Label],
    /// Every note in the vault, newest first.
    pub notes: &'a [NoteSummary],
    pub open_note: Option<OpenNote<'a>>,
    /// What the note list shows now.
    pub filter: NoteFilter,
    /// The Vim mode setting.
    pub vim_mode: bool,
    /// The vault has a password.
    pub encrypted: bool,
    /// What exporting the list makes ("Emma’s book", "3 notes"), or `None`
    /// when the list is empty.
    pub list_export: Option<&'a str>,
    /// Signed in to a Papa account.
    pub signed_in: bool,
    /// The children whose sharing is on.
    pub shared_children: &'a [ChildId],
}

/// The palette's groups, top to bottom: commands first, the long list of
/// notes last. The groups for the open note appear only while one is open.
pub fn groups(data: &PaletteData) -> Vec<PaletteGroup> {
    let mut groups = vec![actions(data), go_to(data)];
    if let Some(note) = data.open_note {
        groups.push(assign(data, note));
    }
    groups.push(sharing(data));
    groups.push(notes(data));
    groups.retain(|group| !group.items.is_empty());
    groups
}

fn actions(data: &PaletteData) -> PaletteGroup {
    use PaletteCommand as C;
    use Shortcut as S;

    let open = data.vault_open;
    let note = data.open_note.is_some();
    let items = vec![
        PaletteItem::new("New note", C::Shortcut(S::NewNote))
            .keywords(["write", "add", "create"])
            .enabled(open),
        PaletteItem::new("Search notes", C::Shortcut(S::FocusSearch))
            .keywords(["find", "filter"])
            .enabled(open),
        PaletteItem::new(
            "Switch between Edit and Preview",
            C::Shortcut(S::TogglePreview),
        )
        .keywords(["mode", "markdown", "read"])
        .enabled(note),
        PaletteItem::new("Add images…", C::Shortcut(S::AddImages))
            .keywords(["photo", "picture", "attach"])
            .enabled(note),
        PaletteItem::new("Export note to PDF…", C::Shortcut(S::ExportNote))
            .keywords(["pdf", "print", "save", "share"])
            .enabled(note),
        PaletteItem::new(
            format!("Export {} to PDF…", data.list_export.unwrap_or("the list")),
            C::Shortcut(S::ExportNotes),
        )
        .keywords(["pdf", "print", "book", "save", "all"])
        .enabled(open && data.list_export.is_some()),
        PaletteItem::new("Delete note…", C::DeleteNote)
            .keywords(["remove", "trash"])
            .enabled(note),
        PaletteItem::new("Add child…", C::AddChild)
            .keywords(["new", "son", "daughter", "kid"])
            .enabled(open),
        PaletteItem::new("Add label…", C::AddLabel)
            .keywords(["new", "tag"])
            .enabled(open),
        PaletteItem::new("Focus the sidebar", C::Shortcut(S::FocusSidebar))
            .keywords(["pane", "children", "labels", "keyboard"]),
        PaletteItem::new("Focus the note list", C::Shortcut(S::FocusNoteList))
            .keywords(["pane", "keyboard"]),
        PaletteItem::new("Focus the note", C::Shortcut(S::FocusEditor))
            .keywords(["pane", "editor", "keyboard"])
            .enabled(note),
        PaletteItem::new("Show or hide the sidebar", C::Shortcut(S::ToggleSidebar))
            .keywords(["toggle", "panel"]),
        PaletteItem::new("Switch light and dark", C::Shortcut(S::ToggleTheme))
            .keywords(["theme", "mode", "dark", "light", "toggle"]),
        PaletteItem::new("Vim mode", C::ToggleVimMode)
            .keywords(["vi", "modal", "keys", "editor", "toggle"])
            .checked(data.vim_mode),
        PaletteItem::new("Lock Papa", C::Shortcut(S::LockVault))
            .keywords(["password", "close", "privacy"])
            .enabled(open && data.encrypted),
        PaletteItem::new(
            if data.encrypted {
                "Change the password…"
            } else {
                "Set a password…"
            },
            C::ChangePassword,
        )
        .keywords(["encrypt", "lock", "security", "privacy"])
        .enabled(open),
        PaletteItem::new("Remove the password…", C::RemovePassword)
            .keywords(["decrypt", "security", "privacy"])
            .enabled(open && data.encrypted),
        PaletteItem::new("Back up the vault…", C::ExportBackup)
            .keywords(["backup", "export", "copy", "save"])
            .enabled(open),
        PaletteItem::new("Restore a backup…", C::RestoreBackup)
            .keywords(["backup", "import", "recover", "replace"]),
        PaletteItem::new("Settings…", C::Shortcut(S::OpenSettings)).keywords([
            "preferences",
            "options",
            "font",
            "theme",
        ]),
        PaletteItem::new("Quit Papa", C::Shortcut(S::Quit)).keywords(["exit", "close"]),
    ];
    PaletteGroup {
        heading: "Actions",
        items,
    }
}

fn go_to(data: &PaletteData) -> PaletteGroup {
    let all = PaletteItem::new("All notes", PaletteCommand::Show(NoteFilter::All))
        .keywords(["everything", "show"])
        .checked(data.filter == NoteFilter::All);
    let children = data.children.iter().map(|child| {
        let filter = NoteFilter::Child(child.id);
        PaletteItem::new(child.name.clone(), PaletteCommand::Show(filter))
            .keywords(["child", "show"])
            .checked(data.filter == filter)
    });
    let labels = data.labels.iter().map(|label| {
        let filter = NoteFilter::Label(label.id);
        PaletteItem::new(format!("#{}", label.name), PaletteCommand::Show(filter))
            .keywords(["label", "show"])
            .checked(data.filter == filter)
    });
    // Before the vault opens there's nothing to go to.
    let items = if data.vault_open {
        std::iter::once(all).chain(children).chain(labels).collect()
    } else {
        Vec::new()
    };
    PaletteGroup {
        heading: "Go to",
        items,
    }
}

fn assign(data: &PaletteData, note: OpenNote) -> PaletteGroup {
    let children = data.children.iter().map(|child| {
        PaletteItem::new(
            format!("About {}", child.name),
            PaletteCommand::ToggleChild(child.id),
        )
        .keywords(["assign", "unassign", "child"])
        .checked(note.children.contains(&child.id))
    });
    let labels = data.labels.iter().map(|label| {
        PaletteItem::new(
            format!("Labelled #{}", label.name),
            PaletteCommand::ToggleLabel(label.id),
        )
        .keywords(["assign", "unassign", "label", "tag"])
        .checked(note.labels.contains(&label.id))
    });
    // Only children it's about, whose sharing is on, can be given it.
    let shared = data
        .children
        .iter()
        .filter(|child| note.children.contains(&child.id))
        .filter(|child| data.shared_children.contains(&child.id))
        .map(|child| {
            PaletteItem::new(
                format!("{} can read it", child.name),
                PaletteCommand::ToggleShared(child.id),
            )
            .keywords(["share", "unshare", "child", "read"])
            .checked(note.shared_with.contains(&child.id))
        });
    PaletteGroup {
        heading: "This note",
        items: children.chain(shared).chain(labels).collect(),
    }
}

fn sharing(data: &PaletteData) -> PaletteGroup {
    let open = data.vault_open;
    let children = data.children.iter().map(|child| {
        PaletteItem::new(
            format!("Sharing with {}…", child.name),
            PaletteCommand::ShareChild(child.id),
        )
        .keywords(["share", "access key", "child", "cloud"])
        .checked(data.shared_children.contains(&child.id))
        .enabled(open)
    });
    let account = if data.signed_in {
        PaletteItem::new("Sign out of Papa", PaletteCommand::SignOut)
            .keywords(["account", "log out", "server"])
    } else {
        PaletteItem::new("Sign in to Papa…", PaletteCommand::SignIn)
            .keywords(["account", "log in", "server", "share"])
    };
    let sync = PaletteItem::new("Sync shared notes now", PaletteCommand::SyncNow)
        .keywords(["share", "upload", "cloud"])
        .enabled(open && data.signed_in && !data.shared_children.is_empty());
    let items = if open {
        children.chain([account.enabled(open), sync]).collect()
    } else {
        Vec::new()
    };
    PaletteGroup {
        heading: "Sharing",
        items,
    }
}

fn notes(data: &PaletteData) -> PaletteGroup {
    let items = data
        .notes
        .iter()
        .map(|note| {
            let title = if note.title.is_empty() {
                "Untitled".to_owned()
            } else {
                note.title.clone()
            };
            // The start of the text too, so a note without a title can be
            // found by what it says.
            let mut item = PaletteItem::new(title, PaletteCommand::Open(note.id));
            if !note.preview.is_empty() {
                item.keywords.push(note.preview.clone());
            }
            item
        })
        .collect();
    PaletteGroup {
        heading: "Notes",
        items,
    }
}

#[cfg(test)]
mod tests {
    use jiff::Timestamp;

    use super::*;

    fn child(name: &str) -> Child {
        Child {
            id: ChildId::new(),
            name: name.to_owned(),
            color: 0,
        }
    }

    fn label(name: &str) -> Label {
        Label {
            id: LabelId::new(),
            name: name.to_owned(),
        }
    }

    fn note(title: &str, preview: &str) -> NoteSummary {
        NoteSummary {
            id: NoteId::new(),
            title: title.to_owned(),
            preview: preview.to_owned(),
            children: Vec::new(),
            labels: Vec::new(),
            images: 0,
            created_at: Timestamp::now(),
        }
    }

    fn data<'a>(
        children: &'a [Child],
        labels: &'a [Label],
        notes: &'a [NoteSummary],
    ) -> PaletteData<'a> {
        PaletteData {
            vault_open: true,
            children,
            labels,
            notes,
            open_note: None,
            filter: NoteFilter::All,
            vim_mode: false,
            encrypted: false,
            list_export: Some("2 notes"),
            signed_in: true,
            shared_children: &[],
        }
    }

    fn headings(groups: &[PaletteGroup]) -> Vec<&'static str> {
        groups.iter().map(|group| group.heading).collect()
    }

    fn find(groups: &[PaletteGroup], command: PaletteCommand) -> &PaletteItem {
        groups
            .iter()
            .flat_map(|group| &group.items)
            .find(|item| item.command == command)
            .unwrap_or_else(|| panic!("no item for {command:?}"))
    }

    #[test]
    fn commands_come_first_and_notes_last() {
        let notes = [note("First bike ride", "")];
        let groups = groups(&data(&[], &[], &notes));
        assert_eq!(headings(&groups), ["Actions", "Go to", "Sharing", "Notes"]);
    }

    #[test]
    fn the_open_notes_group_appears_only_with_a_note_open() {
        let children = [child("Emma")];
        let mut data = data(&children, &[], &[]);
        assert!(!headings(&groups(&data)).contains(&"This note"));

        data.open_note = Some(OpenNote {
            children: &[],
            labels: &[],
            shared_with: &[],
        });
        assert_eq!(
            headings(&groups(&data)),
            ["Actions", "Go to", "This note", "Sharing"]
        );
    }

    #[test]
    fn before_the_vault_opens_only_window_commands_work() {
        let mut data = data(&[], &[], &[]);
        data.vault_open = false;
        let groups = groups(&data);

        assert_eq!(headings(&groups), ["Actions"]);
        let new_note = PaletteCommand::Shortcut(Shortcut::NewNote);
        assert!(!find(&groups, new_note).enabled);
        assert!(!find(&groups, PaletteCommand::AddChild).enabled);
        let theme = PaletteCommand::Shortcut(Shortcut::ToggleTheme);
        assert!(find(&groups, theme).enabled);
    }

    #[test]
    fn note_commands_need_an_open_note() {
        let mut data = data(&[], &[], &[]);
        assert!(!find(&groups(&data), PaletteCommand::DeleteNote).enabled);

        data.open_note = Some(OpenNote {
            children: &[],
            labels: &[],
            shared_with: &[],
        });
        assert!(find(&groups(&data), PaletteCommand::DeleteNote).enabled);
    }

    #[test]
    fn the_current_filter_is_checked() {
        let children = [child("Emma"), child("Leo")];
        let labels = [label("funny")];
        let mut data = data(&children, &labels, &[]);
        data.filter = NoteFilter::Child(children[1].id);
        let groups = groups(&data);

        let checked: Vec<&str> = groups[1]
            .items
            .iter()
            .filter(|item| item.checked)
            .map(|item| item.label.as_str())
            .collect();
        assert_eq!(checked, ["Leo"]);
        let labels: Vec<&str> = groups[1].items.iter().map(|i| i.label.as_str()).collect();
        assert_eq!(labels, ["All notes", "Emma", "Leo", "#funny"]);
    }

    #[test]
    fn the_open_notes_children_and_labels_are_checked() {
        let children = [child("Emma"), child("Leo")];
        let labels = [label("funny"), label("school")];
        let mut data = data(&children, &labels, &[]);
        let assigned = [children[0].id];
        let labelled = [labels[1].id];
        data.open_note = Some(OpenNote {
            children: &assigned,
            labels: &labelled,
            shared_with: &[],
        });
        let groups = groups(&data);

        let emma = find(&groups, PaletteCommand::ToggleChild(children[0].id));
        assert_eq!(emma.label, "About Emma");
        assert!(emma.checked);
        assert!(!find(&groups, PaletteCommand::ToggleChild(children[1].id)).checked);
        assert!(!find(&groups, PaletteCommand::ToggleLabel(labels[0].id)).checked);
        let school = find(&groups, PaletteCommand::ToggleLabel(labels[1].id));
        assert_eq!(school.label, "Labelled #school");
        assert!(school.checked);
    }

    #[test]
    fn a_note_can_be_shared_only_with_shared_children_it_is_about() {
        let children = [child("Emma"), child("Leo"), child("Ada")];
        let mut data = data(&children, &[], &[]);
        // Emma and Leo's sharing is on; the note is about Emma and Ada,
        // and Emma can read it.
        let shared_children = [children[0].id, children[1].id];
        data.shared_children = &shared_children;
        let about = [children[0].id, children[2].id];
        let shared_with = [children[0].id];
        data.open_note = Some(OpenNote {
            children: &about,
            labels: &[],
            shared_with: &shared_with,
        });
        let groups = groups(&data);

        let shareable: Vec<(&str, bool)> = groups
            .iter()
            .flat_map(|group| &group.items)
            .filter(|item| matches!(item.command, PaletteCommand::ToggleShared(_)))
            .map(|item| (item.label.as_str(), item.checked))
            .collect();
        assert_eq!(shareable, [("Emma can read it", true)]);

        let leo = find(&groups, PaletteCommand::ShareChild(children[1].id));
        assert_eq!(leo.label, "Sharing with Leo…");
        assert!(leo.checked, "Leo's sharing is on");
        assert!(!find(&groups, PaletteCommand::ShareChild(children[2].id)).checked);
        assert!(find(&groups, PaletteCommand::SyncNow).enabled);
    }

    #[test]
    fn signing_in_or_out_follows_the_account() {
        let mut data = data(&[], &[], &[]);
        assert!(
            !find(&groups(&data), PaletteCommand::SyncNow).enabled,
            "nothing shared"
        );
        find(&groups(&data), PaletteCommand::SignOut);

        data.signed_in = false;
        let groups = groups(&data);
        find(&groups, PaletteCommand::SignIn);
        assert!(!find(&groups, PaletteCommand::SyncNow).enabled);
    }

    #[test]
    fn notes_are_found_by_title_or_by_their_text() {
        let notes = [
            note("", "She laughed all the way home"),
            note("Recital", ""),
        ];
        let groups = groups(&data(&[], &[], &notes));

        let untitled = find(&groups, PaletteCommand::Open(notes[0].id));
        assert_eq!(untitled.label, "Untitled");
        assert_eq!(untitled.keywords, ["She laughed all the way home"]);
        let recital = find(&groups, PaletteCommand::Open(notes[1].id));
        assert_eq!(recital.label, "Recital");
        assert!(recital.keywords.is_empty());
    }
}
