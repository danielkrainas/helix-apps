//! Helix Tour: first-run welcome and feature tour.

mod illustrations;
mod os_release;
mod pages;
mod state;
mod ui;

use gtk::glib;

const USAGE: &str = "Usage: helix-tour [--if-first-run]\n\n  \
    --if-first-run   Exit at once if the tour was already completed or skipped\n  \
    --version        Print the version\n  \
    -h, --help       Show this help";

fn main() -> glib::ExitCode {
    let mut if_first_run = false;
    for arg in std::env::args().skip(1) {
        match arg.as_str() {
            "--if-first-run" => if_first_run = true,
            "--version" => {
                println!("helix-tour {}", env!("CARGO_PKG_VERSION"));
                return glib::ExitCode::SUCCESS;
            }
            "-h" | "--help" => {
                println!("{USAGE}");
                return glib::ExitCode::SUCCESS;
            }
            _ => {
                eprintln!("{USAGE}");
                return glib::ExitCode::from(2);
            }
        }
    }
    let path = state::default_path();
    if !state::should_show(if_first_run, &state::State::load(&path)) {
        return glib::ExitCode::SUCCESS;
    }
    ui::run(path)
}
