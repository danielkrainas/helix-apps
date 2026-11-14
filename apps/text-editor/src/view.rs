//! Editor widgets: a `GtkTextView` subclass that draws the current-line
//! highlight and right margin, and a line-number gutter.

use gtk::glib;
use gtk::prelude::*;
use gtk::subclass::prelude::*;

mod imp {
    use super::*;
    use std::cell::Cell;

    #[derive(Default)]
    pub struct EditorView {
        pub highlight_line: Cell<bool>,
        /// Right margin column; 0 = hidden.
        pub right_margin: Cell<u32>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for EditorView {
        const NAME: &'static str = "HelixEditorView";
        type Type = super::EditorView;
        type ParentType = gtk::TextView;
    }

    impl ObjectImpl for EditorView {}
    impl WidgetImpl for EditorView {}

    impl TextViewImpl for EditorView {
        fn snapshot_layer(&self, layer: gtk::TextViewLayer, snapshot: gtk::Snapshot) {
            if layer == gtk::TextViewLayer::BelowText {
                let view = self.obj();
                let visible = view.visible_rect();
                let dark = adw::StyleManager::default().is_dark();
                let shade = if dark { 1.0 } else { 0.0 };
                if self.highlight_line.get() {
                    let buffer = view.buffer();
                    let iter = buffer.iter_at_mark(&buffer.get_insert());
                    let (y, h) = view.line_yrange(&iter);
                    let color =
                        gtk::gdk::RGBA::new(shade, shade, shade, if dark { 0.07 } else { 0.06 });
                    snapshot.append_color(
                        &color,
                        &gtk::graphene::Rect::new(
                            visible.x() as f32,
                            y as f32,
                            visible.width() as f32,
                            h as f32,
                        ),
                    );
                }
                let col = self.right_margin.get();
                if col > 0 {
                    let layout = view.create_pango_layout(Some("0000000000"));
                    let char_w = layout.pixel_size().0 as f32 / 10.0;
                    let x = view.left_margin() as f32 + char_w * col as f32;
                    let line = gtk::gdk::RGBA::new(shade, shade, shade, 0.15);
                    let fill = gtk::gdk::RGBA::new(shade, shade, shade, 0.03);
                    let top = visible.y() as f32;
                    let h = visible.height() as f32;
                    let right = (visible.x() + visible.width()) as f32;
                    if right > x {
                        snapshot
                            .append_color(&fill, &gtk::graphene::Rect::new(x, top, right - x, h));
                    }
                    snapshot.append_color(&line, &gtk::graphene::Rect::new(x, top, 1.0, h));
                }
            }
            self.parent_snapshot_layer(layer, snapshot)
        }
    }

    #[derive(Default)]
    pub struct LineGutter {
        pub view: glib::WeakRef<super::EditorView>,
        pub digits: Cell<u32>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for LineGutter {
        const NAME: &'static str = "HelixLineGutter";
        type Type = super::LineGutter;
        type ParentType = gtk::Widget;

        fn class_init(klass: &mut Self::Class) {
            klass.set_css_name("linegutter");
        }
    }

    impl ObjectImpl for LineGutter {}

    impl WidgetImpl for LineGutter {
        fn measure(&self, orientation: gtk::Orientation, _for_size: i32) -> (i32, i32, i32, i32) {
            if orientation == gtk::Orientation::Vertical {
                return (0, 0, -1, -1);
            }
            let Some(view) = self.view.upgrade() else {
                return (0, 0, -1, -1);
            };
            let digits = self.digits.get().max(2) as usize;
            let layout = view.create_pango_layout(Some(&"0".repeat(digits)));
            let w = layout.pixel_size().0 + 20;
            (w, w, -1, -1)
        }

        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            let Some(view) = self.view.upgrade() else {
                return;
            };
            let gutter = self.obj();
            let width = gutter.width() as f32;
            let height = gutter.height();
            let buffer = view.buffer();
            let cursor_line = buffer.iter_at_mark(&buffer.get_insert()).line();
            let fg = gutter.color();
            let dim = gtk::gdk::RGBA::new(fg.red(), fg.green(), fg.blue(), fg.alpha() * 0.45);
            let visible = view.visible_rect();
            let Some(mut iter) = view.iter_at_location(visible.x(), visible.y()) else {
                return;
            };
            iter.set_line_offset(0);
            loop {
                let (y, h) = view.line_yrange(&iter);
                let (_, wy) = view.buffer_to_window_coords(gtk::TextWindowType::Left, 0, y);
                if wy > height {
                    break;
                }
                let line = iter.line();
                let layout = view.create_pango_layout(Some(&(line + 1).to_string()));
                let (lw, lh) = layout.pixel_size();
                let current = line == cursor_line;
                if current {
                    let attrs = gtk::pango::AttrList::new();
                    attrs.insert(gtk::pango::AttrInt::new_weight(gtk::pango::Weight::Bold));
                    layout.set_attributes(Some(&attrs));
                }
                // Align with the first display line of the paragraph.
                let first_h = lh.min(h);
                snapshot.save();
                snapshot.translate(&gtk::graphene::Point::new(
                    width - 10.0 - lw as f32,
                    wy as f32 + (first_h - lh) as f32 / 2.0,
                ));
                snapshot.append_layout(&layout, if current { &fg } else { &dim });
                snapshot.restore();
                if !iter.forward_line() {
                    break;
                }
            }
        }
    }
}

glib::wrapper! {
    pub struct EditorView(ObjectSubclass<imp::EditorView>)
        @extends gtk::TextView, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget, gtk::Scrollable;
}

impl EditorView {
    pub fn new(buffer: &gtk::TextBuffer) -> Self {
        glib::Object::builder().property("buffer", buffer).build()
    }

    pub fn set_highlight_current_line(&self, on: bool) {
        self.imp().highlight_line.set(on);
        self.queue_draw();
    }

    pub fn set_right_margin_column(&self, column: Option<u32>) {
        self.imp().right_margin.set(column.unwrap_or(0));
        self.queue_draw();
    }
}

glib::wrapper! {
    pub struct LineGutter(ObjectSubclass<imp::LineGutter>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl LineGutter {
    pub fn new(view: &EditorView) -> Self {
        let g: LineGutter = glib::Object::new();
        g.imp().view.set(Some(view));
        g.update_width();
        g
    }

    /// Recomputes the width from the buffer's line count.
    pub fn update_width(&self) {
        let Some(view) = self.imp().view.upgrade() else {
            return;
        };
        let digits = view.buffer().line_count().max(1).to_string().len() as u32;
        if digits != self.imp().digits.get() {
            self.imp().digits.set(digits);
            self.queue_resize();
        }
    }
}
