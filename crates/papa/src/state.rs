//! What the user has selected: plain Rust, no GPUI, so it is unit-tested
//! without opening a window.

use papa_core::ChildId;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Selection {
    pub child: Option<ChildId>,
}

impl Selection {
    pub fn select_child(&mut self, child: ChildId) {
        self.child = Some(child);
    }

    /// Keeps the selection valid after the list of children changed: if the
    /// selected child is gone (or nothing is selected), selects the first.
    pub fn reconcile(&mut self, children: &[ChildId]) {
        if self.child.is_none_or(|child| !children.contains(&child)) {
            self.child = children.first().copied();
        }
    }

    /// Moves the selection by `delta` within `children`, stopping at either
    /// end. With nothing selected, any move lands on the first child.
    pub fn move_child(&mut self, delta: isize, children: &[ChildId]) {
        let Some(last) = children.len().checked_sub(1) else {
            self.child = None;
            return;
        };
        let next = match self
            .child
            .and_then(|child| children.iter().position(|id| *id == child))
        {
            Some(current) => current.saturating_add_signed(delta).min(last),
            None => 0,
        };
        self.child = Some(children[next]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids() -> [ChildId; 3] {
        [ChildId::new(), ChildId::new(), ChildId::new()]
    }

    fn selected(child: Option<ChildId>) -> Selection {
        Selection { child }
    }

    #[test]
    fn moves_down_and_up() {
        let children = ids();
        let mut selection = selected(Some(children[0]));

        selection.move_child(1, &children);
        assert_eq!(selection.child, Some(children[1]));

        selection.move_child(-1, &children);
        assert_eq!(selection.child, Some(children[0]));
    }

    #[test]
    fn stops_at_both_ends() {
        let children = ids();

        let mut selection = selected(Some(children[0]));
        selection.move_child(-1, &children);
        assert_eq!(selection.child, Some(children[0]));

        let mut selection = selected(Some(children[2]));
        selection.move_child(1, &children);
        assert_eq!(selection.child, Some(children[2]));
    }

    #[test]
    fn without_a_selection_a_move_lands_on_the_first_child() {
        let children = ids();
        let mut selection = selected(None);

        selection.move_child(-1, &children);

        assert_eq!(selection.child, Some(children[0]));
    }

    #[test]
    fn an_empty_list_clears_the_selection() {
        let mut selection = selected(Some(ChildId::new()));
        selection.move_child(1, &[]);
        assert_eq!(selection.child, None);
    }

    #[test]
    fn reconcile_keeps_a_child_that_still_exists() {
        let children = ids();
        let mut selection = selected(Some(children[1]));

        selection.reconcile(&children);

        assert_eq!(selection.child, Some(children[1]));
    }

    #[test]
    fn reconcile_moves_off_a_deleted_child() {
        let children = ids();
        let mut selection = selected(Some(ChildId::new()));

        selection.reconcile(&children);

        assert_eq!(selection.child, Some(children[0]));
    }

    #[test]
    fn reconcile_selects_the_first_child_once_there_is_one() {
        let children = ids();
        let mut selection = selected(None);

        selection.reconcile(&children);
        assert_eq!(selection.child, Some(children[0]));

        selection.reconcile(&[]);
        assert_eq!(selection.child, None);
    }
}
