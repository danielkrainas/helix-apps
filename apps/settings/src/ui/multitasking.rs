//! Multitasking panel.

use adw::prelude::*;
use gtk::glib;
use std::rc::Rc;

use super::widgets::{self, switch, watch};
use super::{Ctx, Panel};

const INTERFACE: &str = "org.gnome.desktop.interface";
const MUTTER: &str = "org.gnome.mutter";
const WM: &str = "org.gnome.desktop.wm.preferences";
const APP_SWITCHER: &str = "org.gnome.shell.app-switcher";

pub struct MultitaskingPanel;

impl Panel for MultitaskingPanel {
    fn build(&self, ctx: &Rc<Ctx>) -> gtk::Widget {
        let iface = ctx.prefs(INTERFACE);
        let mutter = ctx.prefs(MUTTER);
        let wm = ctx.prefs(WM);
        let switcher = ctx.prefs(APP_SWITCHER);
        let page = widgets::page();

        let general = widgets::group("General");
        general.add(&switch(&iface, "enable-hot-corners", true, false, "Hot Corner", "Touch the top-left corner to open the Activities Overview"));
        general.add(&switch(&mutter, "edge-tiling", true, false, "Active Screen Edges", "Drag windows against the top, left, and right screen edges to resize them"));
        page.add(&general);

        let ws = widgets::group("Workspaces");
        let dynamic = gtk::CheckButton::new();
        let fixed = gtk::CheckButton::new();
        fixed.set_group(Some(&dynamic));
        let dyn_row = adw::ActionRow::builder().title("Dynamic Workspaces").subtitle("Automatically removes empty workspaces").activatable_widget(&dynamic).build();
        dyn_row.add_prefix(&dynamic);
        let fixed_row = adw::ActionRow::builder().title("Fixed Number of Workspaces").subtitle("Specify a number of permanent workspaces").activatable_widget(&fixed).build();
        fixed_row.add_prefix(&fixed);
        let count = adw::SpinRow::with_range(1.0, 36.0, 1.0);
        count.set_title("Number of Workspaces");
        count.set_value(wm.get::<i32>("num-workspaces", 4) as f64);
        count.connect_value_notify(glib::clone!(
            #[strong]
            wm,
            move |r| {
                let v = r.value() as i32;
                if wm.get::<i32>("num-workspaces", 4) != v {
                    wm.set("num-workspaces", v);
                }
            }
        ));
        {
            let (weak, p) = (count.downgrade(), wm.clone());
            watch(&wm, "num-workspaces", move || {
                if let Some(r) = weak.upgrade() {
                    r.set_value(p.get::<i32>("num-workspaces", 4) as f64);
                }
            });
        }
        fixed.bind_property("active", &count, "sensitive").sync_create().build();
        let sync = {
            let (d, f, p) = (dynamic.downgrade(), fixed.downgrade(), mutter.clone());
            move || {
                let (Some(d), Some(f)) = (d.upgrade(), f.upgrade()) else { return };
                if p.get::<bool>("dynamic-workspaces", true) {
                    d.set_active(true);
                } else {
                    f.set_active(true);
                }
            }
        };
        sync();
        dynamic.connect_toggled(glib::clone!(
            #[strong]
            mutter,
            move |b| {
                if mutter.get::<bool>("dynamic-workspaces", true) != b.is_active() {
                    mutter.set("dynamic-workspaces", b.is_active());
                }
            }
        ));
        watch(&mutter, "dynamic-workspaces", sync);
        ws.add(&dyn_row);
        ws.add(&fixed_row);
        ws.add(&count);
        page.add(&ws);

        let multi = widgets::group("Multi-Monitor");
        multi.add(&switch(&mutter, "workspaces-only-on-primary", true, false, "Workspaces on Primary Display Only", "Other displays show the same windows on every workspace"));
        page.add(&multi);

        let apps = widgets::group("App Switching");
        apps.add(&switch(&switcher, "current-workspace-only", false, false, "Include Apps From the Current Workspace Only", "Otherwise apps from all workspaces are included"));
        page.add(&apps);
        page.upcast()
    }
}
