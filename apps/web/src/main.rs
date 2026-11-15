//! Helix Web — a WebKitGTK browser.

mod app;
mod bookmarks;
mod dialogs;
mod downloads;
mod history;
mod htmlparse;
mod overview;
mod passwords;
mod permissions;
mod reader;
mod sandbox;
mod secrets;
mod session;
mod settings;
mod shortcuts;
mod sscrypto;
mod store;
mod tab;
mod urlinput;
mod webapp;
mod window;

use gtk::glib;

const USAGE: &str = "Usage: helix-web [OPTION…] [URL|FILE…]

Options:
  -p, --private      Open the URLs in a new private window
  -n, --new-window   Open the URLs in a new window
  --web-app=ID URL   Run the installed web application ID (used by its launcher)
  -h, --help         Show this help
  -V, --version      Show the version";

fn main() -> glib::ExitCode {
    // Must run before anything starts threads.
    if let Some(code) = sandbox::probe_if_requested() {
        std::process::exit(code);
    }
    let mut web_app = None;
    let mut first_uri = None;
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
            a if a.starts_with("--web-app=") => match webapp::parse_arg(a) {
                Some(id) => web_app = Some(id),
                None => {
                    eprintln!("helix-web: invalid web application ID");
                    return glib::ExitCode::FAILURE;
                }
            },
            a if !a.starts_with('-') && first_uri.is_none() => first_uri = Some(a.to_string()),
            _ => {}
        }
    }
    let web_app = match (web_app, first_uri) {
        (Some(id), Some(url)) => Some(webapp::WebApp { id, url }),
        (Some(_), None) => {
            eprintln!("helix-web: --web-app needs the application's URL");
            return glib::ExitCode::FAILURE;
        }
        _ => None,
    };
    app::run(web_app)
}
