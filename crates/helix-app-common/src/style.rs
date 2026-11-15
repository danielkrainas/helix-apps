//! Following the desktop style in custom-drawn widgets.
//!
//! A `GtkDrawingArea` that reads its colours from the style when drawing
//! (`widget.color()`, the accent colour, [`is_dark`]/[`is_high_contrast`])
//! still keeps showing the old picture until something asks it to redraw.
//! [`redraw_on_style_change`] makes it redraw as soon as the dark or
//! high-contrast style, the accent colour, the interface font or the text
//! scaling changes.

use adw::prelude::*;
use gtk::glib;

/// Whether the dark style is in use.
pub fn is_dark() -> bool {
    adw::StyleManager::default().is_dark()
}

/// Whether the high-contrast style is in use.
pub fn is_high_contrast() -> bool {
    adw::StyleManager::default().is_high_contrast()
}

/// Adwaita's default accent (blue 3), used when the system has none.
const DEFAULT_ACCENT: (f32, f32, f32) = (0.208, 0.518, 0.894);

/// The system accent colour (the background variant, as used for
/// selections), from libadwaita 1.6's `accent-color-rgba`; Adwaita blue on
/// older versions.
pub fn accent_rgba() -> gtk::gdk::RGBA {
    let style = adw::StyleManager::default();
    if style.find_property("accent-color-rgba").is_some() {
        return style.property::<gtk::gdk::RGBA>("accent-color-rgba");
    }
    let (r, g, b) = DEFAULT_ACCENT;
    gtk::gdk::RGBA::new(r, g, b, 1.0)
}

/// Queues a redraw of `widget` whenever the style, accent colour, font or
/// text scaling changes. The handlers are removed when `widget` is
/// destroyed.
pub fn redraw_on_style_change(widget: &impl IsA<gtk::Widget>) {
    let weak = widget.as_ref().downgrade();
    let redraw = move || {
        if let Some(w) = weak.upgrade() {
            w.queue_draw();
        }
    };
    let mut handlers: Vec<(glib::Object, glib::SignalHandlerId)> = Vec::new();
    let style = adw::StyleManager::default();
    for prop in ["dark", "high-contrast", "accent-color"] {
        // `accent-color` needs libadwaita 1.6; skip it on older versions.
        if style.find_property(prop).is_none() {
            continue;
        }
        let r = redraw.clone();
        let id = style.connect_notify_local(Some(prop), move |_, _| r());
        handlers.push((style.clone().upcast(), id));
    }
    if let Some(settings) = gtk::Settings::default() {
        for prop in ["gtk-font-name", "gtk-xft-dpi"] {
            let r = redraw.clone();
            let id = settings.connect_notify_local(Some(prop), move |_, _| r());
            handlers.push((settings.clone().upcast(), id));
        }
    }
    let handlers = std::cell::RefCell::new(handlers);
    widget.as_ref().connect_destroy(move |_| {
        for (obj, id) in handlers.borrow_mut().drain(..) {
            obj.disconnect(id);
        }
    });
}
