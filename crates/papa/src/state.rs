//! What the user has selected: plain Rust, no GPUI, so it is unit-tested
//! without opening a window.

use crate::placeholder::{ChildId, Journal, NoteId};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Selection {
    pub child: Option<ChildId>,
    pub note: Option<NoteId>,
}

impl Selection {
    /// Starts on the first child and that child's newest note.
    pub fn first(journal: &Journal) -> Self {
        let mut selection = Self {
            child: None,
            note: None,
        };
        if let Some(child) = journal.children.first() {
            selection.select_child(child.id, journal);
        }
        selection
    }

    /// Switches child and selects the newest note about them.
    pub fn select_child(&mut self, child: ChildId, journal: &Journal) {
        self.child = Some(child);
        self.note = journal.notes_for(child).next().map(|note| note.id);
    }

    pub fn select_note(&mut self, note: NoteId) {
        self.note = Some(note);
    }

    /// Moves the note selection by `delta` within `visible`, stopping at
    /// either end. With nothing selected, any move lands on the first note.
    pub fn move_note(&mut self, delta: isize, visible: &[NoteId]) {
        let Some(last) = visible.len().checked_sub(1) else {
            self.note = None;
            return;
        };
        let next = match self
            .note
            .and_then(|note| visible.iter().position(|id| *id == note))
        {
            Some(current) => current.saturating_add_signed(delta).min(last),
            None => 0,
        };
        self.note = Some(visible[next]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOTES: [NoteId; 3] = [NoteId(10), NoteId(20), NoteId(30)];

    fn selected(note: Option<NoteId>) -> Selection {
        Selection {
            child: Some(ChildId(1)),
            note,
        }
    }

    #[test]
    fn starts_on_first_child_and_newest_note() {
        let journal = Journal::sample();
        let selection = Selection::first(&journal);

        assert_eq!(selection.child, Some(ChildId(1)));
        assert_eq!(selection.note, Some(NoteId(1)));
    }

    #[test]
    fn switching_child_selects_their_newest_note() {
        let journal = Journal::sample();
        let mut selection = Selection::first(&journal);

        selection.select_child(ChildId(2), &journal);

        assert_eq!(selection.child, Some(ChildId(2)));
        assert_eq!(selection.note, Some(NoteId(3)));
    }

    #[test]
    fn moves_down_and_up() {
        let mut selection = selected(Some(NoteId(10)));

        selection.move_note(1, &NOTES);
        assert_eq!(selection.note, Some(NoteId(20)));

        selection.move_note(-1, &NOTES);
        assert_eq!(selection.note, Some(NoteId(10)));
    }

    #[test]
    fn stops_at_both_ends() {
        let mut selection = selected(Some(NoteId(10)));
        selection.move_note(-1, &NOTES);
        assert_eq!(selection.note, Some(NoteId(10)));

        let mut selection = selected(Some(NoteId(30)));
        selection.move_note(1, &NOTES);
        assert_eq!(selection.note, Some(NoteId(30)));
    }

    #[test]
    fn without_a_selection_lands_on_first_note() {
        let mut selection = selected(None);
        selection.move_note(-1, &NOTES);
        assert_eq!(selection.note, Some(NoteId(10)));
    }

    #[test]
    fn empty_list_clears_selection() {
        let mut selection = selected(Some(NoteId(10)));
        selection.move_note(1, &[]);
        assert_eq!(selection.note, None);
    }
}
