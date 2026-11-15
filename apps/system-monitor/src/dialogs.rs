//! Process dialogs: properties, change priority, end/kill confirmation.

use adw::prelude::*;
use gtk::glib;
use std::cell::RefCell;
use std::rc::Rc;

use crate::control::{self, Action};
use crate::format;
use crate::helper_args::Command;
use crate::proc_view::ProcessObject;
use crate::sampler::{self, ProcessInfo};
use helix_app_common::i18n::tr;
use helix_app_common::trf;

fn opt_size(v: Option<u64>, iec: bool) -> String {
    v.map(|b| format::size(b, iec))
        .unwrap_or_else(|| "N/A".into())
}

fn property_rows(p: &ProcessInfo, iec: bool) -> Vec<(&'static str, String)> {
    let started = glib::DateTime::from_unix_local(p.start_time as i64)
        .ok()
        .and_then(|d| d.format("%c").ok())
        .map(|s| s.to_string())
        .unwrap_or_default();
    vec![
        (tr("Process Name"), p.name.clone()),
        (tr("User"), format!("{} ({})", p.user, p.uid)),
        (tr("Status"), format::state(p.state).to_string()),
        (tr("Memory"), opt_size(p.memory, iec)),
        (tr("Virtual Memory"), opt_size(p.vm_size, iec)),
        (tr("Resident Memory"), opt_size(p.vm_rss, iec)),
        (tr("Shared Memory"), opt_size(p.shared, iec)),
        ("CPU", format::percent(p.cpu)),
        (
            tr("CPU Time"),
            format::cpu_time(p.cpu_ticks, sampler::clock_ticks()),
        ),
        (tr("Started"), started),
        (tr("Nice"), p.nice.to_string()),
        (tr("Priority"), format::priority(p.nice).to_string()),
        ("ID", p.pid.to_string()),
        (tr("Parent ID"), p.ppid.to_string()),
        (tr("Threads"), p.threads.to_string()),
        (tr("Command Line"), p.cmdline.clone()),
    ]
}

/// Shows a live-updating properties dialog for `obj`.
pub fn show_properties(parent: &impl IsA<gtk::Widget>, obj: &ProcessObject, iec: bool) {
    let info = obj.info().clone();
    let group = adw::PreferencesGroup::new();
    let mut values = Vec::new();
    for (title, value) in property_rows(&info, iec) {
        let row = adw::ActionRow::builder().title(title).build();
        let label = gtk::Label::builder()
            .label(&value)
            .selectable(true)
            .wrap(true)
            .wrap_mode(gtk::pango::WrapMode::WordChar)
            .xalign(1.0)
            .max_width_chars(40)
            .build();
        label.add_css_class("dim-label");
        row.add_suffix(&label);
        group.add(&row);
        values.push(label);
    }
    let page = adw::PreferencesPage::new();
    page.add(&group);
    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&adw::HeaderBar::new());
    toolbar.set_content(Some(&page));
    let dialog = adw::Dialog::builder()
        .title(trf!("{} (PID {})", info.name, info.pid))
        .content_width(460)
        .content_height(620)
        .child(&toolbar)
        .build();

    let handler = obj.connect_notify_local(None, move |obj, _| {
        for (label, (_, value)) in values.iter().zip(property_rows(&obj.info(), iec)) {
            if label.label() != value {
                label.set_label(&value);
            }
        }
    });
    let obj_c = obj.clone();
    let handler = RefCell::new(Some(handler));
    dialog.connect_closed(move |_| {
        if let Some(h) = handler.borrow_mut().take() {
            obj_c.disconnect(h);
        }
    });
    dialog.present(Some(parent));
}

/// Reports `direct` (the result of trying the action without privileges)
/// to `on_done`, unless it failed for lack of privileges: then the action is
/// retried through `pkexec` + the helper on a worker thread, and the
/// helper's result is reported instead.
fn escalate_if_needed(
    direct: Result<(), control::Error>,
    command: Command,
    on_done: impl FnOnce(Result<(), control::Error>) + 'static,
) {
    match direct {
        Err(e) if control::needs_privileges(&e) && command.pid() >= 2 => {
            let (tx, rx) = async_channel::bounded(1);
            std::thread::spawn(move || {
                let _ = tx.send_blocking(control::run_privileged(command));
            });
            glib::MainContext::default().spawn_local(async move {
                let res = rx.recv().await.unwrap_or_else(|_| {
                    Err(control::Error::Other(
                        tr("The privileged helper failed").into(),
                    ))
                });
                on_done(res);
            });
        }
        other => on_done(other),
    }
}

