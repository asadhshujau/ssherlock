//! The order of a pane's tabs and which one is active, as plain data so it is tested without
//! gpui.

/// An ordered list of tabs with at most one active.
///
/// Invariant: `active` is `Some` exactly when the list isn't empty, and always in range.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TabList<T> {
    tabs: Vec<T>,
    active: Option<usize>,
}

impl<T> Default for TabList<T> {
    fn default() -> Self {
        Self {
            tabs: Vec::new(),
            active: None,
        }
    }
}

impl<T> TabList<T> {
    /// Adds a tab after the active one (or at the end of an empty list) and activates it.
    pub fn open(&mut self, tab: T) {
        let index = self.active.map_or(self.tabs.len(), |active| active + 1);
        self.tabs.insert(index, tab);
        self.active = Some(index);
    }

    /// Removes the tab at `index`. Closing the active tab activates its right neighbour, or the
    /// left one if it was last.
    pub fn close(&mut self, index: usize) -> Option<T> {
        if index >= self.tabs.len() {
            return None;
        }
        let tab = self.tabs.remove(index);
        self.active = match self.active {
            _ if self.tabs.is_empty() => None,
            Some(active) if index < active => Some(active - 1),
            Some(active) => Some(active.min(self.tabs.len() - 1)),
            None => None,
        };
        Some(tab)
    }

    /// Activates the tab at `index`, if there is one.
    pub fn activate(&mut self, index: usize) {
        if index < self.tabs.len() {
            self.active = Some(index);
        }
    }

    /// Activates the next tab, wrapping from the last to the first.
    pub fn activate_next(&mut self) {
        if let Some(active) = self.active {
            self.active = Some((active + 1) % self.tabs.len());
        }
    }

    /// Activates the previous tab, wrapping from the first to the last.
    pub fn activate_previous(&mut self) {
        if let Some(active) = self.active {
            self.active = Some(active.checked_sub(1).unwrap_or(self.tabs.len() - 1));
        }
    }

    /// Moves the tab at `from` to position `to` (clamped to the list), keeping the same tab
    /// active.
    pub fn move_tab(&mut self, from: usize, to: usize) {
        if from >= self.tabs.len() {
            return;
        }
        let to = to.min(self.tabs.len() - 1);
        let active_tab = self.active;
        let tab = self.tabs.remove(from);
        self.tabs.insert(to, tab);
        self.active = active_tab.map(|active| {
            if active == from {
                to
            } else if from < active && active <= to {
                active - 1
            } else if to <= active && active < from {
                active + 1
            } else {
                active
            }
        });
    }

    /// Moves the active tab one place left (`-1`) or right (`1`); does nothing at the ends.
    pub fn move_active(&mut self, step: Step) {
        let Some(active) = self.active else {
            return;
        };
        let target = match step {
            Step::Left => active.checked_sub(1),
            Step::Right => Some(active + 1).filter(|&to| to < self.tabs.len()),
        };
        if let Some(to) = target {
            self.move_tab(active, to);
        }
    }

    /// The active tab's index.
    pub fn active_index(&self) -> Option<usize> {
        self.active
    }

    /// The active tab.
    pub fn active(&self) -> Option<&T> {
        self.tabs.get(self.active?)
    }

    /// The tab at `index`.
    pub fn get(&self, index: usize) -> Option<&T> {
        self.tabs.get(index)
    }

    /// The tab at `index`, mutably.
    pub fn get_mut(&mut self, index: usize) -> Option<&mut T> {
        self.tabs.get_mut(index)
    }

    /// The index of the first tab matching `predicate`.
    pub fn position(&self, predicate: impl FnMut(&T) -> bool) -> Option<usize> {
        self.tabs.iter().position(predicate)
    }

    /// The tabs in order.
    pub fn iter(&self) -> impl ExactSizeIterator<Item = &T> {
        self.tabs.iter()
    }

