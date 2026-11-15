//! Running file operations on worker threads with a progress popover.
//!
//! The popover never opens by itself (that would take keyboard focus);
//! the header button shows the combined progress instead.

use adw::prelude::*;
use gtk::{gio, glib};
use helix_app_common::Labelled;
use std::rc::Rc;

use super::{dialogs, Window};
use crate::ops::{self, Conflict, Job, Outcome, Progress, Resolution};
use helix_app_common::i18n::tr;
use helix_app_common::trf;

enum Msg {
    Progress(Progress),
    Conflict(Conflict, std::sync::mpsc::Sender<Resolution>),
    Done(Outcome),
}

struct ChannelHandler {
    tx: async_channel::Sender<Msg>,
}

impl ops::Handler for ChannelHandler {
    fn progress(&mut self, p: &Progress) {
        let _ = self.tx.send_blocking(Msg::Progress(p.clone()));
    }

    fn conflict(&mut self, c: &Conflict) -> Resolution {
        let (tx, rx) = std::sync::mpsc::channel();
        if self.tx.send_blocking(Msg::Conflict(c.clone(), tx)).is_err() {
            return Resolution {
                choice: ops::Choice::Cancel,
                apply_to_all: true,
            };
        }
        rx.recv().unwrap_or(Resolution {
            choice: ops::Choice::Cancel,
            apply_to_all: true,
        })
    }
}

fn details(p: &Progress) -> String {
    if p.total_bytes > 0 {
        format!(
            "{} of {}",
            crate::fmt::size(p.done_bytes.min(p.total_bytes)),
            crate::fmt::size(p.total_bytes)
        )
    } else if p.total_items > 0 {
        format!(
            "{} of {}",
            p.done_items.min(p.total_items),
            crate::fmt::item_count(p.total_items)
        )
    } else {
        tr("Preparing…").into()
    }
}

/// Combined progress of the running jobs (the mean of their bars), or
/// `None` when nothing runs.
pub fn overall_fraction(jobs_box: &gtk::Box) -> Option<f64> {
    let mut fractions = Vec::new();
    let mut row = jobs_box.first_child();
    while let Some(r) = row {
        let bar = r
            .first_child()
            .and_then(|top| top.next_sibling())
            .and_downcast::<gtk::ProgressBar>();
        if let Some(bar) = bar {
            fractions.push(bar.fraction());
        }
        row = r.next_sibling();
    }
    mean(&fractions)
}

fn mean(v: &[f64]) -> Option<f64> {
    (!v.is_empty()).then(|| v.iter().sum::<f64>() / v.len() as f64)
}

/// The header button's tooltip and description for `fraction`.
fn progress_text(fraction: f64) -> String {
    trf!(
        "File Operations — {} % done",
        (fraction.clamp(0.0, 1.0) * 100.0).floor()
    )
}

/// Shows the combined progress on the header button: its pie icon and
/// its tooltip and accessible description.
fn update_indicator(w: &Window) {
    w.jobs_icon.queue_draw();
    let text = overall_fraction(&w.jobs_box).map(progress_text);
    let text = text.as_deref().unwrap_or(tr("File Operations"));
    if w.jobs_button.tooltip_text().as_deref() != Some(text) {
        w.jobs_button.set_tooltip_text(Some(text));
        w.jobs_button
            .update_property(&[gtk::accessible::Property::Description(text)]);
    }
}

/// Draws the header button's icon: a pie filled to the combined progress
/// on a faint disc, in the button's text colour.
pub fn draw_indicator(area: &gtk::DrawingArea, cr: &gtk::cairo::Context, jobs_box: &gtk::Box) {
    let (w, h) = (area.width() as f64, area.height() as f64);
    let r = w.min(h) / 2.0 - 1.0;
    let (cx, cy) = (w / 2.0, h / 2.0);
    let c = area.color();
    let set = |a: f64| {
        cr.set_source_rgba(
            c.red() as f64,
            c.green() as f64,
            c.blue() as f64,
            c.alpha() as f64 * a,
        )
    };
    set(0.25);
    cr.arc(cx, cy, r, 0.0, std::f64::consts::TAU);
    let _ = cr.fill();
    let f = overall_fraction(jobs_box).unwrap_or(0.0).clamp(0.0, 1.0);
    if f > 0.0 {
        set(1.0);
        let start = -std::f64::consts::FRAC_PI_2;
        cr.move_to(cx, cy);
        cr.arc(cx, cy, r, start, start + f * std::f64::consts::TAU);
        cr.close_path();
        let _ = cr.fill();
    }
}

/// Accessible name of a job's Cancel button: "Cancel Copying 3 files".
fn cancel_name(title: &str) -> String {
    trf!("Cancel {title}", title = title)
}

