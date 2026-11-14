//! Human-readable formatting of sizes, dates and permissions.

use gtk::glib;

pub fn size(bytes: u64) -> String {
    glib::format_size(bytes).to_string()
}

pub fn item_count(n: u64) -> String {
    if n == 1 {
        "1 item".into()
    } else {
        format!("{n} items")
    }
}

/// Formats `dt` relative to `now`. `detailed` gives the full date and time.
pub fn date(dt: &glib::DateTime, now: &glib::DateTime, detailed: bool) -> String {
    let dt = dt.to_local().unwrap_or_else(|_| dt.clone());
    let now = now.to_local().unwrap_or_else(|_| now.clone());
    let f = |fmt: &str| dt.format(fmt).map(|s| s.to_string()).unwrap_or_default();
    if detailed {
        return f("%-e %B %Y %H:%M:%S").trim().to_string();
    }
    let day = |d: &glib::DateTime| (d.year(), d.day_of_year());
    let days_between = {
        let a = glib::DateTime::from_local(dt.year(), dt.month(), dt.day_of_month(), 0, 0, 0.0);
        let b = glib::DateTime::from_local(now.year(), now.month(), now.day_of_month(), 0, 0, 0.0);
        match (a, b) {
            (Ok(a), Ok(b)) => b.difference(&a).as_days(),
            _ => i64::MAX,
        }
    };
    if day(&dt) == day(&now) {
        f("%H:%M")
    } else if days_between == 1 {
        "Yesterday".into()
    } else if (2..7).contains(&days_between) {
        f("%A")
    } else if dt.year() == now.year() {
        f("%-e %b").trim().to_string()
    } else {
        f("%-e %b %Y").trim().to_string()
    }
}

/// Unix permission classes and bits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Who {
    Owner,
    Group,
    Others,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Perm {
    Read,
    Write,
    Execute,
}

pub fn mode_bit(who: Who, perm: Perm) -> u32 {
    let base = match perm {
        Perm::Read => 4,
        Perm::Write => 2,
        Perm::Execute => 1,
    };
    let shift = match who {
        Who::Owner => 6,
        Who::Group => 3,
        Who::Others => 0,
    };
    base << shift
}

pub fn with_bit(mode: u32, who: Who, perm: Perm, on: bool) -> u32 {
    let bit = mode_bit(who, perm);
    if on {
        mode | bit
    } else {
        mode & !bit
    }
}

/// "rwxr-xr-x" style string for the permission bits of `mode`.
pub fn mode_string(mode: u32) -> String {
    let mut s = String::with_capacity(9);
    for who in [Who::Owner, Who::Group, Who::Others] {
        for (perm, c) in [(Perm::Read, 'r'), (Perm::Write, 'w'), (Perm::Execute, 'x')] {
            s.push(if mode & mode_bit(who, perm) != 0 {
                c
            } else {
                '-'
            });
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modes() {
        assert_eq!(mode_string(0o755), "rwxr-xr-x");
        assert_eq!(mode_string(0o640), "rw-r-----");
        assert_eq!(with_bit(0o644, Who::Owner, Perm::Execute, true), 0o744);
        assert_eq!(with_bit(0o644, Who::Others, Perm::Read, false), 0o640);
        // File type bits are preserved.
        assert_eq!(with_bit(0o100644, Who::Group, Perm::Write, true), 0o100664);
    }

    #[test]
    fn counts() {
        assert_eq!(item_count(1), "1 item");
        assert_eq!(item_count(3), "3 items");
    }

    #[test]
    fn dates() {
        let now = glib::DateTime::from_local(2024, 6, 15, 14, 30, 0.0).unwrap();
        let at = |y, m, d, h| glib::DateTime::from_local(y, m, d, h, 5, 0.0).unwrap();
        assert_eq!(date(&at(2024, 6, 15, 9), &now, false), "09:05");
        assert_eq!(date(&at(2024, 6, 14, 23), &now, false), "Yesterday");
        assert_eq!(
            date(&at(2024, 6, 12, 10), &now, false),
            at(2024, 6, 12, 10).format("%A").unwrap()
        );
        assert_eq!(date(&at(2024, 2, 3, 10), &now, false), "3 Feb");
        assert_eq!(date(&at(2021, 2, 3, 10), &now, false), "3 Feb 2021");
        assert!(date(&at(2021, 2, 3, 10), &now, true).contains("2021"));
    }
}