/// Asks for a new nice value and applies it; errors go to `on_error`.
pub fn change_priority(
    parent: &impl IsA<gtk::Widget>,
    obj: &ProcessObject,
    on_error: Rc<dyn Fn(String)>,
) {
    let (pid, name, nice) = {
        let i = obj.info();
        (i.pid, i.name.clone(), i.nice)
    };
    let dialog = adw::AlertDialog::builder()
        .heading(trf!("Change Priority of “{name}” (PID {pid})", name = name, pid = pid))
        .body(tr("Lower nice values mean higher priority. Raising a process's priority usually requires administrator privileges."))
        .build();
    dialog.add_responses(&[
        ("cancel", tr("_Cancel")),
        ("change", tr("Change _Priority")),
    ]);
    dialog.set_response_appearance("change", adw::ResponseAppearance::Suggested);
    dialog.set_default_response(Some("change"));
    dialog.set_close_response("cancel");

    let content = gtk::Box::new(gtk::Orientation::Vertical, 6);
    // The presets are a radio group labelled "Priority".
    let heading = gtk::Label::builder()
        .label(tr("Priority"))
        .xalign(0.0)
        .css_classes(["heading"])
        .build();
    let options = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(6)
        .accessible_role(gtk::AccessibleRole::RadioGroup)
        .build();
    helix_app_common::label_widget(&options, &heading);
    content.append(&heading);
    content.append(&options);
    let scale = gtk::Scale::with_range(gtk::Orientation::Horizontal, -20.0, 19.0, 1.0);
    helix_app_common::set_accessible_label(&scale, tr("Nice Value"));
    scale.set_value(nice as f64);
    scale.set_digits(0);
    scale.set_draw_value(true);
    scale.set_hexpand(true);
    let mut first: Option<gtk::CheckButton> = None;
    let mut custom_selected = true;
    let mut checks = Vec::new();
    for (label, value) in format::PRIORITY_PRESETS {
        let check = gtk::CheckButton::with_label(&format!("{} ({value})", tr(label)));
        if let Some(f) = &first {
            check.set_group(Some(f));
        } else {
            first = Some(check.clone());
        }
        if value == nice {
            check.set_active(true);
            custom_selected = false;
        }
        let s = scale.clone();
        check.connect_toggled(move |c| {
            if c.is_active() {
                s.set_value(value as f64);
            }
        });
        options.append(&check);
        checks.push(check);
    }
    let custom = gtk::CheckButton::with_label(tr("Custom"));
    custom.set_group(first.as_ref());
    custom.set_active(custom_selected);
    options.append(&custom);
    content.append(&scale);
    {
        let custom = custom.clone();
        scale.connect_value_changed(move |s| {
            let v = s.value().round() as i32;
            let preset = format::PRIORITY_PRESETS.iter().position(|(_, n)| *n == v);
            match preset {
                Some(i) => {
                    if !checks[i].is_active() {
                        checks[i].set_active(true);
                    }
                }
                None => custom.set_active(true),
            }
        });
    }
    dialog.set_extra_child(Some(&content));
    dialog.connect_response(Some("change"), move |_, _| {
        let v = scale.value().round() as i32;
        let name = name.clone();
        let on_error = on_error.clone();
        escalate_if_needed(
            control::renice(pid, v),
            Command::Renice {
                nice: v.clamp(-20, 19),
                pid,
            },
            move |res| {
                if let Err(e) = res {
                    on_error(trf!(
                        "Cannot change priority of “{name}”: {e}",
                        name = name,
                        e = e
                    ));
                }
            },
        );
    });
    dialog.present(Some(parent));
}

/// Sends `action` to `obj`, asking first for End/Kill when `confirm`.
pub fn signal_process(
    parent: &impl IsA<gtk::Widget>,
    obj: &ProcessObject,
    action: Action,
    confirm: bool,
    on_done: Rc<dyn Fn(Result<(), String>)>,
) {
    let (pid, name) = {
        let i = obj.info();
        (i.pid, i.name.clone())
    };
    let run = move || {
        let name = name.clone();
        let on_done = on_done.clone();
        let command = Command::Signal {
            signal: action.helper_signal(),
            pid,
        };
        escalate_if_needed(control::send(pid, action), command, move |res| {
            on_done(res.map_err(|e| {
                trf!(
                    "Cannot {to_lowercase} “{name}”: {e}",
                    to_lowercase = action.verb().to_lowercase(),
                    name = name,
                    e = e
                )
            }));
        });
    };
    if !(confirm && action.needs_confirmation()) {
        run();
        return;
    }
    let (heading, body, button) = match action {
        Action::Kill => (
            trf!("Kill “{}”?", obj.info().name),
            tr("Killing a process stops it immediately without letting it save its work. Unsaved data may be lost."),
            tr("_Kill Process"),
        ),
        _ => (
            trf!("End “{}”?", obj.info().name),
            tr("Ending a process asks it to quit. Unsaved data may be lost, and ending system processes can make the session unstable."),
            tr("_End Process"),
        ),
    };
    let dialog = adw::AlertDialog::builder()
        .heading(heading)
        .body(body)
        .build();
    dialog.add_responses(&[("cancel", tr("_Cancel")), ("ok", button)]);
    dialog.set_response_appearance("ok", adw::ResponseAppearance::Destructive);
    dialog.set_default_response(Some("cancel"));
    dialog.set_close_response("cancel");
    dialog.connect_response(Some("ok"), move |_, _| run());
    dialog.present(Some(parent));
}