/// Starts `job` on a worker thread; `done` runs on the main thread after it
/// finishes (also when cancelled).
pub fn start(win: &Rc<Window>, job: Job, done: impl FnOnce(&Outcome) + 'static) {
    let title = job.title();
    let summary = job.clone();
    let row = gtk::Box::new(gtk::Orientation::Vertical, 4);
    let top = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let label = gtk::Label::builder()
        .label(&title)
        .xalign(0.0)
        .hexpand(true)
        .ellipsize(gtk::pango::EllipsizeMode::Middle)
        .build();
    let cancel_btn = gtk::Button::builder()
        .icon_name("process-stop-symbolic")
        .tooltip_text(tr("Cancel"))
        .valign(gtk::Align::Center)
        .build()
        .labelled(&cancel_name(&title));
    cancel_btn.add_css_class("flat");
    cancel_btn.add_css_class("circular");
    top.append(&label);
    top.append(&cancel_btn);
    let bar = gtk::ProgressBar::new();
    helix_app_common::label_widget(&bar, &label);
    let info = gtk::Label::builder()
        .label(tr("Preparing…"))
        .xalign(0.0)
        .ellipsize(gtk::pango::EllipsizeMode::Middle)
        .build();
    info.add_css_class("caption");
    info.add_css_class("dim-label");
    row.append(&top);
    row.append(&bar);
    row.append(&info);
    win.jobs_box.append(&row);
    win.jobs_button.set_visible(true);
    update_indicator(win);

    let cancel = gio::Cancellable::new();
    let c = cancel.clone();
    cancel_btn.connect_clicked(move |b| {
        c.cancel();
        b.set_sensitive(false);
    });

    let (tx, rx) = async_channel::unbounded::<Msg>();
    let worker_cancel = cancel.clone();
    std::thread::spawn(move || {
        let mut handler = ChannelHandler { tx: tx.clone() };
        let out = ops::run(&job, &worker_cancel, &mut handler);
        let _ = tx.send_blocking(Msg::Done(out));
    });

    let weak = Rc::downgrade(win);
    let mut done = Some(done);
    // Keep the application alive until the job finishes.
    let hold = win.shared.app.hold();
    glib::spawn_future_local(async move {
        while let Ok(msg) = rx.recv().await {
            match msg {
                Msg::Progress(p) => {
                    bar.set_fraction(p.fraction());
                    if let Some(w) = weak.upgrade() {
                        update_indicator(&w);
                    }
                    let mut text = details(&p);
                    if !p.current.is_empty() {
                        text = format!("{text} — {}", p.current);
                    }
                    info.set_text(&text);
                }
                Msg::Conflict(c, reply) => {
                    let resolution = match weak.upgrade() {
                        Some(w) => dialogs::conflict(&w.window, &c).await,
                        None => Resolution {
                            choice: ops::Choice::Cancel,
                            apply_to_all: true,
                        },
                    };
                    let _ = reply.send(resolution);
                }
                Msg::Done(out) => {
                    drop(hold);
                    let Some(w) = weak.upgrade() else { return };
                    w.jobs_box.remove(&row);
                    if w.jobs_box.first_child().is_none() {
                        w.jobs_button.popdown();
                        w.jobs_button.set_visible(false);
                    }
                    update_indicator(&w);
                    if !out.errors.is_empty() {
                        report_errors(&w, &title, &out.errors);
                    } else if out.cancelled {
                        w.toast(tr("Operation cancelled"));
                    } else if let Some(text) = summary.finished(out.created.len()) {
                        // Toasts are spoken by libadwaita.
                        w.toast(&text);
                    }
                    if let Some(d) = done.take() {
                        d(&out);
                    }
                    return;
                }
            }
        }
    });
}

fn report_errors(w: &Window, title: &str, errors: &[String]) {
    let mut body: Vec<String> = errors.iter().take(8).cloned().collect();
    if errors.len() > 8 {
        body.push(trf!("…and {} more", errors.len() - 8));
    }
    let dialog = adw::AlertDialog::builder()
        .heading(tr("Some Items Could Not Be Processed"))
        .body(format!("{title}:\n{}", body.join("\n")))
        .build();
    dialog.add_response("close", tr("_Close"));
    dialog.present(Some(&w.window));
}

#[cfg(test)]
mod tests {
    #[test]
    fn combined_progress() {
        assert_eq!(super::mean(&[]), None);
        assert_eq!(super::mean(&[0.5, 1.0, 0.0]), Some(0.5));
        assert_eq!(super::progress_text(0.456), "File Operations — 45 % done");
        assert_eq!(super::progress_text(1.2), "File Operations — 100 % done");
    }

    #[test]
    fn cancel_names_its_job() {
        assert_eq!(
            super::cancel_name("Copying 3 files"),
            "Cancel Copying 3 files"
        );
    }
}
