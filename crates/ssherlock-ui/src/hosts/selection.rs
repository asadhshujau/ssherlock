//! Which hosts in the list are selected, and where the keyboard cursor is (HOST-13, HOST-14).
//!
//! Plain data over the ids currently shown, so the rules are tested without gpui. The list is
//! a slice of ids in display order; every method takes it, so the selection never points at
//! something that isn't on screen.

use std::collections::BTreeSet;

use ssherlock_core::Id;
use ssherlock_core::hosts::Host;

/// The cursor, the selected hosts, and the anchor a Shift range starts from.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Selection {
    cursor: Option<Id<Host>>,
    selected: BTreeSet<Id<Host>>,
    anchor: Option<Id<Host>>,
}

impl Selection {
    /// The host the cursor is on.
    pub fn cursor(&self) -> Option<Id<Host>> {
        self.cursor
    }

    /// Whether `id` is selected.
    pub fn is_selected(&self, id: Id<Host>) -> bool {
        self.selected.contains(&id)
    }

    /// How many hosts are selected.
    pub fn count(&self) -> usize {
        self.selected.len()
    }

    /// What an action applies to: the selected hosts in list order, or the cursor's host when
    /// nothing is selected.
    pub fn targets(&self, list: &[Id<Host>]) -> Vec<Id<Host>> {
        if self.selected.is_empty() {
            return self.cursor.into_iter().collect();
        }
        list.iter()
            .filter(|id| self.selected.contains(id))
            .copied()
            .collect()
    }

    /// Drops what's no longer in `list` (after a search or a delete) and keeps the cursor on
    /// screen: on the same host if it's still there, else the one now at its old position.
    pub fn retain(&mut self, list: &[Id<Host>], old_position: Option<usize>) {
        self.selected.retain(|id| list.contains(id));
        if self.anchor.is_some_and(|id| !list.contains(&id)) {
            self.anchor = None;
        }
        if self.cursor.is_none_or(|id| !list.contains(&id)) {
            self.cursor = old_position
                .map(|at| at.min(list.len().saturating_sub(1)))
                .and_then(|at| list.get(at))
                .or_else(|| list.first())
                .copied();
        }
    }

    /// Where the cursor is in `list`.
    pub fn position(&self, list: &[Id<Host>]) -> Option<usize> {
        self.cursor
            .and_then(|c| list.iter().position(|&id| id == c))
    }

    /// Moves the cursor by `delta` rows, clamped to the list. With `extend` (Shift held) the
    /// selection becomes the range from the anchor to the new cursor; without it the selection
    /// is cleared, as in a file manager.
    pub fn step(&mut self, list: &[Id<Host>], delta: isize, extend: bool) {
        let Some(end) = list.len().checked_sub(1) else {
            return;
        };
        let from = self.position(list);
        let to = match from {
            Some(at) => at.saturating_add_signed(delta).min(end),
            None if delta < 0 => end,
            None => 0,
        };
        self.move_to(list, to, extend);
    }

    /// Puts the cursor on the first (`end = false`) or last row.
    pub fn jump(&mut self, list: &[Id<Host>], to_end: bool, extend: bool) {
        if let Some(end) = list.len().checked_sub(1) {
            self.move_to(list, if to_end { end } else { 0 }, extend);
        }
    }

    fn move_to(&mut self, list: &[Id<Host>], to: usize, extend: bool) {
        let Some(&target) = list.get(to) else { return };
        if extend {
            let anchor = self.anchor.or(self.cursor).unwrap_or(target);
            self.anchor = Some(anchor);
            self.select_range(list, anchor, target);
        } else {
            self.selected.clear();
            self.anchor = Some(target);
        }
        self.cursor = Some(target);
    }

    fn select_range(&mut self, list: &[Id<Host>], from: Id<Host>, to: Id<Host>) {
        let a = list.iter().position(|&id| id == from);
        let b = list.iter().position(|&id| id == to);
        if let (Some(a), Some(b)) = (a, b) {
            let (lo, hi) = (a.min(b), a.max(b));
            self.selected = list
                .get(lo..=hi)
                .unwrap_or_default()
                .iter()
                .copied()
                .collect();
        }
    }

    /// A plain click: the cursor moves to `id` and the selection is cleared. Actions then
    /// apply to the cursor's host alone, see [`Selection::targets`].
    pub fn click(&mut self, id: Id<Host>) {
        self.selected.clear();
        self.cursor = Some(id);
        self.anchor = Some(id);
    }

    /// Ctrl/Cmd+click or Space: adds `id` to the selection or takes it out. The first toggle
    /// also keeps the host the cursor was on, so Space-Space-Space builds a selection from the
    /// first row onwards.
    pub fn toggle(&mut self, id: Id<Host>) {
        if self.selected.is_empty()
            && let Some(cursor) = self.cursor
            && cursor != id
        {
            self.selected.insert(cursor);
        }
        if !self.selected.remove(&id) {
            self.selected.insert(id);
        }
        self.cursor = Some(id);
        self.anchor = Some(id);
    }

