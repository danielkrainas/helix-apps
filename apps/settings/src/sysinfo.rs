//! Local system information for the About panel (files under /proc, /etc, /sys).

use std::collections::HashMap;

/// Parses an os-release file into key/value pairs (quotes removed).
pub fn parse_os_release(text: &str) -> HashMap<String, String> {
    text.lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.starts_with('#') {
                return None;
            }
            let (k, v) = line.split_once('=')?;
            let v = v.trim();
            let v = v
                .strip_prefix('"')
                .and_then(|v| v.strip_suffix('"'))
                .or_else(|| v.strip_prefix('\'').and_then(|v| v.strip_suffix('\'')))
                .unwrap_or(v);
            Some((k.trim().to_string(), v.replace("\\\"", "\"")))
        })
        .collect()
}

/// Human-readable OS name from os-release fields.
pub fn os_name(fields: &HashMap<String, String>) -> Option<String> {
    if let Some(p) = fields.get("PRETTY_NAME").filter(|s| !s.is_empty()) {
        return Some(p.clone());
    }
    let name = fields.get("NAME")?;
    Some(match fields.get("VERSION") {
        Some(v) if !v.is_empty() => format!("{name} {v}"),
        _ => name.clone(),
    })
}

/// "AMD Ryzen 9 7950X 16-Core Processor × 32" from /proc/cpuinfo.
pub fn cpu_description(cpuinfo: &str) -> Option<String> {
    let mut model: Option<String> = None;
    let mut count = 0usize;
    for line in cpuinfo.lines() {
        let Some((k, v)) = line.split_once(':') else { continue };
        let k = k.trim();
        if k == "processor" {
            count += 1;
        }
        if model.is_none() && (k == "model name" || k == "Model" || k == "cpu model") {
            let v = v.split_whitespace().collect::<Vec<_>>().join(" ");
            if !v.is_empty() {
                model = Some(v);
            }
        }
    }
    let model = model?;
    Some(if count > 1 { format!("{model} × {count}") } else { model })
}

/// Total memory in bytes from /proc/meminfo.
pub fn mem_total_bytes(meminfo: &str) -> Option<u64> {
    meminfo.lines().find_map(|line| {
        let rest = line.strip_prefix("MemTotal:")?;
        let mut it = rest.split_whitespace();
        let n: u64 = it.next()?.parse().ok()?;
        Some(match it.next() {
            Some("kB") | None => n * 1024,
            Some("MB") => n * 1024 * 1024,
            _ => n,
        })
    })
}

/// Joins a hardware vendor and model, skipping empty parts and a vendor
/// already present at the start of the model.
pub fn hardware_model(vendor: &str, model: &str) -> Option<String> {
    let vendor = vendor.trim();
    let model = model.trim();
    match (vendor.is_empty(), model.is_empty()) {
        (true, true) => None,
        (true, false) => Some(model.to_string()),
        (false, true) => Some(vendor.to_string()),
        (false, false) if model.to_lowercase().starts_with(&vendor.to_lowercase()) => Some(model.to_string()),
        _ => Some(format!("{vendor} {model}")),
    }
}

pub fn read(path: &str) -> String {
    std::fs::read_to_string(path).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn os_release() {
        let f = parse_os_release(
            "# comment\nNAME=\"Ubuntu\"\nVERSION='24.04 LTS'\nID=ubuntu\nPRETTY_NAME=\"Ubuntu 24.04.1 LTS\"\n\n",
        );
        assert_eq!(f["ID"], "ubuntu");
        assert_eq!(f["VERSION"], "24.04 LTS");
        assert_eq!(os_name(&f).unwrap(), "Ubuntu 24.04.1 LTS");
        let f = parse_os_release("NAME=Helix\nVERSION=1\n");
        assert_eq!(os_name(&f).unwrap(), "Helix 1");
        let f = parse_os_release("NAME=Helix\n");
        assert_eq!(os_name(&f).unwrap(), "Helix");
        assert_eq!(os_name(&parse_os_release("")), None);
    }

    #[test]
    fn cpu() {
        let info = "processor\t: 0\nmodel name\t: AMD  Ryzen 9 7950X 16-Core Processor\nflags: x\n\n\
                    processor\t: 1\nmodel name\t: AMD  Ryzen 9 7950X 16-Core Processor\n";
        assert_eq!(cpu_description(info).unwrap(), "AMD Ryzen 9 7950X 16-Core Processor × 2");
        assert_eq!(cpu_description("processor : 0\nmodel name : Foo\n").unwrap(), "Foo");
        assert_eq!(cpu_description("processor : 0\n"), None);
    }

    #[test]
    fn memory() {
        assert_eq!(mem_total_bytes("MemTotal:       16314424 kB\nMemFree: 1 kB\n"), Some(16314424 * 1024));
        assert_eq!(mem_total_bytes("MemFree: 1 kB\n"), None);
    }

    #[test]
    fn hardware() {
        assert_eq!(hardware_model("ASUS", "TUF GAMING").unwrap(), "ASUS TUF GAMING");
        assert_eq!(hardware_model("Dell", "Dell XPS 13").unwrap(), "Dell XPS 13");
        assert_eq!(hardware_model("", "X1").unwrap(), "X1");
        assert_eq!(hardware_model(" ", ""), None);
    }
}
