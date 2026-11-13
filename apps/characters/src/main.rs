//! Helix Characters: character map and emoji picker.

mod category;
mod charinfo;
mod recent;
mod search;
mod ui;
mod winstate;

use gtk::glib;

const USAGE: &str = "Usage: helix-characters [--search TEXT | --info TEXT]";

fn usage() -> ! {
    eprintln!("{USAGE}");
    std::process::exit(2);
}

fn main() -> glib::ExitCode {
    let mut args = std::env::args().skip(1);
    let mut search = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-s" | "--search" => search = Some(args.next().unwrap_or_else(|| usage())),
            "--info" => {
                let text = args.next().unwrap_or_else(|| usage());
                println!("Name: {}", charinfo::display_name(&text));
                println!("Code points: {}", charinfo::codepoints(&text));
                println!("UTF-8: {}", charinfo::utf8_hex(&text));
                return glib::ExitCode::SUCCESS;
            }
            "-h" | "--help" => {
                println!("{USAGE}");
                return glib::ExitCode::SUCCESS;
            }
            _ => usage(),
        }
    }
    ui::run(search)
}
