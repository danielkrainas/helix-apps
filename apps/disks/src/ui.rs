//! GTK user interface.

use adw::prelude::*;
use gtk::{gdk, gio, glib, pango};
use helix_app_common::Prefs;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;

use crate::format;
use crate::layout::{self, Node};
use crate::model::{self, Drive, DriveKind, Model, Volume, VolumeKind};
use crate::ops::{self, Op, OpOutput};
use crate::parttypes;
use crate::settings::{self, Settings};
use crate::smart::Attribute;
use crate::udisks;

pub const APP_ID: &str = "org.helix.Disks";
const SCHEMA: &str = "org.gnome.Disks";
const BAR_HEIGHT: i32 = 96;
const BAR_MIN_SEGMENT: f64 = 40.0;
const ACCENT: (f64, f64, f64) = (0.208, 0.518, 0.894); // #3584e4

/// Filesystem choices for Format / Create Partition: (id, label).
/// `ext4+luks` means ext4 inside a LUKS2 container.
const FS_CHOICES: &[(&str, &str)] = &[
    ("ext4", "Ext4 — Linux"),
    ("ext4+luks", "Ext4 in LUKS — encrypted, Linux"),
    ("vfat", "FAT — all systems and devices"),
    ("exfat", "exFAT — all systems, large files"),
    ("ntfs", "NTFS — Windows"),
    ("btrfs", "Btrfs — Linux"),
    ("xfs", "XFS — Linux"),
    ("swap", "Linux Swap"),
    ("empty", "No Filesystem"),
];

const DRIVE_ACTIONS: &[&str] = &[
    "format-disk",
    "create-disk-image",
    "restore-disk-image",
    "smart",
    "power-off",
    "eject",
    "detach",
];
const VOLUME_ACTIONS: &[&str] = &[
    "mount",
    "lock",
    "create-partition",
    "delete-partition",
    "format-partition",
    "edit-partition",
    "edit-filesystem",
    "check-fs",
    "repair-fs",
    "create-partition-image",
    "restore-partition-image",
];

pub fn run(block_device: Option<String>) -> glib::ExitCode {
    let app = adw::Application::builder().application_id(APP_ID).build();
    app.connect_startup(|app| {
        helix_app_common::install_standard_actions(app, "Disks", "Manage drives and media");
        app.set_accels_for_action("win.attach-image", &["<Control>i"]);
        app.set_accels_for_action("win.smart", &["<Control><Shift>s"]);
    });
    let initial = RefCell::new(block_device);
    app.connect_activate(move |app| {
        if let Some(w) = app.active_window() {
            w.present();
            return;
        }
        let ui = Ui::new(app, initial.take());
        ui.window.present();
    });
    // Arguments were parsed in main().
    app.run_with_args::<&str>(&[])
}

struct DriveRows {
    model: adw::ActionRow,
    serial: adw::ActionRow,
    size: adw::ActionRow,
    partitioning: adw::ActionRow,
    assessment: adw::ActionRow,
    backing: adw::ActionRow,
    autoclear: adw::ActionRow,
}

struct VolumeRows {
    group: adw::PreferencesGroup,
    size: adw::ActionRow,
    device: adw::ActionRow,
    uuid: adw::ActionRow,
    ptype: adw::ActionRow,
    contents: adw::ActionRow,
    mount_link: gtk::LinkButton,
}

struct Ui {
    window: adw::ApplicationWindow,
    prefs: Prefs,
    settings: RefCell<Settings>,
    /// Everything UDisks2 reported; `model` is the filtered view of it.
    raw_model: RefCell<Model>,
    model: RefCell<Model>,
    drive_key: RefCell<Option<String>>,
    volume_key: RefCell<Option<String>>,
    /// Device given on the command line, resolved on the first snapshot.
    pending_device: RefCell<Option<String>>,
    /// Block path of a just-attached loop device to select when it appears.
    pending_select: RefCell<Option<String>>,
    busy: Cell<u32>,
    suppress_select: Cell<bool>,
    row_keys: RefCell<Vec<String>>,
    actions: HashMap<&'static str, gio::SimpleAction>,
    split: adw::NavigationSplitView,
    sidebar_stack: gtk::Stack,
    status_page: adw::StatusPage,
    list: gtk::ListBox,
    content_stack: gtk::Stack,
    content_title: adw::WindowTitle,
    spinner: gtk::Spinner,
    eject_btn: gtk::Button,
    power_btn: gtk::Button,
    detach_btn: gtk::Button,
    drive_rows: DriveRows,
    bar: gtk::DrawingArea,
    mount_btn: gtk::Button,
    lock_btn: gtk::Button,
    add_btn: gtk::Button,
    remove_btn: gtk::Button,
    vol_rows: VolumeRows,
}

fn property_row(title: &str) -> adw::ActionRow {
    let row = adw::ActionRow::builder()
        .title(title)
        .subtitle_selectable(true)
        .build();
    row.add_css_class("property");
    row
}

fn icon_button(icon: &str, tooltip: &str, action: &str) -> gtk::Button {
    gtk::Button::builder()
        .icon_name(icon)
        .tooltip_text(tooltip)
        .action_name(action)
        .build()
}

fn node_tree(vols: &[Volume]) -> Vec<Node> {
    vols.iter()
        .map(|v| Node {
            size: v.size,
            children: node_tree(&v.children),
        })
        .collect()
}

fn vol_at<'a>(vols: &'a [Volume], path: &[usize]) -> Option<&'a Volume> {
    let (first, rest) = path.split_first()?;
    let v = vols.get(*first)?;
    if rest.is_empty() {
        Some(v)
    } else {
        vol_at(&v.children, rest)
    }
}

fn describe_volume(v: &Volume) -> String {
    if v.device.is_empty() {
        format!("{} ({})", v.short_name(), format::size(v.size))
    } else {
        format!(
            "“{}” ({}, {})",
            v.short_name(),
            format::size(v.size),
            v.device
        )
    }
}

fn describe_drive(d: &Drive) -> String {
    let m = d.model_name();
    if m.is_empty() {
        format!("{} ({})", d.title(), d.device)
    } else {
        format!("{} — {} ({})", d.title(), m, d.device)
    }
}

fn boxed_list() -> gtk::ListBox {
    let l = gtk::ListBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .build();
    l.add_css_class("boxed-list");
    l
}

