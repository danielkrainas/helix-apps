//! SMART assessment and attribute presentation.

use crate::format;
use helix_app_common::i18n::tr;
use helix_app_common::trf;
use helix_app_common::trnf;

/// SMART state of an ATA drive (`org.freedesktop.UDisks2.Drive.Ata`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Smart {
    pub supported: bool,
    pub enabled: bool,
    pub failing: bool,
    pub temperature_k: f64,
    pub power_on_seconds: u64,
    pub num_bad_sectors: i64,
    pub attrs_failing: i64,
    pub attrs_failed_in_past: i64,
    pub selftest_status: String,
    pub selftest_percent_remaining: i64,
    /// Unix time of last update, 0 if never.
    pub updated: u64,
}

impl Smart {
    /// One-line overall assessment, e.g. `Disk is OK (30° C / 86° F)`.
    pub fn assessment(&self) -> String {
        if !self.supported {
            return tr("SMART is not supported").into();
        }
        if !self.enabled {
            return tr("SMART is not enabled").into();
        }
        if self.updated == 0 {
            return tr("SMART data not yet available").into();
        }
        let base = if self.failing {
            tr("DISK IS LIKELY TO FAIL SOON").to_string()
        } else if self.num_bad_sectors > 0 {
            if self.num_bad_sectors == 1 {
                tr("Disk is OK, one bad sector").into()
            } else {
                trnf!(
                    "Disk is OK, {n} bad sector",
                    "Disk is OK, {n} bad sectors",
                    self.num_bad_sectors
                )
            }
        } else if self.attrs_failing > 0 {
            if self.attrs_failing == 1 {
                tr("Disk is OK, one attribute is exceeding its threshold").into()
            } else {
                trnf!(
                    "Disk is OK, {n} attribute is exceeding its threshold",
                    "Disk is OK, {n} attributes are exceeding their thresholds",
                    self.attrs_failing
                )
            }
        } else if self.attrs_failed_in_past > 0 {
            if self.attrs_failed_in_past == 1 {
                tr("Disk is OK, one attribute failed in the past").into()
            } else {
                trnf!(
                    "Disk is OK, {n} attribute failed in the past",
                    "Disk is OK, {n} attributes failed in the past",
                    self.attrs_failed_in_past
                )
            }
        } else {
            tr("Disk is OK").into()
        };
        if self.temperature_k > 0.0 {
            // Translators: the disk's assessment and its temperature.
            trf!(
                "{assessment} ({temperature})",
                assessment = base,
                temperature = format::temperature_kelvin(self.temperature_k)
            )
        } else {
            base
        }
    }

    /// Whether the assessment should be shown as a warning.
    pub fn is_warning(&self) -> bool {
        self.failing || self.num_bad_sectors > 0 || self.attrs_failing > 0
    }

    pub fn selftest_description(&self) -> String {
        match self.selftest_status.as_str() {
            "success" => tr("Last self-test completed successfully").into(),
            "aborted" => tr("Last self-test was aborted").into(),
            "interrupted" => tr("Last self-test was interrupted").into(),
            "fatal" => tr("Last self-test did not complete").into(),
            "error_unknown" => tr("Last self-test failed").into(),
            "error_electrical" => tr("Last self-test failed (electrical)").into(),
            "error_servo" => tr("Last self-test failed (servo)").into(),
            "error_read" => tr("Last self-test failed (read)").into(),
            "error_handling" => tr("Last self-test failed (damage)").into(),
            "inprogress" => trf!(
                "Self-test in progress — {}% remaining",
                self.selftest_percent_remaining
            ),
            "" => tr("Unknown").into(),
            other => other.to_string(),
        }
    }
}

/// One entry of `Drive.Ata.SmartGetAttributes` (`a(ysqiiixia{sv})`).
#[derive(Debug, Clone, PartialEq)]
pub struct Attribute {
    pub id: u8,
    pub name: String,
    pub flags: u16,
    /// Normalized value, -1 if not available.
    pub value: i32,
    pub worst: i32,
    pub threshold: i32,
    pub pretty: i64,
    /// 0 unknown, 1 none, 2 msec, 3 sectors, 4 millikelvin.
    pub pretty_unit: i32,
}

