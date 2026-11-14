//! Value mappings between GSettings keys and what the panels show.

/// A preset choice: stored value and its label.
#[derive(Debug, Clone, PartialEq)]
pub struct Choice<T> {
    pub value: T,
    pub label: String,
}

/// Formats a duration in seconds as e.g. "30 seconds", "5 minutes",
/// "1 hour", "1 hour 30 minutes".
pub fn format_duration(secs: u32) -> String {
    fn plural(n: u32, unit: &str) -> String {
        if n == 1 {
            format!("1 {unit}")
        } else {
            format!("{n} {unit}s")
        }
    }
    if secs < 60 {
        return plural(secs, "second");
    }
    let hours = secs / 3600;
    let minutes = (secs % 3600) / 60;
    let seconds = secs % 60;
    let mut parts = Vec::new();
    if hours > 0 {
        parts.push(plural(hours, "hour"));
    }
    if minutes > 0 {
        parts.push(plural(minutes, "minute"));
    }
    if seconds > 0 {
        parts.push(plural(seconds, "second"));
    }
    parts.join(" ")
}

fn duration_choices(secs: &[u32], zero_label: Option<&str>) -> Vec<Choice<i64>> {
    secs.iter()
        .map(|&s| Choice {
            value: s as i64,
            label: match (s, zero_label) {
                (0, Some(l)) => l.to_string(),
                _ => format_duration(s),
            },
        })
        .collect()
}

/// `org.gnome.desktop.session idle-delay` presets (seconds; 0 = never).
pub fn blank_delays() -> Vec<Choice<i64>> {
    duration_choices(
        &[60, 120, 180, 240, 300, 480, 600, 720, 900, 0],
        Some("Never"),
    )
}

/// `org.gnome.desktop.screensaver lock-delay` presets (seconds).
pub fn lock_delays() -> Vec<Choice<i64>> {
    duration_choices(
        &[0, 30, 60, 120, 180, 300, 1800, 3600],
        Some("Screen Turns Off"),
    )
}

/// `sleep-inactive-*-timeout` presets (seconds).
pub fn suspend_delays() -> Vec<Choice<i64>> {
    duration_choices(
        &[900, 1200, 1500, 1800, 2700, 3600, 4800, 5400, 6000, 7200],
        None,
    )
}

fn days_label(d: i64) -> String {
    if d == 1 {
        "1 day".into()
    } else {
        format!("{d} days")
    }
}

/// `org.gnome.desktop.privacy recent-files-max-age` presets (days; -1 = forever).
pub fn recent_ages() -> Vec<Choice<i64>> {
    [1, 7, 30, -1]
        .into_iter()
        .map(|d| Choice {
            value: d,
            label: if d < 0 {
                "Forever".into()
            } else {
                days_label(d)
            },
        })
        .collect()
}

/// `org.gnome.desktop.privacy old-files-age` presets (days).
pub fn old_files_ages() -> Vec<Choice<i64>> {
    [1, 2, 3, 4, 5, 6, 7, 14, 30]
        .into_iter()
        .map(|d| Choice {
            value: d,
            label: days_label(d),
        })
        .collect()
}

/// Returns `presets` plus an entry for `current` when it is not one of them,
/// together with the index of `current`. Custom entries are inserted in
/// ascending order among the non-negative, non-zero presets so the list stays
/// sorted; special values (0, negative) keep their place.
pub fn with_current(
    mut presets: Vec<Choice<i64>>,
    current: i64,
    label: impl Fn(i64) -> String,
) -> (Vec<Choice<i64>>, usize) {
    if let Some(i) = presets.iter().position(|c| c.value == current) {
        return (presets, i);
    }
    let pos = presets
        .iter()
        .position(|c| c.value > 0 && c.value > current)
        .or_else(|| presets.iter().rposition(|c| c.value > 0).map(|i| i + 1))
        .unwrap_or(presets.len());
    presets.insert(
        pos,
        Choice {
            value: current,
            label: label(current),
        },
    );
    (presets, pos)
}

