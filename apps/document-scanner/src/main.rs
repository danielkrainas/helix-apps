//! Helix Document Scanner.

mod crop;
mod document;
mod export;
mod options;
mod page;
mod pdf;
mod sane;
mod scanner;
mod settings;
mod ui;

use gtk::glib;

const USAGE: &str = "Usage: helix-document-scanner [OPTIONS] [DEVICE]

Options:
  --list-devices   List available scanners and exit
  -d, --debug      Print SANE debugging messages
  -v, --version    Show the version and exit
  -h, --help       Show this help and exit";

fn main() -> glib::ExitCode {
    let mut device = None;
    for arg in std::env::args().skip(1) {
        match arg.as_str() {
            "-h" | "--help" => {
                println!("{USAGE}");
                return glib::ExitCode::SUCCESS;
            }
            "-v" | "--version" => {
                println!("helix-document-scanner {}", env!("CARGO_PKG_VERSION"));
                return glib::ExitCode::SUCCESS;
            }
            "-d" | "--debug" => scanner::set_debug(true),
            "--list-devices" => {
                return match scanner::list_devices() {
                    Ok(devs) => {
                        for d in devs {
                            println!("{}\t{} ({})", d.name, d.label(), d.kind);
                        }
                        glib::ExitCode::SUCCESS
                    }
                    Err(e) => {
                        eprintln!("Unable to initialise SANE: {e}");
                        glib::ExitCode::FAILURE
                    }
                };
            }
            s if s.starts_with('-') => {
                eprintln!("{USAGE}");
                return glib::ExitCode::from(2);
            }
            s => device = Some(s.to_string()),
        }
    }
    ui::run(device)
}
