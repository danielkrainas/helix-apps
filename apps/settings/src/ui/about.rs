//! About panel: device name, hardware and software information.

use adw::prelude::*;
use gtk::{gdk, gio, glib};
use helix_app_common::Labelled;
use std::rc::Rc;

use super::widgets::{self, value_label};
use super::{spawn_worker, Ctx, Panel};
use crate::{dbus, sysinfo, values};
use helix_app_common::i18n::{n_, tr};
use helix_app_common::trf;

pub struct AboutPanel;

#[derive(Default)]
struct LocalInfo {
    os: String,
    os_type: String,
    cpu: String,
    memory: Option<u64>,
    kernel: String,
    dmi_vendor: String,
    dmi_model: String,
}

fn read_local() -> LocalInfo {
    let os_release = {
        let t = sysinfo::read("/etc/os-release");
        if t.is_empty() {
            sysinfo::read("/usr/lib/os-release")
        } else {
            t
        }
    };
    LocalInfo {
        os: sysinfo::os_name(&sysinfo::parse_os_release(&os_release))
            .unwrap_or_else(|| tr("Unknown").into()),
        os_type: format!("{}-bit", usize::BITS),
        cpu: sysinfo::cpu_description(&sysinfo::read("/proc/cpuinfo"))
            .unwrap_or_else(|| tr("Unknown").into()),
        memory: sysinfo::mem_total_bytes(&sysinfo::read("/proc/meminfo")),
        kernel: sysinfo::read("/proc/sys/kernel/osrelease")
            .trim()
            .to_string(),
        dmi_vendor: sysinfo::read("/sys/class/dmi/id/sys_vendor")
            .trim()
            .to_string(),
        dmi_model: sysinfo::read("/sys/class/dmi/id/product_name")
            .trim()
            .to_string(),
    }
}

fn windowing_system() -> &'static str {
    let Some(display) = gdk::Display::default() else {
        return tr("Unknown");
    };
    match display.type_().name() {
        "GdkWaylandDisplay" => tr("Wayland"),
        "GdkX11Display" => "X11",
        "GdkBroadwayDisplay" => tr("Broadway"),
        _ => tr("Unknown"),
    }
}