impl Attribute {
    /// "Pre-Fail" when flags bit 0 is set, else "Old-Age".
    pub fn kind(&self) -> &'static str {
        if self.flags & 0x0001 != 0 {
            tr("Pre-Fail")
        } else {
            tr("Old-Age")
        }
    }

    /// "Online" when flags bit 1 is set, else "Offline".
    pub fn updates(&self) -> &'static str {
        if self.flags & 0x0002 != 0 {
            tr("Online")
        } else {
            tr("Offline")
        }
    }

    pub fn failing(&self) -> bool {
        self.value > 0 && self.threshold > 0 && self.value <= self.threshold
    }

    pub fn failed_in_past(&self) -> bool {
        self.worst > 0 && self.threshold > 0 && self.worst <= self.threshold
    }

    pub fn assessment(&self) -> &'static str {
        if self.value < 0 {
            "N/A"
        } else if self.failing() {
            "FAILING"
        } else if self.failed_in_past() {
            tr("Failed in the past")
        } else {
            "OK"
        }
    }

    /// Title-cased name: `power-on-hours` → `Power On Hours`.
    pub fn pretty_name(&self) -> String {
        if self.name.is_empty() {
            return trf!("Unknown Attribute {}", self.id);
        }
        self.name
            .split(['-', '_'])
            .filter(|w| !w.is_empty())
            .map(|w| {
                let lower = w.to_ascii_lowercase();
                match lower.as_str() {
                    "ecc" | "crc" | "udma" | "ssd" | "lba" | "gsense" => lower.to_ascii_uppercase(),
                    _ => {
                        let mut c = lower.chars();
                        match c.next() {
                            Some(f) => f.to_ascii_uppercase().to_string() + c.as_str(),
                            None => String::new(),
                        }
                    }
                }
            })
            .collect::<Vec<_>>()
            .join(" ")
    }

    pub fn pretty_value(&self) -> String {
        match self.pretty_unit {
            1 => format::group_thousands(self.pretty.max(0) as u64),
            2 => {
                let secs = self.pretty.max(0) as u64 / 1000;
                if secs == 0 {
                    trf!("{} msec", self.pretty)
                } else {
                    format::duration(secs)
                }
            }
            3 => {
                let n = self.pretty.max(0) as u64;
                trnf!(
                    "{count} sector",
                    "{count} sectors",
                    n,
                    count = format::group_thousands(n)
                )
            }
            4 => format::temperature_kelvin(self.pretty as f64 / 1000.0),
            _ => "N/A".into(),
        }
    }

    pub fn normalized(v: i32) -> String {
        if v < 0 {
            "N/A".into()
        } else {
            v.to_string()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok() -> Smart {
        Smart {
            supported: true,
            enabled: true,
            temperature_k: 303.15,
            updated: 1_700_000_000,
            selftest_status: "success".into(),
            ..Default::default()
        }
    }

    #[test]
    fn assessments() {
        assert_eq!(ok().assessment(), "Disk is OK (30° C / 86° F)");
        let s = Smart {
            failing: true,
            ..ok()
        };
        assert!(s.assessment().starts_with("DISK IS LIKELY TO FAIL SOON"));
        assert!(s.is_warning());
        let s = Smart {
            num_bad_sectors: 3,
            temperature_k: 0.0,
            ..ok()
        };
        assert_eq!(s.assessment(), "Disk is OK, 3 bad sectors");
        let s = Smart {
            attrs_failed_in_past: 1,
            temperature_k: 0.0,
            ..ok()
        };
        assert_eq!(
            s.assessment(),
            "Disk is OK, one attribute failed in the past"
        );
        assert!(!s.is_warning());
        let s = Smart {
            enabled: false,
            ..ok()
        };
        assert_eq!(s.assessment(), "SMART is not enabled");
        assert_eq!(
            ok().selftest_description(),
            "Last self-test completed successfully"
        );
    }

    fn attr(name: &str, unit: i32, pretty: i64) -> Attribute {
        Attribute {
            id: 9,
            name: name.into(),
            flags: 0x32,
            value: 100,
            worst: 100,
            threshold: 0,
            pretty,
            pretty_unit: unit,
        }
    }

    #[test]
    fn attribute_presentation() {
        assert_eq!(
            attr("power-on-hours", 2, 3_600_000 * 5).pretty_value(),
            "5 hours"
        );
        assert_eq!(
            attr("power-on-hours", 2, 3_600_000).pretty_name(),
            "Power On Hours"
        );
        assert_eq!(
            attr("udma-crc-error-count", 1, 1234).pretty_name(),
            "UDMA CRC Error Count"
        );
        assert_eq!(attr("x", 1, 1234).pretty_value(), "1,234");
        assert_eq!(attr("x", 3, 1).pretty_value(), "1 sector");
        assert_eq!(attr("x", 4, 303_150).pretty_value(), "30° C / 86° F");
        assert_eq!(attr("x", 0, 5).pretty_value(), "N/A");
        let a = attr("x", 1, 0);
        assert_eq!(a.kind(), "Old-Age");
        assert_eq!(a.updates(), "Online");
        assert_eq!(a.assessment(), "OK");
        let a = Attribute {
            flags: 1,
            value: 5,
            threshold: 10,
            ..attr("x", 1, 0)
        };
        assert_eq!(a.kind(), "Pre-Fail");
        assert_eq!(a.assessment(), "FAILING");
        let a = Attribute {
            value: 50,
            worst: 5,
            threshold: 10,
            ..attr("x", 1, 0)
        };
        assert_eq!(a.assessment(), "Failed in the past");
        assert_eq!(Attribute::normalized(-1), "N/A");
    }
}
