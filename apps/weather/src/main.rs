//! Helix Weather.

mod api;
mod autoloc;
mod graph;
mod gwdb;
mod import;
mod location;
mod store;
mod timefmt;
mod ui;
mod units;
mod wmo;

use gtk::glib;

const USAGE: &str = "Usage: helix-weather";

fn main() -> glib::ExitCode {
    helix_app_common::i18n::init();
    if let Some(arg) = std::env::args().nth(1) {
        if arg == "-h" || arg == "--help" {
            println!("{USAGE}");
            return glib::ExitCode::SUCCESS;
        }
        eprintln!("{USAGE}");
        return glib::ExitCode::from(2);
    }
    ui::run()
}
