//! Style (`theme` key) and built-in colour palettes.

/// Values of the `org.gnome.Console` `theme` key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Theme {
    /// Follow the system light/dark preference.
    Auto,
    /// Light.
    Day,
    /// Dark.
    Night,
    /// Green on black (dark).
    Hacker,
}

impl Theme {
    pub fn from_nick(nick: &str) -> Theme {
        match nick {
            "auto" => Theme::Auto,
            "day" => Theme::Day,
            "hacker" => Theme::Hacker,
            _ => Theme::Night,
        }
    }

    pub fn nick(self) -> &'static str {
        match self {
            Theme::Auto => "auto",
            Theme::Day => "day",
            Theme::Night => "night",
            Theme::Hacker => "hacker",
        }
    }

    /// Index in the Preferences style combo (Follow System, Light, Dark).
    pub fn combo_index(self) -> u32 {
        match self {
            Theme::Auto => 0,
            Theme::Day => 1,
            Theme::Night | Theme::Hacker => 2,
        }
    }

    pub fn from_combo_index(i: u32) -> Theme {
        match i {
            0 => Theme::Auto,
            1 => Theme::Day,
            _ => Theme::Night,
        }
    }

    /// Whether the UI should be dark, given the system preference.
    pub fn is_dark(self, system_dark: bool) -> bool {
        match self {
            Theme::Auto => system_dark,
            Theme::Day => false,
            Theme::Night | Theme::Hacker => true,
        }
    }
}

/// A terminal palette as `#rrggbb` strings.
#[derive(Debug, PartialEq, Eq)]
pub struct Palette {
    pub name: &'static str,
    pub foreground: &'static str,
    pub background: &'static str,
    pub cursor: &'static str,
    pub cursor_foreground: &'static str,
    pub highlight: &'static str,
    pub highlight_foreground: &'static str,
    /// ANSI colours 0–7 then bright 8–15.
    pub colors: [&'static str; 16],
}

pub const LIGHT: Palette = Palette {
    name: "Helix Light",
    foreground: "#2b2b30",
    background: "#fbfbfc",
    cursor: "#2b2b30",
    cursor_foreground: "#fbfbfc",
    highlight: "#a8c7f0",
    highlight_foreground: "#1b1b1f",
    colors: [
        "#24242a", "#c4262e", "#2a7f3b", "#8d6a00", "#2259b8", "#8a3aa8", "#127a83", "#d6d6db",
        "#5c5c66", "#e2464d", "#3b9b4e", "#b08700", "#3d76d6", "#a855c4", "#1d97a1", "#f4f4f6",
    ],
};

pub const DARK: Palette = Palette {
    name: "Helix Dark",
    foreground: "#e6e6ea",
    background: "#1e1e22",
    cursor: "#e6e6ea",
    cursor_foreground: "#1e1e22",
    highlight: "#3d5a80",
    highlight_foreground: "#ffffff",
    colors: [
        "#2a2a30", "#ee5b5f", "#5fcf74", "#e8c45a", "#5c9cf0", "#c27ae0", "#4cc8d2", "#c8c8ce",
        "#6a6a74", "#ff7f82", "#82e595", "#f6da82", "#86b8ff", "#d89cf0", "#7ee0e8", "#ffffff",
    ],
};

pub const HACKER: Palette = Palette {
    name: "Hacker",
    foreground: "#33ff66",
    background: "#050805",
    cursor: "#33ff66",
    cursor_foreground: "#050805",
    highlight: "#1f5f2f",
    highlight_foreground: "#b8ffc8",
    colors: [
        "#0b120b", "#3fae5a", "#33ff66", "#7ce38b", "#2f9c4a", "#4fd36e", "#66ff99", "#a8f0b8",
        "#1d3a22", "#5fd47a", "#66ff88", "#a0f5ad", "#48c068", "#77ee93", "#99ffbb", "#d8ffe0",
    ],
};

/// The palette for `theme` given the system dark preference.
pub fn palette_for(theme: Theme, system_dark: bool) -> &'static Palette {
    match theme {
        Theme::Hacker => &HACKER,
        t if t.is_dark(system_dark) => &DARK,
        _ => &LIGHT,
    }
}

/// Parses `#rrggbb` into 0.0–1.0 components.
pub fn parse_hex(s: &str) -> Option<(f32, f32, f32)> {
    let h = s.strip_prefix('#')?;
    if h.len() != 6 || !h.is_ascii() {
        return None;
    }
    let c = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).ok().map(|v| v as f32 / 255.0);
    Some((c(0)?, c(2)?, c(4)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nicks_round_trip() {
        for t in [Theme::Auto, Theme::Day, Theme::Night, Theme::Hacker] {
            assert_eq!(Theme::from_nick(t.nick()), t);
        }
        assert_eq!(Theme::from_nick("bogus"), Theme::Night);
    }

    #[test]
    fn combo_mapping() {
        for i in 0..3 {
            assert_eq!(Theme::from_combo_index(i).combo_index(), i);
        }
        assert_eq!(Theme::Hacker.combo_index(), 2);
    }

    #[test]
    fn selection() {
        assert_eq!(palette_for(Theme::Auto, true).name, "Helix Dark");
        assert_eq!(palette_for(Theme::Auto, false).name, "Helix Light");
        assert_eq!(palette_for(Theme::Day, true).name, "Helix Light");
        assert_eq!(palette_for(Theme::Night, false).name, "Helix Dark");
        assert_eq!(palette_for(Theme::Hacker, false).name, "Hacker");
    }

    #[test]
    fn palettes_valid_and_readable() {
        for p in [&LIGHT, &DARK, &HACKER] {
            let all = [p.foreground, p.background, p.cursor, p.cursor_foreground, p.highlight, p.highlight_foreground];
            for c in all.iter().chain(p.colors.iter()) {
                assert!(parse_hex(c).is_some(), "{} {c}", p.name);
            }
            let lum = |s| {
                let (r, g, b) = parse_hex(s).unwrap();
                0.2126 * r + 0.7152 * g + 0.0722 * b
            };
            assert!((lum(p.foreground) - lum(p.background)).abs() > 0.5, "{}", p.name);
        }
    }

    #[test]
    fn hex() {
        assert_eq!(parse_hex("#ff0000"), Some((1.0, 0.0, 0.0)));
        assert_eq!(parse_hex("ff0000"), None);
        assert_eq!(parse_hex("#ff00"), None);
        assert_eq!(parse_hex("#gg0000"), None);
    }
}
