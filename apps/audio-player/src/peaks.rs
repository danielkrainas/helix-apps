//! Reduction of decoded samples to waveform peaks.

/// Peaks per second of audio stored by the decoder.
pub const PEAKS_PER_SECOND: u32 = 100;

/// Accumulates mono samples into fixed-size blocks, keeping max |sample|.
#[derive(Debug, Clone)]
pub struct PeakAccumulator {
    block: usize,
    count: usize,
    current: f32,
    pub peaks: Vec<f32>,
}

impl PeakAccumulator {
    /// `rate` is the sample rate; blocks hold `rate / PEAKS_PER_SECOND` samples.
    pub fn new(rate: u32) -> Self {
        let block = (rate / PEAKS_PER_SECOND).max(1) as usize;
        PeakAccumulator {
            block,
            count: 0,
            current: 0.0,
            peaks: Vec::new(),
        }
    }

    pub fn push(&mut self, samples: &[f32]) {
        for &s in samples {
            let a = if s.is_finite() { s.abs().min(1.0) } else { 0.0 };
            if a > self.current {
                self.current = a;
            }
            self.count += 1;
            if self.count == self.block {
                self.peaks.push(self.current);
                self.count = 0;
                self.current = 0.0;
            }
        }
    }

    /// Flushes a trailing partial block.
    pub fn finish(mut self) -> Vec<f32> {
        if self.count > 0 {
            self.peaks.push(self.current);
        }
        self.peaks
    }
}

/// Max-downsamples `peaks` into `bins` buckets, treating the data as the first
/// `peaks.len()` of `total` slots (`total >= peaks.len()`; slots past the data
/// are not decoded yet). Buckets with no data are `None`.
pub fn bins(peaks: &[f32], total: usize, bins: usize) -> Vec<Option<f32>> {
    let total = total.max(peaks.len());
    if bins == 0 || total == 0 {
        return vec![None; bins];
    }
    (0..bins)
        .map(|i| {
            let start = i * total / bins;
            let end = ((i + 1) * total / bins).max(start + 1).min(total);
            if start >= peaks.len() {
                return None;
            }
            peaks[start..end.min(peaks.len())]
                .iter()
                .copied()
                .reduce(f32::max)
        })
        .collect()
}

/// Loudest peak, used to normalise the drawing (never zero).
pub fn max_peak(peaks: &[f32]) -> f32 {
    peaks.iter().copied().fold(0.0f32, f32::max).max(1e-4)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accumulates_blocks() {
        let mut acc = PeakAccumulator::new(400); // block of 4
        acc.push(&[0.1, -0.5, 0.2, 0.0, 0.3]);
        acc.push(&[0.0, -0.9, 0.1, 0.05]);
        assert_eq!(acc.peaks, vec![0.5, 0.9]);
        assert_eq!(acc.clone().finish(), vec![0.5, 0.9, 0.05]);
    }

    #[test]
    fn clamps_bad_samples() {
        let mut acc = PeakAccumulator::new(1);
        acc.push(&[f32::NAN, 3.0, -0.25]);
        assert_eq!(acc.finish(), vec![0.0, 1.0, 0.25]);
    }

    #[test]
    fn tiny_rate_uses_single_sample_blocks() {
        let mut acc = PeakAccumulator::new(50);
        acc.push(&[0.5, 0.25]);
        assert_eq!(acc.finish(), vec![0.5, 0.25]);
    }

    #[test]
    fn downsample_max() {
        let p = [0.1, 0.4, 0.2, 0.8, 0.3, 0.0];
        assert_eq!(bins(&p, 6, 3), vec![Some(0.4), Some(0.8), Some(0.3)]);
        assert_eq!(bins(&p, 6, 1), vec![Some(0.8)]);
    }

    #[test]
    fn upsample_repeats() {
        let p = [0.1, 0.5];
        assert_eq!(
            bins(&p, 2, 4),
            vec![Some(0.1), Some(0.1), Some(0.5), Some(0.5)]
        );
    }

    #[test]
    fn partial_data() {
        let p = [0.1, 0.5];
        assert_eq!(bins(&p, 8, 4), vec![Some(0.5), None, None, None]);
        assert_eq!(bins(&p, 3, 3), vec![Some(0.1), Some(0.5), None]);
        // total smaller than data is widened to the data.
        assert_eq!(bins(&p, 1, 2), vec![Some(0.1), Some(0.5)]);
    }

    #[test]
    fn empty() {
        assert_eq!(bins(&[], 0, 3), vec![None, None, None]);
        assert!(bins(&[0.5], 1, 0).is_empty());
        assert_eq!(max_peak(&[]), 1e-4);
        assert_eq!(max_peak(&[0.2, 0.7]), 0.7);
    }
}
