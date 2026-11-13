//! Helix System Monitor.

mod control;
mod dialogs;
mod format;
mod helper_args;
mod fs_view;
mod history;
mod model;
mod proc_view;
mod procfs;
mod res_view;
mod sampler;
mod ui;

use gtk::glib;

const USAGE: &str = "Usage: helix-system-monitor [-p|--show-processes-tab | -r|--show-resources-tab | -f|--show-file-systems-tab]";

fn main() -> glib::ExitCode {
    let mut page = None;
    for arg in std::env::args().skip(1) {
        match arg.as_str() {
            "-p" | "--show-processes-tab" => page = Some("processes"),
            "-r" | "--show-resources-tab" => page = Some("resources"),
            "-f" | "--show-file-systems-tab" => page = Some("disks"),
            "-h" | "--help" => {
                println!("{USAGE}");
                return glib::ExitCode::SUCCESS;
            }
            _ => {
                eprintln!("{USAGE}");
                return glib::ExitCode::from(2);
            }
        }
    }
    ui::run(page)
}
