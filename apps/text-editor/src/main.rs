//! Helix Text Editor.

mod app;
mod doc;
mod encoding;
mod prefs;
mod search;
mod session;
mod settings;
mod shortcuts;
mod textops;
mod view;
mod window;

use gtk::glib;

const USAGE: &str = "Usage: helix-text-editor [--standalone] [FILE|URI…]

Options:
  -s, --standalone   Run a separate instance without session restore
  -h, --help         Show this help
  -V, --version      Show the version";

fn main() -> glib::ExitCode {
    let mut standalone = false;
    let mut files = Vec::new();
    let mut only_files = false;
    for arg in std::env::args().skip(1) {
        if only_files {
            files.push(arg);
            continue;
        }
        match arg.as_str() {
            "-s" | "--standalone" => standalone = true,
            "-h" | "--help" => {
                println!("{USAGE}");
                return glib::ExitCode::SUCCESS;
            }
            "-V" | "--version" => {
                println!("helix-text-editor {}", env!("CARGO_PKG_VERSION"));
                return glib::ExitCode::SUCCESS;
            }
            "--" => only_files = true,
            s if s.starts_with('-') && s.len() > 1 => {
                eprintln!("helix-text-editor: unknown option {s}\n{USAGE}");
                return glib::ExitCode::from(2);
            }
            _ => files.push(arg),
        }
    }
    app::run(standalone, files)
}
