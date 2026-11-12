//! Helix Disks — a UDisks2 front-end.

mod format;
mod layout;
mod model;
mod ops;
mod parttypes;
mod props;
mod smart;
mod udisks;
mod ui;

use gtk::glib;

const USAGE: &str = "Usage: helix-disks [--block-device DEVICE | --list]";

fn main() -> glib::ExitCode {
    let mut args = std::env::args().skip(1);
    let mut block_device = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--block-device" => match args.next() {
                Some(d) => block_device = Some(d),
                None => {
                    eprintln!("{USAGE}");
                    return glib::ExitCode::FAILURE;
                }
            },
            "--list" => {
                // Read-only: GetManagedObjects only.
                return match udisks::conn().and_then(|c| udisks::fetch(&c)) {
                    Ok(snap) => {
                        print!("{}", model::dump(&model::build(&snap)));
                        glib::ExitCode::SUCCESS
                    }
                    Err(e) => {
                        eprintln!("helix-disks: {e}");
                        glib::ExitCode::FAILURE
                    }
                };
            }
            "-h" | "--help" => {
                println!("{USAGE}");
                return glib::ExitCode::SUCCESS;
            }
            _ => {
                eprintln!("{USAGE}");
                return glib::ExitCode::FAILURE;
            }
        }
    }
    ui::run(block_device)
}
