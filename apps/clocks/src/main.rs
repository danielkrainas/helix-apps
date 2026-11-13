//! Helix Clocks.

mod alarm;
mod background;
mod clock;
mod gwdb;
mod location;
mod stopwatch;
mod store;
mod sound;
mod sun;
mod timer;
mod tz;
mod ui;

fn main() -> gtk::glib::ExitCode {
    ui::run()
}