impl Ui {
    fn new(app: &adw::Application, block_device: Option<String>) -> Rc<Self> {
        let window = adw::ApplicationWindow::builder()
            .application(app)
            .title("Disks")
            .default_width(980)
            .default_height(680)
            .width_request(360)
            .height_request(300)
            .build();

        // ---- actions
        let mut actions = HashMap::new();
        for name in DRIVE_ACTIONS
            .iter()
            .chain(VOLUME_ACTIONS)
            .chain(&["attach-image"])
        {
            let a = gio::SimpleAction::new(name, None);
            window.add_action(&a);
            actions.insert(*name, a);
        }

        let settings = Settings::load_from(&settings::path());
        let hide_snap = gio::SimpleAction::new_stateful(
            "hide-snap-loops",
            None,
            &settings.hide_snap_loops.to_variant(),
        );
        window.add_action(&hide_snap);

        // ---- sidebar
        let menu = gio::Menu::new();
        menu.append(Some("Attach Disk Image…"), Some("win.attach-image"));
        let view_section = gio::Menu::new();
        view_section.append(Some("Hide Snap Loop Devices"), Some("win.hide-snap-loops"));
        menu.append_section(None, &view_section);
        let section = gio::Menu::new();
        section.append(Some("About Disks"), Some("app.about"));
        menu.append_section(None, &section);
        let menu_btn = gtk::MenuButton::builder()
            .icon_name("open-menu-symbolic")
            .menu_model(&menu)
            .primary(true)
            .tooltip_text("Main Menu")
            .build();
        let sidebar_header = adw::HeaderBar::new();
        sidebar_header.pack_start(&icon_button(
            "list-add-symbolic",
            "Attach Disk Image…",
            "win.attach-image",
        ));
        sidebar_header.pack_end(&menu_btn);
        let list = gtk::ListBox::new();
        list.add_css_class("navigation-sidebar");
        let status_page = adw::StatusPage::builder()
            .icon_name("drive-harddisk-symbolic")
            .title("Unable to Connect to UDisks2")
            .build();
        status_page.add_css_class("compact");
        let sidebar_stack = gtk::Stack::new();
        sidebar_stack.add_named(
            &gtk::ScrolledWindow::builder()
                .child(&list)
                .hscrollbar_policy(gtk::PolicyType::Never)
                .build(),
            Some("list"),
        );
        sidebar_stack.add_named(&status_page, Some("status"));
        let sidebar_view = adw::ToolbarView::new();
        sidebar_view.add_top_bar(&sidebar_header);
        sidebar_view.set_content(Some(&sidebar_stack));
        let sidebar_page = adw::NavigationPage::builder()
            .title("Disks")
            .child(&sidebar_view)
            .build();

        // ---- content header
        let content_title = adw::WindowTitle::new("", "");
        let content_header = adw::HeaderBar::builder()
            .title_widget(&content_title)
            .build();
        let drive_menu = gio::Menu::new();
        let s1 = gio::Menu::new();
        s1.append(Some("Format Disk…"), Some("win.format-disk"));
        let s2 = gio::Menu::new();
        s2.append(Some("Create Disk Image…"), Some("win.create-disk-image"));
        s2.append(Some("Restore Disk Image…"), Some("win.restore-disk-image"));
        let s3 = gio::Menu::new();
        s3.append(Some("SMART Data & Self-Tests…"), Some("win.smart"));
        let s4 = gio::Menu::new();
        s4.append(Some("Power Off"), Some("win.power-off"));
        s4.append(Some("Detach Loop Device"), Some("win.detach"));
        for s in [&s1, &s2, &s3, &s4] {
            drive_menu.append_section(None, s);
        }
        let drive_menu_btn = gtk::MenuButton::builder()
            .icon_name("view-more-symbolic")
            .menu_model(&drive_menu)
            .tooltip_text("Drive Options")
            .build();
        let eject_btn = icon_button("media-eject-symbolic", "Eject", "win.eject");
        let power_btn = icon_button("system-shutdown-symbolic", "Power Off", "win.power-off");
        let detach_btn = icon_button("window-close-symbolic", "Detach Loop Device", "win.detach");
        let spinner = gtk::Spinner::new();
        spinner.set_visible(false);
        content_header.pack_end(&drive_menu_btn);
        content_header.pack_end(&power_btn);
        content_header.pack_end(&eject_btn);
        content_header.pack_end(&detach_btn);
        content_header.pack_start(&spinner);

        // ---- drive group
        let drive_group = adw::PreferencesGroup::new();
        let drive_rows = DriveRows {
            model: property_row("Model"),
            serial: property_row("Serial Number"),
            size: property_row("Size"),
            partitioning: property_row("Partitioning"),
            assessment: property_row("Assessment"),
            backing: property_row("Backing File"),
            autoclear: property_row("Auto-Clear"),
        };
        let smart_btn = gtk::Button::builder()
            .label("SMART Data…")
            .valign(gtk::Align::Center)
            .action_name("win.smart")
            .build();
        smart_btn.add_css_class("flat");
        drive_rows.assessment.add_suffix(&smart_btn);
        for r in [
            &drive_rows.model,
            &drive_rows.serial,
            &drive_rows.size,
            &drive_rows.partitioning,
            &drive_rows.assessment,
            &drive_rows.backing,
            &drive_rows.autoclear,
        ] {
            drive_group.add(r);
        }

        // ---- volume bar + toolbar
        let bar = gtk::DrawingArea::builder()
            .content_height(BAR_HEIGHT)
            .hexpand(true)
            .focusable(true)
            .build();
        bar.update_property(&[gtk::accessible::Property::Label("Volumes")]);
        let mount_btn = icon_button("media-playback-start-symbolic", "Mount", "win.mount");
        let lock_btn = icon_button("changes-allow-symbolic", "Unlock", "win.lock");
        let add_btn = icon_button(
            "list-add-symbolic",
            "Create Partition…",
            "win.create-partition",
        );
        let remove_btn = icon_button(
            "list-remove-symbolic",
            "Delete Partition…",
            "win.delete-partition",
        );
        let vol_menu = gio::Menu::new();
        let v1 = gio::Menu::new();
        v1.append(Some("Format Partition…"), Some("win.format-partition"));
        v1.append(Some("Edit Partition…"), Some("win.edit-partition"));
        v1.append(Some("Edit Filesystem…"), Some("win.edit-filesystem"));
        let v2 = gio::Menu::new();
        v2.append(Some("Check Filesystem…"), Some("win.check-fs"));
        v2.append(Some("Repair Filesystem…"), Some("win.repair-fs"));
        let v3 = gio::Menu::new();
        v3.append(
            Some("Create Partition Image…"),
            Some("win.create-partition-image"),
        );
        v3.append(
            Some("Restore Partition Image…"),
            Some("win.restore-partition-image"),
        );
        for s in [&v1, &v2, &v3] {
            vol_menu.append_section(None, s);
        }
        let vol_menu_btn = gtk::MenuButton::builder()
            .icon_name("emblem-system-symbolic")
            .menu_model(&vol_menu)
            .tooltip_text("Additional Partition Options")
            .build();
        let toolbar = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        for b in [&mount_btn, &lock_btn, &add_btn, &remove_btn] {
            b.add_css_class("flat");
            toolbar.append(b);
        }
        vol_menu_btn.add_css_class("flat");
        toolbar.append(&vol_menu_btn);
        let bar_frame = gtk::Frame::builder().child(&bar).build();
        let bar_box = gtk::Box::new(gtk::Orientation::Vertical, 6);
        bar_box.append(&bar_frame);
        bar_box.append(&toolbar);
        let volumes_group = adw::PreferencesGroup::builder().title("Volumes").build();
        volumes_group.add(&bar_box);

        // ---- volume details
        let vol_group = adw::PreferencesGroup::new();
        let mount_link = gtk::LinkButton::builder()
            .valign(gtk::Align::Center)
            .build();
        let vol_rows = VolumeRows {
            group: vol_group.clone(),
            size: property_row("Size"),
            device: property_row("Device"),
            uuid: property_row("UUID"),
            ptype: property_row("Partition Type"),
            contents: property_row("Contents"),
            mount_link,
        };
        vol_rows.contents.add_suffix(&vol_rows.mount_link);
        for r in [
            &vol_rows.size,
            &vol_rows.device,
            &vol_rows.uuid,
            &vol_rows.ptype,
            &vol_rows.contents,
        ] {
            vol_group.add(r);
        }

        let page_box = gtk::Box::new(gtk::Orientation::Vertical, 24);
        page_box.set_margin_top(24);
        page_box.set_margin_bottom(24);
        page_box.set_margin_start(12);
        page_box.set_margin_end(12);
        page_box.append(&drive_group);
        page_box.append(&volumes_group);
        page_box.append(&vol_group);
        let clamp = adw::Clamp::builder()
            .maximum_size(860)
            .child(&page_box)
            .build();
        let empty = adw::StatusPage::builder()
            .icon_name("drive-harddisk-symbolic")
            .title("No Drive Selected")
            .description("Select a drive from the list")
            .build();
        let content_stack = gtk::Stack::new();
        content_stack.add_named(&empty, Some("empty"));
        content_stack.add_named(
            &gtk::ScrolledWindow::builder()
                .child(&clamp)
                .hscrollbar_policy(gtk::PolicyType::Never)
                .build(),
            Some("drive"),
        );
        let content_view = adw::ToolbarView::new();
        content_view.add_top_bar(&content_header);
        content_view.set_content(Some(&content_stack));
        let content_page = adw::NavigationPage::builder()
            .title("Drive")
            .child(&content_view)
            .build();

        let split = adw::NavigationSplitView::builder()
            .sidebar(&sidebar_page)
            .content(&content_page)
            .min_sidebar_width(240.0)
            .max_sidebar_width(320.0)
            .build();
        window.set_content(Some(&split));
        let bp = adw::Breakpoint::new(
            adw::BreakpointCondition::parse("max-width: 640sp").expect("valid condition"),
        );
        bp.add_setter(&split, "collapsed", Some(&true.to_value()));
        window.add_breakpoint(bp);

        let ui = Rc::new(Ui {
            window,
            prefs: Prefs::new(SCHEMA),
            settings: RefCell::new(settings),
            raw_model: RefCell::new(Model::default()),
            model: RefCell::new(Model::default()),
            drive_key: RefCell::new(None),
            volume_key: RefCell::new(None),
            pending_device: RefCell::new(block_device),
            pending_select: RefCell::new(None),
            busy: Cell::new(0),
            suppress_select: Cell::new(false),
            row_keys: RefCell::new(Vec::new()),
            actions,
            split,
            sidebar_stack,
            status_page,
            list,
            content_stack,
            content_title,
            spinner,
            eject_btn,
            power_btn,
            detach_btn,
            drive_rows,
            bar,
            mount_btn,
            lock_btn,
            add_btn,
            remove_btn,
            vol_rows,
        });
        ui.connect_signals();
        ui.connect_actions();
        ui.update_page();

        let (tx, rx) = async_channel::unbounded();
        udisks::spawn_monitor(tx);
        let weak = Rc::downgrade(&ui);
        glib::spawn_future_local(async move {
            while let Ok(ev) = rx.recv().await {
                let Some(ui) = weak.upgrade() else { break };
                match ev {
                    udisks::Event::Snapshot(s) => ui.set_model(model::build(&s)),
                    udisks::Event::Error(e) => {
                        ui.status_page.set_description(Some(&e));
                        ui.sidebar_stack.set_visible_child_name("status");
                    }
                }
            }
        });
        // Every signal handler holds only a weak reference: keep the Ui
        // alive for as long as its window exists.
        let keep_alive = RefCell::new(Some(ui.clone()));
        ui.window.connect_destroy(move |_| {
            keep_alive.borrow_mut().take();
        });
        ui
    }

    // ------------------------------------------------------------ wiring

