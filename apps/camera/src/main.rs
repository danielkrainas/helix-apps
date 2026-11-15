//! Helix Camera.

mod controller;
mod countdown;
mod devices;
mod encoders;
mod naming;
mod pipeline;
mod portal;
mod qr;
mod settings;
mod shortcuts;
mod ui;

use gtk::glib;

const USAGE: &str = "Usage: helix-camera [--help] [--version]

Environment:
  HELIX_CAMERA_SOURCE=test|none|denied   use virtual cameras / simulate states
  HELIX_CAMERA_OUTPUT_DIR=DIR            save captures in DIR
  HELIX_CAMERA_PORTAL=0|1                force the Camera portal off/on
  HELIX_CAMERA_SMOKE=1                   with SOURCE=test: scripted UI walk-through";

fn main() -> glib::ExitCode {
    helix_app_common::i18n::init();
    if let Some(arg) = std::env::args().nth(1) {
        match arg.as_str() {
            "-h" | "--help" => {
                println!("{USAGE}");
                return glib::ExitCode::SUCCESS;
            }
            "--version" => {
                println!("helix-camera {}", env!("CARGO_PKG_VERSION"));
                return glib::ExitCode::SUCCESS;
            }
            _ => {
                eprintln!("{USAGE}");
                return glib::ExitCode::from(2);
            }
        }
    }
    if let Err(e) = gst::init() {
        eprintln!("helix-camera: could not initialise GStreamer: {e}");
        return glib::ExitCode::FAILURE;
    }
    ui::run()
}
