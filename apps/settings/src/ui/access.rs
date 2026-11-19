//! Accessibility panel: a main page with General settings and rows that
//! open the Seeing, Hearing, Typing, Pointing & Clicking and Zoom
//! sub-pages (GNOME 46+ layout).

use adw::prelude::*;
use gtk::glib;
use helix_app_common::Prefs;
use std::rc::Rc;

use super::widgets::{
    self, expander, has_key, identity, nick_combo, scale_row, spin_row, subpage_row, switch, watch,
    Num,
};
use super::{Ctx, Panel};
use crate::values::{self, CURSOR_SIZES};
use helix_app_common::i18n::{n_, tr};

const INTERFACE: &str = "org.gnome.desktop.interface";
const A11Y: &str = "org.gnome.desktop.a11y";
const A11Y_IFACE: &str = "org.gnome.desktop.a11y.interface";
const A11Y_APPS: &str = "org.gnome.desktop.a11y.applications";
const A11Y_KB: &str = "org.gnome.desktop.a11y.keyboard";
const A11Y_MOUSE: &str = "org.gnome.desktop.a11y.mouse";
const MAGNIFIER: &str = "org.gnome.desktop.a11y.magnifier";
const MOUSE: &str = "org.gnome.desktop.peripherals.mouse";
const SOUND: &str = "org.gnome.desktop.sound";
const WM: &str = "org.gnome.desktop.wm.preferences";

const VISUAL_BELL_TYPES: &[(&str, &str)] = &[
    ("fullscreen-flash", n_("Flash Entire Screen")),
    ("frame-flash", n_("Flash Focused Window")),
];
const SCREEN_POSITIONS: &[(&str, &str)] = &[
    ("full-screen", n_("Full Screen")),
    ("top-half", n_("Top Half")),
    ("bottom-half", n_("Bottom Half")),
    ("left-half", n_("Left Half")),
    ("right-half", n_("Right Half")),
];
const MOUSE_TRACKING: &[(&str, &str)] = &[
    ("proportional", n_("Proportionally")),
    ("centered", n_("Keep Pointer Centered")),
    ("push", n_("Push at Screen Edges")),
    ("none", n_("Do Not Follow")),
];

pub struct AccessibilityPanel;

/// Builds one sub-page's content.
type SubpageFn = fn(&Rc<Ctx>) -> adw::PreferencesPage;

impl Panel for AccessibilityPanel {
    fn own_header(&self) -> bool {
        true
    }

    fn build(&self, ctx: &Rc<Ctx>) -> gtk::Widget {
        let nav = adw::NavigationView::new();
        let a11y = ctx.prefs(A11Y);
        let page = widgets::page();

        let general = widgets::group(tr("General"));
        general.add(&switch(
            &a11y,
            "always-show-universal-access-status",
            false,
            false,
            tr("Always Show Accessibility Menu"),
            tr("Show the accessibility menu in the top bar"),
        ));
        page.add(&general);

        let subpages = widgets::group("");
        let entries: [(&str, &str, &str, SubpageFn); 5] = [
            (
                tr("Seeing"),
                tr("Contrast, text size, cursor, motion and screen reader"),
                "view-reveal-symbolic",
                seeing_page,
            ),
            (
                tr("Hearing"),
                tr("Overamplification and visual alerts"),
                "audio-volume-high-symbolic",
                hearing_page,
            ),
            (
                tr("Typing"),
                tr("Screen keyboard and typing assistance"),
                "input-keyboard-symbolic",
                typing_page,
            ),
            (
                tr("Pointing & Clicking"),
                tr("Mouse keys, click assist and double-click delay"),
                "input-mouse-symbolic",
                pointing_page,
            ),
            (
                tr("Zoom"),
                tr("Magnify part of the screen"),
                "zoom-in-symbolic",
                zoom_page,
            ),
        ];
        for (title, subtitle, icon, build) in entries {
            let row = subpage_row(title, subtitle, icon);
            let (nav, ctx) = (nav.downgrade(), Rc::downgrade(ctx));
            row.connect_activated(move |_| {
                let (Some(nav), Some(ctx)) = (nav.upgrade(), ctx.upgrade()) else {
                    return;
                };
                nav.push(&subpage(title, &build(&ctx)));
            });
            subpages.add(&row);
        }
        page.add(&subpages);

        nav.add(&subpage(tr("Accessibility"), &page));
        nav.upcast()
    }
}

