//! Helix Audio Player.

mod meta;
mod mpris;
mod peaks;
mod player;
mod seek;
mod timefmt;
mod ui;
mod waveform;

use gtk::{gio, glib, prelude::*};

const USAGE: &str = "Usage: helix-audio-player [FILE|URI]";

fn main() -> glib::ExitCode {
    let mut target = None;
    for arg in std::env::args().skip(1) {
        match arg.as_str() {
            "-h" | "--help" => {
                println!("{USAGE}\n\nPlay an audio file.\n\nEnvironment:\n  HELIX_AUDIO_SINK     GStreamer bin description used as the audio sink\n  HELIX_AUDIO_NO_MPRIS Disable the MPRIS D-Bus interface");
                return glib::ExitCode::SUCCESS;
            }
            "--version" => {
                println!("helix-audio-player {}", env!("CARGO_PKG_VERSION"));
                return glib::ExitCode::SUCCESS;
            }
            a if a.starts_with('-') && a.len() > 1 => {
                eprintln!("{USAGE}");
                return glib::ExitCode::from(2);
            }
            _ if target.is_some() => {
                eprintln!("{USAGE}");
                return glib::ExitCode::from(2);
            }
            a => target = Some(gio::File::for_commandline_arg(a).uri().to_string()),
        }
    }
    if let Err(e) = gst::init() {
        eprintln!("helix-audio-player: cannot initialise GStreamer: {e}");
        return glib::ExitCode::FAILURE;
    }
    ui::run(target)
}
