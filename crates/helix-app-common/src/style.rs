//! Following the desktop style in custom-drawn widgets.
//!
//! A `GtkDrawingArea` that reads its colours from the style when drawing
//! (`widget.color()`, the accent colour, [`is_dark`]/[`is_high_contrast`])
//! still keeps showing the old picture until something asks it to redraw.
//! [`redraw_on_style_change`] makes it redraw as soon as the dark or
//! high-contrast style, the accent colour, the interface font or the text
//! scaling changes.
//!
//! App CSS goes through [`add_css`], which can add a high-contrast layer
//! (loaded only while high contrast is on, so it also works where GTK
//! lacks `@media (prefers-contrast: more)`). Custom CSS and drawing use
//! the style's named colours ([`named_color`], `@accent_color`,
//! `@destructive_color`, `currentColor`…) rather than literal colours,
//! and lines drawn in the text colour use [`line_alpha`] so that they
//! reach 3:1 against the background in high contrast.

use adw::prelude::*;
use gtk::glib;

/// Opacity of separators and grid lines drawn in the text colour while
/// high contrast is on: with Adwaita's 80 %-opaque view text this is
/// still over 3:1 against the light and the dark view background.
pub const HIGH_CONTRAST_LINE_ALPHA: f64 = 0.6;

/// The opacity for a separator or grid line drawn in the text colour:
/// `normal`, raised to [`HIGH_CONTRAST_LINE_ALPHA`] in high contrast.
pub fn line_alpha(normal: f64) -> f64 {
    if is_high_contrast() {
        normal.max(HIGH_CONTRAST_LINE_ALPHA)
    } else {
        normal
    }
}

/// A named colour of the current style as `widget` sees it
/// (`"accent_color"`, `"destructive_color"`, `"success_color"`,
/// `"warning_color"`, `"borders"`…), for drawing code.
pub fn named_color(widget: &impl IsA<gtk::Widget>, name: &str) -> Option<gtk::gdk::RGBA> {
    // GtkStyleContext is deprecated, but it is still the only way to
    // resolve a named colour from code.
    #[allow(deprecated)]
    let color = widget.as_ref().style_context().lookup_color(name);
    color
}

/// Loads app CSS for the default display at application priority, plus
/// `high_contrast` (if not empty) on top of it while high contrast is on.
pub fn add_css(css: &str, high_contrast: &str) {
    let Some(display) = gtk::gdk::Display::default() else {
        return;
    };
    let provider = gtk::CssProvider::new();
    provider.load_from_string(css);
    gtk::style_context_add_provider_for_display(
        &display,
        &provider,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
    if high_contrast.trim().is_empty() {
        return;
    }
    let hc = gtk::CssProvider::new();
    hc.load_from_string(high_contrast);
    let apply = move |on: bool| {
        if on {
            gtk::style_context_add_provider_for_display(
                &display,
                &hc,
                gtk::STYLE_PROVIDER_PRIORITY_APPLICATION + 1,
            );
        } else {
            gtk::style_context_remove_provider_for_display(&display, &hc);
        }
    };
    let style = adw::StyleManager::default();
    apply(style.is_high_contrast());
    style.connect_high_contrast_notify(move |s| apply(s.is_high_contrast()));
}

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

#[cfg(test)]
mod tests {
    use super::HIGH_CONTRAST_LINE_ALPHA;

    fn linear(c: f64) -> f64 {
        if c <= 0.03928 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    }

    fn contrast(a: f64, b: f64) -> f64 {
        let (la, lb) = (linear(a), linear(b));
        (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
    }

    /// Grey levels are enough: Adwaita's view colours are near-neutral.
    #[test]
    fn high_contrast_lines_reach_three_to_one() {
        // Light: rgb(0 0 6 / 80%) text on white; dark: white on #1e1e1e.
        let a = HIGH_CONTRAST_LINE_ALPHA * 0.8;
        let light_line = 1.0 - a;
        assert!(contrast(light_line, 1.0) >= 3.0);
        let bg = 30.0 / 255.0;
        let dark_line = HIGH_CONTRAST_LINE_ALPHA + bg * (1.0 - HIGH_CONTRAST_LINE_ALPHA);
        assert!(contrast(dark_line, bg) >= 3.0);
    }
}
