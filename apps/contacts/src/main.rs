//! Helix Contacts.

mod backend;
mod editor;
mod individual;
mod links;
mod ui;

use gtk::glib;

const USAGE: &str = "Usage: helix-contacts [-i|--individual UID] [-e|--email ADDRESS] [-s|--search TERM] [FILE.vcf…]";

fn parse_args(args: impl IntoIterator<Item = String>) -> Result<ui::Options, String> {
    let mut opts = ui::Options::default();
    let mut it = args.into_iter();
    while let Some(a) = it.next() {
        let mut value = |name: &str| it.next().ok_or_else(|| format!("{name} needs a value"));
        match a.as_str() {
            "-i" | "--individual" => opts.individual = Some(value(&a)?),
            "-e" | "--email" => opts.email = Some(value(&a)?),
            "-s" | "--search" => opts.search = Some(value(&a)?),
            "-h" | "--help" => return Err(String::new()),
            s if s.starts_with('-') => return Err(format!("unknown option {s}")),
            s => opts.files.push(s.into()),
        }
    }
    Ok(opts)
}

fn main() -> glib::ExitCode {
    match parse_args(std::env::args().skip(1)) {
        Ok(opts) => ui::run(opts),
        Err(e) if e.is_empty() => {
            println!("{USAGE}");
            glib::ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("helix-contacts: {e}\n{USAGE}");
            glib::ExitCode::from(2)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn cli() {
        let o = parse_args(args(&["-i", "pas-id-1", "--search", "ada", "a.vcf"])).unwrap();
        assert_eq!(o.individual.as_deref(), Some("pas-id-1"));
        assert_eq!(o.search.as_deref(), Some("ada"));
        assert_eq!(o.files, vec![std::path::PathBuf::from("a.vcf")]);
        assert!(parse_args(args(&["--email"])).is_err());
        assert!(parse_args(args(&["--bogus"])).is_err());
        assert_eq!(parse_args(args(&["-h"])).unwrap_err(), "");
    }
}
