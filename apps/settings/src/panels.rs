//! Panel registry and sidebar search (pure logic, no GTK).

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PanelId {
    Wifi,
    Network,
    Bluetooth,
    Display,
    Sound,
    Power,
    Multitasking,
    Appearance,
    Apps,
    Notifications,
    Extensions,
    Mouse,
    Keyboard,
    Accessibility,
    Privacy,
    DateTime,
    Users,
    About,
}

#[derive(Debug)]
pub struct PanelInfo {
    pub id: PanelId,
    /// Stable command-line name.
    pub name: &'static str,
    pub aliases: &'static [&'static str],
    pub title: &'static str,
    pub icon: &'static str,
    pub keywords: &'static [&'static str],
    /// Sidebar group; a separator is drawn between groups.
    pub group: u8,
    pub available: bool,
}

pub const PANELS: &[PanelInfo] = &[
    PanelInfo {
        id: PanelId::Wifi,
        name: "wifi",
        aliases: &["wi-fi", "wireless"],
        title: "Wi-Fi",
        icon: "network-wireless-symbolic",
        keywords: &[
            "network", "wireless", "wlan", "hotspot", "internet", "airplane",
        ],
        group: 0,
        available: false,
    },
    PanelInfo {
        id: PanelId::Network,
        name: "network",
        aliases: &["wired"],
        title: "Network",
        icon: "network-wired-symbolic",
        keywords: &["ethernet", "wired", "vpn", "proxy", "ip", "dns", "internet"],
        group: 0,
        available: false,
    },
    PanelInfo {
        id: PanelId::Bluetooth,
        name: "bluetooth",
        aliases: &[],
        title: "Bluetooth",
        icon: "bluetooth-symbolic",
        keywords: &["share", "devices", "pair", "headset", "wireless"],
        group: 0,
        available: false,
    },
    PanelInfo {
        id: PanelId::Display,
        name: "display",
        aliases: &["displays", "screen"],
        title: "Displays",
        icon: "video-display-symbolic",
        keywords: &[
            "monitor",
            "screen",
            "resolution",
            "refresh",
            "scale",
            "night light",
            "projector",
            "rotation",
        ],
        group: 1,
        available: false,
    },
    PanelInfo {
        id: PanelId::Sound,
        name: "sound",
        aliases: &["audio"],
        title: "Sound",
        icon: "audio-speakers-symbolic",
        keywords: &[
            "volume",
            "speaker",
            "microphone",
            "audio",
            "output",
            "input",
            "alert",
            "mute",
        ],
        group: 1,
        available: false,
    },
    PanelInfo {
        id: PanelId::Power,
        name: "power",
        aliases: &[],
        title: "Power",
        icon: "battery-full-symbolic",
        keywords: &[
            "battery",
            "suspend",
            "sleep",
            "dim",
            "blank",
            "power button",
            "hibernate",
            "energy",
        ],
        group: 1,
        available: true,
    },
    PanelInfo {
        id: PanelId::Multitasking,
        name: "multitasking",
        aliases: &["workspaces"],
        title: "Multitasking",
        icon: "view-grid-symbolic",
        keywords: &[
            "hot corner",
            "workspace",
            "screen edges",
            "tiling",
            "app switching",
            "multi-monitor",
        ],
        group: 1,
        available: true,
    },
    PanelInfo {
        id: PanelId::Appearance,
        name: "appearance",
        aliases: &["background", "wallpaper"],
        title: "Appearance",
        icon: "preferences-desktop-appearance-symbolic",
        keywords: &[
            "dark",
            "light",
            "style",
            "theme",
            "icon",
            "icons",
            "accent",
            "color",
            "colour",
            "wallpaper",
            "background",
        ],
        group: 1,
        available: true,
    },
    PanelInfo {
        id: PanelId::Apps,
        name: "apps",
        aliases: &["applications", "default-apps"],
        title: "Apps",
        icon: "view-app-grid-symbolic",
        keywords: &[
            "default",
            "applications",
            "permissions",
            "browser",
            "storage",
        ],
        group: 2,
        available: false,
    },
    PanelInfo {
        id: PanelId::Notifications,
        name: "notifications",
        aliases: &[],
        title: "Notifications",
        icon: "preferences-system-notifications-symbolic",
        keywords: &[
            "do not disturb",
            "dnd",
            "banner",
            "alerts",
            "lock screen",
            "messages",
        ],
        group: 2,
        available: true,
    },
    PanelInfo {
        id: PanelId::Extensions,
        name: "extensions",
        aliases: &["shell-extensions", "add-ons"],
        title: "Extensions",
        icon: "application-x-addon-symbolic",
        keywords: &[
            "add-on",
            "plugin",
            "panel item",
            "indicator",
            "quick settings",
            "dock",
            "sandbox",
            "permissions",
        ],
        group: 2,
        available: true,
    },
    PanelInfo {
        id: PanelId::Mouse,
        name: "mouse",
        aliases: &["touchpad", "mouse-touchpad"],
        title: "Mouse & Touchpad",
        icon: "input-mouse-symbolic",
        keywords: &[
            "touchpad",
            "pointer",
            "speed",
            "scroll",
            "natural",
            "tap",
            "click",
            "trackpad",
            "primary button",
            "acceleration",
        ],
        group: 3,
        available: true,
    },
    PanelInfo {
        id: PanelId::Keyboard,
        name: "keyboard",
        aliases: &["input-sources"],
        title: "Keyboard",
        icon: "input-keyboard-symbolic",
        keywords: &[
            "layout",
            "input source",
            "language",
            "repeat",
            "delay",
            "typing",
            "xkb",
        ],
        group: 3,
        available: true,
    },
    PanelInfo {
        id: PanelId::Accessibility,
        name: "accessibility",
        aliases: &["universal-access", "a11y"],
        title: "Accessibility",
        icon: "preferences-desktop-accessibility-symbolic",
        keywords: &[
            "high contrast",
            "large text",
            "zoom",
            "magnifier",
            "screen reader",
            "cursor size",
            "reduce animation",
            "sticky keys",
            "slow keys",
            "bounce keys",
            "mouse keys",
            "scrollbars",
            "screen keyboard",
            "double-click",
            "locate pointer",
        ],
        group: 4,
        available: true,
    },
    PanelInfo {
        id: PanelId::Privacy,
        name: "privacy",
        aliases: &["security", "screen-lock", "location"],
        title: "Privacy & Security",
        icon: "preferences-system-privacy-symbolic",
        keywords: &[
            "screen lock",
            "lock",
            "blank",
            "idle",
            "recent files",
            "file history",
            "trash",
            "temporary",
            "location",
            "camera",
            "microphone",
            "problem reporting",
        ],
        group: 4,
        available: true,
    },
    PanelInfo {
        id: PanelId::DateTime,
        name: "datetime",
        aliases: &["date-time", "time"],
        title: "Date & Time",
        icon: "preferences-system-time-symbolic",
        keywords: &[
            "clock",
            "time zone",
            "timezone",
            "ntp",
            "24-hour",
            "am/pm",
            "week numbers",
            "calendar",
        ],
        group: 5,
        available: true,
    },
    PanelInfo {
        id: PanelId::Users,
        name: "users",
        aliases: &["user-accounts"],
        title: "Users",
        icon: "system-users-symbolic",
        keywords: &["account", "password", "login", "avatar", "administrator"],
        group: 5,
        available: false,
    },
    PanelInfo {
        id: PanelId::About,
        name: "about",
        aliases: &["info-overview", "info", "system"],
        title: "About",
        icon: "help-about-symbolic",
        keywords: &[
            "device name",
            "hostname",
            "memory",
            "processor",
            "cpu",
            "disk",
            "os",
            "version",
            "kernel",
            "hardware",
        ],
        group: 5,
        available: true,
    },
];