    /// Number of tabs.
    pub fn len(&self) -> usize {
        self.tabs.len()
    }

    /// Whether there are no tabs.
    pub fn is_empty(&self) -> bool {
        self.tabs.is_empty()
    }
}

/// A direction to move a tab in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    /// Towards the start.
    Left,
    /// Towards the end.
    Right,
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    /// A list of `n` tabs named 'a', 'b', … with `active` active.
    fn list(n: u8, active: usize) -> TabList<char> {
        let mut tabs = TabList::default();
        for i in 0..n {
            tabs.tabs.push(char::from(b'a' + i));
        }
        tabs.active = (n > 0).then_some(active);
        tabs
    }

    fn state(tabs: &TabList<char>) -> (String, Option<usize>) {
        (tabs.iter().collect(), tabs.active_index())
    }

    #[test]
    fn open_inserts_after_the_active_tab() {
        let mut tabs = TabList::default();
        tabs.open('a');
        assert_eq!(state(&tabs), ("a".into(), Some(0)));
        tabs.open('b');
        tabs.activate(0);
        tabs.open('c');
        assert_eq!(state(&tabs), ("acb".into(), Some(1)));
    }

    #[test]
    fn close_picks_a_neighbour() {
        let cases = [
            (
                "active middle → right neighbour",
                list(3, 1),
                1,
                ("ac", Some(1)),
            ),
            (
                "active last → left neighbour",
                list(3, 2),
                2,
                ("ab", Some(1)),
            ),
            ("before active shifts it", list(3, 2), 0, ("bc", Some(1))),
            ("after active keeps it", list(3, 0), 2, ("ab", Some(0))),
            ("only tab", list(1, 0), 0, ("", None)),
            ("out of range", list(2, 1), 5, ("ab", Some(1))),
        ];
        for (name, mut tabs, index, (order, active)) in cases {
            tabs.close(index);
            assert_eq!(state(&tabs), (order.to_owned(), active), "{name}");
        }
    }

    #[test]
    fn next_and_previous_wrap() {
        let mut tabs = list(3, 2);
        tabs.activate_next();
        assert_eq!(tabs.active_index(), Some(0));
        tabs.activate_previous();
        assert_eq!(tabs.active_index(), Some(2));
        tabs.activate_previous();
        assert_eq!(tabs.active_index(), Some(1));

        let mut empty: TabList<char> = TabList::default();
        empty.activate_next();
        empty.activate_previous();
        empty.move_active(Step::Left);
        assert_eq!(empty.active_index(), None);
    }

    #[test]
    fn moving_tabs_keeps_the_same_tab_active() {
        let cases = [
            ("active to end", list(4, 0), (0, 3), ("bcda", Some(3))),
            (
                "other across active, right",
                list(4, 2),
                (0, 3),
                ("bcda", Some(1)),
            ),
            (
                "other across active, left",
                list(4, 1),
                (3, 0),
                ("dabc", Some(2)),
            ),
            ("other not crossing", list(4, 0), (2, 3), ("abdc", Some(0))),
            ("target clamped", list(3, 0), (0, 99), ("bca", Some(2))),
            ("source out of range", list(3, 1), (7, 0), ("abc", Some(1))),
        ];
        for (name, mut tabs, (from, to), (order, active)) in cases {
            tabs.move_tab(from, to);
            assert_eq!(state(&tabs), (order.to_owned(), active), "{name}");
        }
    }

    #[test]
    fn move_active_stops_at_the_ends() {
        let mut tabs = list(3, 1);
        tabs.move_active(Step::Right);
        assert_eq!(state(&tabs), ("acb".into(), Some(2)));
        tabs.move_active(Step::Right);
        assert_eq!(state(&tabs), ("acb".into(), Some(2)));
        tabs.move_active(Step::Left);
        tabs.move_active(Step::Left);
        tabs.move_active(Step::Left);
        assert_eq!(state(&tabs), ("bac".into(), Some(0)));
    }
}
