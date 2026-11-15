//! Helix Terminal.

mod app;
mod cli;
mod palette;
mod paths;
mod prefs_dialog;
mod process;
mod search;
mod shortcuts;
mod transparency;
mod window;

use gtk::glib;

fn main() -> glib::ExitCode {
    helix_app_common::i18n::init();
    // --help, --version and bad options are handled locally so they work
    // without a display; everything else goes to the (possibly remote)
    // primary instance through GApplication's command-line forwarding.
    let cwd = std::env::current_dir().unwrap_or_else(|_| glib::home_dir());
    match cli::parse(std::env::args().skip(1), &cwd) {
        Ok(cli::Parsed::Help) => {
            println!("{}", cli::USAGE);
            return glib::ExitCode::SUCCESS;
        }
        Ok(cli::Parsed::Version) => {
            println!("helix-terminal {}", env!("CARGO_PKG_VERSION"));
            return glib::ExitCode::SUCCESS;
        }
        Err(e) => {
            eprintln!("helix-terminal: {e}\n{}", cli::USAGE);
            return glib::ExitCode::from(2);
        }
        Ok(cli::Parsed::Run(_)) => {}
    }
    app::run()
}
