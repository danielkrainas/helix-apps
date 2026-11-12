//! A small byte-budgeted LRU cache.

use std::collections::{BTreeMap, HashMap};
use std::hash::Hash;

/// LRU cache whose capacity is a total "cost" (e.g. bytes).
///
/// Recency is tracked with a monotonically increasing stamp; the stamp →
/// key index makes eviction O(log n).
pub struct Lru<K, V> {
    map: HashMap<K, (V, usize, u64)>,
    order: BTreeMap<u64, K>,
    stamp: u64,
    used: usize,
    budget: usize,
}

impl<K: Hash + Eq + Clone, V> Lru<K, V> {
    pub fn new(budget: usize) -> Self {
        Lru {
            map: HashMap::new(),
            order: BTreeMap::new(),
            stamp: 0,
            used: 0,
            budget,
        }
    }

    fn touch_stamp(&mut self) -> u64 {
        self.stamp += 1;
        self.stamp
    }

    /// Returns the value and marks it most recently used.
    pub fn get(&mut self, key: &K) -> Option<&V> {
        let stamp = self.touch_stamp();
        let entry = self.map.get_mut(key)?;
        self.order.remove(&entry.2);
        entry.2 = stamp;
        self.order.insert(stamp, key.clone());
        Some(&entry.0)
    }

    /// Returns the value without updating recency.
    pub fn peek(&self, key: &K) -> Option<&V> {
        self.map.get(key).map(|e| &e.0)
    }

    pub fn contains(&self, key: &K) -> bool {
        self.map.contains_key(key)
    }

    /// Inserts a value with a cost, evicting least recently used entries
    /// until the total fits the budget. Returns the evicted keys.
    /// An entry bigger than the whole budget is still kept (alone).
    pub fn insert(&mut self, key: K, value: V, cost: usize) -> Vec<K> {
        self.remove(&key);
        let stamp = self.touch_stamp();
        self.map.insert(key.clone(), (value, cost, stamp));
        self.order.insert(stamp, key);
        self.used += cost;
        let mut evicted = Vec::new();
        while self.used > self.budget && self.map.len() > 1 {
            let (&oldest, _) = self.order.iter().next().expect("non-empty");
            let k = self.order.remove(&oldest).expect("present");
            if let Some((_, c, _)) = self.map.remove(&k) {
                self.used -= c;
            }
            evicted.push(k);
        }
        evicted
    }

    pub fn remove(&mut self, key: &K) -> Option<V> {
        let (v, cost, stamp) = self.map.remove(key)?;
        self.order.remove(&stamp);
        self.used -= cost;
        Some(v)
    }

    pub fn clear(&mut self) {
        self.map.clear();
        self.order.clear();
        self.used = 0;
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn used(&self) -> usize {
        self.used
    }

    /// Keys from least to most recently used.
    #[cfg(test)]
    pub fn keys_lru(&self) -> Vec<K> {
        self.order.values().cloned().collect()
    }

    /// Iterates entries (no particular order, no recency update).
    pub fn iter(&self) -> impl Iterator<Item = (&K, &V)> {
        self.map.iter().map(|(k, v)| (k, &v.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn evicts_least_recently_used() {
        let mut c = Lru::new(30);
        assert!(c.insert(1, "a", 10).is_empty());
        assert!(c.insert(2, "b", 10).is_empty());
        assert!(c.insert(3, "c", 10).is_empty());
        assert_eq!(c.used(), 30);
        // Touch 1 so 2 becomes the oldest.
        assert_eq!(c.get(&1), Some(&"a"));
        assert_eq!(c.insert(4, "d", 10), vec![2]);
        assert!(!c.contains(&2));
        assert_eq!(c.keys_lru(), vec![3, 1, 4]);
        // A large entry evicts several.
        assert_eq!(c.insert(5, "e", 25), vec![3, 1, 4]);
        assert_eq!(c.len(), 1);
        assert_eq!(c.used(), 25);
    }

    #[test]
    fn replace_and_remove_update_cost() {
        let mut c = Lru::new(100);
        c.insert("x", 1, 40);
        c.insert("x", 2, 50);
        assert_eq!(c.used(), 50);
        assert_eq!(c.len(), 1);
        assert_eq!(c.peek(&"x"), Some(&2));
        assert_eq!(c.remove(&"x"), Some(2));
        assert_eq!(c.used(), 0);
        assert!(c.remove(&"x").is_none());
    }

    #[test]
    fn oversized_entry_is_kept_alone() {
        let mut c = Lru::new(10);
        c.insert(1, (), 5);
        assert_eq!(c.insert(2, (), 50), vec![1]);
        assert_eq!(c.len(), 1);
        assert!(c.contains(&2));
        c.clear();
        assert_eq!(c.len(), 0);
        assert_eq!(c.used(), 0);
    }

    #[test]
    fn peek_does_not_touch() {
        let mut c = Lru::new(20);
        c.insert(1, (), 10);
        c.insert(2, (), 10);
        c.peek(&1);
        assert_eq!(c.insert(3, (), 10), vec![1]);
    }
}
