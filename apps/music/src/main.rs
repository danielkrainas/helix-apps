//! Helix Music.

mod index;
mod model;
mod mpris;
mod pages;
mod player;
mod playlist;
mod queue;
mod ui;

use gtk::glib;
use std::path::PathBuf;

const USAGE: &str = "Usage: helix-music [--scan DIR...]";

fn main() -> glib::ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        None => ui::run(),
        Some("-h" | "--help") => {
            println!("{USAGE}");
            glib::ExitCode::SUCCESS
        }
        Some("--scan") if args.len() > 1 => {
            let roots: Vec<PathBuf> = args[1..].iter().map(PathBuf::from).collect();
            let (tracks, report) = index::scan(&roots, Vec::new(), None, 0);
            let lib = model::Library::new(tracks);
            for album in &lib.albums {
                for &t in &album.tracks {
                    let t = &lib.tracks[t];
                    println!(
                        "{} — {} — {}.{} — {}",
                        album.artist,
                        album.title,
                        t.disc.unwrap_or(1),
                        t.track.map_or_else(|| "-".to_string(), |n| n.to_string()),
                        t.title
                    );
                }
            }
            eprintln!("{} tracks, {} albums, {} artists", report.added, lib.albums.len(), lib.artists.len());
            glib::ExitCode::SUCCESS
        }
        _ => {
            eprintln!("{USAGE}");
            glib::ExitCode::from(2)
        }
    }
}