/// Accent colours of `org.gnome.desktop.interface accent-color`.
pub const ACCENT_COLORS: &[(&str, &str, &str)] = &[
    ("blue", "Blue", "#3584e4"),
    ("teal", "Teal", "#2190a4"),
    ("green", "Green", "#3a944a"),
    ("yellow", "Yellow", "#c88800"),
    ("orange", "Orange", "#ed5b00"),
    ("red", "Red", "#e62d42"),
    ("pink", "Pink", "#d56199"),
    ("purple", "Purple", "#9141ac"),
    ("slate", "Slate", "#6f8396"),
];

/// Normalises an `accent-color` nick; unknown values become `blue`.
pub fn accent_nick(nick: &str) -> &'static str {
    ACCENT_COLORS
        .iter()
        .find(|c| c.0 == nick)
        .map(|c| c.0)
        .unwrap_or("blue")
}

/// Style cards of the Appearance panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Style {
    Default,
    Dark,
}

/// Maps `color-scheme` to the card shown as selected.
pub fn style_from_scheme(nick: &str) -> Style {
    if nick == "prefer-dark" {
        Style::Dark
    } else {
        Style::Default
    }
}

pub fn scheme_for_style(style: Style) -> &'static str {
    match style {
        Style::Default => "default",
        Style::Dark => "prefer-dark",
    }
}

pub const LARGE_TEXT_FACTOR: f64 = 1.25;

pub fn large_text_enabled(factor: f64) -> bool {
    factor >= LARGE_TEXT_FACTOR - 1e-6
}

pub fn text_factor(large: bool) -> f64 {
    if large {
        LARGE_TEXT_FACTOR
    } else {
        1.0
    }
}

pub const CURSOR_SIZES: &[(i32, &str)] = &[
    (24, "Default"),
    (32, "Medium"),
    (48, "Large"),
    (64, "Larger"),
    (96, "Largest"),
];

/// Index of the closest cursor size preset.
pub fn cursor_size_index(size: i32) -> usize {
    CURSOR_SIZES
        .iter()
        .enumerate()
        .min_by_key(|(_, (s, _))| (s - size).abs())
        .map(|(i, _)| i)
        .unwrap_or(0)
}

pub const POWER_BUTTON_ACTIONS: &[(&str, &str)] = &[
    ("suspend", "Suspend"),
    ("interactive", "Power Off"),
    ("hibernate", "Hibernate"),
    ("nothing", "Nothing"),
];

/// Keyboard repeat interval bounds (ms) for the speed slider.
pub const REPEAT_FAST_MS: f64 = 10.0;
pub const REPEAT_SLOW_MS: f64 = 110.0;

/// Slider position (0 = slow, 1 = fast) → `repeat-interval` in ms.
pub fn speed_to_interval(pos: f64) -> u32 {
    let p = pos.clamp(0.0, 1.0);
    (REPEAT_SLOW_MS - (REPEAT_SLOW_MS - REPEAT_FAST_MS) * p).round() as u32
}

/// `repeat-interval` in ms → slider position.
pub fn interval_to_speed(ms: u32) -> f64 {
    ((REPEAT_SLOW_MS - ms as f64) / (REPEAT_SLOW_MS - REPEAT_FAST_MS)).clamp(0.0, 1.0)
}

