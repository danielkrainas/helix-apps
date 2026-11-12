//! System D-Bus services (blocking zbus; call only from worker threads).
//!
//! Read-only queries: `read_timedate`, `list_timezones`, `read_hostname`,
//! `read_battery`. Mutating calls (`set_ntp`, `set_timezone`,
//! `set_hostname`) are only issued from explicit user actions.

use zbus::blocking::{Connection, Proxy};

type Result<T> = std::result::Result<T, String>;

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

fn system() -> Result<Connection> {
    Connection::system().map_err(err)
}

fn proxy<'a>(conn: &Connection, dest: &'a str, path: &'a str, iface: &'a str) -> Result<Proxy<'a>> {
    Proxy::new(conn, dest, path, iface).map_err(err)
}

const TIMEDATE: (&str, &str, &str) =
    ("org.freedesktop.timedate1", "/org/freedesktop/timedate1", "org.freedesktop.timedate1");
const HOSTNAME: (&str, &str, &str) =
    ("org.freedesktop.hostname1", "/org/freedesktop/hostname1", "org.freedesktop.hostname1");
const UPOWER_DISPLAY: (&str, &str, &str) = (
    "org.freedesktop.UPower",
    "/org/freedesktop/UPower/devices/DisplayDevice",
    "org.freedesktop.UPower.Device",
);

#[derive(Debug, Clone, Default)]
pub struct TimeDate {
    pub timezone: String,
    pub ntp: bool,
    pub can_ntp: bool,
    pub ntp_synchronized: bool,
}

pub fn read_timedate() -> Result<TimeDate> {
    let conn = system()?;
    let p = proxy(&conn, TIMEDATE.0, TIMEDATE.1, TIMEDATE.2)?;
    Ok(TimeDate {
        timezone: p.get_property("Timezone").map_err(err)?,
        ntp: p.get_property("NTP").unwrap_or(false),
        can_ntp: p.get_property("CanNTP").unwrap_or(false),
        ntp_synchronized: p.get_property("NTPSynchronized").unwrap_or(false),
    })
}

/// Time zones from timedated, falling back to the tz database's zone table.
pub fn list_timezones() -> Vec<String> {
    let from_bus = || -> Result<Vec<String>> {
        let conn = system()?;
        let p = proxy(&conn, TIMEDATE.0, TIMEDATE.1, TIMEDATE.2)?;
        p.call("ListTimezones", &()).map_err(err)
    };
    let mut zones = from_bus().unwrap_or_default();
    if zones.is_empty() {
        zones = parse_zone_tab(&std::fs::read_to_string("/usr/share/zoneinfo/zone1970.tab").unwrap_or_default());
        zones.push("UTC".into());
    }
    zones.sort();
    zones.dedup();
    zones
}

/// Zone names (third column) from a `zone1970.tab`/`zone.tab` file.
pub fn parse_zone_tab(text: &str) -> Vec<String> {
    text.lines()
        .filter(|l| !l.starts_with('#'))
        .filter_map(|l| l.split('\t').nth(2))
        .map(|z| z.trim().to_string())
        .filter(|z| !z.is_empty())
        .collect()
}

/// System change: enables/disables NTP (polkit may prompt).
pub fn set_ntp(enabled: bool) -> Result<()> {
    let conn = system()?;
    let p = proxy(&conn, TIMEDATE.0, TIMEDATE.1, TIMEDATE.2)?;
    p.call::<_, _, ()>("SetNTP", &(enabled, true)).map_err(err)
}

/// System change: sets the system time zone (polkit may prompt).
pub fn set_timezone(tz: &str) -> Result<()> {
    let conn = system()?;
    let p = proxy(&conn, TIMEDATE.0, TIMEDATE.1, TIMEDATE.2)?;
    p.call::<_, _, ()>("SetTimezone", &(tz, true)).map_err(err)
}

#[derive(Debug, Clone, Default)]
pub struct HostInfo {
    pub hostname: String,
    pub pretty_hostname: String,
    pub vendor: String,
    pub model: String,
}

impl HostInfo {
    pub fn device_name(&self) -> &str {
        if self.pretty_hostname.is_empty() {
            &self.hostname
        } else {
            &self.pretty_hostname
        }
    }
}

pub fn read_hostname() -> Result<HostInfo> {
    let conn = system()?;
    let p = proxy(&conn, HOSTNAME.0, HOSTNAME.1, HOSTNAME.2)?;
    let s = |name: &str| p.get_property::<String>(name).unwrap_or_default();
    Ok(HostInfo {
        hostname: s("Hostname"),
        pretty_hostname: s("PrettyHostname"),
        vendor: s("HardwareVendor"),
        model: s("HardwareModel"),
    })
}

/// System change: renames the device (polkit may prompt).
pub fn set_hostname(pretty: &str, static_name: &str) -> Result<()> {
    let conn = system()?;
    let p = proxy(&conn, HOSTNAME.0, HOSTNAME.1, HOSTNAME.2)?;
    p.call::<_, _, ()>("SetPrettyHostname", &(pretty, true)).map_err(err)?;
    p.call::<_, _, ()>("SetStaticHostname", &(static_name, true)).map_err(err)
}

#[derive(Debug, Clone, Default)]
pub struct Battery {
    pub percentage: f64,
    pub state: u32,
    pub time_to_empty: i64,
    pub time_to_full: i64,
}

/// The UPower display device, when it is a present battery.
pub fn read_battery() -> Result<Option<Battery>> {
    let conn = system()?;
    let p = proxy(&conn, UPOWER_DISPLAY.0, UPOWER_DISPLAY.1, UPOWER_DISPLAY.2)?;
    let present: bool = p.get_property("IsPresent").map_err(err)?;
    let kind: u32 = p.get_property("Type").unwrap_or(0);
    if !present || kind != 2 {
        return Ok(None);
    }
    Ok(Some(Battery {
        percentage: p.get_property("Percentage").unwrap_or(0.0),
        state: p.get_property("State").unwrap_or(0),
        time_to_empty: p.get_property("TimeToEmpty").unwrap_or(0),
        time_to_full: p.get_property("TimeToFull").unwrap_or(0),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zone_tab() {
        let text = "# comment\nAD\t+4230+00131\tEurope/Andorra\nAE,OM\t+2518+05518\tAsia/Dubai\textra\nbad line\n";
        assert_eq!(parse_zone_tab(text), vec!["Europe/Andorra", "Asia/Dubai"]);
    }

    #[test]
    fn device_name_prefers_pretty() {
        let mut h = HostInfo { hostname: "box".into(), ..Default::default() };
        assert_eq!(h.device_name(), "box");
        h.pretty_hostname = "My Box".into();
        assert_eq!(h.device_name(), "My Box");
    }

    /// Read-only checks against the live system bus:
    /// `cargo test -p helix-settings-app -- --ignored live_bus --nocapture`.
    #[test]
    #[ignore]
    fn live_bus_reads() {
        let t = std::time::Instant::now();
        println!("hostname: {:?}", read_hostname());
        println!("timedate: {:?}", read_timedate());
        println!("battery: {:?}", read_battery());
        println!("zones: {}", list_timezones().len());
        println!("elapsed: {:?}", t.elapsed());
    }
}
