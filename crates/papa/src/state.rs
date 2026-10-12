//! What the user has selected: plain Rust, no GPUI, so it is unit-tested
//! without opening a window.

use papa_core::{ChildId, LabelId, NoteFilter, NoteId};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Selection {
    /// The sidebar entry: which notes the list shows.
    pub filter: NoteFilter,
    /// The note open in the editor.
    pub note: Option<NoteId>,
}

impl Selection {
    /// Keeps the filter valid after children or labels changed: a filter on
    /// one that's gone falls back to all notes.
    pub fn reconcile_filter(&mut self, children: &[ChildId], labels: &[LabelId]) {
        let exists = match self.filter {
            NoteFilter::All => true,
            NoteFilter::Child(id) => children.contains(&id),
            NoteFilter::Label(id) => labels.contains(&id),
        };
        if !exists {
            self.filter = NoteFilter::All;
        }
    }

    /// Moves the filter by `delta` through the sidebar's `entries`.
    pub fn move_filter(&mut self, delta: isize, entries: &[NoteFilter]) {
        if let Some(filter) = step(Some(self.filter), delta, entries) {
            self.filter = filter;
        }
    }

    /// Keeps the open note if the list still shows it; otherwise opens the
    /// first note, or none if the list is empty.
    pub fn reconcile_note(&mut self, notes: &[NoteId]) {
        if self.note.is_none_or(|note| !notes.contains(&note)) {
            self.note = notes.first().copied();
        }
    }

    /// After the open note `removed` left the list (`notes`, as it was
    /// before), opens the note that took its place, or the one above it if
    /// it was last.
    pub fn after_removing(&mut self, removed: NoteId, notes: &[NoteId]) {
        if self.note != Some(removed) {
            return;
        }
        let index = notes.iter().position(|id| *id == removed);
        self.note = index.and_then(|index| {
            let above = index.checked_sub(1).and_then(|above| notes.get(above));
            notes.get(index + 1).or(above).copied()
        });
    }

    /// Moves to the note `delta` rows away in `notes`.
    pub fn move_note(&mut self, delta: isize, notes: &[NoteId]) {
        self.note = step(self.note, delta, notes);
    }
}

/// The item `delta` places after `current` in `items`, stopping at either
/// end. With no current item (or one not in `items`), lands on the first.
fn step<T: Copy + PartialEq>(current: Option<T>, delta: isize, items: &[T]) -> Option<T> {
    let last = items.len().checked_sub(1)?;
    let next = match current.and_then(|current| items.iter().position(|item| *item == current)) {
        Some(index) => index.saturating_add_signed(delta).min(last),
        None => 0,
    };
    Some(items[next])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note_ids() -> [NoteId; 3] {
        [NoteId::new(), NoteId::new(), NoteId::new()]
    }

    #[test]
    fn step_moves_and_stops_at_both_ends() {
        let items = [1, 2, 3];
        assert_eq!(step(Some(1), 1, &items), Some(2));
        assert_eq!(step(Some(2), -1, &items), Some(1));
        assert_eq!(step(Some(1), -1, &items), Some(1));
        assert_eq!(step(Some(3), 1, &items), Some(3));
    }

    #[test]
    fn step_without_a_current_item_lands_on_the_first() {
        assert_eq!(step(None, -1, &[1, 2]), Some(1));
        assert_eq!(step(Some(9), 1, &[1, 2]), Some(1));
        assert_eq!(step::<i32>(Some(1), 1, &[]), None);
    }

    #[test]
    fn moving_the_filter_walks_the_sidebar() {
        let emma = ChildId::new();
        let funny = LabelId::new();
        let entries = [
            NoteFilter::All,
            NoteFilter::Child(emma),
            NoteFilter::Label(funny),
        ];
        let mut selection = Selection::default();

        selection.move_filter(1, &entries);
        assert_eq!(selection.filter, NoteFilter::Child(emma));
        selection.move_filter(1, &entries);
        selection.move_filter(1, &entries);
        assert_eq!(selection.filter, NoteFilter::Label(funny));
    }

    #[test]
    fn a_filter_on_a_deleted_child_or_label_falls_back_to_all() {
        let emma = ChildId::new();
        let funny = LabelId::new();
        let mut selection = Selection {
            filter: NoteFilter::Child(emma),
            note: None,
        };

        selection.reconcile_filter(&[emma], &[]);
        assert_eq!(selection.filter, NoteFilter::Child(emma));
        selection.reconcile_filter(&[], &[funny]);
        assert_eq!(selection.filter, NoteFilter::All);

        selection.filter = NoteFilter::Label(funny);
        selection.reconcile_filter(&[], &[]);
        assert_eq!(selection.filter, NoteFilter::All);
    }

    #[test]
    fn reconcile_keeps_a_note_that_is_still_listed() {
        let notes = note_ids();
        let mut selection = Selection {
            note: Some(notes[1]),
            ..Selection::default()
        };

        selection.reconcile_note(&notes);

        assert_eq!(selection.note, Some(notes[1]));
    }

    #[test]
    fn reconcile_opens_the_first_note_when_the_open_one_is_not_listed() {
        let notes = note_ids();
        let mut selection = Selection {
            note: Some(NoteId::new()),
            ..Selection::default()
        };

        selection.reconcile_note(&notes);
        assert_eq!(selection.note, Some(notes[0]));

        selection.reconcile_note(&[]);
        assert_eq!(selection.note, None);
    }

    #[test]
    fn removing_the_open_note_opens_its_neighbour() {
        let notes = note_ids();
        let mut selection = Selection {
            note: Some(notes[1]),
            ..Selection::default()
        };

        selection.after_removing(notes[1], &notes);
        assert_eq!(selection.note, Some(notes[2]), "the one below");

        selection.after_removing(notes[2], &notes);
        assert_eq!(selection.note, Some(notes[1]), "the last: the one above");

        selection.after_removing(notes[0], &notes);
        assert_eq!(selection.note, Some(notes[1]), "another note: unchanged");

        selection.after_removing(notes[0], &notes[..1]);
        assert_eq!(selection.note, Some(notes[1]));
        let only = [notes[1]];
        selection.after_removing(notes[1], &only);
        assert_eq!(selection.note, None, "the only one: nothing left");
    }

    #[test]
    fn moving_through_notes() {
        let notes = note_ids();
        let mut selection = Selection::default();

        selection.move_note(1, &notes);
        assert_eq!(selection.note, Some(notes[0]), "starts at the top");
        selection.move_note(1, &notes);
        assert_eq!(selection.note, Some(notes[1]));
        selection.move_note(-5, &notes);
        assert_eq!(selection.note, Some(notes[0]));
    }
}