/// Turns a free-form device name into a valid static hostname.
pub fn pretty_to_static_hostname(pretty: &str) -> String {
    let mut out = String::new();
    for c in pretty.trim().chars() {
        let c = c.to_ascii_lowercase();
        let mapped = match c {
            'a'..='z' | '0'..='9' => Some(c),
            ' ' | '_' | '-' | '.' => Some('-'),
            _ => None,
        };
        if let Some(m) = mapped {
            if m == '-' && (out.is_empty() || out.ends_with('-')) {
                continue;
            }
            out.push(m);
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    out.truncate(64);
    while out.ends_with('-') {
        out.pop();
    }
    if out.is_empty() {
        "localhost".into()
    } else {
        out
    }
}

/// UPower device state → label.
pub fn battery_state_label(state: u32) -> &'static str {
    match state {
        1 => "Charging",
        2 => "Discharging",
        3 => "Empty",
        4 => "Fully Charged",
        5 => "Pending Charge",
        6 => "Pending Discharge",
        _ => "Unknown",
    }
}

/// "2 hours 5 minutes until fully charged" style hint, or `None`.
pub fn battery_time_label(state: u32, time_to_empty: i64, time_to_full: i64) -> Option<String> {
    let fmt = |secs: i64| {
        let mins = (secs / 60) as u32;
        format_duration(mins.max(1) * 60)
    };
    match state {
        1 if time_to_full > 0 => Some(format!("{} until fully charged", fmt(time_to_full))),
        2 if time_to_empty > 0 => Some(format!("{} remaining", fmt(time_to_empty))),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compat_schema_has_nicks() {
        let xml = include_str!("../data/org.helix.Settings.gnome-compat.gschema.xml");
        let nicks = ACCENT_COLORS
            .iter()
            .map(|c| c.0)
            .chain(POWER_BUTTON_ACTIONS.iter().map(|a| a.0))
            .chain([
                "default",
                "prefer-dark",
                "prefer-light",
                "24h",
                "12h",
                "fingers",
                "areas",
                "flat",
                "enabled",
                "disabled",
            ]);
        for nick in nicks {
            assert!(xml.contains(&format!("nick=\"{nick}\"")), "{nick}");
        }
    }

    #[test]
    fn durations() {
        assert_eq!(format_duration(1), "1 second");
        assert_eq!(format_duration(30), "30 seconds");
        assert_eq!(format_duration(60), "1 minute");
        assert_eq!(format_duration(300), "5 minutes");
        assert_eq!(format_duration(3600), "1 hour");
        assert_eq!(format_duration(5400), "1 hour 30 minutes");
        assert_eq!(format_duration(7200), "2 hours");
    }

    #[test]
    fn delay_presets() {
        let b = blank_delays();
        assert_eq!(b[0].value, 60);
        assert_eq!(b.last().unwrap().label, "Never");
        assert_eq!(b.last().unwrap().value, 0);
        let l = lock_delays();
        assert_eq!(l[0].label, "Screen Turns Off");
        assert_eq!(l[1].label, "30 seconds");
        assert_eq!(l.last().unwrap().label, "1 hour");
        assert_eq!(suspend_delays()[0].label, "15 minutes");
        assert_eq!(recent_ages().last().unwrap().value, -1);
        assert_eq!(old_files_ages()[0].label, "1 day");
    }

    #[test]
    fn current_value_is_preset() {
        let (list, i) = with_current(blank_delays(), 300, |v| format_duration(v as u32));
        assert_eq!(list.len(), blank_delays().len());
        assert_eq!(list[i].value, 300);
        let (_, i) = with_current(blank_delays(), 0, |v| format_duration(v as u32));
        assert_eq!(i, blank_delays().len() - 1);
    }

    #[test]
    fn custom_value_inserted_sorted() {
        let (list, i) = with_current(blank_delays(), 420, |v| format_duration(v as u32));
        assert_eq!(list.len(), blank_delays().len() + 1);
        assert_eq!(list[i].label, "7 minutes");
        assert_eq!(list[i - 1].value, 300);
        assert_eq!(list[i + 1].value, 480);
        // Larger than every preset: after the largest positive, before "Never".
        let (list, i) = with_current(blank_delays(), 3600, |v| format_duration(v as u32));
        assert_eq!(list[i].value, 3600);
        assert_eq!(list[i + 1].label, "Never");
        // Lock delay: 45 s goes between 30 s and 1 minute.
        let (list, i) = with_current(lock_delays(), 45, |v| format_duration(v as u32));
        assert_eq!((list[i - 1].value, list[i + 1].value), (30, 60));
        // Days list with "Forever" (-1) at the end.
        let (list, i) = with_current(recent_ages(), 90, days_label);
        assert_eq!(list[i].label, "90 days");
        assert_eq!(list[i + 1].label, "Forever");
    }

    #[test]
    fn accent_nicks() {
        assert_eq!(ACCENT_COLORS.len(), 9);
        assert_eq!(accent_nick("slate"), "slate");
        assert_eq!(accent_nick("teal"), "teal");
        assert_eq!(accent_nick("magenta"), "blue");
        assert_eq!(accent_nick(""), "blue");
    }

    #[test]
    fn styles() {
        assert_eq!(style_from_scheme("prefer-dark"), Style::Dark);
        assert_eq!(style_from_scheme("default"), Style::Default);
        assert_eq!(style_from_scheme("prefer-light"), Style::Default);
        assert_eq!(scheme_for_style(Style::Dark), "prefer-dark");
        assert_eq!(scheme_for_style(Style::Default), "default");
    }

    #[test]
    fn large_text() {
        assert!(!large_text_enabled(1.0));
        assert!(large_text_enabled(1.25));
        assert!(large_text_enabled(1.5));
        assert_eq!(text_factor(true), 1.25);
        assert_eq!(text_factor(false), 1.0);
    }

    #[test]
    fn cursor_sizes() {
        assert_eq!(cursor_size_index(24), 0);
        assert_eq!(cursor_size_index(48), 2);
        assert_eq!(cursor_size_index(50), 2);
        assert_eq!(cursor_size_index(200), 4);
        assert_eq!(cursor_size_index(0), 0);
    }

    #[test]
    fn power_button() {
        let nicks: Vec<&str> = POWER_BUTTON_ACTIONS.iter().map(|a| a.0).collect();
        assert_eq!(nicks, ["suspend", "interactive", "hibernate", "nothing"]);
    }

    #[test]
    fn repeat_speed_roundtrip() {
        assert_eq!(speed_to_interval(0.0), 110);
        assert_eq!(speed_to_interval(1.0), 10);
        assert_eq!(speed_to_interval(2.0), 10);
        assert_eq!(speed_to_interval(0.8), 30);
        assert!((interval_to_speed(30) - 0.8).abs() < 1e-9);
        assert_eq!(interval_to_speed(500), 0.0);
        assert_eq!(interval_to_speed(1), 1.0);
        for ms in 10..=110 {
            assert_eq!(speed_to_interval(interval_to_speed(ms)), ms);
        }
    }

    #[test]
    fn hostnames() {
        assert_eq!(pretty_to_static_hostname("Danny's Laptop"), "dannys-laptop");
        assert_eq!(pretty_to_static_hostname("  My  Box_2 "), "my-box-2");
        assert_eq!(pretty_to_static_hostname("--x--"), "x");
        assert_eq!(pretty_to_static_hostname("Ünïcode"), "ncode");
        assert_eq!(pretty_to_static_hostname("!!!"), "localhost");
        assert_eq!(pretty_to_static_hostname(&"a".repeat(80)).len(), 64);
        let s = pretty_to_static_hostname(&format!("{}-b", "a".repeat(63)));
        assert_eq!(s, "a".repeat(63));
    }

    #[test]
    fn battery_labels() {
        assert_eq!(battery_state_label(1), "Charging");
        assert_eq!(battery_state_label(4), "Fully Charged");
        assert_eq!(battery_state_label(99), "Unknown");
        assert_eq!(
            battery_time_label(2, 3900, 0).unwrap(),
            "1 hour 5 minutes remaining"
        );
        assert_eq!(
            battery_time_label(1, 0, 600).unwrap(),
            "10 minutes until fully charged"
        );
        assert_eq!(battery_time_label(4, 0, 0), None);
        assert_eq!(battery_time_label(2, 0, 0), None);
    }
}