/// A navigation page with a header bar around `content`.
fn subpage(title: &str, content: &adw::PreferencesPage) -> adw::NavigationPage {
    widgets::nav_page(title, content, None)
}

fn seeing_page(ctx: &Rc<Ctx>) -> adw::PreferencesPage {
    let iface = ctx.prefs(INTERFACE);
    let a11y_iface = ctx.prefs(A11Y_IFACE);
    let apps = ctx.prefs(A11Y_APPS);
    let page = widgets::page();
    let group = widgets::group("");
    group.add(&switch(
        &a11y_iface,
        "high-contrast",
        false,
        false,
        tr("High Contrast"),
        tr("Increase the contrast of interface elements"),
    ));
    group.add(&switch(
        &a11y_iface,
        "show-status-shapes",
        false,
        false,
        tr("Show Status Shapes"),
        tr("Use shapes as well as colour to show the state of switches and other controls"),
    ));
    group.add(&large_text_row(&iface));
    group.add(&cursor_size_row(&iface));
    group.add(&switch(
        &iface,
        "enable-animations",
        true,
        true,
        tr("Reduce Animation"),
        tr("Turn off animations in the interface"),
    ));
    group.add(&switch(
        &iface,
        "overlay-scrolling",
        true,
        true,
        tr("Always Show Scrollbars"),
        "",
    ));
    page.add(&group);
    let reader = widgets::group(tr("Screen Reader"));
    reader.add(&switch(
        &apps,
        "screen-reader-enabled",
        false,
        false,
        tr("Screen Reader"),
        tr("Read the screen aloud"),
    ));
    page.add(&reader);
    page
}

fn hearing_page(ctx: &Rc<Ctx>) -> adw::PreferencesPage {
    let sound = ctx.prefs(SOUND);
    let wm = ctx.prefs(WM);
    let page = widgets::page();
    let group = widgets::group("");
    group.add(&switch(
        &sound,
        "allow-volume-above-100-percent",
        false,
        false,
        tr("Overamplification"),
        tr("Allow volume above 100 %, which can reduce sound quality"),
    ));
    let visual = expander(
        &wm,
        "visual-bell",
        false,
        tr("Visual Alerts"),
        tr("Flash the screen or window when an alert sound plays"),
    );
    visual.add_row(&nick_combo(
        &wm,
        "visual-bell-type",
        "fullscreen-flash",
        VISUAL_BELL_TYPES,
        tr("Flash Area"),
    ));
    group.add(&visual);
    page.add(&group);
    page
}

fn delay_row(prefs: &Prefs, key: &'static str, title: &str) -> adw::ActionRow {
    scale_row(
        prefs,
        key,
        Num::I32,
        300.0,
        (0.0, 1000.0, 10.0),
        identity,
        identity,
        title,
        Some((tr("Short"), tr("Long"))),
    )
}

/// The blink-time slider runs from slow (long period) to fast.
fn blink_to_ui(ms: f64) -> f64 {
    2600.0 - ms
}

