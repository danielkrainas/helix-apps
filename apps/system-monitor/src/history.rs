//! Fixed-length sample histories for the resource graphs.

use std::collections::VecDeque;

/// Several parallel series (e.g. one per CPU) sharing a time axis.
#[derive(Debug, Clone)]
pub struct History {
    capacity: usize,
    series: Vec<VecDeque<f64>>,
}

impl History {
    pub fn new(capacity: usize) -> Self {
        History { capacity: capacity.max(2), series: Vec::new() }
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// Appends one value per series. If the number of series changes, the
    /// history restarts.
    pub fn push(&mut self, values: &[f64]) {
        if values.len() != self.series.len() {
            self.series = vec![VecDeque::with_capacity(self.capacity); values.len()];
        }
        for (s, v) in self.series.iter_mut().zip(values) {
            if s.len() == self.capacity {
                s.pop_front();
            }
            s.push_back(*v);
        }
    }

    pub fn series(&self) -> &[VecDeque<f64>] {
        &self.series
    }

    pub fn max(&self) -> f64 {
        self.series.iter().flatten().copied().fold(0.0, f64::max)
    }
}

/// Rounds a maximum up to a "nice" axis bound (1, 2, 5 × 10ⁿ), at least
/// `floor`.
pub fn nice_ceiling(v: f64, floor: f64) -> f64 {
    let v = v.max(floor);
    if v <= 0.0 {
        return 1.0;
    }
    let exp = v.log10().floor();
    let base = 10f64.powf(exp);
    for m in [1.0, 2.0, 5.0, 10.0] {
        if v <= m * base * (1.0 + 1e-9) {
            return m * base;
        }
    }
    10.0 * base
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ring() {
        let mut h = History::new(3);
        for i in 0..5 {
            h.push(&[i as f64, 10.0 * i as f64]);
        }
        assert_eq!(h.series().len(), 2);
        assert_eq!(h.series()[0].iter().copied().collect::<Vec<_>>(), vec![2.0, 3.0, 4.0]);
        assert_eq!(h.max(), 40.0);
        assert_eq!(h.series()[1].iter().copied().collect::<Vec<_>>(), vec![20.0, 30.0, 40.0]);
        h.push(&[1.0]);
        assert_eq!(h.series().len(), 1);
        assert_eq!(h.series()[0].len(), 1);
    }

    #[test]
    fn ceilings() {
        assert_eq!(nice_ceiling(0.0, 1024.0), 2000.0);
        assert_eq!(nice_ceiling(3_400.0, 1.0), 5_000.0);
        assert_eq!(nice_ceiling(1_000.0, 1.0), 1_000.0);
        assert_eq!(nice_ceiling(7_000_001.0, 1.0), 10_000_000.0);
        assert_eq!(nice_ceiling(0.0, 0.0), 1.0);
    }
}
