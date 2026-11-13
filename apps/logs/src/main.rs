//! Helix Logs: a journal viewer.

mod category;
mod format;
mod journal;
mod live;
mod search;
mod ui;
mod worker;

use gtk::glib;

fn main() -> glib::ExitCode {
    if let Some(arg) = std::env::args().nth(1) {
        match arg.as_str() {
            "-h" | "--help" => {
                println!("Usage: helix-logs\n\nView the systemd journal.");
                return glib::ExitCode::SUCCESS;
            }
            "--version" => {
                println!("helix-logs {}", env!("CARGO_PKG_VERSION"));
                return glib::ExitCode::SUCCESS;
            }
            other => {
                eprintln!("helix-logs: unknown option '{other}'");
                return glib::ExitCode::FAILURE;
            }
        }
    }
    ui::run()
}
