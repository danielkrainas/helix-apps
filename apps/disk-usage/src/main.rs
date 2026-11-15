//! Helix Disk Usage Analyzer.

mod chart;
mod layout;
mod mounts;
mod scanner;
mod shortcuts;
mod ui;

use gtk::glib;

const USAGE: &str = "Usage: helix-disk-usage [DIRECTORY]";

fn main() -> glib::ExitCode {
    helix_app_common::i18n::init();
    let mut initial = None;
    for arg in std::env::args().skip(1) {
        match arg.as_str() {
            "-h" | "--help" => {
                println!("{USAGE}");
                return glib::ExitCode::SUCCESS;
            }
            a if a.starts_with('-') || initial.is_some() => {
                eprintln!("{USAGE}");
                return glib::ExitCode::FAILURE;
            }
            _ => initial = Some(arg),
        }
    }
    ui::run(initial)
}
