//! Helix Video Player.

mod fit;
mod mpris;
mod paintable;
mod player;
mod resume;
mod screenshot;
mod settings;
mod sink;
mod subtitles;
mod timefmt;
mod ui;

use gtk::prelude::*;
use gtk::{gio, glib};

const USAGE: &str = "Usage: helix-video-player [FILE|URI]";

fn main() -> glib::ExitCode {
    let mut target = None;
    let mut only_files = false;
    for arg in std::env::args_os().skip(1) {
        if !only_files {
            match arg.to_str() {
                Some("-h" | "--help") => {
                    println!("{USAGE}");
                    return glib::ExitCode::SUCCESS;
                }
                Some("--") => {
                    only_files = true;
                    continue;
                }
                Some(s) if s.starts_with('-') && s.len() > 1 => {
                    eprintln!("Unknown option: {s}\n{USAGE}");
                    return glib::ExitCode::from(2);
                }
                _ => {}
            }
        }
        if target.is_none() {
            // Resolves relative paths and keeps URIs as they are.
            target = Some(gio::File::for_commandline_arg(&arg).uri().to_string());
        }
    }
    ui::run(target)
}