    fn connect_signals(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        self.list.connect_row_selected(move |_, row| {
            let (Some(ui), Some(row)) = (weak.upgrade(), row) else {
                return;
            };
            if ui.suppress_select.get() {
                return;
            }
            let key = ui.row_keys.borrow().get(row.index() as usize).cloned();
            if key.is_some() && key != *ui.drive_key.borrow() {
                *ui.drive_key.borrow_mut() = key;
                *ui.volume_key.borrow_mut() = None;
                ui.update_page();
            }
            ui.split.set_show_content(true);
        });

        let weak = Rc::downgrade(self);
        self.bar.set_draw_func(move |area, cr, w, h| {
            if let Some(ui) = weak.upgrade() {
                ui.draw_bar(area, cr, w as f64, h as f64);
            }
        });

        let click = gtk::GestureClick::new();
        let weak = Rc::downgrade(self);
        click.connect_pressed(move |_, _, x, y| {
            let Some(ui) = weak.upgrade() else { return };
            ui.bar.grab_focus();
            let key = {
                let model = ui.model.borrow();
                let Some(drive) = ui.current_drive(&model) else {
                    return;
                };
                let rects = layout::bar(
                    &node_tree(&drive.volumes),
                    ui.bar.width() as f64,
                    ui.bar.height() as f64,
                    BAR_MIN_SEGMENT,
                );
                // Deepest hit wins (children come after their parent).
                rects
                    .iter()
                    .rev()
                    .find(|(_, r)| r.contains(x, y))
                    .and_then(|(p, _)| vol_at(&drive.volumes, p))
                    .map(|v| v.key())
            };
            if let Some(k) = key {
                ui.select_volume(Some(k));
            }
        });
        self.bar.add_controller(click);

        let keys = gtk::EventControllerKey::new();
        let weak = Rc::downgrade(self);
        keys.connect_key_pressed(move |_, key, _, _| {
            let Some(ui) = weak.upgrade() else {
                return glib::Propagation::Proceed;
            };
            let step: isize = match key {
                gdk::Key::Left | gdk::Key::KP_Left | gdk::Key::Up => -1,
                gdk::Key::Right | gdk::Key::KP_Right | gdk::Key::Down => 1,
                _ => return glib::Propagation::Proceed,
            };
            let next = {
                let model = ui.model.borrow();
                let Some(drive) = ui.current_drive(&model) else {
                    return glib::Propagation::Proceed;
                };
                let all: Vec<String> = drive.all_volumes().iter().map(|v| v.key()).collect();
                let cur = ui
                    .volume_key
                    .borrow()
                    .as_ref()
                    .and_then(|k| all.iter().position(|x| x == k))
                    .unwrap_or(0) as isize;
                all.get((cur + step).clamp(0, all.len() as isize - 1) as usize)
                    .cloned()
            };
            ui.select_volume(next);
            glib::Propagation::Stop
        });
        self.bar.add_controller(keys);
    }

