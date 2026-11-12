//! Helix Clocks.

mod alarm;
mod clock;
mod stopwatch;
mod store;
mod sun;
mod timer;
mod tz;
mod ui;

fn main() -> gtk::glib::ExitCode {
    ui::run()
}