pub fn info(id: PanelId) -> &'static PanelInfo {
    PANELS
        .iter()
        .find(|p| p.id == id)
        .expect("every PanelId is registered")
}

/// Resolves a command-line panel name or alias (case-insensitive).
pub fn lookup(name: &str) -> Option<PanelId> {
    let name = name.trim().to_lowercase();
    PANELS
        .iter()
        .find(|p| p.name == name || p.aliases.contains(&name.as_str()))
        .map(|p| p.id)
}

/// Returns the matching panels in display order (best matches first).
/// An empty query returns every panel in registry order.
pub fn search(query: &str) -> Vec<PanelId> {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return PANELS.iter().map(|p| p.id).collect();
    }
    let terms: Vec<&str> = q.split_whitespace().collect();
    let mut ranked: Vec<(u8, usize, PanelId)> = PANELS
        .iter()
        .enumerate()
        .filter_map(|(i, p)| rank(p, &q, &terms).map(|r| (r, i, p.id)))
        .collect();
    ranked.sort_by_key(|&(r, i, _)| (r, i));
    ranked.into_iter().map(|(_, _, id)| id).collect()
}

/// The sidebar's rows for `query`: [`search`] order, but panels that
/// work come first and placeholders ("Not yet available") last.
pub fn sidebar_order(query: &str) -> Vec<PanelId> {
    let mut ids = search(query);
    ids.sort_by_key(|id| !info(*id).available);
    ids
}

