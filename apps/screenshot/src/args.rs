//! Command-line parsing (options compatible with `gnome-screenshot`).

use std::path::PathBuf;

use crate::shell::Mode;
use helix_app_common::trf;

pub const MAX_DELAY: u32 = 99;

pub const USAGE: &str = "\
Usage: helix-screenshot [OPTION…]

Take a picture of the screen, a window or an area.

  -w, --window            Grab the focused window instead of the whole screen
  -a, --area              Grab an area of the screen instead of the whole screen
  -d, --delay=SECONDS     Take the screenshot after a delay (0-99)
  -p, --include-pointer   Include the pointer in the screenshot
  -b, --include-border    Include the window frame (default)
  -B, --remove-border     Leave out the window frame
  -c, --clipboard         Send the screenshot directly to the clipboard
  -f, --file=PATH         Save the screenshot directly to this file
  -i, --interactive       Open the window with these choices
  -h, --help              Show this help
      --version           Show the version";

/// What the command line asks for.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Args {
    /// `None` when neither `--window` nor `--area` was given.
    pub mode: Option<Mode>,
    pub delay: Option<u32>,
    pub pointer: bool,
    pub frame: bool,
    pub clipboard: bool,
    pub file: Option<PathBuf>,
    pub interactive: bool,
}

/// What the program should do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Run(Args),
    Help,
    Version,
}

impl Args {
    /// Whether the screenshot is delivered without showing the window.
    pub fn is_direct(&self) -> bool {
        self.clipboard || self.file.is_some()
    }

    /// Whether a screenshot is taken at once and shown in the preview
    /// (capture options given, but no destination and no `--interactive`).
    pub fn capture_now(&self) -> bool {
        !self.interactive
            && !self.is_direct()
            && (self.mode.is_some() || self.delay.is_some() || self.pointer)
    }
}

fn parse_delay(value: &str) -> Result<u32, String> {
    match value.trim().parse::<u32>() {
        Ok(d) if d <= MAX_DELAY => Ok(d),
        _ => Err(trf!(
            "invalid delay “{value}”: expected a number of seconds from 0 to {MAX_DELAY}",
            value = value,
            MAX_DELAY = MAX_DELAY
        )),
    }
}

