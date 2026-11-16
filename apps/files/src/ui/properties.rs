//! Properties dialog.

use adw::prelude::*;
use gtk::{gio, glib};
use helix_app_common::Labelled;
use std::rc::Rc;

use super::Window;
use crate::fmt::{self, Perm, Who};
use helix_app_common::i18n::tr;
use helix_app_common::nc_;
use helix_app_common::trf;
use helix_app_common::trnf;

const ATTRS: &str = "standard::*,time::modified,time::access,time::created,unix::mode,unix::uid,owner::user,owner::group,access::can-write,trash::orig-path,trash::deletion-date";

fn row(title: &str, value: &str) -> adw::ActionRow {
    let r = adw::ActionRow::builder()
        .title(title)
        .subtitle(value)
        .subtitle_selectable(true)
        .build();
    r.add_css_class("property");
    r
}

fn time_attr(info: &gio::FileInfo, attr: &str) -> Option<String> {
    let secs = info.attribute_uint64(attr);
    if !info.has_attribute(attr) || secs == 0 {
        return None;
    }
    let dt = glib::DateTime::from_unix_local(secs as i64).ok()?;
    Some(fmt::date(&dt, &glib::DateTime::now_local().ok()?, true))
}

/// Measures `files` on a worker thread, updating `target` as it goes.
fn measure_into(files: Vec<gio::File>, target: adw::ActionRow, cancel: gio::Cancellable) {
    let (tx, rx) = async_channel::unbounded::<(u64, u64, bool)>();
    let c = cancel.clone();
    std::thread::spawn(move || {
        let mut items = 0;
        let mut bytes = 0;
        for f in &files {
            let (base_i, base_b) = (items, bytes);
            let (i, b) = crate::ops::measure(f, &c, &mut |i, b| {
                tx.send_blocking((base_i + i, base_b + b, false)).is_ok()
            });
            items += i;
            bytes += b;
        }
        let _ = tx.send_blocking((items, bytes, true));
    });
    glib::spawn_future_local(async move {
        while let Ok((items, bytes, done)) = rx.recv().await {
            if cancel.is_cancelled() {
                return;
            }
            let suffix = if done { "" } else { "…" };
            target.set_subtitle(&trf!(
                "{items}, totalling {bytes}{suffix}",
                items = fmt::item_count(items),
                bytes = fmt::size(bytes),
                suffix = suffix
            ));
        }
    });
}

/// Shows properties for `files` (one or more).
pub fn show(win: &Rc<Window>, files: Vec<gio::File>) {
    if files.is_empty() {
        return;
    }
    let win = win.clone();
    glib::spawn_future_local(async move {
        let mut infos = Vec::new();
        for f in &files {
            match f
                .query_info_future(
                    ATTRS,
                    gio::FileQueryInfoFlags::NONE,
                    glib::Priority::DEFAULT,
                )
                .await
            {
                Ok(i) => infos.push((f.clone(), i)),
                Err(e) => {
                    win.toast(&trf!("Could not read properties: {}", e.message()));
                    return;
                }
            }
        }
        build(&win, infos);
    });
}

