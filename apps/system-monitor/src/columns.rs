//! Mapping between the process list's columns and the GNOME
//! `org.gnome.gnome-system-monitor.proctree` GSettings keys.
//!
//! GNOME System Monitor numbers its process columns 0…26 and stores, per
//! column N, `col-N-width` (i) and `col-N-visible` (b), plus `sort-col` (i, a
//! column number), `sort-order` (i, GtkSortType: 0 ascending, 1 descending)
//! and `columns-order` (ai, column numbers in display order). Helix shows a
//! subset of those columns; the numbers and defaults below are taken from
//! the published schema so existing user settings carry over.

use crate::model::ProcColumn;

/// Child schema holding the process-list column state.
pub const PROCTREE_SCHEMA: &str = "org.gnome.gnome-system-monitor.proctree";

pub const SORT_COL_KEY: &str = "sort-col";
pub const SORT_ORDER_KEY: &str = "sort-order";
pub const COLUMNS_ORDER_KEY: &str = "columns-order";

/// Schema defaults for `sort-col` / `sort-order` (Name, ascending).
pub const DEFAULT_SORT_COL: i32 = 0;
pub const DEFAULT_SORT_ORDER: i32 = 0;

/// Columns Helix shows, in their default display order.
pub const ALL: [ProcColumn; 8] = [
    ProcColumn::Name,
    ProcColumn::User,
    ProcColumn::Cpu,
    ProcColumn::Memory,
    ProcColumn::Pid,
    ProcColumn::Priority,
    ProcColumn::State,
    ProcColumn::Command,
];

/// Narrowest width applied from settings, so a bogus stored value cannot
/// make a column disappear.
pub const MIN_WIDTH: i32 = 30;

impl ProcColumn {
    /// GNOME's column number for this column.
    pub fn gnome_index(self) -> i32 {
        match self {
            ProcColumn::Name => 0,
            ProcColumn::User => 1,
            ProcColumn::State => 2, // “Status”
            ProcColumn::Cpu => 8,   // “CPU %”
            ProcColumn::Pid => 12,  // “ID”
            ProcColumn::Command => 14,
            ProcColumn::Memory => 15,
            ProcColumn::Priority => 26,
        }
    }

    pub fn from_gnome_index(i: i32) -> Option<Self> {
        ALL.into_iter().find(|c| c.gnome_index() == i)
    }

    pub fn width_key(self) -> String {
        format!("col-{}-width", self.gnome_index())
    }

    pub fn visible_key(self) -> String {
        format!("col-{}-visible", self.gnome_index())
    }

    /// Schema default for `col-N-width`.
    pub fn default_width(self) -> i32 {
        match self {
            ProcColumn::Name => 225,
            ProcColumn::User => 98,
            ProcColumn::State => 37,
            ProcColumn::Cpu => 71,
            ProcColumn::Pid => 60,
            ProcColumn::Command => 120,
            ProcColumn::Memory => 80,
            ProcColumn::Priority => 100,
        }
    }

    /// Schema default for `col-N-visible`.
    pub fn default_visible(self) -> bool {
        !matches!(self, ProcColumn::State | ProcColumn::Command)
    }

    /// Short action-name-safe identifier.
    pub fn id(self) -> &'static str {
        match self {
            ProcColumn::Name => "name",
            ProcColumn::User => "user",
            ProcColumn::Cpu => "cpu",
            ProcColumn::Memory => "memory",
            ProcColumn::Pid => "pid",
            ProcColumn::Priority => "priority",
            ProcColumn::State => "state",
            ProcColumn::Command => "command",
        }
    }
}

/// A stored width, or `None` when it should not be applied (unset/invalid).
pub fn width_from_setting(w: i32) -> Option<i32> {
    (w > 0).then(|| w.max(MIN_WIDTH))
}

/// Sort state from `sort-col` / `sort-order`. Columns Helix does not show
/// fall back to the schema default (Name, ascending).
pub fn sort_from_settings(col: i32, order: i32) -> (ProcColumn, bool) {
    match ProcColumn::from_gnome_index(col) {
        Some(c) => (c, order == 1),
        None => (ProcColumn::Name, false),
    }
}

/// `(sort-col, sort-order)` values for a sort state.
pub fn sort_to_settings(col: ProcColumn, descending: bool) -> (i32, i32) {
    (col.gnome_index(), i32::from(descending))
}

/// Display order from `columns-order`: known columns in stored order
/// (duplicates ignored), then any remaining ones in default order.
pub fn order_from_setting(stored: &[i32]) -> Vec<ProcColumn> {
    let mut out: Vec<ProcColumn> = Vec::with_capacity(ALL.len());
    for c in stored
        .iter()
        .filter_map(|&i| ProcColumn::from_gnome_index(i))
    {
        if !out.contains(&c) {
            out.push(c);
        }
    }
    for c in ALL {
        if !out.contains(&c) {
            out.push(c);
        }
    }
    out
}

