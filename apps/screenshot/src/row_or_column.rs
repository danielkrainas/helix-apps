//! A layout for a few buttons: one row when the width has room for it,
//! else a column of equally wide buttons. Its smallest width is the widest
//! button's, so a long row (labels in some languages) never makes the
//! window wider than it needs to be.

use gtk::glib;
use gtk::prelude::*;
use gtk::subclass::prelude::*;

mod imp {
    use super::*;
    use std::cell::Cell;

    #[derive(Default)]
    pub struct RowOrColumn {
        pub spacing: Cell<i32>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for RowOrColumn {
        const NAME: &'static str = "HelixRowOrColumn";
        type Type = super::RowOrColumn;
        type ParentType = gtk::LayoutManager;
    }

    impl ObjectImpl for RowOrColumn {}

    impl LayoutManagerImpl for RowOrColumn {
        fn request_mode(&self, _: &gtk::Widget) -> gtk::SizeRequestMode {
            gtk::SizeRequestMode::HeightForWidth
        }

        fn measure(
            &self,
            widget: &gtk::Widget,
            orientation: gtk::Orientation,
            for_size: i32,
        ) -> (i32, i32, i32, i32) {
            let children = children(widget);
            let spacing = self.spacing.get();
            let gaps = spacing * (children.len() as i32 - 1).max(0);
            let widths: Vec<(i32, i32)> = children
                .iter()
                .map(|c| {
                    let (min, nat, _, _) = c.measure(gtk::Orientation::Horizontal, -1);
                    (min, nat)
                })
                .collect();
            let row = widths.iter().map(|w| w.1).sum::<i32>() + gaps;
            match orientation {
                gtk::Orientation::Horizontal => {
                    let min = widths.iter().map(|w| w.0).max().unwrap_or(0);
                    (min, row.max(min), -1, -1)
                }
                _ => {
                    let heights: Vec<(i32, i32)> = children
                        .iter()
                        .map(|c| {
                            let (min, nat, _, _) = c.measure(gtk::Orientation::Vertical, -1);
                            (min, nat)
                        })
                        .collect();
                    if for_size < 0 || for_size >= row {
                        let min = heights.iter().map(|h| h.0).max().unwrap_or(0);
                        let nat = heights.iter().map(|h| h.1).max().unwrap_or(0);
                        (min, nat, -1, -1)
                    } else {
                        let min = heights.iter().map(|h| h.0).sum::<i32>() + gaps;
                        let nat = heights.iter().map(|h| h.1).sum::<i32>() + gaps;
                        (min, nat, -1, -1)
                    }
                }
            }
        }

        fn allocate(&self, widget: &gtk::Widget, width: i32, height: i32, _baseline: i32) {
            let children = children(widget);
            let spacing = self.spacing.get();
            let gaps = spacing * (children.len() as i32 - 1).max(0);
            let nat: Vec<(i32, i32)> = children
                .iter()
                .map(|c| {
                    (
                        c.measure(gtk::Orientation::Horizontal, -1).1,
                        c.measure(gtk::Orientation::Vertical, -1).1,
                    )
                })
                .collect();
            let row = nat.iter().map(|n| n.0).sum::<i32>() + gaps;
            if width >= row {
                // A row, centred.
                let mut x = (width - row) / 2;
                for (c, (w, _)) in children.iter().zip(&nat) {
                    c.size_allocate(&gtk::Allocation::new(x, 0, *w, height), -1);
                    x += w + spacing;
                }
            } else {
                // A column of equally wide buttons, centred.
                let w = nat.iter().map(|n| n.0).max().unwrap_or(0).min(width);
                let x = (width - w) / 2;
                let mut y = 0;
                for (c, (_, h)) in children.iter().zip(&nat) {
                    c.size_allocate(&gtk::Allocation::new(x, y, w, *h), -1);
                    y += h + spacing;
                }
            }
        }
    }

    fn children(widget: &gtk::Widget) -> Vec<gtk::Widget> {
        let mut out = Vec::new();
        let mut child = widget.first_child();
        while let Some(c) = child {
            if c.should_layout() {
                out.push(c.clone());
            }
            child = c.next_sibling();
        }
        out
    }
}

glib::wrapper! {
    pub struct RowOrColumn(ObjectSubclass<imp::RowOrColumn>) @extends gtk::LayoutManager;
}

impl RowOrColumn {
    pub fn new(spacing: i32) -> Self {
        let layout: Self = glib::Object::new();
        layout.imp().spacing.set(spacing);
        layout
    }
}

/// Wide enough: one row; narrower: a column, and the smallest width is
/// one button's, not the row's. (Run from the app's one GTK test, as GTK
/// is set up once per test process.)
#[cfg(test)]
pub fn check_row_then_column() {
    let bin = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    bin.set_layout_manager(Some(RowOrColumn::new(12)));
    let buttons: Vec<gtk::Button> = ["Retake", "Copy to Clipboard", "Save…"]
        .iter()
        .map(|l| gtk::Button::with_label(l))
        .collect();
    for b in &buttons {
        bin.append(b);
    }
    let (min, nat, _, _) = bin.measure(gtk::Orientation::Horizontal, -1);
    let widest = buttons
        .iter()
        .map(|b| b.measure(gtk::Orientation::Horizontal, -1).0)
        .max()
        .unwrap();
    assert_eq!(min, widest, "smallest width: the widest button");
    assert!(nat > min);
    let one = buttons[0].measure(gtk::Orientation::Vertical, -1).1;
    let row_h = bin.measure(gtk::Orientation::Vertical, nat).1;
    let column_h = bin.measure(gtk::Orientation::Vertical, min).1;
    assert_eq!(row_h, one, "a row is one button high");
    assert!(column_h >= 3 * one + 24, "a column stacks the three");
}
