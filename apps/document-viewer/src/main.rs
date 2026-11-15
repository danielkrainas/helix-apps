//! Helix Document Viewer.

mod cache;
mod dialogs;
mod docview;
mod layout;
mod links;
mod pagetext;
mod presentation;
mod render;
mod search;
mod shortcuts;
mod state;
mod ui;

use gtk::prelude::*;
use gtk::{gio, glib};

const USAGE: &str = "Usage: helix-document-viewer [OPTION…] [FILE|URI…]

Options:
  -p, --page-label=LABEL   Open the first document at the page with LABEL
  -i, --page-index=N       Open the first document at page N (1-based)
  -f, --fullscreen         Start in full screen
  -s, --presentation       Start in presentation mode
  -h, --help               Show this help";

/// Parses the command line (without argv[0]).
fn parse_args(args: &[String]) -> Result<Option<ui::Options>, String> {
    let mut opts = ui::Options::default();
    let mut files = Vec::new();
    let mut only_files = false;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if only_files || !a.starts_with('-') || a == "-" {
            files.push(a.clone());
            continue;
        }
        let (name, inline) = match a.split_once('=') {
            Some((n, v)) => (n, Some(v.to_string())),
            None => (a.as_str(), None),
        };
        let mut value = |n: &str| {
            inline
                .clone()
                .or_else(|| it.next().cloned())
                .ok_or(format!("Missing value for {n}"))
        };
        match name {
            "--" => only_files = true,
            "-h" | "--help" => return Ok(None),
            "-f" | "--fullscreen" => opts.fullscreen = true,
            "-s" | "--presentation" => opts.presentation = true,
            "-p" | "--page-label" => opts.start = Some(ui::Start::Label(value(name)?)),
            "-i" | "--page-index" => {
                let v = value(name)?;
                let n: usize = v.parse().map_err(|_| format!("Invalid page index: {v}"))?;
                opts.start = Some(ui::Start::Index(n.saturating_sub(1)));
            }
            _ => return Err(format!("Unknown option: {a}")),
        }
    }
    opts.uris = files;
    Ok(Some(opts))
}

fn main() -> glib::ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut opts = match parse_args(&args) {
        Ok(Some(o)) => o,
        Ok(None) => {
            println!("{USAGE}");
            return glib::ExitCode::SUCCESS;
        }
        Err(e) => {
            eprintln!("{e}\n{USAGE}");
            return glib::ExitCode::from(2);
        }
    };
    // Resolve relative paths and URIs.
    opts.uris = opts
        .uris
        .iter()
        .map(|a| gio::File::for_commandline_arg(a).uri().to_string())
        .collect();
    ui::run(opts)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(a: &[&str]) -> Result<Option<ui::Options>, String> {
        parse_args(&a.iter().map(|s| s.to_string()).collect::<Vec<_>>())
    }

    #[test]
    fn cli() {
        let o = p(&["-i", "3", "a.pdf", "--fullscreen", "b.pdf"])
            .unwrap()
            .unwrap();
        assert_eq!(o.uris, vec!["a.pdf", "b.pdf"]);
        assert!(matches!(o.start, Some(ui::Start::Index(2))));
        assert!(o.fullscreen && !o.presentation);
        let o = p(&["--page-label=iv", "-s", "--", "-x.pdf"])
            .unwrap()
            .unwrap();
        assert!(matches!(o.start, Some(ui::Start::Label(ref l)) if l == "iv"));
        assert!(o.presentation);
        assert_eq!(o.uris, vec!["-x.pdf"]);
        assert!(p(&["--help"]).unwrap().is_none());
        assert!(p(&["--bogus"]).is_err());
        assert!(p(&["-i"]).is_err());
        assert!(p(&["-i", "x"]).is_err());
        assert_eq!(ui::parse_zoom_percent("150 %"), Some(1.5));
        assert_eq!(ui::parse_zoom_percent("abc"), None);
    }
}