/// New `columns-order` value for the current display order. Positions held
/// by Helix columns in `stored` are refilled in the new order; numbers of
/// columns Helix does not show keep their place, so GNOME's own ordering
/// survives a round trip.
pub fn merge_order(stored: &[i32], current: &[ProcColumn]) -> Vec<i32> {
    let mut queue = current.iter().map(|c| c.gnome_index());
    let mut out = Vec::with_capacity(stored.len().max(current.len()));
    let mut seen = Vec::new();
    for &i in stored {
        if ProcColumn::from_gnome_index(i).is_some() {
            if seen.contains(&i) {
                continue;
            }
            seen.push(i);
            if let Some(next) = queue.next() {
                out.push(next);
            }
        } else if !out.contains(&i) {
            out.push(i);
        }
    }
    out.extend(queue);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn indices_round_trip_and_are_unique() {
        let mut seen = std::collections::HashSet::new();
        for c in ALL {
            assert!(seen.insert(c.gnome_index()), "{c:?} index reused");
            assert!((0..=26).contains(&c.gnome_index()));
            assert_eq!(ProcColumn::from_gnome_index(c.gnome_index()), Some(c));
        }
        for unused in [
            3, 4, 5, 6, 7, 9, 10, 11, 13, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 27, -1,
        ] {
            assert_eq!(ProcColumn::from_gnome_index(unused), None);
        }
    }

    #[test]
    fn key_names() {
        let expect = [
            (ProcColumn::Name, "col-0-width", "col-0-visible"),
            (ProcColumn::User, "col-1-width", "col-1-visible"),
            (ProcColumn::State, "col-2-width", "col-2-visible"),
            (ProcColumn::Cpu, "col-8-width", "col-8-visible"),
            (ProcColumn::Pid, "col-12-width", "col-12-visible"),
            (ProcColumn::Command, "col-14-width", "col-14-visible"),
            (ProcColumn::Memory, "col-15-width", "col-15-visible"),
            (ProcColumn::Priority, "col-26-width", "col-26-visible"),
        ];
        for (c, w, v) in expect {
            assert_eq!(c.width_key(), w);
            assert_eq!(c.visible_key(), v);
        }
    }

    #[test]
    fn schema_defaults() {
        let visible: Vec<_> = ALL.into_iter().filter(|c| c.default_visible()).collect();
        assert_eq!(
            visible,
            [
                ProcColumn::Name,
                ProcColumn::User,
                ProcColumn::Cpu,
                ProcColumn::Memory,
                ProcColumn::Pid,
                ProcColumn::Priority
            ]
        );
        assert_eq!(ProcColumn::Name.default_width(), 225);
        assert_eq!(ProcColumn::Priority.default_width(), 100);
    }

    /// The shipped schema must declare exactly the keys (and defaults) the
    /// mapping uses.
    #[test]
    fn shipped_schema_matches() {
        let xml = include_str!("../data/org.gnome.gnome-system-monitor.gschema.xml");
        let tree = &xml[xml
            .find(&format!("id=\"{PROCTREE_SCHEMA}\""))
            .expect("proctree schema")..];
        for c in ALL {
            let w = format!(
                "<key name=\"{}\" type=\"i\"><default>{}</default>",
                c.width_key(),
                c.default_width()
            );
            let v = format!(
                "<key name=\"{}\" type=\"b\"><default>{}</default>",
                c.visible_key(),
                c.default_visible()
            );
            assert!(tree.contains(&w), "missing {w}");
            assert!(tree.contains(&v), "missing {v}");
        }
        assert_eq!(tree.matches("<key ").count(), ALL.len() * 2 + 3);
        for k in [SORT_COL_KEY, SORT_ORDER_KEY, COLUMNS_ORDER_KEY] {
            assert!(tree.contains(&format!("<key name=\"{k}\"")));
        }
    }

    #[test]
    fn widths() {
        assert_eq!(width_from_setting(0), None);
        assert_eq!(width_from_setting(-1), None);
        assert_eq!(width_from_setting(5), Some(MIN_WIDTH));
        assert_eq!(width_from_setting(140), Some(140));
    }

    #[test]
    fn sort_mapping() {
        assert_eq!(
            sort_from_settings(DEFAULT_SORT_COL, DEFAULT_SORT_ORDER),
            (ProcColumn::Name, false)
        );
        assert_eq!(sort_from_settings(8, 1), (ProcColumn::Cpu, true));
        assert_eq!(sort_from_settings(15, 0), (ProcColumn::Memory, false));
        // “CPU Time” (9) is not shown by Helix.
        assert_eq!(sort_from_settings(9, 1), (ProcColumn::Name, false));
        for c in ALL {
            for d in [false, true] {
                let (col, order) = sort_to_settings(c, d);
                assert_eq!(sort_from_settings(col, order), (c, d));
            }
        }
    }

    #[test]
    fn order_from_stored() {
        assert_eq!(order_from_setting(&[0]), ALL.to_vec());
        assert_eq!(order_from_setting(&[]), ALL.to_vec());
        let o = order_from_setting(&[12, 9, 0, 12, 26]);
        assert_eq!(
            &o[..3],
            &[ProcColumn::Pid, ProcColumn::Name, ProcColumn::Priority]
        );
        assert_eq!(o.len(), ALL.len());
    }

    #[test]
    fn order_merge_keeps_unknown_columns() {
        use ProcColumn::*;
        // Default schema value.
        let cur = [Pid, Name, User, Cpu, Memory, Priority, State, Command];
        assert_eq!(merge_order(&[0], &cur), vec![12, 0, 1, 8, 15, 26, 2, 14]);
        // GNOME-style full order: unknown 9 (CPU Time) and 11 (Nice) stay put.
        let stored = [0, 1, 9, 8, 11, 15, 12, 26, 2, 14];
        assert_eq!(
            merge_order(&stored, &ALL),
            vec![0, 1, 9, 8, 11, 15, 12, 26, 2, 14]
        );
        let cur = [User, Name, Cpu, Memory, Pid, Priority, State, Command];
        assert_eq!(
            merge_order(&stored, &cur),
            vec![1, 0, 9, 8, 11, 15, 12, 26, 2, 14]
        );
        // Round trip.
        assert_eq!(
            order_from_setting(&merge_order(&stored, &cur)),
            cur.to_vec()
        );
    }
}