fn typing_page(ctx: &Rc<Ctx>) -> adw::PreferencesPage {
    let iface = ctx.prefs(INTERFACE);
    let apps = ctx.prefs(A11Y_APPS);
    let kb = ctx.prefs(A11Y_KB);
    let page = widgets::page();

    let general = widgets::group("");
    general.add(&switch(
        &apps,
        "screen-keyboard-enabled",
        false,
        false,
        tr("Screen Keyboard"),
        tr("Use an on-screen keyboard to type"),
    ));
    let blink = expander(
        &iface,
        "cursor-blink",
        true,
        tr("Cursor Blinking"),
        tr("Text cursor blinks in text fields"),
    );
    blink.add_row(&scale_row(
        &iface,
        "cursor-blink-time",
        Num::I32,
        1200.0,
        (100.0, 2500.0, 10.0),
        blink_to_ui,
        blink_to_ui,
        tr("Blink Speed"),
        Some((tr("Slow"), tr("Fast"))),
    ));
    general.add(&blink);
    general.add(&switch(
        &kb,
        "togglekeys-enable",
        false,
        false,
        tr("Sound Keys"),
        tr("Beep when Num Lock or Caps Lock are turned on or off"),
    ));
    page.add(&general);

    let assist = widgets::group(tr("Typing Assist"));
    assist.add(&switch(
        &kb,
        "enable",
        false,
        false,
        tr("Enable by Keyboard"),
        tr("Turn accessibility features on and off using the keyboard"),
    ));
    let sticky = expander(
        &kb,
        "stickykeys-enable",
        false,
        tr("Sticky Keys"),
        tr("Treat a sequence of modifier keys as a key combination"),
    );
    sticky.add_row(&switch(
        &kb,
        "stickykeys-two-key-off",
        false,
        false,
        tr("Disable if Two Keys Are Pressed Together"),
        "",
    ));
    sticky.add_row(&switch(
        &kb,
        "stickykeys-modifier-beep",
        false,
        false,
        tr("Beep When a Modifier Key Is Pressed"),
        "",
    ));
    assist.add(&sticky);
    let slow = expander(
        &kb,
        "slowkeys-enable",
        false,
        tr("Slow Keys"),
        tr("Put a delay between when a key is pressed and when it is accepted"),
    );
    slow.add_row(&delay_row(&kb, "slowkeys-delay", tr("Acceptance Delay")));
    for (key, title) in [
        ("slowkeys-beep-press", tr("Beep When a Key Is Pressed")),
        ("slowkeys-beep-accept", tr("Beep When a Key Is Accepted")),
        ("slowkeys-beep-reject", tr("Beep When a Key Is Rejected")),
    ] {
        slow.add_row(&switch(&kb, key, false, false, title, ""));
    }
    assist.add(&slow);
    let bounce = expander(
        &kb,
        "bouncekeys-enable",
        false,
        tr("Bounce Keys"),
        tr("Ignore fast duplicate key presses"),
    );
    bounce.add_row(&delay_row(&kb, "bouncekeys-delay", tr("Acceptance Delay")));
    bounce.add_row(&switch(
        &kb,
        "bouncekeys-beep-reject",
        false,
        false,
        tr("Beep When a Key Is Rejected"),
        "",
    ));
    assist.add(&bounce);
    page.add(&assist);
    page
}

fn pointing_page(ctx: &Rc<Ctx>) -> adw::PreferencesPage {
    let iface = ctx.prefs(INTERFACE);
    let kb = ctx.prefs(A11Y_KB);
    let mouse = ctx.prefs(MOUSE);
    let a11y_mouse = ctx.prefs(A11Y_MOUSE);
    let page = widgets::page();

    let group = widgets::group("");
    group.add(&switch(
        &kb,
        "mousekeys-enable",
        false,
        false,
        tr("Mouse Keys"),
        tr("Control the pointer using the keypad"),
    ));
    group.add(&switch(
        &iface,
        "locate-pointer",
        false,
        false,
        tr("Locate Pointer"),
        tr("Highlight the pointer when the Control key is pressed"),
    ));
    group.add(&scale_row(
        &mouse,
        "double-click",
        Num::I32,
        400.0,
        (100.0, 1000.0, 10.0),
        identity,
        identity,
        tr("Double-Click Delay"),
        Some((tr("Short"), tr("Long"))),
    ));
    page.add(&group);

    let assist = widgets::group(tr("Click Assist"));
    let secondary = expander(
        &a11y_mouse,
        "secondary-click-enabled",
        false,
        tr("Simulated Secondary Click"),
        tr("Trigger a secondary click by holding down the primary button"),
    );
    secondary.add_row(&scale_row(
        &a11y_mouse,
        "secondary-click-time",
        Num::F64,
        1.2,
        (0.5, 3.0, 0.1),
        identity,
        identity,
        tr("Delay"),
        Some((tr("Short"), tr("Long"))),
    ));
    assist.add(&secondary);
    let hover = expander(
        &a11y_mouse,
        "dwell-click-enabled",
        false,
        tr("Hover Click"),
        tr("Trigger a click when the pointer hovers"),
    );
    hover.add_row(&scale_row(
        &a11y_mouse,
        "dwell-time",
        Num::F64,
        1.2,
        (0.2, 3.0, 0.1),
        identity,
        identity,
        tr("Delay"),
        Some((tr("Short"), tr("Long"))),
    ));
    hover.add_row(&scale_row(
        &a11y_mouse,
        "dwell-threshold",
        Num::I32,
        10.0,
        (0.0, 30.0, 1.0),
        identity,
        identity,
        tr("Motion Threshold"),
        Some((tr("Small"), tr("Large"))),
    ));
    assist.add(&hover);
    page.add(&assist);
    page
}

