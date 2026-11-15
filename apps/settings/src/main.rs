//! Helix Settings.

mod bluez;
mod bus;
mod dbus;
mod extensions;
#[cfg(test)]
mod fakes;
mod icon_themes;
mod netconf;
mod nm;
mod panels;
mod rfkill;
mod sound;
mod sysinfo;
mod ui;
mod values;
mod wallpaper;
mod xkb;

use gtk::glib;

const USAGE: &str = "Usage: helix-settings-app [PANEL]\n       helix-settings-app --list\n\nOpens Settings, optionally on PANEL (see --list).";

fn main() -> glib::ExitCode {
    helix_app_common::i18n::init();
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        None => {}
        Some("-h" | "--help") => {
            println!("{USAGE}");
            return glib::ExitCode::SUCCESS;
        }
        Some("-l" | "--list") => {
            for p in panels::PANELS {
                let state = if p.available {
                    "available"
                } else {
                    "unavailable"
                };
                println!("{}\t{}\t{}", p.name, p.title, state);
            }
            return glib::ExitCode::SUCCESS;
        }
        Some(name) if args.len() == 2 && panels::lookup(name).is_some() => {}
        Some(name) => {
            eprintln!("helix-settings-app: unknown panel '{name}'");
            let names: Vec<&str> = panels::PANELS.iter().map(|p| p.name).collect();
            eprintln!("Panels: {}", names.join(", "));
            return glib::ExitCode::from(2);
        }
    }
    ui::run(args)
}
