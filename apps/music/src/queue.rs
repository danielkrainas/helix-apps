//! Play queue: ordering, shuffle and repeat logic.

use helix_app_common::i18n::tr;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Repeat {
    #[default]
    None,
    All,
    One,
}

impl Repeat {
    pub fn cycle(self) -> Self {
        match self {
            Repeat::None => Repeat::All,
            Repeat::All => Repeat::One,
            Repeat::One => Repeat::None,
        }
    }

    /// Nick used by the `org.gnome.Music` `repeat` key.
    pub fn nick(self) -> &'static str {
        match self {
            Repeat::None => "none",
            Repeat::All => "all",
            Repeat::One => "song",
        }
    }

    pub fn from_nick(s: &str) -> Option<Self> {
        match s {
            "none" => Some(Repeat::None),
            "all" => Some(Repeat::All),
            "song" | "one" => Some(Repeat::One),
            _ => None,
        }
    }

    /// MPRIS `LoopStatus` value.
    pub fn mpris(self) -> &'static str {
        match self {
            Repeat::None => tr("None"),
            Repeat::All => tr("Playlist"),
            Repeat::One => tr("Track"),
        }
    }

    pub fn from_mpris(s: &str) -> Option<Self> {
        match s {
            "None" => Some(Repeat::None),
            "Playlist" => Some(Repeat::All),
            "Track" => Some(Repeat::One),
            _ => None,
        }
    }
}

/// Small xorshift PRNG; good enough for shuffling, no dependency.
#[derive(Clone, Debug)]
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Rng(seed.max(1))
    }

    pub fn from_time() -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0x9e37_79b9);
        Rng::new(nanos ^ ((std::process::id() as u64) << 32))
    }

    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    pub fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }

    fn shuffle(&mut self, v: &mut [usize]) {
        for i in (1..v.len()).rev() {
            let j = self.below(i + 1);
            v.swap(i, j);
        }
    }
}

/// The play queue. `items` keep their insertion order (what the user queued);
/// `order` is the play order (a permutation of item indices when shuffling).
#[derive(Clone, Debug)]
pub struct Queue<T> {
    items: Vec<T>,
    order: Vec<usize>,
    cursor: Option<usize>,
    shuffle: bool,
    pub repeat: Repeat,
    rng: Rng,
    /// Next shuffled cycle, drawn ahead of time so `peek_next` and `advance`
    /// agree when repeat-all wraps.
    pending_cycle: Option<Vec<usize>>,
}

