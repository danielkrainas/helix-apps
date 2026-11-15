//! Command-line parsing.

use helix_app_common::trf;

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Options {
    pub new_window: bool,
    pub select: bool,
    pub quit: bool,
    pub help: bool,
    pub locations: Vec<String>,
}

pub const USAGE: &str = "Usage: helix-files [OPTION…] [URI…]

  -w, --new-window   Always open a new window
  -s, --select       Select the given items in their parent folders
  -q, --quit         Quit the running instance
  -h, --help         Show this help";

pub fn parse<I: IntoIterator<Item = String>>(args: I) -> Result<Options, String> {
    let mut o = Options::default();
    let mut only_files = false;
    for arg in args {
        if only_files || !arg.starts_with('-') || arg == "-" {
            o.locations.push(arg);
            continue;
        }
        match arg.as_str() {
            "--" => only_files = true,
            "-w" | "--new-window" => o.new_window = true,
            "-s" | "--select" => o.select = true,
            "-q" | "--quit" => o.quit = true,
            "-h" | "--help" => o.help = true,
            // Accepted for compatibility, no effect.
            "--no-desktop" | "-n" | "--no-default-window" => {}
            _ => return Err(trf!("Unknown option {arg}", arg = arg)),
        }
    }
    Ok(o)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(args: &[&str]) -> Result<Options, String> {
        parse(args.iter().map(|s| s.to_string()))
    }

    #[test]
    fn parsing() {
        assert_eq!(p(&[]).unwrap(), Options::default());
        let o = p(&["-w", "/tmp", "sftp://host/"]).unwrap();
        assert!(o.new_window);
        assert_eq!(o.locations, ["/tmp", "sftp://host/"]);
        let o = p(&["--select", "--", "-weird-name"]).unwrap();
        assert!(o.select);
        assert_eq!(o.locations, ["-weird-name"]);
        assert!(p(&["--bogus"]).is_err());
        assert!(p(&["-q"]).unwrap().quit);
    }
}
