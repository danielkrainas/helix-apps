//! Per-tab back/forward navigation history.

#[derive(Debug, Default, Clone)]
pub struct History {
    back: Vec<String>,
    forward: Vec<String>,
    current: Option<String>,
}

const LIMIT: usize = 100;

impl History {
    #[cfg(test)]
    pub fn current(&self) -> Option<&str> {
        self.current.as_deref()
    }

    /// Records a new visit; clears the forward stack.
    pub fn visit(&mut self, uri: &str) {
        if self.current.as_deref() == Some(uri) {
            return;
        }
        if let Some(cur) = self.current.take() {
            self.back.push(cur);
            if self.back.len() > LIMIT {
                self.back.remove(0);
            }
        }
        self.forward.clear();
        self.current = Some(uri.to_string());
    }

    pub fn can_go_back(&self) -> bool {
        !self.back.is_empty()
    }

    pub fn can_go_forward(&self) -> bool {
        !self.forward.is_empty()
    }

    pub fn go_back(&mut self) -> Option<String> {
        let prev = self.back.pop()?;
        if let Some(cur) = self.current.replace(prev.clone()) {
            self.forward.push(cur);
        }
        Some(prev)
    }

    pub fn go_forward(&mut self) -> Option<String> {
        let next = self.forward.pop()?;
        if let Some(cur) = self.current.replace(next.clone()) {
            self.back.push(cur);
        }
        Some(next)
    }

    /// Locations reachable with "back", most recent first.
    #[cfg(test)]
    pub fn back_list(&self) -> impl Iterator<Item = &str> {
        self.back.iter().rev().map(String::as_str)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn back_and_forward() {
        let mut h = History::default();
        assert!(!h.can_go_back());
        h.visit("a");
        h.visit("b");
        h.visit("c");
        assert_eq!(h.go_back().as_deref(), Some("b"));
        assert_eq!(h.go_back().as_deref(), Some("a"));
        assert_eq!(h.go_back(), None);
        assert_eq!(h.current(), Some("a"));
        assert_eq!(h.go_forward().as_deref(), Some("b"));
        assert!(h.can_go_forward());
        h.visit("d");
        assert!(!h.can_go_forward());
        assert_eq!(h.back_list().collect::<Vec<_>>(), ["b", "a"]);
    }

    #[test]
    fn duplicate_visit_ignored() {
        let mut h = History::default();
        h.visit("a");
        h.visit("a");
        assert!(!h.can_go_back());
    }

    #[test]
    fn limited() {
        let mut h = History::default();
        for i in 0..150 {
            h.visit(&i.to_string());
        }
        assert_eq!(h.back_list().count(), LIMIT);
    }
}
