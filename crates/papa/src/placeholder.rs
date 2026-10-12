//! Invented sample data for the app shell.
//!
//! Phase 2 replaces this with `papa-core::Store`. Until then the UI renders
//! these fixed children, labels and notes. Every name here is made up: no real
//! family data ever goes into the repository.

/// Identifies a child. A newtype rather than a bare `u32`, so the compiler
/// rejects a `NoteId` where a `ChildId` is expected.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ChildId(pub u32);

/// Identifies a note.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct NoteId(pub u32);

#[derive(Clone)]
pub struct Child {
    pub id: ChildId,
    pub name: &'static str,
    /// Accent colour as `0xRRGGBB`, shown as a dot next to the name.
    pub color: u32,
}

#[derive(Clone)]
pub struct Note {
    pub id: NoteId,
    pub title: &'static str,
    pub date: &'static str,
    pub body: &'static str,
    pub labels: &'static [&'static str],
    /// A note can be about several children.
    pub children: &'static [ChildId],
}

/// Everything the shell displays.
pub struct Journal {
    pub children: Vec<Child>,
    pub labels: Vec<&'static str>,
    pub notes: Vec<Note>,
}

impl Journal {
    pub fn sample() -> Self {
        const EMMA: ChildId = ChildId(1);
        const LEO: ChildId = ChildId(2);

        Self {
            children: vec![
                Child {
                    id: EMMA,
                    name: "Emma",
                    color: 0xe0795b,
                },
                Child {
                    id: LEO,
                    name: "Leo",
                    color: 0x5b9be0,
                },
            ],
            labels: vec!["milestone", "funny", "school"],
            notes: vec![
                Note {
                    id: NoteId(1),
                    title: "First bike ride",
                    date: "Mar 3, 2026",
                    body: "She refused the training wheels. Two laps of the park, \
                           one scraped knee, and a grin that lasted all evening.\n\n\
                           At dinner she asked whether bikes can go to school.",
                    labels: &["milestone"],
                    children: &[EMMA],
                },
                Note {
                    id: NoteId(2),
                    title: "School play",
                    date: "Feb 18, 2026",
                    body: "Cast as Tree Number Three. Took the role very seriously \
                           and did not move once, not even for the applause.",
                    labels: &["school"],
                    children: &[EMMA],
                },
                Note {
                    id: NoteId(3),
                    title: "Pancake disaster",
                    date: "Jan 25, 2026",
                    body: "Both of them insisted on flipping. The ceiling now has \
                           a small pancake-shaped mark we have agreed not to mention.",
                    labels: &["funny"],
                    children: &[EMMA, LEO],
                },
                Note {
                    id: NoteId(4),
                    title: "Lost the first tooth",
                    date: "Dec 9, 2025",
                    body: "Wobbled it for a week, then lost it in an apple. \
                           Negotiated hard with the tooth fairy.",
                    labels: &["milestone", "funny"],
                    children: &[LEO],
                },
                Note {
                    id: NoteId(5),
                    title: "Counting to a hundred",
                    date: "Nov 2, 2025",
                    body: "Counted all the way to one hundred in the car, skipping \
                           only seventy-something, which he says doesn't count.",
                    labels: &["milestone", "school"],
                    children: &[LEO],
                },
            ],
        }
    }

    pub fn child(&self, id: ChildId) -> Option<&Child> {
        self.children.iter().find(|child| child.id == id)
    }

    pub fn note(&self, id: NoteId) -> Option<&Note> {
        self.notes.iter().find(|note| note.id == id)
    }

    /// Notes about `child`, newest first (the sample data is already in that order).
    pub fn notes_for(&self, child: ChildId) -> impl Iterator<Item = &Note> {
        self.notes
            .iter()
            .filter(move |note| note.children.contains(&child))
    }

    /// Ids of the notes about `child`, in display order.
    pub fn note_ids_for(&self, child: ChildId) -> Vec<NoteId> {
        self.notes_for(child).map(|note| note.id).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_note_about_two_children_is_listed_for_both() {
        let journal = Journal::sample();
        let pancakes = NoteId(3);

        for child in &journal.children {
            assert!(journal.note_ids_for(child.id).contains(&pancakes));
        }
    }

    #[test]
    fn notes_for_a_child_exclude_other_childrens_notes() {
        let journal = Journal::sample();

        assert_eq!(
            journal.note_ids_for(ChildId(1)),
            [NoteId(1), NoteId(2), NoteId(3)]
        );
        assert_eq!(
            journal.note_ids_for(ChildId(2)),
            [NoteId(3), NoteId(4), NoteId(5)]
        );
    }
}
