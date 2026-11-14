//! Helix Fonts: view and install fonts.

mod fclist;
mod fontinfo;
mod install;
mod render;
mod ui;

use gtk::{gio::prelude::*, glib};
use std::path::PathBuf;

const USAGE: &str = "Usage: helix-fonts [FILE...]\n\nWithout FILE, shows the installed fonts. Each FILE (.ttf, .otf, .ttc, .otc)\nopens in its own window with an Install button.";

fn main() -> glib::ExitCode {
    let mut files = Vec::new();
    let mut only_files = false;
    for arg in std::env::args_os().skip(1) {
        let s = arg.to_string_lossy();
        if !only_files && s.starts_with('-') {
            match s.as_ref() {
                "-h" | "--help" => {
                    println!("{USAGE}");
                    return glib::ExitCode::SUCCESS;
                }
                "--" => only_files = true,
                _ => {
                    eprintln!("Unknown option {s}\n{USAGE}");
                    return glib::ExitCode::from(2);
                }
            }
        } else if s.starts_with("file://") {
            files.push(
                gtk::gio::File::for_uri(&s)
                    .path()
                    .unwrap_or_else(|| PathBuf::from(&arg)),
            );
        } else {
            files.push(PathBuf::from(arg));
        }
    }
    ui::run(files)
}
