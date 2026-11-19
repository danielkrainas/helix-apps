//! Helix Image Viewer.

mod canvas;
mod decode;
mod fit;
mod folder;
mod metadata;
mod natsort;
mod orientation;
mod save;
mod shortcuts;
mod thumbs;
mod trash;
mod ui;
mod view;
mod wallpaper;

use gtk::prelude::*;
use gtk::{gio, glib};

const USAGE: &str = "Usage: helix-image-viewer [FILE|DIR|URI ...]";

fn main() -> glib::ExitCode {
    helix_app_common::i18n::init();
    let mut paths = Vec::new();
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
        // Resolves relative paths and file:// (or other local) URIs.
        match gio::File::for_commandline_arg(&arg).path() {
            Some(p) => paths.push(p),
            None => eprintln!("Not a local file: {}", arg.to_string_lossy()),
        }
    }
    ui::run(paths)
}
