//! Free-space computation and volume-bar geometry (pure, unit tested).

pub const MIB: u64 = 1024 * 1024;
/// Sectors reserved at the end of a GPT disk for the backup header + table.
const GPT_BACKUP_BYTES: u64 = 33 * 512;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    pub offset: u64,
    pub size: u64,
}

impl Span {
    pub fn end(&self) -> u64 {
        self.offset + self.size
    }
}

/// Usable byte range of a partition table on a disk of `disk_size` bytes.
pub fn table_region(table_type: &str, disk_size: u64) -> (u64, u64) {
    let start = MIB.min(disk_size);
    let end = if table_type == "gpt" { disk_size.saturating_sub(GPT_BACKUP_BYTES) } else { disk_size };
    (start, end.max(start))
}

/// Smallest gap worth showing as free space on a disk of `disk_size` bytes:
/// 0.1% of the disk, clamped to 1–16 MiB, so alignment slack left by
/// partitioning tools is not shown as tiny free-space slivers.
pub fn min_free_gap(disk_size: u64) -> u64 {
    (disk_size / 1000).clamp(MIB, 16 * MIB)
}

/// Gaps of at least `min_gap` bytes inside `[start, end)` not covered by
/// `used` (which may be unsorted and may overlap the region edges).
pub fn free_spans(start: u64, end: u64, used: &[Span], min_gap: u64) -> Vec<Span> {
    let mut used: Vec<Span> = used.to_vec();
    used.sort_by_key(|s| s.offset);
    let mut out = Vec::new();
    let mut cursor = start;
    for s in used {
        if s.offset > cursor {
            let gap_end = s.offset.min(end);
            if gap_end > cursor && gap_end - cursor >= min_gap {
                out.push(Span { offset: cursor, size: gap_end - cursor });
            }
        }
        cursor = cursor.max(s.end());
        if cursor >= end {
            break;
        }
    }
    if end > cursor && end - cursor >= min_gap {
        out.push(Span { offset: cursor, size: end - cursor });
    }
    out
}

/// Widths for segments of the given sizes filling exactly `total` pixels,
/// proportional to size but each at least `min` wide (when possible).
pub fn widths(sizes: &[u64], total: f64, min: f64) -> Vec<f64> {
    let n = sizes.len();
    if n == 0 {
        return Vec::new();
    }
    let sum: u64 = sizes.iter().sum();
    if sum == 0 || min * n as f64 >= total {
        return vec![total / n as f64; n];
    }
    let mut fixed = vec![false; n];
    loop {
        let nfixed = fixed.iter().filter(|f| **f).count();
        let remaining = total - min * nfixed as f64;
        let free_sum: u64 = sizes.iter().zip(&fixed).filter(|(_, f)| !**f).map(|(s, _)| *s).sum();
        let mut changed = false;
        let out: Vec<f64> = sizes
            .iter()
            .zip(fixed.iter_mut())
            .map(|(&s, f)| {
                if *f {
                    min
                } else {
                    let w = if free_sum == 0 { remaining / (n - nfixed) as f64 } else { remaining * s as f64 / free_sum as f64 };
                    if w < min {
                        *f = true;
                        changed = true;
                    }
                    w
                }
            })
            .collect();
        if !changed {
            return out;
        }
    }
}

