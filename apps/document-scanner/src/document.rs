//! The multi-page document being assembled.

use crate::page::Page;

/// Bulk reorder operations from the Reorder Pages dialog.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reorder {
    /// Interleave the first half (fronts) with the second half (backs):
    /// 1 2 3 4 5 6 → 1 4 2 5 3 6.
    Combine,
    /// Interleave with the second half reversed (backs scanned by flipping
    /// the whole stack): 1 2 3 4 5 6 → 1 6 2 5 3 4.
    CombineReversed,
    /// Reverse the page order.
    Reverse,
    /// Rotate pages 1, 3, 5 … by 180°.
    FlipOdd,
}

#[derive(Default)]
pub struct Document {
    pages: Vec<Page>,
    dirty: bool,
}

impl Document {
    #[cfg(test)]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.pages.len()
    }

    pub fn is_empty(&self) -> bool {
        self.pages.is_empty()
    }

    pub fn pages(&self) -> &[Page] {
        &self.pages
    }

    pub fn page(&self, i: usize) -> Option<&Page> {
        self.pages.get(i)
    }

    /// Unsaved changes exist.
    pub fn is_dirty(&self) -> bool {
        self.dirty && !self.pages.is_empty()
    }

    pub fn mark_saved(&mut self) {
        self.dirty = false;
    }

    pub fn clear(&mut self) {
        self.pages.clear();
        self.dirty = false;
    }

    /// Inserts `page` at `index` (clamped) and returns its index.
    pub fn insert(&mut self, index: usize, page: Page) -> usize {
        let i = index.min(self.pages.len());
        self.pages.insert(i, page);
        self.dirty = true;
        i
    }

    #[cfg(test)]
    pub fn push(&mut self, page: Page) -> usize {
        self.insert(usize::MAX, page)
    }

    pub fn remove(&mut self, i: usize) -> Option<Page> {
        if i < self.pages.len() {
            self.dirty = true;
            Some(self.pages.remove(i))
        } else {
            None
        }
    }

    /// Replaces page `i` using `f`.
    pub fn update(&mut self, i: usize, f: impl FnOnce(&Page) -> Page) -> bool {
        match self.pages.get(i) {
            Some(p) => {
                let new = f(p);
                self.pages[i] = new;
                self.dirty = true;
                true
            }
            None => false,
        }
    }

    pub fn set_crop(&mut self, i: usize, crop: Option<crate::page::Rect>) -> bool {
        match self.pages.get_mut(i) {
            Some(p) if p.crop != crop => {
                p.crop = crop;
                self.dirty = true;
                true
            }
            _ => false,
        }
    }

    pub fn rotate(&mut self, i: usize, clockwise: bool) -> bool {
        self.update(i, |p| p.rotate(clockwise))
    }

    /// Moves page `from` to position `to`; returns the new index.
    pub fn move_page(&mut self, from: usize, to: usize) -> Option<usize> {
        if from >= self.pages.len() {
            return None;
        }
        let to = to.min(self.pages.len() - 1);
        if from != to {
            let p = self.pages.remove(from);
            self.pages.insert(to, p);
            self.dirty = true;
        }
        Some(to)
    }

    pub fn reorder(&mut self, op: Reorder) {
        if self.pages.is_empty() {
            return;
        }
        let order = reorder_indices(self.pages.len(), op);
        let mut old: Vec<Option<Page>> = std::mem::take(&mut self.pages).into_iter().map(Some).collect();
        self.pages = order.into_iter().filter_map(|i| old[i].take()).collect();
        if op == Reorder::FlipOdd {
            for p in self.pages.iter_mut().step_by(2) {
                *p = p.rotate_half();
            }
        }
        self.dirty = true;
    }
}

/// The new page order (as old indices) for `op` on `n` pages.
pub fn reorder_indices(n: usize, op: Reorder) -> Vec<usize> {
    match op {
        Reorder::Reverse => (0..n).rev().collect(),
        Reorder::FlipOdd => (0..n).collect(),
        Reorder::Combine | Reorder::CombineReversed => {
            let front = n.div_ceil(2);
            let mut out = Vec::with_capacity(n);
            for i in 0..front {
                out.push(i);
                let back = if op == Reorder::Combine { front + i } else { n - 1 - i };
                if back >= front && back < n && !out.contains(&back) {
                    out.push(back);
                }
            }
            out
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page(v: u8) -> Page {
        Page::new(2, 1, 1, vec![v, 0], 100.0)
    }

    fn values(d: &Document) -> Vec<u8> {
        d.pages().iter().map(|p| p.pixels[0]).collect()
    }

    fn doc(n: u8) -> Document {
        let mut d = Document::new();
        for i in 1..=n {
            d.push(page(i));
        }
        d
    }

    #[test]
    fn combine_orders() {
        assert_eq!(reorder_indices(6, Reorder::Combine), vec![0, 3, 1, 4, 2, 5]);
        assert_eq!(reorder_indices(6, Reorder::CombineReversed), vec![0, 5, 1, 4, 2, 3]);
        assert_eq!(reorder_indices(5, Reorder::Combine), vec![0, 3, 1, 4, 2]);
        assert_eq!(reorder_indices(5, Reorder::CombineReversed), vec![0, 4, 1, 3, 2]);
        assert_eq!(reorder_indices(1, Reorder::Combine), vec![0]);
        assert_eq!(reorder_indices(3, Reorder::Reverse), vec![2, 1, 0]);
    }

    #[test]
    fn reorder_document() {
        let mut d = doc(4);
        d.reorder(Reorder::Combine);
        assert_eq!(values(&d), vec![1, 3, 2, 4]);
        d.reorder(Reorder::Reverse);
        assert_eq!(values(&d), vec![4, 2, 3, 1]);
        d.reorder(Reorder::FlipOdd);
        // Pages 1 and 3 are rotated 180° (first pixel moves to the end).
        assert_eq!(d.pages()[0].pixels[1], 4);
        assert_eq!(d.pages()[1].pixels[0], 2);
        assert_eq!(d.pages()[2].pixels[1], 3);
    }

    #[test]
    fn move_insert_remove() {
        let mut d = doc(3);
        assert!(d.is_dirty());
        d.mark_saved();
        assert!(!d.is_dirty());
        assert_eq!(d.move_page(0, 2), Some(2));
        assert_eq!(values(&d), vec![2, 3, 1]);
        assert!(d.is_dirty());
        assert_eq!(d.move_page(2, 99), Some(2));
        assert_eq!(d.move_page(5, 0), None);
        assert_eq!(d.insert(1, page(9)), 1);
        assert_eq!(values(&d), vec![2, 9, 3, 1]);
        assert_eq!(d.remove(0).map(|p| p.pixels[0]), Some(2));
        assert!(d.remove(10).is_none());
        assert!(d.rotate(0, true));
        assert_eq!(d.pages()[0].height, 2);
        d.clear();
        assert!(d.is_empty() && !d.is_dirty());
    }
}