    fn on<F: Fn(&Rc<Ui>) + 'static>(self: &Rc<Self>, name: &str, f: F) {
        let weak = Rc::downgrade(self);
        self.actions[name].connect_activate(move |_, _| {
            if let Some(ui) = weak.upgrade() {
                f(&ui);
            }
        });
    }

    fn set_hide_snap_loops(self: &Rc<Self>, hide: bool) {
        let mut st = self.settings.borrow_mut();
        if st.hide_snap_loops == hide {
            return;
        }
        st.hide_snap_loops = hide;
        if let Err(e) = st.save_to(&settings::path()) {
            eprintln!("helix-disks: cannot save settings: {e}");
        }
        drop(st);
        // Not connected to UDisks2 (status page shown): nothing to refilter.
        if self.sidebar_stack.visible_child_name().as_deref() == Some("list") {
            let raw = self.raw_model.borrow().clone();
            self.set_model(raw);
        }
    }

    fn connect_actions(self: &Rc<Self>) {
        if let Some(a) = self
            .window
            .lookup_action("hide-snap-loops")
            .and_downcast::<gio::SimpleAction>()
        {
            let weak = Rc::downgrade(self);
            a.connect_change_state(move |a, v| {
                let Some(hide) = v.and_then(|v| v.get::<bool>()) else {
                    return;
                };
                a.set_state(&hide.to_variant());
                if let Some(ui) = weak.upgrade() {
                    ui.set_hide_snap_loops(hide);
                }
            });
        }
        self.on("attach-image", |ui| ui.attach_image());
        self.on("format-disk", |ui| ui.format_disk());
        self.on("create-disk-image", |ui| ui.create_image(true));
        self.on("restore-disk-image", |ui| ui.restore_image(true));
        self.on("smart", |ui| ui.show_smart());
        self.on("power-off", |ui| ui.remove_drive(RemoveKind::PowerOff));
        self.on("eject", |ui| ui.remove_drive(RemoveKind::Eject));
        self.on("detach", |ui| ui.remove_drive(RemoveKind::Detach));
        self.on("mount", |ui| ui.toggle_mount());
        self.on("lock", |ui| ui.toggle_lock());
        self.on("create-partition", |ui| ui.create_partition());
        self.on("delete-partition", |ui| ui.delete_partition());
        self.on("format-partition", |ui| ui.format_volume());
        self.on("edit-partition", |ui| ui.edit_partition());
        self.on("edit-filesystem", |ui| ui.edit_filesystem());
        self.on("check-fs", |ui| ui.check_fs(false));
        self.on("repair-fs", |ui| ui.check_fs(true));
        self.on("create-partition-image", |ui| ui.create_image(false));
        self.on("restore-partition-image", |ui| ui.restore_image(false));
    }

    // ------------------------------------------------------------ state

    fn current_drive<'a>(&self, model: &'a Model) -> Option<&'a Drive> {
        self.drive_key
            .borrow()
            .as_deref()
            .and_then(|k| model.find_drive(k))
    }

    /// Clones of the selected drive and volume (to avoid holding borrows
    /// across dialogs).
    fn selection(&self) -> (Option<Drive>, Option<Volume>) {
        let model = self.model.borrow();
        let drive = self.current_drive(&model).cloned();
        let vol = drive.as_ref().and_then(|d| {
            self.volume_key
                .borrow()
                .as_deref()
                .and_then(|k| d.find_volume(k))
                .cloned()
        });
        (drive, vol)
    }

    fn set_model(self: &Rc<Self>, m: Model) {
        self.sidebar_stack.set_visible_child_name("list");
        let hide_snap = self.settings.borrow().hide_snap_loops;
        *self.model.borrow_mut() = m.visible(hide_snap);
        *self.raw_model.borrow_mut() = m;
        if let Some(dev) = self.pending_device.borrow_mut().take() {
            match self.model.borrow().find_device(&dev) {
                Some((d, v)) => {
                    *self.drive_key.borrow_mut() = Some(d);
                    *self.volume_key.borrow_mut() = v;
                }
                None if self.raw_model.borrow().find_device(&dev).is_some() => {
                    eprintln!("helix-disks: {dev} is a snap loop device, which is hidden (see the main menu)")
                }
                None => eprintln!("helix-disks: no such block device: {dev}"),
            }
        }
        let pending = self.pending_select.borrow().clone();
        if let Some(p) = pending {
            if self.model.borrow().find_drive(&p).is_some() {
                *self.drive_key.borrow_mut() = Some(p);
                *self.volume_key.borrow_mut() = None;
                *self.pending_select.borrow_mut() = None;
            }
        }
        {
            let model = self.model.borrow();
            if self.current_drive(&model).is_none() {
                *self.drive_key.borrow_mut() = model.drives.first().map(|d| d.key.clone());
            }
        }
        self.rebuild_list();
        self.update_page();
    }

    fn rebuild_list(&self) {
        self.suppress_select.set(true);
        while let Some(child) = self.list.first_child() {
            self.list.remove(&child);
        }
        let model = self.model.borrow();
        let mut keys = Vec::new();
        let mut selected = None;
        let mut last_kind = None;
        for d in &model.drives {
            let row = adw::ActionRow::builder()
                .title(glib::markup_escape_text(&d.title()))
                .subtitle(glib::markup_escape_text(&d.subtitle()))
                .build();
            row.add_prefix(&gtk::Image::from_icon_name(d.icon_name()));
            row.set_tooltip_text(Some(&d.device));
            if last_kind.is_some_and(|k| k != d.kind) {
                // Visual separation between drives and loop devices.
                row.set_margin_top(12);
            }
            last_kind = Some(d.kind);
            if Some(&d.key) == self.drive_key.borrow().as_ref() {
                selected = Some(row.clone());
            }
            self.list.append(&row);
            keys.push(d.key.clone());
        }
        *self.row_keys.borrow_mut() = keys;
        if let Some(row) = selected {
            if let Some(lbr) = row.dynamic_cast_ref::<gtk::ListBoxRow>() {
                self.list.select_row(Some(lbr));
            }
        }
        self.suppress_select.set(false);
    }

    fn select_volume(&self, key: Option<String>) {
        *self.volume_key.borrow_mut() = key;
        self.update_volume();
        self.bar.queue_draw();
    }

    fn set_busy(&self, on: bool) {
        let n = if on {
            self.busy.get() + 1
        } else {
            self.busy.get().saturating_sub(1)
        };
        self.busy.set(n);
        self.spinner.set_visible(n > 0);
        self.spinner.set_spinning(n > 0);
        self.update_actions();
    }

    fn update_page(&self) {
        let model = self.model.borrow();
        let Some(d) = self.current_drive(&model) else {
            self.content_stack.set_visible_child_name("empty");
            self.content_title.set_title("");
            self.content_title.set_subtitle("");
            drop(model);
            self.update_actions();
            return;
        };
        self.content_stack.set_visible_child_name("drive");
        self.content_title.set_title(&d.title());
        self.content_title.set_subtitle(&d.device);

        let r = &self.drive_rows;
        let mut model_text = d.model_name();
        if d.kind == DriveKind::Disk && !d.revision.is_empty() {
            model_text = format!("{model_text} ({})", d.revision);
        }
        r.model.set_subtitle(&glib::markup_escape_text(&model_text));
        r.model
            .set_visible(d.kind == DriveKind::Disk && !model_text.trim().is_empty());
        r.serial.set_subtitle(&glib::markup_escape_text(&d.serial));
        r.serial.set_visible(!d.serial.is_empty());
        r.size.set_subtitle(&if d.size > 0 {
            format::size_long(d.size)
        } else {
            "No Media".into()
        });
        r.partitioning.set_subtitle(&d.partitioning());
        match &d.smart {
            Some(s) => {
                r.assessment
                    .set_subtitle(&glib::markup_escape_text(&s.assessment()));
                if s.is_warning() {
                    r.assessment.add_css_class("error");
                } else {
                    r.assessment.remove_css_class("error");
                }
                r.assessment.set_visible(true);
            }
            None => r.assessment.set_visible(false),
        }
        r.backing.set_visible(d.kind == DriveKind::Loop);
        r.backing.set_subtitle(&glib::markup_escape_text(
            d.backing_file.as_deref().unwrap_or(""),
        ));
        r.autoclear.set_visible(d.kind == DriveKind::Loop);
        r.autoclear
            .set_subtitle(if d.autoclear { "On" } else { "Off" });

        // Keep the selected volume if it still exists, else pick the first.
        let keep = self
            .volume_key
            .borrow()
            .as_deref()
            .is_some_and(|k| d.find_volume(k).is_some());
        if !keep {
            *self.volume_key.borrow_mut() = d.volumes.first().map(|v| v.key());
        }
        drop(model);
        self.bar.queue_draw();
        self.update_volume();
    }

    fn update_volume(&self) {
        let (_, vol) = self.selection();
        let rows = &self.vol_rows;
        rows.group.set_visible(vol.is_some());
        if let Some(v) = &vol {
            rows.size.set_subtitle(&format::size_long(v.size));
            rows.device.set_subtitle(&v.device);
            rows.device.set_visible(!v.device.is_empty());
            rows.uuid.set_subtitle(&v.uuid);
            rows.uuid.set_visible(!v.uuid.is_empty());
            match v.partition_type() {
                Some(t) => {
                    rows.ptype.set_subtitle(&glib::markup_escape_text(&t));
                    rows.ptype.set_visible(true);
                }
                None => rows.ptype.set_visible(false),
            }
            rows.contents
                .set_subtitle(&glib::markup_escape_text(&v.contents()));
            match v.mount_points().first() {
                Some(mp) => {
                    rows.mount_link.set_label(mp);
                    rows.mount_link.set_uri(
                        &glib::filename_to_uri(mp, None)
                            .map(|u| u.to_string())
                            .unwrap_or_default(),
                    );
                    rows.mount_link
                        .set_tooltip_text(Some("Open in File Manager"));
                    rows.mount_link.set_visible(true);
                }
                None => rows.mount_link.set_visible(false),
            }
        }
        self.update_actions();
    }

    fn update_actions(&self) {
        let busy = self.busy.get() > 0;
        let (drive, vol) = self.selection();
        let en = |name: &str, on: bool| self.actions[name].set_enabled(on && !busy);
        self.actions["attach-image"].set_enabled(true);

        let d = drive.as_ref();
        let has_block = d.is_some_and(|d| d.block_path.is_some() && d.size > 0);
        let writable = d.is_some_and(|d| !d.read_only);
        en("format-disk", has_block && writable);
        en("create-disk-image", has_block);
        en("restore-disk-image", has_block && writable);
        en("smart", d.is_some_and(|d| d.smart.is_some()));
        en("power-off", d.is_some_and(|d| d.can_power_off));
        en("eject", d.is_some_and(|d| d.ejectable));
        en("detach", d.is_some_and(|d| d.kind == DriveKind::Loop));
        self.power_btn
            .set_visible(d.is_some_and(|d| d.can_power_off));
        self.eject_btn.set_visible(d.is_some_and(|d| d.ejectable));
        self.detach_btn
            .set_visible(d.is_some_and(|d| d.kind == DriveKind::Loop));

        let v = vol.as_ref();
        let is_block = v.is_some_and(|v| !v.is_free());
        let is_fs = v.is_some_and(|v| v.filesystem.is_some());
        let is_swap = v.is_some_and(|v| v.swap_active.is_some());
        let mounted = v.is_some_and(|v| v.is_mounted());
        let container = v.is_some_and(|v| v.is_container());
        let ro = v.is_some_and(|v| v.read_only);
        en("mount", is_fs || is_swap);
        self.mount_btn.set_visible(is_fs || is_swap);
        let active = mounted || v.is_some_and(|v| v.swap_active == Some(true));
        self.mount_btn.set_icon_name(if active {
            "media-playback-stop-symbolic"
        } else {
            "media-playback-start-symbolic"
        });
        self.mount_btn
            .set_tooltip_text(Some(match (is_swap, active) {
                (true, true) => "Deactivate Swap",
                (true, false) => "Activate Swap",
                (false, true) => "Unmount",
                (false, false) => "Mount",
            }));
        let encrypted = v.is_some_and(|v| v.encrypted.is_some());
        let unlocked = v.is_some_and(|v| v.is_unlocked());
        en("lock", encrypted);
        self.lock_btn.set_visible(encrypted);
        self.lock_btn.set_icon_name(if unlocked {
            "changes-prevent-symbolic"
        } else {
            "changes-allow-symbolic"
        });
        self.lock_btn
            .set_tooltip_text(Some(if unlocked { "Lock" } else { "Unlock…" }));
        let free = v.is_some_and(|v| v.is_free());
        en("create-partition", free && writable);
        self.add_btn.set_visible(free);
        let part = v.is_some_and(|v| v.partition.is_some());
        en("delete-partition", part && writable);
        self.remove_btn.set_visible(part);
        en("format-partition", is_block && !container && !ro);
        en("edit-partition", part && writable);
        en("edit-filesystem", is_fs && !ro);
        en("check-fs", is_fs && !mounted);
        en("repair-fs", is_fs && !mounted && !ro);
        en("create-partition-image", is_block && !container);
        en("restore-partition-image", is_block && !container && !ro);
    }

    // ------------------------------------------------------------ drawing

    fn draw_bar(&self, area: &gtk::DrawingArea, cr: &gtk::cairo::Context, w: f64, h: f64) {
        let model = self.model.borrow();
        let Some(drive) = self.current_drive(&model) else {
            return;
        };
        let fg = area.color();
        let (fr, fgc, fb) = (fg.red() as f64, fg.green() as f64, fg.blue() as f64);
        if drive.volumes.is_empty() {
            let layout = area.create_pango_layout(Some("No Media"));
            let (lw, lh) = layout.pixel_size();
            cr.set_source_rgba(fr, fgc, fb, 0.6);
            cr.move_to((w - lw as f64) / 2.0, (h - lh as f64) / 2.0);
            pangocairo::functions::show_layout(cr, &layout);
            return;
        }
        let selected = self.volume_key.borrow().clone();
        let focus = area.has_focus();
        let rects = layout::bar(&node_tree(&drive.volumes), w, h, BAR_MIN_SEGMENT);
        for (path, r) in &rects {
            let Some(v) = vol_at(&drive.volumes, path) else {
                continue;
            };
            let (x, y, rw, rh) = (
                r.x + 1.0,
                r.y + 1.0,
                (r.w - 2.0).max(1.0),
                (r.h - 2.0).max(1.0),
            );
            let is_sel = selected.as_deref() == Some(v.key().as_str());
            cr.rectangle(x, y, rw, rh);
            if v.is_free() {
                cr.set_source_rgba(fr, fgc, fb, 0.03);
            } else if v.is_container() {
                cr.set_source_rgba(fr, fgc, fb, 0.05);
            } else {
                cr.set_source_rgba(fr, fgc, fb, 0.10);
            }
            let _ = cr.fill();
            if v.is_free() {
                // Hatching for unallocated space.
                cr.save().ok();
                cr.rectangle(x, y, rw, rh);
                cr.clip();
                cr.set_source_rgba(fr, fgc, fb, 0.12);
                cr.set_line_width(1.0);
                let mut sx = x - rh;
                while sx < x + rw {
                    cr.move_to(sx, y + rh);
                    cr.line_to(sx + rh, y);
                    sx += 8.0;
                }
                let _ = cr.stroke();
                cr.restore().ok();
            }
            if is_sel {
                cr.rectangle(x, y, rw, rh);
                cr.set_source_rgba(
                    ACCENT.0,
                    ACCENT.1,
                    ACCENT.2,
                    if focus { 0.35 } else { 0.22 },
                );
                let _ = cr.fill();
            }
            cr.rectangle(x + 0.5, y + 0.5, rw - 1.0, rh - 1.0);
            if is_sel {
                cr.set_source_rgba(ACCENT.0, ACCENT.1, ACCENT.2, 1.0);
                cr.set_line_width(2.0);
            } else {
                cr.set_source_rgba(fr, fgc, fb, 0.35);
                cr.set_line_width(1.0);
            }
            let _ = cr.stroke();

            if v.is_mounted() || v.swap_active == Some(true) || v.is_unlocked() {
                // Small "in use" marker in the top-left corner.
                cr.set_source_rgba(fr, fgc, fb, 0.7);
                cr.move_to(x + 5.0, y + 5.0);
                cr.line_to(x + 12.0, y + 9.0);
                cr.line_to(x + 5.0, y + 13.0);
                cr.close_path();
                let _ = cr.fill();
            }

            // Labels: name, then size and type.
            let has_children = !v.children.is_empty();
            let mut text = glib::markup_escape_text(&v.short_name()).to_string();
            let mut second = format::size(v.size);
            if let Some(dev) = v.device.strip_prefix("/dev/") {
                second = format!("{second} · {dev}");
            }
            if !has_children && rh > 40.0 {
                text = format!(
                    "<b>{text}</b>\n<small>{}</small>",
                    glib::markup_escape_text(&second)
                );
            } else {
                text = format!(
                    "<small><b>{text}</b> {}</small>",
                    glib::markup_escape_text(&second)
                );
            }
            let layout = area.create_pango_layout(None);
            layout.set_markup(&text);
            layout.set_width(((rw - 8.0).max(1.0) * pango::SCALE as f64) as i32);
            layout.set_ellipsize(pango::EllipsizeMode::End);
            layout.set_alignment(pango::Alignment::Center);
            let (_, lh) = layout.pixel_size();
            if (lh as f64) < rh {
                cr.save().ok();
                cr.rectangle(x, y, rw, rh);
                cr.clip();
                cr.set_source_rgba(fr, fgc, fb, if v.is_free() { 0.6 } else { 0.95 });
                cr.move_to(x + 4.0, y + (rh - lh as f64) / 2.0);
                pangocairo::functions::show_layout(cr, &layout);
                cr.restore().ok();
            }
        }
    }

    // ------------------------------------------------------------ helpers

    fn error(&self, heading: &str, body: &str) {
        let d = adw::AlertDialog::new(Some(heading), Some(body));
        d.add_response("close", "_Close");
        d.present(Some(&self.window));
    }

    fn info(&self, heading: &str, body: &str) {
        self.error(heading, body);
    }

    /// Confirmation for destructive operations. The confirm button is
    /// destructive; Cancel is the default and close response.
    fn confirm<F: Fn(&Rc<Ui>) + 'static>(
        self: &Rc<Self>,
        heading: &str,
        body: &str,
        confirm_label: &str,
        on_yes: F,
    ) {
        let d = adw::AlertDialog::new(Some(heading), Some(body));
        d.add_response("cancel", "_Cancel");
        d.add_response("confirm", confirm_label);
        d.set_response_appearance("confirm", adw::ResponseAppearance::Destructive);
        d.set_default_response(Some("cancel"));
        d.set_close_response("cancel");
        let weak = Rc::downgrade(self);
        d.connect_response(None, move |_, r| {
            if r == "confirm" {
                if let Some(ui) = weak.upgrade() {
                    on_yes(&ui);
                }
            }
        });
        d.present(Some(&self.window));
    }

    /// Runs operations in order on a worker thread; stops at the first error.
    fn run_ops<F: FnOnce(&Rc<Ui>, OpOutput) + 'static>(self: &Rc<Self>, ops: Vec<Op>, done: F) {
        if ops.is_empty() {
            done(self, OpOutput::None);
            return;
        }
        self.set_busy(true);
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let res = gio::spawn_blocking(move || -> Result<OpOutput, (String, &'static str)> {
                let c = udisks::conn().map_err(|e| (e, "connecting to UDisks2"))?;
                let mut out = OpOutput::None;
                for op in &ops {
                    out = udisks::run(&c, op).map_err(|e| (e, op.verb()))?;
                }
                Ok(out)
            })
            .await;
            let Some(ui) = weak.upgrade() else { return };
            ui.set_busy(false);
            match res {
                Ok(Ok(out)) => done(&ui, out),
                Ok(Err((msg, verb))) => ui.error(&format!("Error {verb}"), &msg),
                Err(_) => ui.error("Error", "The operation failed unexpectedly."),
            }
        });
    }

    fn image_dir(&self) -> gio::File {
        let uri = self.prefs.get::<String>("image-dir-uri", String::new());
        if !uri.is_empty() {
            return gio::File::for_uri(&uri);
        }
        let dir =
            glib::user_special_dir(glib::UserDirectory::Documents).unwrap_or_else(glib::home_dir);
        gio::File::for_path(dir)
    }

    fn remember_dir(&self, file: &gio::File) {
        if let Some(parent) = file.parent() {
            self.prefs.set("image-dir-uri", parent.uri().to_string());
        }
    }

    // ------------------------------------------------------------ volume actions

    fn toggle_mount(self: &Rc<Self>) {
        let (_, Some(v)) = self.selection() else {
            return;
        };
        let Some(block) = v.block_path.clone() else {
            return;
        };
        let op = match (v.swap_active, v.is_mounted()) {
            (Some(true), _) => Op::SwapStop { block },
            (Some(false), _) => Op::SwapStart { block },
            (None, true) => Op::Unmount { block },
            (None, false) => Op::Mount { block },
        };
        self.run_ops(vec![op], |_, _| {});
    }

    fn toggle_lock(self: &Rc<Self>) {
        let (_, Some(v)) = self.selection() else {
            return;
        };
        let Some(block) = v.block_path.clone() else {
            return;
        };
        if v.is_unlocked() {
            self.run_ops(ops::teardown(&v), |_, _| {});
            return;
        }
        let d = adw::AlertDialog::new(
            Some("Unlock Encrypted Device"),
            Some(&format!(
                "Enter the passphrase for {}.",
                describe_volume(&v)
            )),
        );
        let list = boxed_list();
        let pass = adw::PasswordEntryRow::builder()
            .title("Passphrase")
            .activates_default(true)
            .build();
        list.append(&pass);
        d.set_extra_child(Some(&list));
        d.add_response("cancel", "_Cancel");
        d.add_response("unlock", "_Unlock");
        d.set_response_appearance("unlock", adw::ResponseAppearance::Suggested);
        d.set_default_response(Some("unlock"));
        d.set_close_response("cancel");
        d.set_response_enabled("unlock", false);
        let dd = d.clone();
        pass.connect_changed(move |p| dd.set_response_enabled("unlock", !p.text().is_empty()));
        let weak = Rc::downgrade(self);
        d.connect_response(None, move |_, r| {
            if r != "unlock" {
                return;
            }
            let Some(ui) = weak.upgrade() else { return };
            ui.run_ops(
                vec![Op::Unlock {
                    block: block.clone(),
                    passphrase: pass.text().to_string(),
                }],
                |_, _| {},
            );
        });
        d.present(Some(&self.window));
    }

    /// Rows for choosing filesystem type, name and (for LUKS) passphrase.
    /// Returns (rows box, getter of (fs_type, label, passphrase), validity notifier).
    fn fs_form(
        list: &gtk::ListBox,
        default_label: &str,
        on_change: Rc<dyn Fn(bool)>,
    ) -> Rc<dyn Fn() -> (String, String, Option<String>)> {
        let name = adw::EntryRow::builder()
            .title("Volume Name")
            .text(default_label)
            .build();
        let labels: Vec<&str> = FS_CHOICES.iter().map(|(_, l)| *l).collect();
        let kind = adw::ComboRow::builder()
            .title("Type")
            .model(&gtk::StringList::new(&labels))
            .build();
        let pass1 = adw::PasswordEntryRow::builder()
            .title("Passphrase")
            .visible(false)
            .build();
        let pass2 = adw::PasswordEntryRow::builder()
            .title("Confirm Passphrase")
            .visible(false)
            .build();
        list.append(&name);
        list.append(&kind);
        list.append(&pass1);
        list.append(&pass2);
        let validate = {
            let (kind, pass1, pass2) = (kind.clone(), pass1.clone(), pass2.clone());
            Rc::new(move || {
                let luks = FS_CHOICES[kind.selected() as usize].0 == "ext4+luks";
                pass1.set_visible(luks);
                pass2.set_visible(luks);
                let ok = !luks || (!pass1.text().is_empty() && pass1.text() == pass2.text());
                on_change(ok);
            })
        };
        let v = validate.clone();
        kind.connect_selected_notify(move |_| v());
        let v = validate.clone();
        pass1.connect_changed(move |_| v());
        let v = validate.clone();
        pass2.connect_changed(move |_| v());
        validate();
        Rc::new(move || {
            let id = FS_CHOICES[kind.selected() as usize].0;
            let (fs, pass) = if id == "ext4+luks" {
                ("ext4", Some(pass1.text().to_string()))
            } else {
                (id, None)
            };
            (fs.to_string(), name.text().to_string(), pass)
        })
    }

    fn format_volume(self: &Rc<Self>) {
        let (_, Some(v)) = self.selection() else {
            return;
        };
        let Some(block) = v.block_path.clone() else {
            return;
        };
        let d = adw::AlertDialog::new(Some("Format Volume"), Some(&describe_volume(&v)));
        d.add_response("cancel", "_Cancel");
        d.add_response("format", "_Next");
        let list = boxed_list();
        let dd = d.clone();
        let get = Ui::fs_form(
            &list,
            &v.label,
            Rc::new(move |ok| dd.set_response_enabled("format", ok)),
        );
        let erase = adw::SwitchRow::builder()
            .title("Erase")
            .subtitle("Overwrite existing data with zeroes (slow)")
            .build();
        list.append(&erase);
        d.set_extra_child(Some(&list));
        d.set_response_appearance("format", adw::ResponseAppearance::Suggested);
        d.set_default_response(Some("cancel"));
        d.set_close_response("cancel");
        let weak = Rc::downgrade(self);
        let is_partition = v.partition.is_some();
        d.connect_response(None, move |_, r| {
            if r != "format" {
                return;
            }
            let Some(ui) = weak.upgrade() else { return };
            let (fs_type, label, passphrase) = get();
            let op = Op::Format {
                block: block.clone(),
                fs_type: fs_type.clone(),
                label,
                erase: erase.is_active(),
                passphrase,
                update_partition_type: is_partition,
            };
            let mut body = format!(
                "All data on the volume {} will be lost and cannot be recovered.\n\nNew type: {}",
                describe_volume(&v),
                model::fs_name(&fs_type, "")
            );
            if v.is_mounted() {
                body.push_str(&format!(
                    "\n\nThe volume is mounted at {} and will be unmounted first.",
                    v.mount_points()[0]
                ));
            }
            ui.confirm("Format Volume?", &body, "_Format", move |ui| {
                ui.run_ops(vec![op.clone()], |_, _| {})
            });
        });
        d.present(Some(&self.window));
    }

    fn create_partition(self: &Rc<Self>) {
        let (Some(drive), Some(v)) = self.selection() else {
            return;
        };
        let VolumeKind::Free { table, .. } = v.kind.clone() else {
            return;
        };
        let table_type = drive.table_type.clone().unwrap_or_default();
        let max_mb = (v.size as f64 / 1e6).floor().max(1.0);
        let d = adw::AlertDialog::new(
            Some("Create Partition"),
            Some(&format!(
                "{} of free space on {}",
                format::size(v.size),
                describe_drive(&drive)
            )),
        );
        let list = boxed_list();
        let size = adw::SpinRow::with_range(1.0, max_mb, 1.0);
        size.set_title("Partition Size (MB)");
        size.set_value(max_mb);
        list.append(&size);
        d.add_response("cancel", "_Cancel");
        d.add_response("create", "C_reate");
        let dd = d.clone();
        let get = Ui::fs_form(
            &list,
            "",
            Rc::new(move |ok| dd.set_response_enabled("create", ok)),
        );
        d.set_extra_child(Some(&list));
        d.set_response_appearance("create", adw::ResponseAppearance::Suggested);
        d.set_default_response(Some("create"));
        d.set_close_response("cancel");
        let weak = Rc::downgrade(self);
        d.connect_response(None, move |_, r| {
            if r != "create" {
                return;
            }
            let Some(ui) = weak.upgrade() else { return };
            let (fs_type, label, passphrase) = get();
            let mb = size.value();
            let bytes = if mb >= max_mb {
                v.size
            } else {
                ((mb * 1e6) as u64).min(v.size)
            };
            let part_type = if passphrase.is_some() && table_type == "gpt" {
                parttypes::LINUX_LUKS_GPT.to_string()
            } else {
                parttypes::default_for_fs(&table_type, &fs_type).to_string()
            };
            let op = Op::CreatePartition {
                table: table.clone(),
                offset: v.offset,
                size: bytes,
                part_type,
                name: if table_type == "gpt" {
                    label.clone()
                } else {
                    String::new()
                },
                fs_type,
                label,
                passphrase,
            };
            ui.run_ops(vec![op], |ui, out| {
                if let OpOutput::Path(p) = out {
                    // Select the new partition once it appears.
                    *ui.volume_key.borrow_mut() = Some(p);
                    ui.update_volume();
                }
            });
        });
        d.present(Some(&self.window));
    }

    fn delete_partition(self: &Rc<Self>) {
        let (_, Some(v)) = self.selection() else {
            return;
        };
        let Some(block) = v.block_path.clone() else {
            return;
        };
        let mut body = format!(
            "All data on the partition {} will be lost and cannot be recovered.",
            describe_volume(&v)
        );
        if v.is_container() {
            body.push_str("\n\nThis is an extended partition: all logical partitions inside it will be deleted too.");
        }
        if v.is_mounted() {
            body.push_str(&format!(
                "\n\nThe partition is mounted at {} and will be unmounted first.",
                v.mount_points()[0]
            ));
        }
        self.confirm("Delete Partition?", &body, "_Delete", move |ui| {
            ui.run_ops(
                vec![Op::DeletePartition {
                    block: block.clone(),
                }],
                |_, _| {},
            );
        });
    }

    fn edit_partition(self: &Rc<Self>) {
        let (_, Some(v)) = self.selection() else {
            return;
        };
        let (Some(block), Some(p)) = (v.block_path.clone(), v.partition.clone()) else {
            return;
        };
        let mut types: Vec<(String, String)> = parttypes::for_table(&p.table_type)
            .iter()
            .map(|t| (t.id.to_string(), t.name.to_string()))
            .collect();
        let current = parttypes::normalize(&p.table_type, &p.type_id);
        let idx = match types.iter().position(|(id, _)| *id == current) {
            Some(i) => i,
            None => {
                types.insert(
                    0,
                    (
                        p.type_id.clone(),
                        parttypes::name(&p.table_type, &p.type_id),
                    ),
                );
                0
            }
        };
        let d = adw::AlertDialog::new(Some("Edit Partition"), Some(&describe_volume(&v)));
        let list = boxed_list();
        let labels: Vec<String> = types.iter().map(|(_, n)| n.clone()).collect();
        let label_refs: Vec<&str> = labels.iter().map(|s| s.as_str()).collect();
        let kind = adw::ComboRow::builder()
            .title("Type")
            .model(&gtk::StringList::new(&label_refs))
            .selected(idx as u32)
            .build();
        list.append(&kind);
        let name = adw::EntryRow::builder()
            .title("Name")
            .text(&p.name)
            .visible(p.table_type == "gpt")
            .build();
        list.append(&name);
        let bootable = adw::SwitchRow::builder()
            .title("Bootable")
            .active(p.flags & parttypes::DOS_FLAG_BOOTABLE != 0)
            .visible(p.table_type == "dos")
            .build();
        list.append(&bootable);
        d.set_extra_child(Some(&list));
        d.add_response("cancel", "_Cancel");
        d.add_response("change", "C_hange");
        d.set_response_appearance("change", adw::ResponseAppearance::Suggested);
        d.set_close_response("cancel");
        let weak = Rc::downgrade(self);
        d.connect_response(None, move |_, r| {
            if r != "change" {
                return;
            }
            let Some(ui) = weak.upgrade() else { return };
            let mut ops = Vec::new();
            let new_type = &types[kind.selected() as usize].0;
            if parttypes::normalize(&p.table_type, new_type) != current {
                ops.push(Op::SetPartitionType {
                    block: block.clone(),
                    part_type: new_type.clone(),
                });
            }
            if p.table_type == "gpt" && name.text() != p.name {
                ops.push(Op::SetPartitionName {
                    block: block.clone(),
                    name: name.text().to_string(),
                });
            }
            if p.table_type == "dos" {
                let flags = if bootable.is_active() {
                    p.flags | parttypes::DOS_FLAG_BOOTABLE
                } else {
                    p.flags & !parttypes::DOS_FLAG_BOOTABLE
                };
                if flags != p.flags {
                    ops.push(Op::SetPartitionFlags {
                        block: block.clone(),
                        flags,
                    });
                }
            }
            ui.run_ops(ops, |_, _| {});
        });
        d.present(Some(&self.window));
    }

    fn edit_filesystem(self: &Rc<Self>) {
        let (_, Some(v)) = self.selection() else {
            return;
        };
        let Some(block) = v.block_path.clone() else {
            return;
        };
        let d = adw::AlertDialog::new(Some("Edit Filesystem"), Some(&describe_volume(&v)));
        let list = boxed_list();
        let label = adw::EntryRow::builder()
            .title("Label")
            .text(&v.label)
            .build();
        list.append(&label);
        d.set_extra_child(Some(&list));
        d.add_response("cancel", "_Cancel");
        d.add_response("change", "C_hange");
        d.set_response_appearance("change", adw::ResponseAppearance::Suggested);
        d.set_close_response("cancel");
        let weak = Rc::downgrade(self);
        d.connect_response(None, move |_, r| {
            if r != "change" || label.text() == v.label {
                return;
            }
            if let Some(ui) = weak.upgrade() {
                ui.run_ops(
                    vec![Op::SetLabel {
                        block: block.clone(),
                        label: label.text().to_string(),
                    }],
                    |_, _| {},
                );
            }
        });
        d.present(Some(&self.window));
    }

    fn check_fs(self: &Rc<Self>, repair: bool) {
        let (_, Some(v)) = self.selection() else {
            return;
        };
        let Some(block) = v.block_path.clone() else {
            return;
        };
        let desc = describe_volume(&v);
        let report = move |ui: &Rc<Ui>, out: OpOutput, repair: bool| {
            let ok = matches!(out, OpOutput::Bool(true));
            match (repair, ok) {
                (false, true) => ui.info("Filesystem Is Undamaged", &format!("The filesystem on {desc} has no errors.")),
                (false, false) => ui.info("Filesystem Is Damaged", &format!("The filesystem on {desc} has errors. Use “Repair Filesystem” after making a backup.")),
                (true, true) => ui.info("Repair Successful", &format!("The filesystem on {desc} was repaired.")),
                (true, false) => ui.info("Repair Failed", &format!("The filesystem on {desc} could not be repaired.")),
            }
        };
        if repair {
            let body = format!(
                "Repairing the filesystem on {} may result in loss of data. Make a backup first; repairing is a risky operation.",
                describe_volume(&v)
            );
            self.confirm("Repair Filesystem?", &body, "_Repair", move |ui| {
                let report = report.clone();
                ui.run_ops(
                    vec![Op::Repair {
                        block: block.clone(),
                    }],
                    move |ui, out| report(ui, out, true),
                );
            });
        } else {
            self.run_ops(vec![Op::Check { block }], move |ui, out| {
                report(ui, out, false)
            });
        }
    }

    // ------------------------------------------------------------ drive actions

    fn format_disk(self: &Rc<Self>) {
        let (Some(drive), _) = self.selection() else {
            return;
        };
        let Some(block) = drive.block_path.clone() else {
            return;
        };
        const SCHEMES: &[(&str, &str)] = &[
            ("gpt", "GPT — modern systems and disks over 2 TB"),
            ("dos", "MBR / DOS — legacy systems and devices"),
            ("empty", "No partitioning (empty)"),
        ];
        let d = adw::AlertDialog::new(Some("Format Disk"), Some(&describe_drive(&drive)));
        let list = boxed_list();
        let labels: Vec<&str> = SCHEMES.iter().map(|(_, l)| *l).collect();
        let scheme = adw::ComboRow::builder()
            .title("Partitioning")
            .model(&gtk::StringList::new(&labels))
            .build();
        let erase = adw::SwitchRow::builder()
            .title("Erase")
            .subtitle("Overwrite existing data with zeroes (slow)")
            .build();
        list.append(&scheme);
        list.append(&erase);
        d.set_extra_child(Some(&list));
        d.add_response("cancel", "_Cancel");
        d.add_response("format", "_Next");
        d.set_response_appearance("format", adw::ResponseAppearance::Suggested);
        d.set_close_response("cancel");
        let weak = Rc::downgrade(self);
        d.connect_response(None, move |_, r| {
            if r != "format" {
                return;
            }
            let Some(ui) = weak.upgrade() else { return };
            let mut ops = ops::teardown_drive(&drive);
            ops.push(Op::Format {
                block: block.clone(),
                fs_type: SCHEMES[scheme.selected() as usize].0.to_string(),
                label: String::new(),
                erase: erase.is_active(),
                passphrase: None,
                update_partition_type: false,
            });
            let body = format!(
                "All data on the disk {} will be lost and cannot be recovered. All its volumes will be unmounted first.",
                describe_drive(&drive)
            );
            ui.confirm("Format Disk?", &body, "_Format", move |ui| ui.run_ops(ops.clone(), |_, _| {}));
        });
        d.present(Some(&self.window));
    }

    fn remove_drive(self: &Rc<Self>, kind: RemoveKind) {
        let (Some(drive), _) = self.selection() else {
            return;
        };
        let last = match kind {
            RemoveKind::PowerOff => Op::PowerOff {
                drive: drive.drive_path.clone().unwrap_or_default(),
            },
            RemoveKind::Eject => Op::Eject {
                drive: drive.drive_path.clone().unwrap_or_default(),
            },
            RemoveKind::Detach => Op::LoopDetach {
                block: drive.block_path.clone().unwrap_or_default(),
            },
        };
        self.run_ops(ops::plan_remove(&drive, last), |_, _| {});
    }

    fn show_smart(self: &Rc<Self>) {
        let (Some(drive), _) = self.selection() else {
            return;
        };
        let (Some(path), Some(smart)) = (drive.drive_path.clone(), drive.smart.clone()) else {
            return;
        };
        self.run_ops(vec![Op::SmartAttributes { drive: path }], move |ui, out| {
            let attrs = match out {
                OpOutput::Attributes(a) => a,
                _ => Vec::new(),
            };
            ui.smart_dialog(&drive, &smart, &attrs);
        });
    }

    fn smart_dialog(&self, drive: &Drive, smart: &crate::smart::Smart, attrs: &[Attribute]) {
        let summary = adw::PreferencesGroup::new();
        let now = glib::real_time() as u64 / 1_000_000;
        let rows = [
            (
                "Updated",
                if smart.updated == 0 {
                    "Never".into()
                } else {
                    format::ago(now.saturating_sub(smart.updated))
                },
            ),
            ("Self-test Result", smart.selftest_description()),
            (
                "Powered On",
                if smart.power_on_seconds == 0 {
                    "Unknown".into()
                } else {
                    format::duration(smart.power_on_seconds)
                },
            ),
            (
                "Temperature",
                if smart.temperature_k > 0.0 {
                    format::temperature_kelvin(smart.temperature_k)
                } else {
                    "Unknown".into()
                },
            ),
            ("Overall Assessment", smart.assessment()),
        ];
        for (title, value) in rows {
            let r = property_row(title);
            r.set_subtitle(&glib::markup_escape_text(&value));
            if title == "Overall Assessment" && smart.is_warning() {
                r.add_css_class("error");
            }
            summary.add(&r);
        }
        let grid = gtk::Grid::builder()
            .column_spacing(18)
            .row_spacing(6)
            .build();
        let headers = [
            "ID",
            "Attribute",
            "Value",
            "Normalized",
            "Threshold",
            "Worst",
            "Type",
            "Updates",
            "Assessment",
        ];
        for (c, h) in headers.iter().enumerate() {
            let l = gtk::Label::builder().label(*h).xalign(0.0).build();
            l.add_css_class("heading");
            grid.attach(&l, c as i32, 0, 1, 1);
        }
        for (r, a) in attrs.iter().enumerate() {
            let cells = [
                a.id.to_string(),
                a.pretty_name(),
                a.pretty_value(),
                Attribute::normalized(a.value),
                Attribute::normalized(a.threshold),
                Attribute::normalized(a.worst),
                a.kind().to_string(),
                a.updates().to_string(),
                a.assessment().to_string(),
            ];
            for (c, text) in cells.iter().enumerate() {
                let l = gtk::Label::builder()
                    .label(text)
                    .xalign(0.0)
                    .selectable(true)
                    .build();
                if a.failing() || (c == 8 && a.failed_in_past()) {
                    l.add_css_class("error");
                }
                if c == 0 || (3..=5).contains(&c) {
                    l.add_css_class("numeric");
                }
                grid.attach(&l, c as i32, r as i32 + 1, 1, 1);
            }
        }
        let attrs_group = adw::PreferencesGroup::builder()
            .title("SMART Attributes")
            .build();
        if attrs.is_empty() {
            attrs_group.set_description(Some("No attributes reported."));
        }
        let grid_scroll = gtk::ScrolledWindow::builder()
            .child(&grid)
            .vscrollbar_policy(gtk::PolicyType::Never)
            .build();
        attrs_group.add(&grid_scroll);
        let body = gtk::Box::new(gtk::Orientation::Vertical, 24);
        body.set_margin_top(12);
        body.set_margin_bottom(24);
        body.set_margin_start(12);
        body.set_margin_end(12);
        body.append(&summary);
        body.append(&attrs_group);
        let view = adw::ToolbarView::new();
        view.add_top_bar(&adw::HeaderBar::new());
        view.set_content(Some(
            &gtk::ScrolledWindow::builder()
                .child(&body)
                .hscrollbar_policy(gtk::PolicyType::Never)
                .build(),
        ));
        let dialog = adw::Dialog::builder()
            .title(format!("SMART Data — {}", drive.title()))
            .content_width(860)
            .content_height(640)
            .child(&view)
            .build();
        dialog.present(Some(&self.window));
    }

    // ------------------------------------------------------------ images

    fn attach_image(self: &Rc<Self>) {
        let filters = gio::ListStore::new::<gtk::FileFilter>();
        let f = gtk::FileFilter::new();
        f.set_name(Some("Disk Images (*.img, *.iso)"));
        for p in ["*.img", "*.iso", "*.raw", "*.IMG", "*.ISO"] {
            f.add_pattern(p);
        }
        filters.append(&f);
        let all = gtk::FileFilter::new();
        all.set_name(Some("All Files"));
        all.add_pattern("*");
        filters.append(&all);
        let dialog = gtk::FileDialog::builder()
            .title("Select Disk Image to Attach")
            .modal(true)
            .initial_folder(&self.image_dir())
            .filters(&filters)
            .build();
        let weak = Rc::downgrade(self);
        dialog.open(Some(&self.window), gio::Cancellable::NONE, move |res| {
            let (Some(ui), Ok(file)) = (weak.upgrade(), res) else {
                return;
            };
            let Some(path) = file.path() else {
                ui.error(
                    "Error attaching disk image",
                    "Only local files can be attached.",
                );
                return;
            };
            ui.remember_dir(&file);
            let d =
                adw::AlertDialog::new(Some("Attach Disk Image"), Some(&path.display().to_string()));
            let list = boxed_list();
            let ro = adw::SwitchRow::builder()
                .title("Read-Only")
                .subtitle("Prevent changes to the image file")
                .active(true)
                .build();
            list.append(&ro);
            d.set_extra_child(Some(&list));
            d.add_response("cancel", "_Cancel");
            d.add_response("attach", "_Attach");
            d.set_response_appearance("attach", adw::ResponseAppearance::Suggested);
            d.set_default_response(Some("attach"));
            d.set_close_response("cancel");
            let weak = Rc::downgrade(&ui);
            d.connect_response(None, move |_, r| {
                if r != "attach" {
                    return;
                }
                let Some(ui) = weak.upgrade() else { return };
                ui.loop_setup(path.clone(), ro.is_active());
            });
            d.present(Some(&ui.window));
        });
    }

    fn loop_setup(self: &Rc<Self>, path: PathBuf, read_only: bool) {
        self.set_busy(true);
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let res = gio::spawn_blocking(move || {
                udisks::conn().and_then(|c| udisks::loop_setup(&c, &path, read_only))
            })
            .await;
            let Some(ui) = weak.upgrade() else { return };
            ui.set_busy(false);
            match res {
                Ok(Ok(block)) => {
                    let exists = ui.model.borrow().find_drive(&block).is_some();
                    if exists {
                        *ui.drive_key.borrow_mut() = Some(block);
                        ui.rebuild_list();
                        ui.update_page();
                    } else {
                        *ui.pending_select.borrow_mut() = Some(block);
                    }
                }
                Ok(Err(e)) => ui.error("Error attaching disk image", &e),
                Err(_) => ui.error(
                    "Error attaching disk image",
                    "The operation failed unexpectedly.",
                ),
            }
        });
    }

    /// Target block + description + size for image operations.
    fn image_target(&self, whole_disk: bool) -> Option<(String, String, u64, String, Vec<Op>)> {
        let (drive, vol) = self.selection();
        if whole_disk {
            let d = drive?;
            let name = if d.model_name().is_empty() {
                d.title()
            } else {
                d.model_name()
            };
            Some((
                d.block_path.clone()?,
                describe_drive(&d),
                d.size,
                name,
                ops::teardown_drive(&d),
            ))
        } else {
            let v = vol?;
            let name = if v.label.is_empty() {
                v.short_name()
            } else {
                v.label.clone()
            };
            Some((
                v.block_path.clone()?,
                describe_volume(&v),
                v.size,
                name,
                ops::teardown(&v),
            ))
        }
    }

    fn create_image(self: &Rc<Self>, whole_disk: bool) {
        let Some((block, desc, size, name, _)) = self.image_target(whole_disk) else {
            return;
        };
        let safe: String = name
            .chars()
            .map(|c| if c == '/' { '_' } else { c })
            .collect();
        let dialog = gtk::FileDialog::builder()
            .title(if whole_disk {
                "Create Disk Image"
            } else {
                "Create Partition Image"
            })
            .modal(true)
            .initial_folder(&self.image_dir())
            .initial_name(format!("{}.img", safe.trim()))
            .accept_label("_Start Creating")
            .build();
        let weak = Rc::downgrade(self);
        dialog.save(Some(&self.window), gio::Cancellable::NONE, move |res| {
            let (Some(ui), Ok(file)) = (weak.upgrade(), res) else {
                return;
            };
            let Some(path) = file.path() else {
                ui.error(
                    "Error creating disk image",
                    "Only local files are supported.",
                );
                return;
            };
            ui.remember_dir(&file);
            ui.copy_image(
                CopyDir::Backup,
                block.clone(),
                path,
                size,
                format!("Creating image of {desc}"),
            );
        });
    }

    fn restore_image(self: &Rc<Self>, whole_disk: bool) {
        let Some((block, desc, size, _, teardown)) = self.image_target(whole_disk) else {
            return;
        };
        let dialog = gtk::FileDialog::builder()
            .title(if whole_disk {
                "Select Disk Image to Restore"
            } else {
                "Select Partition Image to Restore"
            })
            .modal(true)
            .initial_folder(&self.image_dir())
            .build();
        let weak = Rc::downgrade(self);
        dialog.open(Some(&self.window), gio::Cancellable::NONE, move |res| {
            let (Some(ui), Ok(file)) = (weak.upgrade(), res) else { return };
            let Some(path) = file.path() else {
                ui.error("Error restoring disk image", "Only local files are supported.");
                return;
            };
            ui.remember_dir(&file);
            let image_size = match std::fs::metadata(&path) {
                Ok(m) => m.len(),
                Err(e) => {
                    ui.error("Error restoring disk image", &e.to_string());
                    return;
                }
            };
            if image_size > size {
                ui.error(
                    "Image Too Large",
                    &format!("The image is {} but the target {desc} is only {}.", format::size(image_size), format::size(size)),
                );
                return;
            }
            let mut body = format!(
                "All existing data on {desc} will be overwritten by the image “{}” ({}) and cannot be recovered.",
                path.display(),
                format::size(image_size)
            );
            if image_size < size {
                body.push_str(&format!("\n\nThe image is smaller than the target; the remaining {} will be left as is.", format::size(size - image_size)));
            }
            if !teardown.is_empty() {
                body.push_str("\n\nMounted volumes will be unmounted first.");
            }
            let block = block.clone();
            let desc = desc.clone();
            let teardown = teardown.clone();
            ui.confirm("Restore Disk Image?", &body, "_Restore", move |ui| {
                let (block, path, desc) = (block.clone(), path.clone(), desc.clone());
                ui.run_ops(teardown.clone(), move |ui, _| {
                    ui.copy_image(CopyDir::Restore, block, path, image_size, format!("Restoring image to {desc}"));
                });
            });
        });
    }

    /// Copies between a block device and a file with a progress dialog.
    fn copy_image(
        self: &Rc<Self>,
        dir: CopyDir,
        block: String,
        path: PathBuf,
        total: u64,
        title: String,
    ) {
        let cancel = Arc::new(AtomicBool::new(false));
        let progress = gtk::ProgressBar::builder().show_text(true).build();
        let detail = gtk::Label::builder()
            .label("Starting…")
            .xalign(0.0)
            .wrap(true)
            .build();
        detail.add_css_class("dim-label");
        let heading = gtk::Label::builder()
            .label(&title)
            .xalign(0.0)
            .wrap(true)
            .build();
        heading.add_css_class("title-4");
        let file_label = gtk::Label::builder()
            .label(path.display().to_string())
            .xalign(0.0)
            .wrap(true)
            .build();
        let cancel_btn = gtk::Button::builder()
            .label("_Cancel")
            .use_underline(true)
            .halign(gtk::Align::End)
            .build();
        cancel_btn.add_css_class("destructive-action");
        let body = gtk::Box::new(gtk::Orientation::Vertical, 12);
        body.set_margin_top(12);
        body.set_margin_bottom(18);
        body.set_margin_start(18);
        body.set_margin_end(18);
        body.append(&heading);
        body.append(&file_label);
        body.append(&progress);
        body.append(&detail);
        body.append(&cancel_btn);
        let view = adw::ToolbarView::new();
        let hb = adw::HeaderBar::builder()
            .show_end_title_buttons(false)
            .build();
        view.add_top_bar(&hb);
        view.set_content(Some(&body));
        let dialog = adw::Dialog::builder()
            .title(if dir == CopyDir::Backup {
                "Creating Image"
            } else {
                "Restoring Image"
            })
            .content_width(480)
            .child(&view)
            .can_close(false)
            .build();
        let c = cancel.clone();
        cancel_btn.connect_clicked(move |b| {
            c.store(true, Ordering::Relaxed);
            b.set_sensitive(false);
        });
        dialog.present(Some(&self.window));
        self.set_busy(true);

        let (tx, rx) = async_channel::unbounded::<CopyMsg>();
        let c = cancel.clone();
        std::thread::spawn(move || {
            let res = copy_thread(dir, &block, &path, total, &c, &tx);
            let _ = tx.send_blocking(CopyMsg::Done(res));
        });
        let weak = Rc::downgrade(self);
        let started = Instant::now();
        glib::spawn_future_local(async move {
            while let Ok(msg) = rx.recv().await {
                match msg {
                    CopyMsg::Progress(done) => {
                        progress.set_fraction(if total > 0 {
                            done as f64 / total as f64
                        } else {
                            0.0
                        });
                        let secs = started.elapsed().as_secs_f64().max(0.001);
                        let rate = done as f64 / secs;
                        let remaining = if rate > 0.0 {
                            ((total - done.min(total)) as f64 / rate) as u64
                        } else {
                            0
                        };
                        detail.set_label(&format!(
                            "{} of {} — {} — {} remaining",
                            format::size(done),
                            format::size(total),
                            format::rate(rate),
                            format::duration(remaining)
                        ));
                    }
                    CopyMsg::Done(res) => {
                        dialog.force_close();
                        let Some(ui) = weak.upgrade() else { return };
                        ui.set_busy(false);
                        match res {
                            Ok(n) => {
                                if dir == CopyDir::Backup {
                                    ui.info(
                                        "Image Created",
                                        &format!("{} were copied.", format::size_long(n)),
                                    );
                                } else {
                                    ui.info(
                                        "Image Restored",
                                        &format!("{} were written.", format::size_long(n)),
                                    );
                                }
                            }
                            Err(e) => ui.error(
                                if dir == CopyDir::Backup {
                                    "Error creating disk image"
                                } else {
                                    "Error restoring disk image"
                                },
                                &e,
                            ),
                        }
                        return;
                    }
                }
            }
        });
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum RemoveKind {
    PowerOff,
    Eject,
    Detach,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum CopyDir {
    Backup,
    Restore,
}

enum CopyMsg {
    Progress(u64),
    Done(Result<u64, String>),
}

fn copy_thread(
    dir: CopyDir,
    block: &str,
    path: &std::path::Path,
    total: u64,
    cancel: &AtomicBool,
    tx: &async_channel::Sender<CopyMsg>,
) -> Result<u64, String> {
    let conn = udisks::conn()?;
    let mut last = Instant::now();
    let mut report = |done: u64| {
        if last.elapsed().as_millis() >= 200 || done == total {
            last = Instant::now();
            let _ = tx.send_blocking(CopyMsg::Progress(done));
        }
    };
    match dir {
        CopyDir::Backup => {
            let mut src = udisks::open_for_backup(&conn, block)?;
            let mut dst = std::fs::File::create(path).map_err(|e| e.to_string())?;
            let res = udisks::copy_with_progress(&mut src, &mut dst, total, cancel, &mut report)
                .and_then(|n| dst.sync_all().map(|_| n));
            if res.is_err() {
                // Do not leave a partial image behind.
                let _ = std::fs::remove_file(path);
            }
            res.map_err(|e| e.to_string())
        }
        CopyDir::Restore => {
            let mut src = std::fs::File::open(path).map_err(|e| e.to_string())?;
            let mut dst = udisks::open_for_restore(&conn, block)?;
            udisks::copy_with_progress(&mut src, &mut dst, total, cancel, &mut report)
                .and_then(|n| dst.sync_all().map(|_| n))
                .map_err(|e| e.to_string())
        }
    }
}
