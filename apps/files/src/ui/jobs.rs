//! Running file operations on worker threads with a progress popover.

use adw::prelude::*;
use gtk::{gio, glib};
use helix_app_common::Labelled;
use std::rc::Rc;

use super::{dialogs, Window};
use crate::ops::{self, Conflict, Job, Outcome, Progress, Resolution};

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
        "Preparing…".into()
    }
}

/// Starts `job` on a worker thread; `done` runs on the main thread after it
/// finishes (also when cancelled).
pub fn start(win: &Rc<Window>, job: Job, done: impl FnOnce(&Outcome) + 'static) {
    let title = job.title();
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
        .tooltip_text("Cancel")
        .valign(gtk::Align::Center)
        .build()
        .labelled("Cancel");
    cancel_btn.add_css_class("flat");
    cancel_btn.add_css_class("circular");
    top.append(&label);
    top.append(&cancel_btn);
    let bar = gtk::ProgressBar::new();
    let info = gtk::Label::builder()
        .label("Preparing…")
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

    // Long-running jobs pop the progress popover open.
    let weak = Rc::downgrade(win);
    let pulse_row = row.clone();
    glib::timeout_add_local_once(std::time::Duration::from_millis(1500), move || {
        if let Some(w) = weak.upgrade() {
            if pulse_row.parent().is_some() {
                w.jobs_button.popup();
            }
        }
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
                    if !out.errors.is_empty() {
                        report_errors(&w, &title, &out.errors);
                    } else if out.cancelled {
                        w.toast("Operation cancelled");
                    } else {
                        helix_app_common::announce(
                            &w.window,
                            &format!("Finished {}", lowercase_first(&title)),
                            helix_app_common::Urgency::Polite,
                        );
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

/// "Copying 3 items…" → "copying 3 items…", to follow "Finished".
fn lowercase_first(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_lowercase().chain(c).collect(),
        None => String::new(),
    }
}

fn report_errors(w: &Window, title: &str, errors: &[String]) {
    let mut body: Vec<String> = errors.iter().take(8).cloned().collect();
    if errors.len() > 8 {
        body.push(format!("…and {} more", errors.len() - 8));
    }
    let dialog = adw::AlertDialog::builder()
        .heading("Some Items Could Not Be Processed")
        .body(format!("{title}:\n{}", body.join("\n")))
        .build();
    dialog.add_response("close", "_Close");
    dialog.present(Some(&w.window));
}

#[cfg(test)]
mod tests {
    #[test]
    fn finished_message() {
        assert_eq!(
            super::lowercase_first("Copying 2 items to “Docs”"),
            "copying 2 items to “Docs”"
        );
        assert_eq!(super::lowercase_first(""), "");
    }
}
