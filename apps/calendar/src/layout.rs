//! Placement of events in the week grid and month cells.

/// Column assignment for overlapping timed events in one day column.
/// Input: (start minute, end minute) pairs. Output per input: (column,
/// column count of its overlap cluster).
pub fn columns(spans: &[(i64, i64)]) -> Vec<(usize, usize)> {
    let mut order: Vec<usize> = (0..spans.len()).collect();
    order.sort_by_key(|&i| (spans[i].0, -(spans[i].1)));
    let mut out = vec![(0, 1); spans.len()];
    let mut cluster: Vec<usize> = Vec::new();
    let mut col_ends: Vec<i64> = Vec::new();
    let mut cluster_end = i64::MIN;
    let flush = |cluster: &mut Vec<usize>, col_ends: &mut Vec<i64>, out: &mut Vec<(usize, usize)>| {
        let n = col_ends.len().max(1);
        for &i in cluster.iter() {
            out[i].1 = n;
        }
        cluster.clear();
        col_ends.clear();
    };
    for i in order {
        let (s, e) = spans[i];
        // Zero-length events still occupy a sliver.
        let e = e.max(s + 15);
        if s >= cluster_end && !cluster.is_empty() {
            flush(&mut cluster, &mut col_ends, &mut out);
        }
        let col = match col_ends.iter().position(|&end| end <= s) {
            Some(c) => {
                col_ends[c] = e;
                c
            }
            None => {
                col_ends.push(e);
                col_ends.len() - 1
            }
        };
        out[i].0 = col;
        cluster.push(i);
        cluster_end = if cluster.len() == 1 { e } else { cluster_end.max(e) };
    }
    if !cluster.is_empty() {
        flush(&mut cluster, &mut col_ends, &mut out);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn separate_events_get_full_width() {
        assert_eq!(columns(&[(60, 120), (120, 180)]), vec![(0, 1), (0, 1)]);
    }

    #[test]
    fn overlapping_events_share() {
        let c = columns(&[(60, 180), (90, 120), (150, 240), (300, 360)]);
        assert_eq!(c[0], (0, 2));
        assert_eq!(c[1], (1, 2));
        assert_eq!(c[2], (1, 2), "reuses the freed column");
        assert_eq!(c[3], (0, 1));
    }

    #[test]
    fn three_way_overlap() {
        let c = columns(&[(0, 100), (10, 100), (20, 100)]);
        assert_eq!(c, vec![(0, 3), (1, 3), (2, 3)]);
        assert!(columns(&[]).is_empty());
    }
}
