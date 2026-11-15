//! Helix Clocks.

mod alarm;
mod background;
mod clock;
mod gwdb;
mod location;
mod shortcuts;
mod sound;
mod stopwatch;
mod store;
mod sun;
mod timer;
mod tz;
mod ui;

fn main() -> gtk::glib::ExitCode {
    ui::run()
}
