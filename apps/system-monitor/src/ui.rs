//! Application window, actions and preferences.

use adw::prelude::*;
use gtk::{gio, glib};
use helix_app_common::Prefs;
use std::rc::Rc;
use std::sync::atomic::Ordering;
use std::sync::{mpsc, Arc};

use crate::control::Action;
use crate::dialogs;
use crate::fs_view::FsView;
use crate::model::Whose;
use crate::proc_view::{DisplayOpts, ProcView};
use crate::res_view::{parse_color, Colors, ResView};
use crate::sampler::{self, clamp_interval, Config, Control, Msg};

pub const APP_ID: &str = "org.helix.SystemMonitor";
const SCHEMA: &str = "org.gnome.gnome-system-monitor";

const PAGES: [(&str, &str, &str); 3] = [
    ("processes", "Processes", "view-list-symbolic"),
    ("resources", "Resources", "speedometer-symbolic"),
    ("disks", "File Systems", "drive-harddisk-symbolic"),
];

type WinFn = Box<dyn Fn(&Rc<Win>)>;
type BoolFn = Box<dyn Fn(&Rc<Win>, bool)>;
type U32Fn = Box<dyn Fn(&Rc<Win>, u32)>;

struct Win {
    window: adw::ApplicationWindow,
    toasts: adw::ToastOverlay,
    stack: adw::ViewStack,
    prefs: Prefs,
    config: Arc<Config>,
    ctl: mpsc::Sender<Control>,
    procs: Rc<ProcView>,
    res: Rc<ResView>,
    fs: Rc<FsView>,
    search_button: gtk::ToggleButton,
}

impl Win {
    fn toast(&self, msg: &str) {
        self.toasts.add_toast(adw::Toast::new(msg));
    }

    fn refresh(&self) {
        let _ = self.ctl.send(Control::Refresh);
    }

    fn update_action_state(&self) {
        let on_procs = self.stack.visible_child_name().as_deref() == Some("processes");
        let has_sel = on_procs && self.procs.selected().is_some();
        for name in ["proc-properties", "proc-priority", "proc-stop", "proc-continue", "proc-end", "proc-kill", "proc-menu"] {
            if let Some(a) = self.window.lookup_action(name).and_downcast::<gio::SimpleAction>() {
                a.set_enabled(has_sel);
            }
        }
        self.search_button.set_visible(on_procs);
    }

    fn display_opts(&self) -> DisplayOpts {
        DisplayOpts { iec: self.prefs.get("process-memory-in-iec", false) }
    }
}

pub fn run(initial_page: Option<&'static str>) -> glib::ExitCode {
    let app = adw::Application::builder().application_id(APP_ID).flags(gio::ApplicationFlags::NON_UNIQUE).build();
    helix_app_common::install_standard_actions(&app, "System Monitor", "View current processes and monitor system state");
    app.connect_activate(move |app| {
        let win = build(app, initial_page);
        win.window.present();
    });
    app.run_with_args::<&str>(&[])
}