/// A node of the volume bar: a size and contained volumes drawn below it.
#[derive(Debug, Clone, PartialEq)]
pub struct Node {
    pub size: u64,
    pub children: Vec<Node>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Rect {
    pub fn contains(&self, px: f64, py: f64) -> bool {
        px >= self.x && px < self.x + self.w && py >= self.y && py < self.y + self.h
    }
}

fn depth(nodes: &[Node]) -> usize {
    nodes.iter().map(|n| 1 + depth(&n.children)).max().unwrap_or(0)
}

/// Lays out a tree of volumes in a `width × height` bar. Returns one rect
/// per node in pre-order, each tagged with its index path. Rows have equal
/// height; nodes without children extend down to the bottom of the bar.
pub fn bar(nodes: &[Node], width: f64, height: f64, min: f64) -> Vec<(Vec<usize>, Rect)> {
    let rows = depth(nodes).max(1);
    let row_h = height / rows as f64;
    let mut out = Vec::new();
    #[allow(clippy::too_many_arguments)]
    fn rec(
        nodes: &[Node],
        x: f64,
        w: f64,
        row: usize,
        row_h: f64,
        height: f64,
        min: f64,
        prefix: &mut Vec<usize>,
        out: &mut Vec<(Vec<usize>, Rect)>,
    ) {
        let sizes: Vec<u64> = nodes.iter().map(|n| n.size).collect();
        let ws = widths(&sizes, w, min);
        let mut cx = x;
        let y = row as f64 * row_h;
        for (i, (node, nw)) in nodes.iter().zip(ws).enumerate() {
            prefix.push(i);
            let h = if node.children.is_empty() { height - y } else { row_h };
            out.push((prefix.clone(), Rect { x: cx, y, w: nw, h }));
            if !node.children.is_empty() {
                rec(&node.children, cx, nw, row + 1, row_h, height, min, prefix, out);
            }
            prefix.pop();
            cx += nw;
        }
    }
    rec(nodes, 0.0, width, 0, row_h, height, min, &mut Vec::new(), &mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn span(o: u64, s: u64) -> Span {
        Span { offset: o, size: s }
    }

    #[test]
    fn regions() {
        assert_eq!(table_region("dos", 100 * MIB), (MIB, 100 * MIB));
        assert_eq!(table_region("gpt", 100 * MIB), (MIB, 100 * MIB - 33 * 512));
        assert_eq!(table_region("gpt", 0), (0, 0));
        assert_eq!(min_free_gap(100 * MIB), MIB);
        assert_eq!(min_free_gap(4000 * MIB), 4 * MIB);
        assert_eq!(min_free_gap(6_000_000 * MIB), 16 * MIB);
    }

    #[test]
    fn empty_table_is_all_free() {
        let (s, e) = table_region("gpt", 1000 * MIB);
        assert_eq!(free_spans(s, e, &[], MIB), vec![span(MIB, 999 * MIB - 33 * 512)]);
    }

    #[test]
    fn gaps_between_and_after_partitions() {
        let (s, e) = table_region("dos", 1000 * MIB);
        let used = [span(500 * MIB, 100 * MIB), span(MIB, 99 * MIB)];
        let free = free_spans(s, e, &used, MIB);
        assert_eq!(free, vec![span(100 * MIB, 400 * MIB), span(600 * MIB, 400 * MIB)]);
    }

    #[test]
    fn small_gaps_ignored_and_full_disk_has_no_free() {
        let (s, e) = table_region("gpt", 1000 * MIB);
        // Partition fills the region except for a few sectors at the end.
        let used = [span(MIB, e - MIB - 4096)];
        assert!(free_spans(s, e, &used, MIB).is_empty());
        // 512 KiB gap between partitions is ignored.
        let used = [span(MIB, 10 * MIB), span(11 * MIB + 512 * 1024, e - 11 * MIB - 512 * 1024)];
        assert!(free_spans(s, e, &used, MIB).is_empty());
    }

    #[test]
    fn overlapping_partitions_do_not_produce_bogus_gaps() {
        // Extended partition containing logical ones is passed alongside.
        let used = [span(MIB, 50 * MIB), span(10 * MIB, 5 * MIB)];
        assert_eq!(free_spans(MIB, 100 * MIB, &used, MIB), vec![span(51 * MIB, 49 * MIB)]);
    }

    #[test]
    fn widths_proportional() {
        let w = widths(&[1, 3], 400.0, 10.0);
        assert_eq!(w, vec![100.0, 300.0]);
    }

    #[test]
    fn widths_minimum_enforced_and_fill_total() {
        let w = widths(&[1, 1000, 1000], 300.0, 40.0);
        assert_eq!(w[0], 40.0);
        assert!((w[1] - 130.0).abs() < 1e-9);
        assert!((w.iter().sum::<f64>() - 300.0).abs() < 1e-9);
        // Cascading: shrinking others pushes another below the minimum.
        let w = widths(&[1, 50, 1000], 200.0, 40.0);
        assert_eq!(&w[..2], &[40.0, 40.0]);
        assert!((w[2] - 120.0).abs() < 1e-9);
    }

    #[test]
    fn widths_degenerate() {
        assert!(widths(&[], 100.0, 10.0).is_empty());
        assert_eq!(widths(&[0, 0], 100.0, 10.0), vec![50.0, 50.0]);
        assert_eq!(widths(&[1, 1000], 30.0, 40.0), vec![15.0, 15.0]);
    }

    #[test]
    fn bar_with_nested_row() {
        let nodes = vec![
            Node { size: 100, children: vec![] },
            Node {
                size: 300,
                children: vec![Node { size: 100, children: vec![] }, Node { size: 200, children: vec![] }],
            },
        ];
        let rects = bar(&nodes, 400.0, 80.0, 1.0);
        assert_eq!(rects.len(), 4);
        assert_eq!(rects[0], (vec![0], Rect { x: 0.0, y: 0.0, w: 100.0, h: 80.0 }));
        assert_eq!(rects[1], (vec![1], Rect { x: 100.0, y: 0.0, w: 300.0, h: 40.0 }));
        assert_eq!(rects[2], (vec![1, 0], Rect { x: 100.0, y: 40.0, w: 100.0, h: 40.0 }));
        assert_eq!(rects[3], (vec![1, 1], Rect { x: 200.0, y: 40.0, w: 200.0, h: 40.0 }));
        assert!(rects[3].1.contains(250.0, 50.0));
        assert!(!rects[3].1.contains(250.0, 10.0));
    }

    #[test]
    fn bar_single_row() {
        let rects = bar(&[Node { size: 5, children: vec![] }], 200.0, 50.0, 40.0);
        assert_eq!(rects, vec![(vec![0], Rect { x: 0.0, y: 0.0, w: 200.0, h: 50.0 })]);
        assert!(bar(&[], 200.0, 50.0, 40.0).is_empty());
    }
}
