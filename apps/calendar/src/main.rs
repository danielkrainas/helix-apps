//! Helix Calendar.

mod backend;
mod colors;
mod dates;
mod editor;
mod keynav;
mod layout;
mod manage;
mod model;
mod recur;
mod ui;

use gtk::glib;

const USAGE: &str = "Usage: helix-calendar [-d|--date YYYY-MM-DD] [-u|--uuid UID] [FILE.ics…]";

fn parse_args(args: impl IntoIterator<Item = String>) -> Result<ui::Options, String> {
    let mut opts = ui::Options::default();
    let mut it = args.into_iter();
    while let Some(a) = it.next() {
        let mut value = |name: &str| it.next().ok_or_else(|| format!("{name} needs a value"));
        match a.as_str() {
            "-d" | "--date" => {
                let v = value(&a)?;
                opts.date =
                    Some(dates::parse_date(&v).ok_or_else(|| format!("invalid date “{v}”"))?);
            }
            "-u" | "--uuid" => opts.uuid = Some(value(&a)?),
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
            eprintln!("helix-calendar: {e}\n{USAGE}");
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
        let o = parse_args(args(&["--date", "2026-11-13", "-u", "abc", "x.ics"])).unwrap();
        assert_eq!(o.date, chrono::NaiveDate::from_ymd_opt(2026, 11, 13));
        assert_eq!(o.uuid.as_deref(), Some("abc"));
        assert_eq!(o.files.len(), 1);
        assert!(parse_args(args(&["--date", "13/11/2026"])).is_err());
        assert!(parse_args(args(&["--uuid"])).is_err());
        assert_eq!(parse_args(args(&["--help"])).unwrap_err(), "");
    }
}
