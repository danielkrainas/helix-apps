//! Tour content. Text describes only what the Helix desktop actually does.

use crate::illustrations::Art;
use helix_app_common::i18n::n_;

pub struct Page {
    pub art: Art,
    pub title: &'static str,
    /// Pango markup.
    pub body: &'static str,
}

pub const WELCOME_TITLE: &str = n_("Welcome to Helix");
pub const WELCOME_BODY: &str =
    n_("Helix is a fast, friendly desktop written in Rust. Take a quick look around — it only takes a minute.");

pub const PAGES: &[Page] = &[
    Page {
        art: Art::Overview,
        title: n_("The Overview"),
        body: n_(
            "Press the <b>Super</b> key to step back and see everything at once: \
               your workspaces along the top and live previews of your open windows. \
               Click a window to jump to it, or just start typing to search. \
               <b>Super+Alt+Up</b> and <b>Super+Alt+Down</b> move between the desktop, \
               the overview and the app launcher.",
        ),
    },
    Page {
        art: Art::Launcher,
        title: n_("Launching Apps"),
        body: n_(
            "Click <b>Apps</b> in the top panel or press <b>Super+A</b> to open the launcher. \
               Open windows come first, followed by every installed app. \
               Type a few letters to narrow the list, then press <b>Enter</b> to start it.",
        ),
    },
    Page {
        art: Art::Workspaces,
        title: n_("Workspaces"),
        body: n_("Spread your work across workspaces to keep things tidy. \
               <b>Super+Page Up</b> and <b>Super+Page Down</b> switch between them; \
               add <b>Shift</b> to take the current window with you. \
               The dots in the top panel show where you are, and by default a fresh, \
               empty workspace is always waiting at the end."),
    },
    Page {
        art: Art::Windows,
        title: n_("Managing Windows"),
        body: n_(
            "Drag a window by its header bar. Drop it against the left or right edge of \
               the screen to fill that half, or against the top edge to maximize it. \
               From the keyboard, <b>Super+Left</b> and <b>Super+Right</b> tile, \
               <b>Super+Up</b> maximizes and <b>Super+Down</b> restores. \
               <b>Alt+Tab</b> switches apps and <b>Alt+Space</b> opens the window menu.",
        ),
    },
    Page {
        art: Art::Touchpad,
        title: n_("Touchpad Gestures"),
        body: n_(
            "On a touchpad, swipe sideways with <b>three or four fingers</b> to glide \
               to the next or previous workspace. Tap-to-click, natural scrolling and \
               pointer speed can be adjusted in <b>Settings</b>.",
        ),
    },
    Page {
        art: Art::Help,
        title: n_("Getting Help"),
        body: n_(
            "Stuck? The <b>Help</b> app has guides for the desktop and your apps — \
               search for it in the launcher. Most apps also offer help from their main \
               menu or with <b>F1</b>.",
        ),
    },
    Page {
        art: Art::Done,
        title: n_("That's it! Have a nice day"),
        body: n_(
            "You're ready to go. You can take this tour again at any time by \
               searching for <b>Tour</b> in the launcher.",
        ),
    },
];

/// Index of the page that carries the "Learn More" link.
pub const HELP_PAGE: usize = 5;
pub const PROJECT_URL: &str = "https://helixdesktop.org";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_layout() {
        assert_eq!(PAGES.len(), 7);
        assert_eq!(PAGES[HELP_PAGE].art, Art::Help);
        assert!(PAGES.last().unwrap().title.starts_with("That's it"));
    }

    #[test]
    fn markup_is_valid() {
        for page in PAGES {
            assert!(
                gtk::pango::parse_markup(page.body, '\0').is_ok(),
                "{}",
                page.title
            );
            assert!(!page.body.contains("  "), "double space in {}", page.title);
        }
    }
}