    /// Shift+click: selects from the anchor to `id`.
    pub fn extend_to(&mut self, list: &[Id<Host>], id: Id<Host>) {
        let anchor = self.anchor.or(self.cursor).unwrap_or(id);
        self.anchor = Some(anchor);
        self.select_range(list, anchor, id);
        self.cursor = Some(id);
    }

    /// Selects every host in `list`.
    pub fn select_all(&mut self, list: &[Id<Host>]) {
        self.selected = list.iter().copied().collect();
        if self.cursor.is_none() {
            self.cursor = list.first().copied();
        }
    }

    /// Clears the selection, keeping the cursor.
    pub fn clear(&mut self) {
        self.selected.clear();
        self.anchor = self.cursor;
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::indexing_slicing)]

    use pretty_assertions::assert_eq;

    use super::*;

    fn ids(n: usize) -> Vec<Id<Host>> {
        (0..n).map(|_| Id::generate()).collect()
    }

    /// The selection as indices into `list`, plus the cursor's index.
    fn shape(selection: &Selection, list: &[Id<Host>]) -> (Vec<usize>, Option<usize>) {
        let selected = (0..list.len())
            .filter(|&i| selection.is_selected(list[i]))
            .collect();
        (selected, selection.position(list))
    }

    #[test]
    fn arrows_move_the_cursor_and_clamp() {
        let list = ids(3);
        let mut s = Selection::default();
        s.step(&list, 1, false);
        assert_eq!(
            shape(&s, &list),
            (vec![], Some(0)),
            "first press lands on the first row"
        );
        s.step(&list, 1, false);
        s.step(&list, 5, false);
        assert_eq!(shape(&s, &list), (vec![], Some(2)));
        s.step(&list, -10, false);
        assert_eq!(shape(&s, &list), (vec![], Some(0)));
        let mut fresh = Selection::default();
        fresh.step(&list, -1, false);
        assert_eq!(
            fresh.position(&list),
            Some(2),
            "Up with no cursor starts at the end"
        );
        s.jump(&list, true, false);
        assert_eq!(s.position(&list), Some(2));
        Selection::default().step(&[], 1, false);
    }

    #[test]
    fn shift_arrows_select_a_range_from_the_anchor() {
        let list = ids(5);
        let mut s = Selection::default();
        s.click(list[1]);
        s.step(&list, 1, true);
        s.step(&list, 1, true);
        assert_eq!(shape(&s, &list), (vec![1, 2, 3], Some(3)));
        s.step(&list, -3, true);
        assert_eq!(
            shape(&s, &list),
            (vec![0, 1], Some(0)),
            "back past the anchor"
        );
        s.step(&list, 1, false);
        assert_eq!(shape(&s, &list), (vec![], Some(1)), "a plain arrow clears");
        s.jump(&list, true, true);
        assert_eq!(shape(&s, &list), (vec![1, 2, 3, 4], Some(4)));
    }

    #[test]
    fn toggling_and_clicking() {
        let list = ids(4);
        let mut s = Selection::default();
        s.click(list[0]);
        s.toggle(list[2]);
        assert_eq!(
            shape(&s, &list),
            (vec![0, 2], Some(2)),
            "the first toggle keeps the cursor's host"
        );
        s.toggle(list[0]);
        assert_eq!(shape(&s, &list), (vec![2], Some(0)));
        s.extend_to(&list, list[3]);
        assert_eq!(shape(&s, &list), (vec![0, 1, 2, 3], Some(3)));
        s.click(list[1]);
        assert_eq!(shape(&s, &list), (vec![], Some(1)));
        s.select_all(&list);
        assert_eq!(shape(&s, &list).0, vec![0, 1, 2, 3]);
        s.clear();
        assert_eq!(shape(&s, &list), (vec![], Some(1)));
    }

    #[test]
    fn targets_are_the_selection_in_list_order_else_the_cursor() {
        let list = ids(3);
        let mut s = Selection::default();
        assert_eq!(s.targets(&list), vec![]);
        s.click(list[1]);
        assert_eq!(s.targets(&list), vec![list[1]]);
        s.toggle(list[2]);
        s.toggle(list[0]);
        assert_eq!(s.targets(&list), vec![list[0], list[1], list[2]]);
    }

    #[test]
    fn retain_keeps_the_cursor_on_screen() {
        let list = ids(4);
        let mut s = Selection::default();
        s.click(list[2]);
        s.toggle(list[3]);
        // The cursor's host (list[3]) and list[2] vanish, e.g. moved to the trash.
        let shorter = vec![list[0], list[1]];
        let old = s.position(&list);
        s.retain(&shorter, old);
        assert_eq!(
            shape(&s, &shorter),
            (vec![], Some(1)),
            "the row now at the end"
        );
        s.retain(&[], Some(0));
        assert_eq!(s.cursor(), None);
        // Still there: the cursor stays put.
        let mut kept = Selection::default();
        kept.click(list[1]);
        kept.retain(&[list[1], list[0]], Some(0));
        assert_eq!(kept.cursor(), Some(list[1]));
    }
}
