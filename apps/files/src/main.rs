//! Helix Files.

mod bookmarks;
mod cli;
mod clipformat;
mod fmt;
mod history;
mod naming;
mod ops;
mod pathbar;
mod search;
mod sorting;
mod ui;

fn main() -> gtk::glib::ExitCode {
    helix_app_common::i18n::init();
    ui::run()
}
