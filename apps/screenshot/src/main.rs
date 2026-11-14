//! Helix Screenshot: screenshots of the screen, a window or an area,
//! taken by the desktop shell.

mod args;
mod naming;
mod settings;
mod shell;
mod ui;

use gtk::glib;

use args::Command;
use shell::CaptureError;

fn main() -> glib::ExitCode {
    let argv = std::env::args_os()
        .skip(1)
        .map(|a| a.to_string_lossy().into_owned());
    let args = match args::parse(argv) {
        Ok(Command::Run(args)) => args,
        Ok(Command::Help) => {
            println!("{}", args::USAGE);
            return glib::ExitCode::SUCCESS;
        }
        Ok(Command::Version) => {
            println!("helix-screenshot {}", env!("CARGO_PKG_VERSION"));
            return glib::ExitCode::SUCCESS;
        }
        Err(e) => {
            eprintln!("helix-screenshot: {e}\n\n{}", args::USAGE);
            return glib::ExitCode::from(2);
        }
    };
    match &args.file {
        // `--file` alone needs no GTK at all.
        Some(file) if !args.clipboard => {
            let req = ui::request_from_args(&args);
            let result = shell::take(&req).and_then(|png| {
                let path = naming::absolute(file);
                std::fs::write(&path, png).map_err(|e| {
                    CaptureError::Failed(format!("Could not save {}: {e}", path.display()))
                })
            });
            match result {
                Ok(()) => glib::ExitCode::SUCCESS,
                Err(CaptureError::Cancelled) => glib::ExitCode::FAILURE,
                Err(CaptureError::Failed(reason)) => {
                    eprintln!("helix-screenshot: {reason}");
                    glib::ExitCode::FAILURE
                }
            }
        }
        _ => ui::run(args),
    }
}
