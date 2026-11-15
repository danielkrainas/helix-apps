//! Helix Calculator.

mod engine;
mod financial;
mod format;
mod ui;
mod units;

use gtk::glib;

fn usage() -> ! {
    eprintln!("Usage: helix-calculator [--solve EXPRESSION | -e EXPRESSION]");
    std::process::exit(2);
}

fn main() -> glib::ExitCode {
    helix_app_common::i18n::init();
    let mut args = std::env::args().skip(1);
    let mut initial = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-s" | "--solve" => {
                let expr = args.next().unwrap_or_else(|| usage());
                let mut ctx = engine::Context::default();
                return match ctx.evaluate(&expr) {
                    Ok(v) => {
                        println!("{}", format::format(v, &format::Options::default()));
                        glib::ExitCode::SUCCESS
                    }
                    Err(e) => {
                        eprintln!("Error: {e}");
                        glib::ExitCode::FAILURE
                    }
                };
            }
            "-e" | "--equation" => initial = Some(args.next().unwrap_or_else(|| usage())),
            "-h" | "--help" => {
                println!("Usage: helix-calculator [--solve EXPRESSION | -e EXPRESSION]");
                return glib::ExitCode::SUCCESS;
            }
            _ => usage(),
        }
    }
    ui::run(initial)
}