fn zoom_page(ctx: &Rc<Ctx>) -> adw::PreferencesPage {
    let apps = ctx.prefs(A11Y_APPS);
    let mag = ctx.prefs(MAGNIFIER);
    let page = widgets::page();

    let group = widgets::group("");
    group.add(&switch(
        &apps,
        "screen-magnifier-enabled",
        false,
        false,
        tr("Zoom"),
        tr("Magnify part of the screen"),
    ));
    group.add(&spin_row(
        &mag,
        "mag-factor",
        2.0,
        (1.0, 20.0, 0.25),
        tr("Magnification"),
    ));
    group.add(&nick_combo(
        &mag,
        "screen-position",
        "full-screen",
        SCREEN_POSITIONS,
        tr("Magnifier Position"),
    ));
    group.add(&nick_combo(
        &mag,
        "mouse-tracking",
        "proportional",
        MOUSE_TRACKING,
        tr("Follow the Pointer"),
    ));
    page.add(&group);

    let cross = widgets::group(tr("Crosshairs"));
    let show = expander(
        &mag,
        "show-cross-hairs",
        false,
        tr("Crosshairs"),
        tr("Show lines through the pointer"),
    );
    show.add_row(&switch(
        &mag,
        "cross-hairs-clip",
        false,
        true,
        tr("Overlap Pointer"),
        "",
    ));
    show.add_row(&scale_row(
        &mag,
        "cross-hairs-thickness",
        Num::I32,
        8.0,
        (1.0, 100.0, 1.0),
        identity,
        identity,
        tr("Thickness"),
        Some((tr("Thin"), tr("Thick"))),
    ));
    show.add_row(&scale_row(
        &mag,
        "cross-hairs-length",
        Num::I32,
        4096.0,
        (20.0, 4096.0, 4.0),
        identity,
        identity,
        tr("Length"),
        Some((tr("Short"), tr("Long"))),
    ));
    cross.add(&show);
    page.add(&cross);

    let color = widgets::group(tr("Color Effects"));
    color.add(&switch(
        &mag,
        "invert-lightness",
        false,
        false,
        tr("Invert Colors"),
        "",
    ));
    color.add(&rgb_scale(
        &mag,
        "brightness",
        tr("Brightness"),
        (tr("Low"), tr("High")),
    ));
    color.add(&rgb_scale(
        &mag,
        "contrast",
        tr("Contrast"),
        (tr("Low"), tr("High")),
    ));
    color.add(&scale_row(
        &mag,
        "color-saturation",
        Num::F64,
        1.0,
        (0.0, 1.0, 0.05),
        identity,
        identity,
        tr("Color"),
        Some((tr("Grayscale"), tr("Full Color"))),
    ));
    page.add(&color);
    page
}

