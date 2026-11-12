//! Accessibility panel.

use adw::prelude::*;
use gtk::glib;
use std::rc::Rc;

use super::widgets::{self, identity, scale_row, switch, watch, Num};
use super::{Ctx, Panel};
use crate::values::{self, CURSOR_SIZES};

const INTERFACE: &str = "org.gnome.desktop.interface";
const A11Y: &str = "org.gnome.desktop.a11y";
const A11Y_IFACE: &str = "org.gnome.desktop.a11y.interface";
const A11Y_APPS: &str = "org.gnome.desktop.a11y.applications";
const A11Y_KB: &str = "org.gnome.desktop.a11y.keyboard";
const MOUSE: &str = "org.gnome.desktop.peripherals.mouse";

pub struct AccessibilityPanel;

impl Panel for AccessibilityPanel {
    fn build(&self, ctx: &Rc<Ctx>) -> gtk::Widget {
        let iface = ctx.prefs(INTERFACE);
        let a11y = ctx.prefs(A11Y);
        let a11y_iface = ctx.prefs(A11Y_IFACE);
        let apps = ctx.prefs(A11Y_APPS);
        let kb = ctx.prefs(A11Y_KB);
        let mouse = ctx.prefs(MOUSE);
        let page = widgets::page();

        let general = widgets::group("General");
        general.add(&switch(&a11y, "always-show-universal-access-status", false, false, "Always Show Accessibility Menu", "Show the accessibility menu in the top bar"));
        page.add(&general);

        let seeing = widgets::group("Seeing");
        seeing.add(&switch(&a11y_iface, "high-contrast", false, false, "High Contrast", "Increase the contrast of interface elements"));
        // Large text: text-scaling-factor 1.25 vs 1.0.
        let large = adw::SwitchRow::builder().title("Large Text").subtitle("Increase the size of text").build();
        large.set_active(values::large_text_enabled(iface.get::<f64>("text-scaling-factor", 1.0)));
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
        {
            let (weak, p) = (large.downgrade(), iface.clone());
            watch(&iface, "text-scaling-factor", move || {
                if let Some(r) = weak.upgrade() {
                    r.set_active(values::large_text_enabled(p.get::<f64>("text-scaling-factor", 1.0)));
                }
            });
        }
        seeing.add(&large);
        seeing.add(&cursor_size_row(&iface));
        seeing.add(&switch(&iface, "enable-animations", true, true, "Reduce Animation", "Turn off animations in the interface"));
        seeing.add(&switch(&iface, "overlay-scrolling", true, true, "Always Show Scrollbars", ""));
        seeing.add(&switch(&apps, "screen-reader-enabled", false, false, "Screen Reader", "Read the screen aloud"));
        seeing.add(&switch(&apps, "screen-magnifier-enabled", false, false, "Zoom", "Magnify part of the screen"));
        page.add(&seeing);

        let typing = widgets::group("Typing");
        typing.add(&switch(&apps, "screen-keyboard-enabled", false, false, "Screen Keyboard", "Use an on-screen keyboard to type"));
        typing.add(&switch(&iface, "cursor-blink", true, false, "Cursor Blinking", "Text cursor blinks in text fields"));
        typing.add(&switch(&kb, "stickykeys-enable", false, false, "Sticky Keys", "Treat a sequence of modifier keys as a key combination"));
        typing.add(&switch(&kb, "slowkeys-enable", false, false, "Slow Keys", "Put a delay between when a key is pressed and when it is accepted"));
        typing.add(&switch(&kb, "bouncekeys-enable", false, false, "Bounce Keys", "Ignore fast duplicate key presses"));
        page.add(&typing);

        let pointing = widgets::group("Pointing & Clicking");
        pointing.add(&scale_row(&mouse, "double-click", Num::I32, 400.0, (100.0, 1000.0, 10.0), identity, identity, "Double-Click Delay", Some(("Short", "Long"))));
        pointing.add(&switch(&iface, "locate-pointer", false, false, "Locate Pointer", "Highlight the pointer when the Control key is pressed"));
        pointing.add(&switch(&kb, "mousekeys-enable", false, false, "Mouse Keys", "Control the pointer using the keypad"));
        page.add(&pointing);
        page.upcast()
    }
}

fn cursor_size_row(iface: &helix_app_common::Prefs) -> adw::ComboRow {
    let labels: Vec<String> = CURSOR_SIZES.iter().map(|(s, l)| format!("{l} ({s} px)")).collect();
    let refs: Vec<&str> = labels.iter().map(String::as_str).collect();
    let row = adw::ComboRow::builder().title("Cursor Size").model(&gtk::StringList::new(&refs)).build();
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