impl Panel for AboutPanel {
    fn build(&self, ctx: &Rc<Ctx>) -> gtk::Widget {
        let page = widgets::page();

        let header = gtk::Label::builder()
            .label("…")
            .css_classes(["title-1"])
            .wrap(true)
            .justify(gtk::Justification::Center)
            .build();
        let head = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(12)
            .margin_bottom(12)
            .build();
        head.append(
            &gtk::Image::builder()
                .icon_name("computer-symbolic")
                .pixel_size(96)
                .css_classes(["dim-label"])
                .build(),
        );
        head.append(&header);
        let head_group = widgets::group("");
        head_group.add(&head);
        page.add(&head_group);

        let name_group = widgets::group("");
        let name_row = adw::EntryRow::builder()
            .title(tr("Device Name"))
            .show_apply_button(true)
            .sensitive(false)
            .build();
        name_group.add(&name_row);
        page.add(&name_group);

        let hw = widgets::group(tr("Hardware"));
        let model = value_label("…");
        let memory = value_label("…");
        let cpu = value_label("…");
        let disk = value_label("…");
        let sw = widgets::group(tr("Software"));
        let os = value_label("…");
        let os_type = value_label("…");
        let desktop_name =
            std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_else(|_| tr("Unknown").into());
        let desktop = value_label(&desktop_name);
        let windowing = value_label(windowing_system());
        let kernel = value_label("…");
        for (title, label) in [
            (tr("Hardware Model"), &model),
            (tr("Memory"), &memory),
            (tr("Processor"), &cpu),
            (tr("Disk Capacity"), &disk),
        ] {
            hw.add(&row_for(title, label));
        }
        for (title, label) in [
            (tr("OS Name"), &os),
            (tr("OS Type"), &os_type),
            (tr("Desktop"), &desktop),
            (tr("Windowing System"), &windowing),
            (tr("Kernel Version"), &kernel),
        ] {
            sw.add(&row_for(title, label));
        }
        let copy = gtk::Button::builder()
            .icon_name("edit-copy-symbolic")
            .tooltip_text(tr("Copy System Information"))
            .css_classes(["flat"])
            .build()
            .labelled(tr("Copy System Information"));
        hw.set_header_suffix(Some(&copy));
        page.add(&hw);
        page.add(&sw);

        let labels: Vec<(&'static str, gtk::Label)> = vec![
            (tr("Hardware Model"), model.clone()),
            (tr("Memory"), memory.clone()),
            (tr("Processor"), cpu.clone()),
            (tr("Disk Capacity"), disk.clone()),
            (tr("OS Name"), os.clone()),
            (tr("OS Type"), os_type.clone()),
            (tr("Desktop"), desktop.clone()),
            (tr("Windowing System"), windowing.clone()),
            (tr("Kernel Version"), kernel.clone()),
        ];
        let toast_ctx = Rc::downgrade(ctx);
        copy.connect_clicked(move |b| {
            let text: String = labels
                .iter()
                .map(|(t, l)| format!("{t}: {}\n", l.label()))
                .collect();
            b.clipboard().set_text(&text);
            // libadwaita speaks toasts, so this is also the announcement.
            if let Some(ctx) = toast_ctx.upgrade() {
                ctx.toast(tr("System information copied"));
            }
        });

        // Local files and D-Bus on worker threads.
        {
            let (model, memory, cpu, os, os_type, kernel) =
                (model.clone(), memory, cpu, os, os_type, kernel);
            let name_row = name_row.clone();
            let header = header.clone();
            let host_ctx = Rc::downgrade(ctx);
            spawn_worker(
                || (read_local(), dbus::read_hostname()),
                move |(local, host)| {
                    os.set_label(&local.os);
                    header.set_label(&local.os);
                    os_type.set_label(&local.os_type);
                    cpu.set_label(&local.cpu);
                    memory.set_label(
                        &local
                            .memory
                            .map(|b| glib::format_size(b).to_string())
                            .unwrap_or_else(|| tr("Unknown").into()),
                    );
                    kernel.set_label(&local.kernel);
                    let (vendor, hw_model) = match &host {
                        Ok(h) if !h.vendor.is_empty() || !h.model.is_empty() => {
                            (h.vendor.clone(), h.model.clone())
                        }
                        _ => (local.dmi_vendor.clone(), local.dmi_model.clone()),
                    };
                    model.set_label(
                        &sysinfo::hardware_model(&vendor, &hw_model)
                            .unwrap_or_else(|| tr("Unknown").into()),
                    );
                    match host {
                        Ok(h) => {
                            name_row.set_text(h.device_name());
                            name_row.set_sensitive(true);
                        }
                        Err(e) => {
                            name_row.set_text(&glib::host_name());
                            eprintln!("helix-settings-app: hostname1: {e}");
                            // The name stays read-only; say why.
                            name_row.set_tooltip_text(Some(
                                tr("The device name cannot be changed: the hostname service is unavailable"),
                            ));
                            if let Some(ctx) = host_ctx.upgrade() {
                                ctx.toast(tr("The device name cannot be changed right now"));
                            }
                        }
                    }
                },
            );
        }
        let root = gio::File::for_path("/");
        root.query_filesystem_info_async(
            "filesystem::size",
            glib::Priority::DEFAULT,
            gio::Cancellable::NONE,
            move |res| {
                let size = res.ok().map(|i| i.attribute_uint64("filesystem::size"));
                disk.set_label(
                    &size
                        .filter(|s| *s > 0)
                        .map(|s| glib::format_size(s).to_string())
                        .unwrap_or_else(|| tr("Unknown").into()),
                );
            },
        );

        let ctx_weak = Rc::downgrade(ctx);
        helix_app_common::clear_input_error_on_edit(&name_row, None);
        name_row.connect_apply(move |row| {
            let pretty = row.text().trim().to_string();
            if pretty.is_empty() {
                const EMPTY: &str = n_("The device name cannot be empty");
                helix_app_common::set_input_error(row, None, Some(tr(EMPTY)));
                if let Some(ctx) = ctx_weak.upgrade() {
                    ctx.toast(tr(EMPTY));
                }
                return;
            }
            let static_name = values::pretty_to_static_hostname(&pretty);
            let ctx_weak = ctx_weak.clone();
            // System change, only on user action.
            spawn_worker(
                move || dbus::set_hostname(&pretty, &static_name),
                move |res| {
                    if let (Err(e), Some(ctx)) = (res, ctx_weak.upgrade()) {
                        ctx.toast(&trf!("Could not rename the device: {e}", e = e));
                    }
                },
            );
        });
        page.upcast()
    }
}

fn row_for(title: &str, label: &gtk::Label) -> adw::ActionRow {
    let row = adw::ActionRow::builder()
        .use_markup(false)
        .title(title)
        .build();
    row.add_suffix(label);
    row
}