fn build(app: &adw::Application, initial_page: Option<&'static str>) -> Rc<Win> {
    let prefs = Prefs::new(SCHEMA);
    let interval = clamp_interval(prefs.get::<i32>("update-interval", 3000).max(0) as u32);
    let config = Config::new(
        interval,
        prefs.get::<i32>("disks-interval", 5000).max(0) as u32,
        prefs.get("solaris-mode", true),
        prefs.get("show-all-fs", false),
    );
    let (rx, ctl) = sampler::spawn(config.clone());

    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("System Monitor")
        .default_width(prefs.get::<i32>("window-width", 700).max(360))
        .default_height(prefs.get::<i32>("window-height", 500).max(300))
        .build();
    if prefs.get("maximized", false) {
        window.maximize();
    }

    let whose = Whose::from_nick(&prefs.get("show-whose-processes", "user".to_string()));
    let procs = ProcView::new(whose, sampler::current_uid(), DisplayOpts { iec: prefs.get("process-memory-in-iec", false) });
    let points = prefs.get::<i32>("graph-data-points", 60).clamp(10, 600) as usize;
    let colors = Colors {
        mem: parse_color(&prefs.get("mem-color", "#e01b24".to_string()), "#e01b24"),
        swap: parse_color(&prefs.get("swap-color", "#33d17a".to_string()), "#33d17a"),
        net_in: parse_color(&prefs.get("net-in-color", "#3584e4".to_string()), "#3584e4"),
        net_out: parse_color(&prefs.get("net-out-color", "#e66100".to_string()), "#e66100"),
    };
    let res = ResView::new(points, colors);
    res.iec.set(prefs.get("resources-memory-in-iec", false));
    res.set_bits(prefs.get("network-in-bits", false));
    res.set_interval(interval);
    let fs = FsView::new();

    let stack = adw::ViewStack::new();
    let widgets: [gtk::Widget; 3] = [procs.page.clone().upcast(), res.page.clone().upcast(), fs.page.clone().upcast()];
    for ((name, title, icon), w) in PAGES.iter().zip(widgets) {
        stack.add_titled_with_icon(&w, Some(name), title, icon);
    }
    let start_page = initial_page.map(str::to_string).unwrap_or_else(|| prefs.get("current-tab", "processes".to_string()));
    if PAGES.iter().any(|(n, _, _)| *n == start_page) {
        stack.set_visible_child_name(&start_page);
    }

    let header = adw::HeaderBar::new();
    let switcher = adw::ViewSwitcher::builder().stack(&stack).policy(adw::ViewSwitcherPolicy::Wide).build();
    header.set_title_widget(Some(&switcher));
    let search_button = gtk::ToggleButton::builder().icon_name("system-search-symbolic").tooltip_text("Search").build();
    procs.search_bar.bind_property("search-mode-enabled", &search_button, "active").bidirectional().sync_create().build();
    header.pack_start(&search_button);

    let menu = gio::Menu::new();
    let show = gio::Menu::new();
    show.append(Some("_My Processes"), Some("win.show-whose::user"));
    show.append(Some("_All Processes"), Some("win.show-whose::all"));
    show.append(Some("A_ctive Processes"), Some("win.show-whose::active"));
    menu.append_section(None, &show);
    let s = gio::Menu::new();
    s.append(Some("_Refresh"), Some("win.refresh"));
    menu.append_section(None, &s);
    let s = gio::Menu::new();
    s.append(Some("_Preferences"), Some("win.preferences"));
    s.append(Some("_Keyboard Shortcuts"), Some("win.shortcuts"));
    s.append(Some("_About System Monitor"), Some("app.about"));
    menu.append_section(None, &s);
    let menu_button = gtk::MenuButton::builder().icon_name("open-menu-symbolic").menu_model(&menu).primary(true).tooltip_text("Main Menu").build();
    header.pack_end(&menu_button);

    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&header);
    toolbar.set_content(Some(&stack));
    let toasts = adw::ToastOverlay::new();
    toasts.set_child(Some(&toolbar));
    window.set_content(Some(&toasts));
    procs.search_bar.set_key_capture_widget(Some(&procs.page));

    let win = Rc::new(Win { window, toasts, stack, prefs, config, ctl, procs, res, fs, search_button });
    install_actions(&win, whose);
    win.update_action_state();

    {
        let w = Rc::downgrade(&win);
        win.stack.connect_visible_child_name_notify(move |s| {
            let Some(w) = w.upgrade() else { return };
            if let Some(name) = s.visible_child_name() {
                w.prefs.set("current-tab", name.to_string());
            }
            w.update_action_state();
        });
    }
    {
        let w = Rc::downgrade(&win);
        win.procs.selection.connect_selected_notify(move |_| {
            if let Some(w) = w.upgrade() {
                w.update_action_state();
            }
        });
    }
    {
        let w = Rc::downgrade(&win);
        win.window.connect_close_request(move |window| {
            if let Some(w) = w.upgrade() {
                if !window.is_maximized() {
                    w.prefs.set("window-width", window.default_width());
                    w.prefs.set("window-height", window.default_height());
                }
                w.prefs.set("maximized", window.is_maximized());
            }
            glib::Propagation::Proceed
        });
    }

    // Receive samples from the worker thread.
    let w = Rc::downgrade(&win);
    glib::spawn_future_local(async move {
        while let Ok(msg) = rx.recv().await {
            let Some(w) = w.upgrade() else { break };
            match msg {
                Msg::Processes(p, r) => {
                    w.procs.update(p);
                    w.res.push(&r);
                }
                Msg::FileSystems(f) => w.fs.update(f),
            }
        }
    });
    win
}

