#[derive(Debug, Clone)]
pub(crate) struct SelectionCore<T> {
    pub(crate) items: Vec<T>,
    pub(crate) selected: usize,
}

impl<T> Default for SelectionCore<T> {
    fn default() -> Self {
        Self {
            items: Vec::new(),
            selected: 0,
        }
    }
}

impl<T> SelectionCore<T> {
    pub(crate) fn new(items: Vec<T>) -> Self {
        Self { items, selected: 0 }
    }

    pub(crate) fn cycle_by(&mut self, direction: isize) {
        if self.items.is_empty() {
            self.selected = 0;
            return;
        }
        let len = self.items.len() as isize;
        self.selected = (self.selected as isize + direction).rem_euclid(len) as usize;
    }

    pub(crate) fn selected_item(&self) -> Option<&T> {
        self.items.get(self.selected)
    }
}

#[cfg(test)]
mod tests {
    use super::SelectionCore;

    #[test]
    fn cyclic_movement_preserves_completion_selection_behavior() {
        let mut selection = SelectionCore::new(vec!["a", "b"]);
        selection.cycle_by(-1);
        assert_eq!(selection.selected, 1);
        selection.cycle_by(1);
        assert_eq!(selection.selected, 0);
    }
}