fn build(win: &Rc<Window>, infos: Vec<(gio::File, gio::FileInfo)>) {
    let cancel = gio::Cancellable::new();
    let page = adw::PreferencesPage::new();

    // Header: icon + name.
    let header = gtk::Box::new(gtk::Orientation::Vertical, 6);
    header.set_margin_bottom(6);
    let img = gtk::Image::new();
    img.set_pixel_size(96);
    let title = gtk::Label::builder()
        .wrap(true)
        .justify(gtk::Justification::Center)
        .build();
    title.add_css_class("title-2");
    header.append(&img);
    header.append(&title);
    let header_group = adw::PreferencesGroup::new();
    header_group.add(&header);
    page.add(&header_group);

    let group = adw::PreferencesGroup::new();
    let single = infos.len() == 1;
    if single {
        let (file, info) = &infos[0];
        match info.icon() {
            Some(i) => img.set_from_gicon(&i),
            None => img.set_icon_name(Some("text-x-generic")),
        }
        title.set_text(&info.display_name());
        let is_dir = info.file_type() == gio::FileType::Directory;
        group.add(&row(tr("Type"), &super::tab::type_description(info)));
        if is_dir {
            let contents = row(tr("Contents"), tr("Calculating…"));
            group.add(&contents);
            measure_into(vec![file.clone()], contents, cancel.clone());
        } else {
            let bytes = info.size().max(0) as u64;
            group.add(&row(
                tr("Size"),
                &trf!("{} ({} bytes)", fmt::size(bytes), bytes),
            ));
        }
        if let Some(parent) = file.parent() {
            let r = row(
                tr("Parent Folder"),
                &super::window::display_location(&parent),
            );
            let btn = gtk::Button::builder()
                .icon_name("folder-open-symbolic")
                .tooltip_text(tr("Open Parent Folder"))
                .valign(gtk::Align::Center)
                .build()
                .labelled(tr("Open Parent Folder"));
            btn.add_css_class("flat");
            let w = Rc::downgrade(win);
            let f = file.clone();
            btn.connect_clicked(move |b| {
                if let Some(w) = w.upgrade() {
                    if let Some(t) = w.current_tab() {
                        t.load(&parent, true, vec![f.clone()]);
                    }
                }
                if let Some(d) = b
                    .ancestor(adw::Dialog::static_type())
                    .and_downcast::<adw::Dialog>()
                {
                    d.close();
                }
            });
            r.add_suffix(&btn);
            group.add(&r);
        }
        if info.is_symlink() {
            if let Some(t) = info.symlink_target() {
                group.add(&row(tr("Link Target"), &t.display().to_string()));
            }
        }
        if let Some(orig) = info.attribute_byte_string("trash::orig-path") {
            group.add(&row(tr("Original Location"), orig.as_str()));
        }
        for (label, attr) in [
            (tr("Modified"), "time::modified"),
            (tr("Accessed"), "time::access"),
            (tr("Created"), "time::created"),
        ] {
            if let Some(t) = time_attr(info, attr) {
                group.add(&row(label, &t));
            }
        }
        if let Some(user) = info.attribute_string("owner::user") {
            let g = info
                .attribute_string("owner::group")
                .map(|g| format!(" ({g})"))
                .unwrap_or_default();
            group.add(&row(tr("Owner"), &format!("{user}{g}")));
        }
        page.add(&group);
        let free = row(tr("Free Space"), "…");
        group.add(&free);
        let f = file.clone();
        glib::spawn_future_local(async move {
            match f
                .query_filesystem_info_future(
                    "filesystem::free,filesystem::size",
                    glib::Priority::LOW,
                )
                .await
            {
                Ok(fs) if fs.has_attribute("filesystem::free") => {
                    let free_b = fs.attribute_uint64("filesystem::free");
                    let total = fs.attribute_uint64("filesystem::size");
                    free.set_subtitle(&if total > 0 {
                        trf!(
                            "{free} of {total}",
                            free = fmt::size(free_b),
                            total = fmt::size(total)
                        )
                    } else {
                        fmt::size(free_b)
                    });
                }
                _ => free.set_visible(false),
            }
        });

        if info.has_attribute("unix::mode") && file.uri_scheme().as_deref() != Some("trash") {
            page.add(&permissions_group(win, file, info));
        }
    } else {
        img.set_icon_name(Some("edit-copy-symbolic"));
        title.set_text(&trnf!(
            "{n} item selected",
            "{n} items selected",
            infos.len()
        ));
        let contents = row(tr("Contents"), tr("Calculating…"));
        group.add(&contents);
        measure_into(
            infos.iter().map(|(f, _)| f.clone()).collect(),
            contents,
            cancel.clone(),
        );
        if let Some(parent) = infos[0].0.parent() {
            if infos
                .iter()
                .all(|(f, _)| f.parent().is_some_and(|p| p.equal(&parent)))
            {
                group.add(&row(
                    tr("Parent Folder"),
                    &super::window::display_location(&parent),
                ));
            }
        }
        page.add(&group);
    }

    let tv = adw::ToolbarView::new();
    tv.add_top_bar(&adw::HeaderBar::new());
    tv.set_content(Some(&page));
    let dialog = adw::Dialog::builder()
        .title(tr("Properties"))
        .content_width(420)
        .content_height(620)
        .child(&tv)
        .build();
    dialog.connect_closed(move |_| cancel.cancel());
    dialog.present(Some(&win.window));
}

