//! Helix Music.

mod index;
mod model;
mod mpris;
mod pages;
mod player;
mod playlist;
mod queue;
mod tracker;
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
            // Uses the LocalSearch/Tracker index for folders it covers
            // (read-only), unless HELIX_MUSIC_NO_TRACKER is set.
            let indexed = tracker::Endpoint::connect().and_then(|ep| match tracker::library(&ep, &roots) {
                Ok(l) => Some(l),
                Err(e) => {
                    eprintln!("helix-music: Tracker query failed: {e}");
                    None
                }
            });
            let (tracks, report) = index::scan_hybrid(&roots, indexed, Vec::new(), None, 0);
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
            eprintln!(
                "{} tracks ({} from Tracker), {} albums, {} artists",
                report.added,
                report.from_tracker,
                lib.albums.len(),
                lib.artists.len()
            );
            glib::ExitCode::SUCCESS
        }
        _ => {
            eprintln!("{USAGE}");
            glib::ExitCode::from(2)
        }
    }
}
