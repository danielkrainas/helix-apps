//! Desktop style for help pages: the accent colour and high contrast,
//! as a user style sheet injected over `style.css`.
//!
//! `style.css` keeps its own light and dark palettes (chosen by
//! `prefers-color-scheme`, which WebKit follows). This adds, for both
//! schemes, a link colour derived from the system accent and made dark
//! (or light) enough to read on the page, and in high contrast pure
//! foreground and background colours, underlined links and solid rules.
//!
//! Selectors are `html:root` / `html …` so they outrank the page's own
//! `:root` / element rules whatever order WebKit applies the sheets in.

/// An opaque colour, components 0–1.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rgb(pub f64, pub f64, pub f64);

impl Rgb {
    fn hex(self) -> String {
        let c = |v: f64| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
        format!("#{:02x}{:02x}{:02x}", c(self.0), c(self.1), c(self.2))
    }

    fn mix(self, other: Rgb, t: f64) -> Rgb {
        let m = |a: f64, b: f64| a + (b - a) * t;
        Rgb(m(self.0, other.0), m(self.1, other.1), m(self.2, other.2))
    }

    fn luminance(self) -> f64 {
        let lin = |v: f64| {
            if v <= 0.03928 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * lin(self.0) + 0.7152 * lin(self.1) + 0.0722 * lin(self.2)
    }
}

const BLACK: Rgb = Rgb(0.0, 0.0, 0.0);
const WHITE: Rgb = Rgb(1.0, 1.0, 1.0);

/// Page backgrounds of `style.css` (`--bg`), light and dark.
const LIGHT_BG: Rgb = Rgb(1.0, 1.0, 1.0);
const DARK_BG: Rgb = Rgb(
    0x1e as f64 / 255.0,
    0x1e as f64 / 255.0,
    0x22 as f64 / 255.0,
);

/// WCAG contrast ratio.
pub fn contrast(a: Rgb, b: Rgb) -> f64 {
    let (la, lb) = (a.luminance(), b.luminance());
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

/// `accent` darkened (on a light page) or lightened (on a dark one) just
/// enough for text on `bg` to reach `ratio`.
pub fn readable(accent: Rgb, bg: Rgb, ratio: f64) -> Rgb {
    let toward = if bg.luminance() > 0.5 { BLACK } else { WHITE };
    let mut t = 0.0;
    loop {
        let c = accent.mix(toward, t);
        if contrast(c, bg) >= ratio || t >= 1.0 {
            return c;
        }
        t += 0.02;
    }
}

/// The injected style sheet for `accent` (the system accent colour) and
/// the high-contrast setting.
pub fn css(accent: Rgb, high_contrast: bool) -> String {
    let (light_bg, dark_bg) = if high_contrast {
        (WHITE, BLACK)
    } else {
        (LIGHT_BG, DARK_BG)
    };
    // Body text needs 4.5:1; high contrast asks for 7:1.
    let ratio = if high_contrast { 7.0 } else { 4.5 };
    let light = readable(accent, light_bg, ratio).hex();
    let dark = readable(accent, dark_bg, ratio).hex();
    let mut s = format!(
        "html:root {{ --accent: {light}; --note: {light}; }}\n\
         @media (prefers-color-scheme: dark) {{\n  \
         html:root {{ --accent: {dark}; --note: {dark}; }}\n}}\n"
    );
    if high_contrast {
        s.push_str(&format!(
            "html:root {{ --bg: {lb}; --fg: {lf}; --muted: {lf}; --rule: {lf}; \
             --code-bg: {lb}; --code-fg: {lf}; --kbd-bg: {lb}; --kbd-border: {lf}; \
             --sidebar-bg: {lb}; --accent-visited: var(--accent); }}\n\
             @media (prefers-color-scheme: dark) {{\n  \
             html:root {{ --bg: {db}; --fg: {df}; --muted: {df}; --rule: {df}; \
             --code-bg: {db}; --code-fg: {df}; --kbd-bg: {db}; --kbd-border: {df}; \
             --sidebar-bg: {db}; }}\n}}\n\
             html a, html a:visited {{ text-decoration: underline; }}\n\
             html pre, html code, html .note, html .figure, html blockquote {{ \
             border: 2px solid var(--fg); }}\n\
             html .note {{ border-left-width: 4px; }}\n\
             html h2 {{ border-bottom-width: 2px; }}\n\
             html :focus-visible {{ outline: 3px solid var(--accent); outline-offset: 2px; }}\n",
            lb = light_bg.hex(),
            lf = BLACK.hex(),
            db = dark_bg.hex(),
            df = WHITE.hex(),
        ));
    }
    s
}

/// The page backgrounds, for tests.
#[cfg(test)]
pub const BACKGROUNDS: [Rgb; 2] = [LIGHT_BG, DARK_BG];

#[cfg(test)]
mod tests {
    use super::*;

    const ACCENTS: [Rgb; 4] = [
        Rgb(0.208, 0.518, 0.894), // blue
        Rgb(0.914, 0.329, 0.125), // orange
        Rgb(0.961, 0.761, 0.067), // yellow
        Rgb(0.435, 0.514, 0.588), // slate
    ];

    #[test]
    fn links_are_readable() {
        for a in ACCENTS {
            for bg in BACKGROUNDS {
                assert!(contrast(readable(a, bg, 4.5), bg) >= 4.5);
            }
            assert!(contrast(readable(a, BLACK, 7.0), BLACK) >= 7.0);
            assert!(contrast(readable(a, WHITE, 7.0), WHITE) >= 7.0);
        }
    }

    #[test]
    fn readable_keeps_a_readable_accent() {
        // Already 4.5:1 on white: unchanged.
        let dark_blue = Rgb(0.1, 0.2, 0.6);
        assert_eq!(readable(dark_blue, LIGHT_BG, 4.5), dark_blue);
    }

    #[test]
    fn css_follows_accent_and_contrast() {
        let orange = css(ACCENTS[1], false);
        let blue = css(ACCENTS[0], false);
        assert_ne!(orange, blue);
        assert!(orange.contains("--accent: #"));
        assert!(!orange.contains("text-decoration"));
        let hc = css(ACCENTS[0], true);
        assert!(hc.contains("text-decoration: underline"));
        assert!(hc.contains("--muted: #000000"));
        assert!(hc.contains("--bg: #000000"));
    }
}
