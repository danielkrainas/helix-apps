//! Editor settings backed by the `org.gnome.TextEditor` schema.

use helix_app_common::Prefs;

pub const SCHEMA: &str = "org.gnome.TextEditor";

#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    pub show_line_numbers: bool,
    pub wrap_text: bool,
    pub tab_width: u32,
    pub indent_spaces: bool,
    pub auto_indent: bool,
    pub highlight_current_line: bool,
    pub use_system_font: bool,
    pub custom_font: String,
    pub style_variant: String,
    pub restore_session: bool,
    pub show_right_margin: bool,
    pub right_margin_position: u32,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            show_line_numbers: false,
            wrap_text: true,
            tab_width: 8,
            indent_spaces: false,
            auto_indent: true,
            highlight_current_line: false,
            use_system_font: true,
            custom_font: "Monospace 11".into(),
            style_variant: "follow".into(),
            restore_session: true,
            show_right_margin: false,
            right_margin_position: 80,
        }
    }
}

impl Settings {
    pub fn load(p: &Prefs) -> Self {
        let d = Settings::default();
        Settings {
            show_line_numbers: p.get("show-line-numbers", d.show_line_numbers),
            wrap_text: p.get("wrap-text", d.wrap_text),
            tab_width: p.get::<u32>("tab-width", d.tab_width).clamp(1, 32),
            indent_spaces: p.get_enum_nick("indent-style", "tab") == "space",
            auto_indent: p.get("auto-indent", d.auto_indent),
            highlight_current_line: p.get("highlight-current-line", d.highlight_current_line),
            use_system_font: p.get("use-system-font", d.use_system_font),
            custom_font: p.get("custom-font", d.custom_font),
            style_variant: p.get_enum_nick("style-variant", "follow"),
            restore_session: p.get("restore-session", d.restore_session),
            show_right_margin: p.get("show-right-margin", d.show_right_margin),
            right_margin_position: p.get::<u32>("right-margin-position", d.right_margin_position).clamp(1, 1000),
        }
    }

    pub fn indent(&self) -> crate::textops::Indent {
        crate::textops::Indent { tab_width: self.tab_width, use_spaces: self.indent_spaces }
    }
}

/// Minimum / maximum zoom steps (each step is 10 %).
pub const ZOOM_MIN: i32 = -7;
pub const ZOOM_MAX: i32 = 30;

pub fn zoom_factor(step: i32) -> f64 {
    1.0 + 0.1 * step.clamp(ZOOM_MIN, ZOOM_MAX) as f64
}

/// A parsed font: family list, size in points (or pixels), weight, italic.
#[derive(Debug, Clone, PartialEq)]
pub struct FontSpec {
    pub family: String,
    pub size: f64,
    pub size_px: bool,
    pub weight: u32,
    pub italic: bool,
}

/// CSS applying `font` scaled by `zoom` to `selector`.
pub fn font_css(selector: &str, font: &FontSpec, zoom: f64) -> String {
    let family = font
        .family
        .split(',')
        .map(|f| f.trim())
        .filter(|f| !f.is_empty())
        .map(|f| format!("\"{}\"", f.replace(['\\', '"'], "")))
        .chain(std::iter::once("monospace".to_string()))
        .collect::<Vec<_>>()
        .join(", ");
    let size = if font.size > 0.0 { font.size } else { 11.0 } * zoom;
    format!(
        "{selector} {{ font-family: {family}; font-size: {size:.1}{unit}; font-weight: {weight}; font-style: {style}; }}",
        unit = if font.size_px { "px" } else { "pt" },
        weight = font.weight,
        style = if font.italic { "italic" } else { "normal" },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn css() {
        let f = FontSpec { family: "Source Code Pro".into(), size: 10.0, size_px: false, weight: 400, italic: false };
        assert_eq!(
            font_css(".ed", &f, 1.2),
            ".ed { font-family: \"Source Code Pro\", monospace; font-size: 12.0pt; font-weight: 400; font-style: normal; }"
        );
        let f = FontSpec { family: "A\"b, Mono".into(), size: 0.0, size_px: true, weight: 700, italic: true };
        assert_eq!(
            font_css("x", &f, 1.0),
            "x { font-family: \"Ab\", \"Mono\", monospace; font-size: 11.0px; font-weight: 700; font-style: italic; }"
        );
    }

    #[test]
    fn zoom() {
        assert_eq!(zoom_factor(0), 1.0);
        assert!((zoom_factor(-100) - 0.3).abs() < 1e-9);
        assert!((zoom_factor(100) - 4.0).abs() < 1e-9);
    }
}
