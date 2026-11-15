//! Helix Help — a viewer for Mallard, DocBook and HTML help documents.

mod discovery;
mod docbook;
mod html;
mod library;
mod locale;
mod mallard;
mod sandbox;
mod search;
mod textsize;
mod theme;
mod ui;
mod uri;
mod xmlutil;

use gtk::glib;

const USAGE: &str = "Usage: helix-help [URI]\n\n\
URI may be help:DOCUMENT[/PAGE], ghelp:DOCUMENT, a file:// URI or a path to a help directory or file.";

fn main() -> glib::ExitCode {
    // Must run before anything starts threads.
    if let Some(code) = sandbox::probe_if_requested() {
        std::process::exit(code);
    }
    let mut initial = None;
    for arg in std::env::args().skip(1) {
        match arg.as_str() {
            "-h" | "--help" => {
                println!("{USAGE}");
                return glib::ExitCode::SUCCESS;
            }
            a if a.starts_with('-') && a.len() > 1 => {
                eprintln!("{USAGE}");
                return glib::ExitCode::FAILURE;
            }
            a if initial.is_none() => initial = Some(a.to_string()),
            _ => {
                eprintln!("{USAGE}");
                return glib::ExitCode::FAILURE;
            }
        }
    }
    ui::run(initial)
}