fn install_actions(win: &Rc<Win>, whose: Whose) {
    let window = &win.window;
    let app = window.application().expect("application");

    let add = |name: &str, f: WinFn| {
        let a = gio::SimpleAction::new(name, None);
        let w = Rc::downgrade(win);
        a.connect_activate(move |_, _| {
            if let Some(w) = w.upgrade() {
                f(&w);
            }
        });
        window.add_action(&a);
    };

    add("refresh", Box::new(|w| w.refresh()));
    add("search", Box::new(|w| {
        w.stack.set_visible_child_name("processes");
        w.procs.search_bar.set_search_mode(true);
        w.procs.search_entry.grab_focus();
    }));
    add("preferences", Box::new(show_preferences));
    add("shortcuts", Box::new(show_shortcuts));
    add("proc-properties", Box::new(|w| {
        if let Some(p) = w.procs.selected() {
            dialogs::show_properties(&w.window, &p, w.display_opts().iec);
        }
    }));
    add("proc-priority", Box::new(|w| {
        if let Some(p) = w.procs.selected() {
            let weak = Rc::downgrade(w);
            dialogs::change_priority(&w.window, &p, Rc::new(move |msg| {
                if let Some(w) = weak.upgrade() {
                    w.toast(&msg);
                    w.refresh();
                }
            }));
        }
    }));
    add("proc-menu", Box::new(|w| w.procs.popup_menu()));
    for (name, action) in [("proc-stop", Action::Stop), ("proc-continue", Action::Continue), ("proc-end", Action::End), ("proc-kill", Action::Kill)] {
        add(name, Box::new(move |w| {
            let Some(p) = w.procs.selected() else { return };
            let weak = Rc::downgrade(w);
            let confirm = w.prefs.get("kill-dialog", true);
            dialogs::signal_process(&w.window, &p, action, confirm, Rc::new(move |res| {
                if let Some(w) = weak.upgrade() {
                    if let Err(msg) = res {
                        w.toast(&msg);
                    }
                    w.refresh();
                }
            }));
        }));
    }

    let page = gio::SimpleAction::new("show-page", Some(glib::VariantTy::STRING));
    {
        let w = Rc::downgrade(win);
        page.connect_activate(move |_, v| {
            if let (Some(w), Some(name)) = (w.upgrade(), v.and_then(|v| v.str().map(str::to_string))) {
                w.stack.set_visible_child_name(&name);
            }
        });
    }
    window.add_action(&page);

    let show = gio::SimpleAction::new_stateful("show-whose", Some(glib::VariantTy::STRING), &whose.nick().to_variant());
    {
        let w = Rc::downgrade(win);
        show.connect_change_state(move |a, v| {
            let (Some(w), Some(v)) = (w.upgrade(), v) else { return };
            let whose = Whose::from_nick(v.str().unwrap_or("user"));
            a.set_state(&whose.nick().to_variant());
            w.prefs.set("show-whose-processes", whose.nick().to_string());
            w.procs.set_whose(whose);
            w.stack.set_visible_child_name("processes");
        });
    }
    window.add_action(&show);

    for (i, (name, _, _)) in PAGES.iter().enumerate() {
        app.set_accels_for_action(&format!("win.show-page::{name}"), &[&format!("<Control>{}", i + 1)]);
    }
    let accels: [(&str, &[&str]); 11] = [
        ("win.refresh", &["<Control>r", "F5"]),
        ("win.search", &["<Control>f"]),
        ("win.preferences", &["<Control>comma"]),
        ("win.shortcuts", &["<Control>question"]),
        ("win.proc-properties", &["<Alt>Return"]),
        ("win.proc-priority", &["<Control>n"]),
        ("win.proc-stop", &["<Control>s"]),
        ("win.proc-continue", &["<Control>c"]),
        ("win.proc-end", &["<Control>e"]),
        ("win.proc-kill", &["<Control>k"]),
        ("win.proc-menu", &["Menu", "<Shift>F10"]),
    ];
    for (action, keys) in accels {
        app.set_accels_for_action(action, keys);
    }
}

