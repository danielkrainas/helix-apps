//! Helix Maps.

use helix_app_common::i18n::tr;
use helix_app_common::trf;

mod favorites;
mod geo;
mod http;
mod location;
mod nominatim;
mod osrm;
mod service;
mod settings;
mod shortcuts;
mod tiles;
mod ui;

use gtk::glib;

const USAGE: &str = "Usage: helix-maps [geo:URI | maps:q=QUERY | --search QUERY]";

fn usage_error(msg: &str) -> glib::ExitCode {
    eprintln!("{msg}\n{USAGE}");
    glib::ExitCode::from(2)
}

fn main() -> glib::ExitCode {
    helix_app_common::i18n::init();
    let mut args = std::env::args().skip(1);
    let mut target = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-h" | "--help" => {
                println!("{USAGE}");
                return glib::ExitCode::SUCCESS;
            }
            "--version" => {
                println!("helix-maps {}", env!("CARGO_PKG_VERSION"));
                return glib::ExitCode::SUCCESS;
            }
            "-S" | "--search" => match args.next() {
                Some(q) if !q.trim().is_empty() => {
                    target = Some(geo::LaunchTarget::Search(q.trim().to_string()))
                }
                _ => return usage_error(tr("--search needs a query")),
            },
            other => match geo::parse_launch_arg(other) {
                Some(t) => target = Some(t),
                None => return usage_error(&trf!("Unrecognised argument: {other}", other = other)),
            },
        }
    }
    ui::run(target)
}
