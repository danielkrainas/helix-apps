//! Editor widgets: a `GtkTextView` subclass that draws the current-line
//! highlight and right margin, and a line-number gutter, plus the colours
//! of search matches.
//!
//! Colours come from the style (the view's foreground colour, the accent
//! colour, dark and high contrast) and are redrawn when it changes.

use gtk::gdk::RGBA;
use gtk::glib;
use gtk::prelude::*;
use gtk::subclass::prelude::*;

/// How strongly line numbers are drawn, as a share of the foreground
/// colour's alpha: at least 4.5:1 against the view in light and dark
/// (see the tests), full strength in high contrast.
pub fn gutter_alpha(high_contrast: bool) -> f32 {
    if high_contrast {
        1.0
    } else {
        0.75
    }
}

/// Current-line highlight, right-margin line and the fill beyond the
/// margin, as alphas of the foreground colour.
fn decoration_alphas(dark: bool, high_contrast: bool) -> (f32, f32, f32) {
    match (high_contrast, dark) {
        (true, _) => (0.14, 0.6, 0.05),
        (false, true) => (0.08, 0.2, 0.03),
        (false, false) => (0.06, 0.18, 0.03),
    }
}

/// Text colours for a search match.
#[derive(Debug, Clone, PartialEq)]
pub struct MatchStyle {
    pub background: RGBA,
    pub foreground: RGBA,
    /// Underlined in high contrast (double for the current match), so
    /// matches are marked by more than colour.
    pub underline: gtk::pango::Underline,
}

/// Adwaita yellow 3 and 5: the usual highlighter colour for matches.
const MATCH_LIGHT: (f32, f32, f32) = (0.965, 0.827, 0.176);
const MATCH_DARK: (f32, f32, f32) = (0.898, 0.647, 0.039);

/// Colours for other matches (yellow) and the current one (the accent
/// colour). Each comes with black or white text, whichever contrasts more
/// with its background — at least 4.5:1 for any background.
pub fn match_style(current: bool, dark: bool, high_contrast: bool, accent: &RGBA) -> MatchStyle {
    let background = if current {
        RGBA::new(accent.red(), accent.green(), accent.blue(), 1.0)
    } else {
        let (r, g, b) = if dark { MATCH_DARK } else { MATCH_LIGHT };
        RGBA::new(r, g, b, 1.0)
    };
    let underline = match (high_contrast, current) {
        (false, _) => gtk::pango::Underline::None,
        (true, false) => gtk::pango::Underline::Single,
        (true, true) => gtk::pango::Underline::Double,
    };
    MatchStyle {
        foreground: readable_on(&background),
        background,
        underline,
    }
}

/// WCAG relative luminance of an opaque colour.
fn luminance(c: &RGBA) -> f64 {
    let lin = |v: f32| {
        let v = v as f64;
        if v <= 0.03928 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * lin(c.red()) + 0.7152 * lin(c.green()) + 0.0722 * lin(c.blue())
}

/// WCAG contrast ratio of two opaque colours.
pub fn contrast(a: &RGBA, b: &RGBA) -> f64 {
    let (la, lb) = (luminance(a), luminance(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

/// Black or white, whichever contrasts more with `bg`.
fn readable_on(bg: &RGBA) -> RGBA {
    if contrast(bg, &RGBA::BLACK) >= contrast(bg, &RGBA::WHITE) {
        RGBA::BLACK
    } else {
        RGBA::WHITE
    }
}

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
                let fg = view.color();
                let shade = |a: f32| RGBA::new(fg.red(), fg.green(), fg.blue(), a);
                let (line_a, margin_a, fill_a) = decoration_alphas(
                    helix_app_common::style::is_dark(),
                    helix_app_common::style::is_high_contrast(),
                );
                if self.highlight_line.get() {
                    let buffer = view.buffer();
                    let iter = buffer.iter_at_mark(&buffer.get_insert());
                    let (y, h) = view.line_yrange(&iter);
                    let color = shade(line_a);
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
                    let line = shade(margin_a);
                    let fill = shade(fill_a);
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
            let dim = RGBA::new(
                fg.red(),
                fg.green(),
                fg.blue(),
                fg.alpha() * gutter_alpha(helix_app_common::style::is_high_contrast()),
            );
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
        let view: Self = glib::Object::builder().property("buffer", buffer).build();
        helix_app_common::style::redraw_on_style_change(&view);
        view
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
        helix_app_common::style::redraw_on_style_change(&g);
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

#[cfg(test)]
mod tests {
    use super::*;

    /// `fg` at `alpha` over the opaque `bg`.
    fn over(fg: &RGBA, alpha: f32, bg: &RGBA) -> RGBA {
        let mix = |f: f32, b: f32| f * alpha + b * (1.0 - alpha);
        RGBA::new(
            mix(fg.red(), bg.red()),
            mix(fg.green(), bg.green()),
            mix(fg.blue(), bg.blue()),
            1.0,
        )
    }

    #[test]
    fn line_numbers_meet_contrast() {
        // Adwaita's view colours: light #ffffff with rgb(0 0 6 / 80%)
        // text, dark #1d1d20 with white text.
        let light_bg = RGBA::WHITE;
        let light_fg = RGBA::new(0.0, 0.0, 6.0 / 255.0, 1.0);
        let dark_bg = RGBA::new(29.0 / 255.0, 29.0 / 255.0, 32.0 / 255.0, 1.0);
        for hc in [false, true] {
            let a = gutter_alpha(hc);
            let light = over(&light_fg, 0.8 * a, &light_bg);
            let dark = over(&RGBA::WHITE, a, &dark_bg);
            assert!(contrast(&light, &light_bg) >= 4.5, "light hc={hc}");
            assert!(contrast(&dark, &dark_bg) >= 4.5, "dark hc={hc}");
        }
    }

    #[test]
    fn matches_meet_contrast_and_differ() {
        let accents = [
            RGBA::new(0.208, 0.518, 0.894, 1.0), // blue
            RGBA::new(0.914, 0.329, 0.125, 1.0), // orange
            RGBA::new(0.796, 0.286, 0.729, 1.0), // pink
            RGBA::new(0.435, 0.514, 0.588, 1.0), // slate
        ];
        for accent in &accents {
            for dark in [false, true] {
                for hc in [false, true] {
                    let other = match_style(false, dark, hc, accent);
                    let current = match_style(true, dark, hc, accent);
                    for s in [&other, &current] {
                        assert!(contrast(&s.foreground, &s.background) >= 4.5);
                    }
                    assert_ne!(other.background, current.background);
                    if hc {
                        assert_ne!(other.underline, current.underline);
                        assert_ne!(other.underline, gtk::pango::Underline::None);
                    }
                }
            }
        }
    }
}