fn show_preferences(w: &Rc<Win>) {
    let dialog = adw::PreferencesDialog::builder().title("Preferences").build();

    let switch = |title: &str, subtitle: &str, key: &'static str, default: bool, on_change: BoolFn| {
        let row = adw::SwitchRow::builder().title(title).subtitle(subtitle).active(w.prefs.get(key, default)).build();
        let weak = Rc::downgrade(w);
        row.connect_active_notify(move |r| {
            if let Some(w) = weak.upgrade() {
                w.prefs.set(key, r.is_active());
                on_change(&w, r.is_active());
            }
        });
        row
    };
    let spin = |title: &str, key: &'static str, default: i32, on_change: U32Fn| {
        let ms = clamp_interval(w.prefs.get::<i32>(key, default).max(0) as u32);
        let row = adw::SpinRow::with_range(0.25, 100.0, 0.25);
        row.set_title(title);
        row.set_subtitle("Seconds");
        row.set_digits(2);
        row.set_value(ms as f64 / 1000.0);
        let weak = Rc::downgrade(w);
        row.connect_value_notify(move |r| {
            if let Some(w) = weak.upgrade() {
                let ms = clamp_interval((r.value() * 1000.0).round() as u32);
                w.prefs.set(key, ms as i32);
                on_change(&w, ms);
            }
        });
        row
    };

    let page = adw::PreferencesPage::builder().title("General").icon_name("preferences-system-symbolic").build();

    let g = adw::PreferencesGroup::builder().title("Processes").build();
    g.add(&spin("Update Interval", "update-interval", 3000, Box::new(|w, ms| {
        w.config.interval_ms.store(ms, Ordering::Relaxed);
        w.res.set_interval(ms);
        w.refresh();
    })));
    g.add(&switch("Divide CPU Usage by CPU Count", "“Solaris mode”: a process using every core shows 100 %", "solaris-mode", true, Box::new(|w, v| {
        w.config.solaris.store(v, Ordering::Relaxed);
        w.refresh();
    })));
    g.add(&switch("Alert Before Ending or Killing Processes", "", "kill-dialog", true, Box::new(|_, _| {})));
    g.add(&switch("Show Memory in IEC Units", "KiB, MiB, GiB instead of kB, MB, GB", "process-memory-in-iec", false, Box::new(|w, v| {
        w.procs.opts.set(DisplayOpts { iec: v });
        w.procs.reformat();
    })));
    page.add(&g);

    let g = adw::PreferencesGroup::builder().title("Resources").build();
    g.add(&switch("Show Memory in IEC Units", "", "resources-memory-in-iec", false, Box::new(|w, v| w.res.iec.set(v))));
    g.add(&switch("Show Network Speed in Bits", "", "network-in-bits", false, Box::new(|w, v| w.res.set_bits(v))));
    page.add(&g);

    let g = adw::PreferencesGroup::builder().title("File Systems").build();
    g.add(&spin("Update Interval", "disks-interval", 5000, Box::new(|w, ms| {
        w.config.disks_interval_ms.store(ms, Ordering::Relaxed);
    })));
    g.add(&switch("Show All File Systems", "Include virtual and duplicate mounts", "show-all-fs", false, Box::new(|w, v| {
        w.config.show_all_fs.store(v, Ordering::Relaxed);
        w.refresh();
    })));
    page.add(&g);

    dialog.add(&page);
    dialog.present(Some(&w.window));
}

fn show_shortcuts(w: &Rc<Win>) {
    let dialog = adw::PreferencesDialog::builder().title("Keyboard Shortcuts").build();
    let page = adw::PreferencesPage::new();
    let groups: [(&str, &[(&str, &str)]); 3] = [
        ("General", &[("Processes / Resources / File Systems", "Ctrl+1 / 2 / 3"), ("Refresh", "Ctrl+R"), ("Search", "Ctrl+F"), ("Preferences", "Ctrl+,"), ("Close Window", "Ctrl+W"), ("Quit", "Ctrl+Q")]),
        ("Process", &[("Properties", "Alt+Return"), ("Context Menu", "Menu / Shift+F10"), ("Change Priority", "Ctrl+N")]),
        ("Signals", &[("Stop", "Ctrl+S"), ("Continue", "Ctrl+C"), ("End", "Ctrl+E"), ("Kill", "Ctrl+K")]),
    ];
    for (title, rows) in groups {
        let g = adw::PreferencesGroup::builder().title(title).build();
        for (name, keys) in rows {
            let row = adw::ActionRow::builder().title(*name).build();
            let l = gtk::Label::new(Some(keys));
            l.add_css_class("dim-label");
            row.add_suffix(&l);
            g.add(&row);
        }
        page.add(&g);
    }
    dialog.add(&page);
    dialog.present(Some(&w.window));
}