fn permissions_group(
    win: &Rc<Window>,
    file: &gio::File,
    info: &gio::FileInfo,
) -> adw::PreferencesGroup {
    let group = adw::PreferencesGroup::builder()
        .title(tr("Permissions"))
        .build();
    let mode = Rc::new(std::cell::Cell::new(info.attribute_uint32("unix::mode")));
    let editable = file.is_native() && {
        let uid = info.attribute_uint32("unix::uid");
        uid == current_uid()
    };
    let summary = gtk::Label::new(Some(&fmt::mode_string(mode.get())));
    summary.add_css_class("monospace");
    summary.add_css_class("dim-label");
    group.set_header_suffix(Some(&summary));
    for (who, label) in [
        (Who::Owner, tr("Owner")),
        (Who::Group, tr("Group")),
        (Who::Others, tr("Others")),
    ] {
        let r = adw::ActionRow::builder().title(label).build();
        let b = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        b.set_valign(gtk::Align::Center);
        for (perm, plabel) in [
            (Perm::Read, tr(nc_!("file permission", "Read"))),
            (Perm::Write, tr(nc_!("file permission", "Write"))),
            (Perm::Execute, tr(nc_!("file permission", "Execute"))),
        ] {
            let check = gtk::CheckButton::with_label(plabel);
            // The visible label is just "Read"; say whose permission it is.
            let spoken = match (who, perm) {
                (Who::Owner, Perm::Read) => tr("Owner can read"),
                (Who::Owner, Perm::Write) => tr("Owner can write"),
                (Who::Owner, Perm::Execute) => tr("Owner can execute"),
                (Who::Group, Perm::Read) => tr("Group can read"),
                (Who::Group, Perm::Write) => tr("Group can write"),
                (Who::Group, Perm::Execute) => tr("Group can execute"),
                (Who::Others, Perm::Read) => tr("Others can read"),
                (Who::Others, Perm::Write) => tr("Others can write"),
                (Who::Others, Perm::Execute) => tr("Others can execute"),
            };
            helix_app_common::set_accessible_label(&check, spoken);
            check.set_active(mode.get() & fmt::mode_bit(who, perm) != 0);
            check.set_sensitive(editable);
            let mode = mode.clone();
            let file = file.clone();
            let summary = summary.clone();
            let win = Rc::downgrade(win);
            check.connect_toggled(move |c| {
                let new = fmt::with_bit(mode.get(), who, perm, c.is_active());
                if new == mode.get() {
                    return;
                }
                let info = gio::FileInfo::new();
                info.set_attribute_uint32("unix::mode", new);
                let file = file.clone();
                let mode = mode.clone();
                let summary = summary.clone();
                let c = c.clone();
                let win = win.clone();
                glib::spawn_future_local(async move {
                    match file
                        .set_attributes_future(
                            &info,
                            gio::FileQueryInfoFlags::NONE,
                            glib::Priority::DEFAULT,
                        )
                        .await
                    {
                        Ok(_) => {
                            mode.set(new);
                            summary.set_text(&fmt::mode_string(new));
                        }
                        Err(e) => {
                            if let Some(w) = win.upgrade() {
                                w.toast(&trf!("Could not change permissions: {}", e.message()));
                            }
                            c.set_active(!c.is_active());
                        }
                    }
                });
            });
            b.append(&check);
        }
        r.add_suffix(&b);
        group.add(&r);
    }
    if !editable {
        group.set_description(Some(tr(
            "You are not the owner, so you cannot change these permissions.",
        )));
    }
    group
}

/// Current user id (owner of /proc/self).
fn current_uid() -> u32 {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata("/proc/self")
        .map(|m| m.uid())
        .unwrap_or(u32::MAX)
}
