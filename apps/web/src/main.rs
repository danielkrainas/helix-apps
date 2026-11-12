//! Helix Web — a WebKitGTK browser.

mod app;
mod bookmarks;
mod dialogs;
mod downloads;
mod history;
mod overview;
mod permissions;
mod session;
mod settings;
mod store;
mod tab;
mod urlinput;
mod window;

use gtk::glib;

const USAGE: &str = "Usage: helix-web [OPTION…] [URL|FILE…]

Options:
  -p, --private      Open the URLs in a new private window
  -n, --new-window   Open the URLs in a new window
  -h, --help         Show this help
  -V, --version      Show the version";

fn main() -> glib::ExitCode {
    for arg in std::env::args().skip(1) {
        match arg.as_str() {
            "--" => break,
            "-h" | "--help" => {
                println!("{USAGE}");
                return glib::ExitCode::SUCCESS;
            }
            "-V" | "--version" => {
                println!("helix-web {}", env!("CARGO_PKG_VERSION"));
                return glib::ExitCode::SUCCESS;
            }
            _ => {}
        }
    }
    app::run()
}