/// Lower rank is a better match; `None` means no match.
fn rank(p: &PanelInfo, query: &str, terms: &[&str]) -> Option<u8> {
    let title = p.title.to_lowercase();
    let all_match = terms
        .iter()
        .all(|t| title.contains(t) || p.keywords.iter().any(|k| k.contains(t)));
    if !all_match {
        return None;
    }
    if title.starts_with(query) {
        Some(0)
    } else if terms.iter().any(|t| title.contains(t)) {
        Some(1)
    } else {
        Some(2)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_id_registered_once() {
        let ids = [
            PanelId::Wifi,
            PanelId::Network,
            PanelId::Bluetooth,
            PanelId::Display,
            PanelId::Sound,
            PanelId::Power,
            PanelId::Multitasking,
            PanelId::Appearance,
            PanelId::Apps,
            PanelId::Notifications,
            PanelId::Extensions,
            PanelId::Mouse,
            PanelId::Keyboard,
            PanelId::Accessibility,
            PanelId::Privacy,
            PanelId::DateTime,
            PanelId::Users,
            PanelId::About,
        ];
        assert_eq!(ids.len(), PANELS.len());
        for id in ids {
            assert_eq!(PANELS.iter().filter(|p| p.id == id).count(), 1, "{id:?}");
        }
    }

    #[test]
    fn names_and_aliases_unique() {
        let mut all: Vec<&str> = Vec::new();
        for p in PANELS {
            all.push(p.name);
            all.extend(p.aliases);
        }
        let n = all.len();
        all.sort();
        all.dedup();
        assert_eq!(n, all.len());
    }

    #[test]
    fn groups_are_contiguous() {
        let groups: Vec<u8> = PANELS.iter().map(|p| p.group).collect();
        assert!(groups.windows(2).all(|w| w[0] <= w[1]));
    }

    #[test]
    fn availability() {
        assert!(info(PanelId::Appearance).available);
        assert!(info(PanelId::About).available);
        for id in [
            PanelId::Wifi,
            PanelId::Network,
            PanelId::Bluetooth,
            PanelId::Display,
            PanelId::Sound,
            PanelId::Users,
            PanelId::Apps,
        ] {
            assert!(!info(id).available, "{id:?}");
        }
    }

    #[test]
    fn lookup_names_and_aliases() {
        assert_eq!(lookup("appearance"), Some(PanelId::Appearance));
        assert_eq!(lookup("background"), Some(PanelId::Appearance));
        assert_eq!(lookup("Universal-Access"), Some(PanelId::Accessibility));
        assert_eq!(lookup("info-overview"), Some(PanelId::About));
        assert_eq!(lookup(" datetime "), Some(PanelId::DateTime));
        assert_eq!(lookup("display"), Some(PanelId::Display));
        assert_eq!(lookup("add-ons"), Some(PanelId::Extensions));
        assert_eq!(lookup("nonsense"), None);
    }

    #[test]
    fn empty_query_lists_all() {
        assert_eq!(search("").len(), PANELS.len());
        assert_eq!(search("   ")[0], PanelId::Wifi);
    }

    #[test]
    fn placeholders_listed_last() {
        let order = sidebar_order("");
        assert_eq!(order.len(), PANELS.len());
        let first_placeholder = order.iter().position(|id| !info(*id).available).unwrap();
        assert!(order[first_placeholder..]
            .iter()
            .all(|id| !info(*id).available));
        // Working panels keep their GNOME order.
        assert_eq!(order[0], PanelId::Power);
        // Searching keeps the rule: a working match before a placeholder.
        let r = sidebar_order("wireless");
        assert!(r.iter().all(|id| !info(*id).available) || info(r[0]).available);
    }

    #[test]
    fn title_match() {
        assert_eq!(search("power"), vec![PanelId::Power]);
        assert_eq!(search("Date")[0], PanelId::DateTime);
        assert_eq!(search("KEYB")[0], PanelId::Keyboard);
    }

    #[test]
    fn keyword_match() {
        assert_eq!(search("dark"), vec![PanelId::Appearance]);
        assert_eq!(search("wallpaper"), vec![PanelId::Appearance]);
        assert_eq!(search("hot corner"), vec![PanelId::Multitasking]);
        assert!(search("timezone").contains(&PanelId::DateTime));
        assert!(search("battery").contains(&PanelId::Power));
    }

    #[test]
    fn all_terms_must_match() {
        assert_eq!(search("screen reader"), vec![PanelId::Accessibility]);
        assert!(search("dark zebra").is_empty());
    }

    #[test]
    fn ranking_title_prefix_first() {
        // "lock" is a keyword of Privacy and Notifications, and no title contains it.
        let r = search("lock");
        assert!(r.contains(&PanelId::Privacy));
        assert!(r.contains(&PanelId::Notifications));
        // "mouse" is in the title of Mouse & Touchpad and a keyword of Accessibility.
        let r = search("mouse");
        assert_eq!(r[0], PanelId::Mouse);
        assert!(r.contains(&PanelId::Accessibility));
        // Title-contains ranks above keyword-only.
        let r = search("time");
        assert_eq!(r[0], PanelId::DateTime);
    }
}
