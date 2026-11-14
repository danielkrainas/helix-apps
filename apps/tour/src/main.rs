//! Helix Tour: first-run welcome and feature tour.

mod illustrations;
mod os_release;
mod pages;
mod state;
mod ui;

use gtk::glib;

const USAGE: &str = "Usage: helix-tour [--autostart]\n\n  \
    --autostart      Exit at once unless the tour never ran or Helix had a\n                   \
    major upgrade since it was last seen (used by the XDG autostart entry)\n  \
    --version        Print the version\n  \
    -h, --help       Show this help";

fn main() -> glib::ExitCode {
    let mut autostart = false;
    for arg in std::env::args().skip(1) {
        match arg.as_str() {
            // `--if-first-run` is the earlier name of the flag, kept as an alias.
            "--autostart" | "--if-first-run" => autostart = true,
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
    if !state::should_show(
        autostart,
        &state::State::load(&path),
        env!("CARGO_PKG_VERSION"),
    ) {
        return glib::ExitCode::SUCCESS;
    }
    ui::run(path)
}
