//! Command-line parsing.

use helix_app_common::trf;
use std::path::{Path, PathBuf};

pub const USAGE: &str = "Usage: helix-terminal [--working-directory DIR] [--tab] [-- COMMAND ARGS…]

Options:
  -w, --working-directory DIR   Start in DIR
      --tab                     Open a new tab in an existing window
  -e, -x, --                    Run the remaining arguments instead of the shell
  -h, --help                    Show this help
  -V, --version                 Show the version";

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Options {
    pub working_directory: Option<PathBuf>,
    pub tab: bool,
    pub command: Vec<String>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Parsed {
    Run(Options),
    Help,
    Version,
}

/// Parses `args` (without the program name). Relative working directories
/// are resolved against `cwd`.
pub fn parse<I, S>(args: I, cwd: &Path) -> Result<Parsed, String>
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    let mut opts = Options::default();
    let mut args = args.into_iter().map(Into::into);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-h" | "--help" => return Ok(Parsed::Help),
            "-V" | "--version" => return Ok(Parsed::Version),
            "--tab" => opts.tab = true,
            "-w" | "--working-directory" => {
                let dir = args
                    .next()
                    .ok_or_else(|| trf!("{arg} needs a directory", arg = arg))?;
                opts.working_directory = Some(cwd.join(dir));
            }
            "--" | "-e" | "-x" | "--command" => {
                opts.command = args.by_ref().collect();
                if opts.command.is_empty() {
                    return Err(trf!("{arg} needs a command", arg = arg));
                }
            }
            s => {
                if let Some(dir) = s.strip_prefix("--working-directory=") {
                    opts.working_directory = Some(cwd.join(dir));
                } else {
                    return Err(trf!("unknown option {s}", s = s));
                }
            }
        }
    }
    Ok(Parsed::Run(opts))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(args: &[&str]) -> Options {
        match parse(args.iter().copied(), Path::new("/home/u")).unwrap() {
            Parsed::Run(o) => o,
            p => panic!("{p:?}"),
        }
    }

    #[test]
    fn empty() {
        assert_eq!(run(&[]), Options::default());
    }

    #[test]
    fn working_directory() {
        assert_eq!(
            run(&["--working-directory", "/tmp"]).working_directory,
            Some("/tmp".into())
        );
        assert_eq!(
            run(&["-w", "src"]).working_directory,
            Some("/home/u/src".into())
        );
        assert_eq!(
            run(&["--working-directory=a/b"]).working_directory,
            Some("/home/u/a/b".into())
        );
        assert!(parse(["--working-directory"], Path::new("/")).is_err());
    }

    #[test]
    fn command_and_tab() {
        let o = run(&["--tab", "--", "echo", "--tab", "hi"]);
        assert!(o.tab);
        assert_eq!(o.command, vec!["echo", "--tab", "hi"]);
        assert_eq!(run(&["-e", "top"]).command, vec!["top"]);
        assert!(parse(["--"], Path::new("/")).is_err());
    }

    #[test]
    fn help_version_unknown() {
        assert_eq!(parse(["--tab", "-h"], Path::new("/")), Ok(Parsed::Help));
        assert_eq!(parse(["-V"], Path::new("/")), Ok(Parsed::Version));
        assert!(parse(["--bogus"], Path::new("/")).is_err());
        assert!(parse(["file"], Path::new("/")).is_err());
    }
}