/// A slider writing the same value to `<prefix>-red`, `-green` and
/// `-blue` (the magnifier's brightness and contrast), shown from red.
fn rgb_scale(prefs: &Prefs, prefix: &str, title: &str, marks: (&str, &str)) -> adw::ActionRow {
    let keys = ["red", "green", "blue"].map(|c| format!("{prefix}-{c}"));
    let row = adw::ActionRow::builder()
        .use_markup(false)
        .title(title)
        .build();
    if !keys.iter().all(|k| has_key(prefs, k)) {
        row.set_subtitle(widgets::UNSUPPORTED);
    }
    let scale = gtk::Scale::with_range(gtk::Orientation::Horizontal, -1.0, 1.0, 0.05);
    scale.set_hexpand(true);
    scale.set_valign(gtk::Align::Center);
    scale.set_draw_value(false);
    scale.set_width_request(150);
    scale.add_mark(-1.0, gtk::PositionType::Bottom, Some(marks.0));
    scale.add_mark(1.0, gtk::PositionType::Bottom, Some(marks.1));
    helix_app_common::set_accessible_label(&scale, title);
    scale.set_value(prefs.get::<f64>(&keys[0], 0.0));
    {
        let (prefs, keys) = (prefs.clone(), keys.clone());
        scale.connect_value_changed(move |s| {
            for k in &keys {
                widgets::set_num(&prefs, k, Num::F64, s.value());
            }
        });
    }
    let (weak, p, red) = (scale.downgrade(), prefs.clone(), keys[0].clone());
    watch(prefs, &keys[0], move || {
        if let Some(s) = weak.upgrade() {
            let v = p.get::<f64>(&red, 0.0);
            if (s.value() - v).abs() > 1e-6 {
                s.set_value(v);
            }
        }
    });
    row.add_suffix(&scale);
    row
}

/// Large text: `text-scaling-factor` 1.25 vs 1.0.
fn large_text_row(iface: &Prefs) -> adw::SwitchRow {
    let large = adw::SwitchRow::builder()
        .title(tr("Large Text"))
        .subtitle(tr("Increase the size of text"))
        .build();
    large.set_active(values::large_text_enabled(
        iface.get::<f64>("text-scaling-factor", 1.0),
    ));
    large.connect_active_notify(glib::clone!(
        #[strong]
        iface,
        move |r| {
            let current = iface.get::<f64>("text-scaling-factor", 1.0);
            if values::large_text_enabled(current) != r.is_active() {
                iface.set("text-scaling-factor", values::text_factor(r.is_active()));
            }
        }
    ));
    let (weak, p) = (large.downgrade(), iface.clone());
    watch(iface, "text-scaling-factor", move || {
        if let Some(r) = weak.upgrade() {
            r.set_active(values::large_text_enabled(
                p.get::<f64>("text-scaling-factor", 1.0),
            ));
        }
    });
    large
}

fn cursor_size_row(iface: &Prefs) -> adw::ComboRow {
    let labels: Vec<String> = CURSOR_SIZES
        .iter()
        .map(|(s, l)| format!("{} ({s} px)", tr(l)))
        .collect();
    let refs: Vec<&str> = labels.iter().map(String::as_str).collect();
    let row = adw::ComboRow::builder()
        .title(tr("Cursor Size"))
        .model(&gtk::StringList::new(&refs))
        .build();
    row.set_selected(values::cursor_size_index(iface.get::<i32>("cursor-size", 24)) as u32);
    row.connect_selected_notify(glib::clone!(
        #[strong]
        iface,
        move |r| {
            if let Some((size, _)) = CURSOR_SIZES.get(r.selected() as usize) {
                if iface.get::<i32>("cursor-size", 24) != *size {
                    iface.set("cursor-size", *size);
                }
            }
        }
    ));
    let (weak, p) = (row.downgrade(), iface.clone());
    watch(iface, "cursor-size", move || {
        if let Some(r) = weak.upgrade() {
            r.set_selected(values::cursor_size_index(p.get::<i32>("cursor-size", 24)) as u32);
        }
    });
    row
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blink_slider_runs_slow_to_fast() {
        // Longest period (slowest) at the left end, and the mapping is its
        // own inverse.
        assert!(blink_to_ui(2500.0) < blink_to_ui(100.0));
        assert_eq!(blink_to_ui(blink_to_ui(1200.0)), 1200.0);
    }

    #[test]
    fn enum_choices_use_gnome_nicks() {
        let xml = include_str!("../../data/org.helixdesktop.Settings.gnome-compat.gschema.xml");
        for (nick, _) in VISUAL_BELL_TYPES
            .iter()
            .chain(SCREEN_POSITIONS)
            .chain(MOUSE_TRACKING)
        {
            assert!(xml.contains(&format!("nick=\"{nick}\"")), "{nick}");
        }
    }
}