impl<T: Clone + PartialEq> Queue<T> {
    pub fn new(rng: Rng) -> Self {
        Queue {
            items: Vec::new(),
            order: Vec::new(),
            cursor: None,
            shuffle: false,
            repeat: Repeat::None,
            rng,
            pending_cycle: None,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn shuffle(&self) -> bool {
        self.shuffle
    }

    /// Replaces the queue and makes `items[start]` current.
    pub fn set(&mut self, items: Vec<T>, start: usize) {
        self.items = items;
        self.pending_cycle = None;
        if self.items.is_empty() {
            self.order.clear();
            self.cursor = None;
            return;
        }
        let start = start.min(self.items.len() - 1);
        self.order = self.build_order(Some(start));
        self.cursor = if self.shuffle {
            Some(0)
        } else {
            self.order.iter().position(|&i| i == start)
        };
    }

    fn build_order(&mut self, first: Option<usize>) -> Vec<usize> {
        let mut order: Vec<usize> = (0..self.items.len()).collect();
        if self.shuffle {
            self.rng.shuffle(&mut order);
            if let Some(f) = first {
                if let Some(p) = order.iter().position(|&i| i == f) {
                    order.remove(p);
                    order.insert(0, f);
                }
            }
        }
        order
    }

    pub fn current(&self) -> Option<&T> {
        self.current_index().map(|i| &self.items[i])
    }

    /// Index into `items()` of the current track.
    pub fn current_index(&self) -> Option<usize> {
        self.cursor.map(|c| self.order[c])
    }

    /// Items in play order with their item indices.
    pub fn in_play_order(&self) -> Vec<(usize, &T)> {
        self.order.iter().map(|&i| (i, &self.items[i])).collect()
    }

    /// The item that would play after the current one, without moving.
    /// `auto` is a natural track end (Repeat One repeats); manual skips move on.
    pub fn peek_next(&mut self, auto: bool) -> Option<&T> {
        let i = self.next_target(auto)?;
        Some(&self.items[i])
    }

    fn next_target(&mut self, auto: bool) -> Option<usize> {
        let c = self.cursor?;
        if auto && self.repeat == Repeat::One {
            return Some(self.order[c]);
        }
        if c + 1 < self.order.len() {
            return Some(self.order[c + 1]);
        }
        let wraps = self.repeat == Repeat::All || (!auto && self.repeat == Repeat::One);
        if !wraps {
            return None;
        }
        if self.shuffle {
            if self.pending_cycle.is_none() {
                let last = self.order[c];
                let mut next = self.build_order(None);
                if next.len() > 1 && next[0] == last {
                    let j = 1 + self.rng.below(next.len() - 1);
                    next.swap(0, j);
                }
                self.pending_cycle = Some(next);
            }
            self.pending_cycle.as_ref().map(|o| o[0])
        } else {
            Some(self.order[0])
        }
    }

    /// Moves to the next item. Returns `None` at the end of the queue
    /// (no repeat); the cursor then stays on the last item.
    pub fn advance(&mut self, auto: bool) -> Option<&T> {
        let c = self.cursor?;
        let target = self.next_target(auto)?;
        if auto && self.repeat == Repeat::One {
            return self.current();
        }
        if c + 1 < self.order.len() {
            self.cursor = Some(c + 1);
        } else {
            if let Some(next) = self.pending_cycle.take() {
                self.order = next;
            }
            self.cursor = Some(0);
        }
        debug_assert_eq!(self.current_index(), Some(target));
        self.current()
    }

    /// Moves to the previous item; on the first item it wraps with
    /// Repeat All/One, otherwise stays put.
    pub fn previous(&mut self) -> Option<&T> {
        let c = self.cursor?;
        self.pending_cycle = None;
        if c > 0 {
            self.cursor = Some(c - 1);
        } else if self.repeat != Repeat::None {
            self.cursor = Some(self.order.len() - 1);
        }
        self.current()
    }

    pub fn has_next(&self) -> bool {
        self.cursor
            .is_some_and(|c| c + 1 < self.order.len() || self.repeat != Repeat::None)
    }

    pub fn has_previous(&self) -> bool {
        self.cursor.is_some()
    }

    /// Makes `items[index]` current.
    pub fn jump(&mut self, index: usize) -> Option<&T> {
        self.pending_cycle = None;
        if let Some(p) = self.order.iter().position(|&i| i == index) {
            self.cursor = Some(p);
        }
        self.current()
    }

    pub fn set_shuffle(&mut self, on: bool) {
        if self.shuffle == on {
            return;
        }
        self.shuffle = on;
        self.pending_cycle = None;
        if self.items.is_empty() {
            return;
        }
        let cur = self.current_index();
        if on {
            // Already-played part keeps its order; the rest is shuffled after
            // the current track.
            let c = self.cursor.unwrap_or(0);
            let mut rest: Vec<usize> = self.order[c + 1..].to_vec();
            self.rng.shuffle(&mut rest);
            self.order.truncate(c + 1);
            self.order.extend(rest);
        } else {
            self.order = (0..self.items.len()).collect();
            self.cursor = cur;
        }
    }

    /// Removes `items[index]`; returns true if it was the current item (the
    /// cursor then points at the item that followed it, if any).
    pub fn remove(&mut self, index: usize) -> bool {
        if index >= self.items.len() {
            return false;
        }
        self.pending_cycle = None;
        let pos = self.order.iter().position(|&i| i == index).unwrap();
        let was_current = self.cursor == Some(pos);
        self.items.remove(index);
        self.order.remove(pos);
        for i in self.order.iter_mut() {
            if *i > index {
                *i -= 1;
            }
        }
        if let Some(c) = self.cursor {
            if self.order.is_empty() {
                self.cursor = None;
            } else if pos < c {
                self.cursor = Some(c - 1);
            } else if was_current && c >= self.order.len() {
                self.cursor = Some(self.order.len() - 1);
            }
        }
        was_current
    }

    /// Appends to the end of the queue (in shuffle mode: at random places
    /// after the current track).
    pub fn append(&mut self, new: Vec<T>) {
        self.pending_cycle = None;
        let start = self.items.len();
        self.items.extend(new);
        for i in start..self.items.len() {
            if self.shuffle {
                let from = self.cursor.map_or(0, |c| c + 1);
                let span = self.order.len() - from + 1;
                let at = from + self.rng.below(span);
                self.order.insert(at, i);
            } else {
                self.order.push(i);
            }
        }
        if self.cursor.is_none() && !self.items.is_empty() {
            self.cursor = Some(0);
        }
    }

    /// Inserts `item` to play right after the current one.
    pub fn play_next(&mut self, item: T) {
        self.pending_cycle = None;
        self.items.push(item);
        let i = self.items.len() - 1;
        match self.cursor {
            Some(c) => self.order.insert(c + 1, i),
            None => {
                self.order.push(i);
                self.cursor = Some(self.order.len() - 1);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn q(n: usize) -> Queue<usize> {
        let mut q = Queue::new(Rng::new(42));
        q.set((0..n).collect(), 0);
        q
    }

    #[test]
    fn linear_no_repeat_stops_at_end() {
        let mut q = q(3);
        assert_eq!(q.current(), Some(&0));
        assert_eq!(q.advance(true), Some(&1));
        assert_eq!(q.advance(false), Some(&2));
        assert!(!q.has_next());
        assert_eq!(q.peek_next(true), None);
        assert_eq!(q.advance(true), None);
        assert_eq!(q.current(), Some(&2));
    }

    #[test]
    fn repeat_all_wraps_both_ways() {
        let mut q = q(3);
        q.repeat = Repeat::All;
        assert_eq!(q.previous(), Some(&2));
        assert_eq!(q.advance(true), Some(&0));
    }

    #[test]
    fn repeat_one_repeats_only_on_auto() {
        let mut q = q(3);
        q.repeat = Repeat::One;
        assert_eq!(q.peek_next(true), Some(&0));
        assert_eq!(q.advance(true), Some(&0));
        assert_eq!(q.advance(false), Some(&1));
        q.jump(2);
        assert_eq!(
            q.advance(false),
            Some(&0),
            "manual next wraps with repeat one"
        );
    }

    #[test]
    fn previous_without_repeat_stays_on_first() {
        let mut q = q(3);
        assert_eq!(q.previous(), Some(&0));
        q.jump(2);
        assert_eq!(q.previous(), Some(&1));
    }

    #[test]
    fn shuffle_starts_with_chosen_track_and_never_repeats() {
        let mut q = Queue::new(Rng::new(7));
        q.set_shuffle(true);
        q.set((0..20).collect(), 5);
        assert_eq!(q.current(), Some(&5));
        let mut seen = HashSet::from([5]);
        while let Some(&x) = q.advance(true) {
            assert!(seen.insert(x), "{x} repeated");
        }
        assert_eq!(seen.len(), 20);
    }

    #[test]
    fn shuffle_repeat_all_reshuffles_without_back_to_back() {
        let mut q = Queue::new(Rng::new(3));
        q.set_shuffle(true);
        q.repeat = Repeat::All;
        q.set((0..6).collect(), 0);
        let mut first: Vec<usize> = vec![*q.current().unwrap()];
        for _ in 0..5 {
            first.push(*q.advance(true).unwrap());
        }
        let last = *first.last().unwrap();
        let peek = *q.peek_next(true).unwrap();
        let next = *q.advance(true).unwrap();
        assert_eq!(peek, next, "peek agrees with advance across the wrap");
        assert_ne!(next, last);
        let mut second = vec![next];
        for _ in 0..5 {
            second.push(*q.advance(true).unwrap());
        }
        let mut sorted = second.clone();
        sorted.sort();
        assert_eq!(sorted, (0..6).collect::<Vec<_>>());
    }

    #[test]
    fn toggling_shuffle_keeps_current() {
        let mut q = q(10);
        q.jump(4);
        q.set_shuffle(true);
        assert_eq!(q.current(), Some(&4));
        let rest: HashSet<usize> = std::iter::from_fn(|| q.advance(true).copied()).collect();
        assert_eq!(
            rest.len(),
            5,
            "only the tracks after the current one remain"
        );
        q.set_shuffle(false);
        let cur = *q.current().unwrap();
        assert_eq!(q.current_index(), Some(cur));
    }

    #[test]
    fn remove_adjusts_cursor() {
        let mut q = q(4);
        q.jump(2);
        assert!(!q.remove(0));
        assert_eq!(q.current(), Some(&2));
        assert!(q.remove(1)); // item "2" is now at index 1
        assert_eq!(q.current(), Some(&3));
        assert!(q.remove(1));
        assert_eq!(q.current(), Some(&1));
        assert!(q.remove(0));
        assert_eq!(q.current(), None);
        assert!(q.is_empty());
    }

    #[test]
    fn append_and_play_next() {
        let mut q: Queue<&str> = Queue::new(Rng::new(1));
        q.append(vec!["a", "b"]);
        assert_eq!(q.current(), Some(&"a"));
        q.play_next("z");
        assert_eq!(q.advance(false), Some(&"z"));
        assert_eq!(q.advance(false), Some(&"b"));
    }

    #[test]
    fn repeat_nicks_round_trip() {
        for r in [Repeat::None, Repeat::All, Repeat::One] {
            assert_eq!(Repeat::from_nick(r.nick()), Some(r));
            assert_eq!(Repeat::from_mpris(r.mpris()), Some(r));
        }
        assert_eq!(Repeat::None.cycle().cycle().cycle(), Repeat::None);
    }
}