/// Parses the arguments after the program name.
pub fn parse<I, S>(args: I) -> Result<Command, String>
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    let mut out = Args {
        frame: true,
        ..Args::default()
    };
    let mut iter = args.into_iter().map(Into::into);
    let mut only_files = false;
    while let Some(arg) = iter.next() {
        if only_files || !arg.starts_with('-') || arg == "-" {
            return Err(trf!("unexpected argument “{arg}”", arg = arg));
        }
        if arg == "--" {
            only_files = true;
            continue;
        }
        if let Some(long) = arg.strip_prefix("--") {
            let (name, inline) = match long.split_once('=') {
                Some((n, v)) => (n, Some(v.to_string())),
                None => (long, None),
            };
            let mut value = |what: &str| -> Result<String, String> {
                inline
                    .clone()
                    .or_else(|| iter.next())
                    .ok_or_else(|| trf!("option --{what} needs a value", what = what))
            };
            let takes_value = matches!(name, "delay" | "file");
            if inline.is_some() && !takes_value {
                return Err(trf!("option --{name} takes no value", name = name));
            }
            match name {
                "window" => out.mode = Some(Mode::Window),
                "area" => out.mode = Some(Mode::Area),
                "delay" => out.delay = Some(parse_delay(&value("delay")?)?),
                "include-pointer" => out.pointer = true,
                "include-border" => out.frame = true,
                "remove-border" => out.frame = false,
                "clipboard" => out.clipboard = true,
                "file" => out.file = Some(PathBuf::from(value("file")?)),
                "interactive" => out.interactive = true,
                "help" => return Ok(Command::Help),
                "version" => return Ok(Command::Version),
                _ => return Err(trf!("unknown option --{name}", name = name)),
            }
            continue;
        }
        // A group of short options: `-ac`, `-d5`, `-d 5`, `-fpath`.
        let shorts: Vec<char> = arg[1..].chars().collect();
        let mut i = 0;
        while i < shorts.len() {
            let c = shorts[i];
            i += 1;
            match c {
                'w' => out.mode = Some(Mode::Window),
                'a' => out.mode = Some(Mode::Area),
                'p' => out.pointer = true,
                'b' => out.frame = true,
                'B' => out.frame = false,
                'c' => out.clipboard = true,
                'i' => out.interactive = true,
                'h' => return Ok(Command::Help),
                'd' | 'f' => {
                    let rest: String = shorts[i..].iter().collect();
                    i = shorts.len();
                    let v = if rest.is_empty() {
                        iter.next()
                            .ok_or_else(|| trf!("option -{c} needs a value", c = c))?
                    } else {
                        rest
                    };
                    if c == 'd' {
                        out.delay = Some(parse_delay(&v)?);
                    } else {
                        out.file = Some(PathBuf::from(v));
                    }
                }
                _ => return Err(trf!("unknown option -{c}", c = c)),
            }
        }
    }
    if out.file.as_ref().is_some_and(|f| f.as_os_str().is_empty()) {
        return Err("option --file needs a file name".into());
    }
    Ok(Command::Run(out))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(args: &[&str]) -> Args {
        match parse(args.iter().copied()) {
            Ok(Command::Run(a)) => a,
            other => panic!("{args:?} gave {other:?}"),
        }
    }

    #[test]
    fn no_arguments_open_the_window() {
        let a = run(&[]);
        assert_eq!(a.mode, None);
        assert_eq!(a.delay, None);
        assert!(a.frame);
        assert!(!a.pointer && !a.clipboard && !a.interactive);
        assert!(!a.is_direct());
        assert!(!a.capture_now());
    }

    #[test]
    fn long_options() {
        let a = run(&[
            "--area",
            "--delay",
            "5",
            "--include-pointer",
            "--clipboard",
            "--file",
            "/tmp/x.png",
        ]);
        assert_eq!(a.mode, Some(Mode::Area));
        assert_eq!(a.delay, Some(5));
        assert!(a.pointer && a.clipboard);
        assert_eq!(a.file, Some(PathBuf::from("/tmp/x.png")));
        assert!(a.is_direct());
        assert!(!a.capture_now());
    }

    #[test]
    fn long_options_with_equals() {
        let a = run(&[
            "--window",
            "--delay=12",
            "--file=shot.png",
            "--remove-border",
        ]);
        assert_eq!(a.mode, Some(Mode::Window));
        assert_eq!(a.delay, Some(12));
        assert_eq!(a.file, Some(PathBuf::from("shot.png")));
        assert!(!a.frame);
    }

    #[test]
    fn short_options_and_groups() {
        let a = run(&["-ac", "-d3"]);
        assert_eq!(a.mode, Some(Mode::Area));
        assert!(a.clipboard);
        assert_eq!(a.delay, Some(3));

        let a = run(&["-w", "-p", "-d", "0", "-f", "out.png"]);
        assert_eq!(a.mode, Some(Mode::Window));
        assert!(a.pointer);
        assert_eq!(a.delay, Some(0));
        assert_eq!(a.file, Some(PathBuf::from("out.png")));

        let a = run(&["-pfout.png"]);
        assert!(a.pointer);
        assert_eq!(a.file, Some(PathBuf::from("out.png")));

        let a = run(&["-wB"]);
        assert!(!a.frame);
    }

    #[test]
    fn last_mode_wins() {
        assert_eq!(run(&["-a", "-w"]).mode, Some(Mode::Window));
        assert_eq!(run(&["--window", "--area"]).mode, Some(Mode::Area));
    }

    #[test]
    fn capture_now_and_interactive() {
        assert!(run(&["-a"]).capture_now());
        assert!(run(&["-d", "2"]).capture_now());
        assert!(run(&["-p"]).capture_now());
        assert!(!run(&["-a", "-i"]).capture_now());
        assert!(!run(&["-a", "-c"]).capture_now());
        let a = run(&["--interactive", "--window", "-d", "4"]);
        assert!(a.interactive && !a.is_direct());
    }

    #[test]
    fn help_and_version() {
        assert_eq!(parse(["-h"]), Ok(Command::Help));
        assert_eq!(parse(["--help"]), Ok(Command::Help));
        assert_eq!(parse(["-a", "--version"]), Ok(Command::Version));
    }

    #[test]
    fn errors() {
        for bad in [
            &["--delay"][..],
            &["-d"],
            &["-d", "100"],
            &["-d", "-1"],
            &["--delay=abc"],
            &["-x"],
            &["--bogus"],
            &["picture.png"],
            &["-f"],
            &["--file="],
            &["--area=1"],
            &["--", "x"],
        ] {
            assert!(parse(bad.iter().copied()).is_err(), "{bad:?} should fail");
        }
        assert_eq!(run(&["-d", "99"]).delay, Some(99));
    }
}
