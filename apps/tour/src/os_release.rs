//! Parsing of the freedesktop `os-release` file.

use std::collections::HashMap;
use std::path::Path;

/// Locations searched, in order, per the os-release specification.
pub const PATHS: [&str; 2] = ["/etc/os-release", "/usr/lib/os-release"];

/// Parses `KEY=value` lines. Values may be bare, single-quoted or
/// double-quoted; inside double quotes `\"`, `\\`, `` \` `` and `\$` are
/// unescaped. Comments, blank lines and malformed lines are skipped.
pub fn parse(content: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else { continue };
        let key = key.trim();
        if key.is_empty() || !key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            continue;
        }
        map.insert(key.to_string(), unquote(value.trim()));
    }
    map
}

fn unquote(value: &str) -> String {
    if let Some(inner) = value.strip_prefix('\'') {
        return inner.strip_suffix('\'').unwrap_or(inner).to_string();
    }
    let Some(inner) = value.strip_prefix('"') else { return value.to_string() };
    let mut out = String::new();
    let mut escaped = false;
    for c in inner.chars() {
        if escaped {
            if !matches!(c, '"' | '\\' | '`' | '$') {
                out.push('\\');
            }
            out.push(c);
            escaped = false;
        } else if c == '\\' {
            escaped = true;
        } else if c == '"' {
            break;
        } else {
            out.push(c);
        }
    }
    if escaped {
        out.push('\\');
    }
    out
}

fn non_empty<'a>(map: &'a HashMap<String, String>, key: &str) -> Option<&'a str> {
    map.get(key).map(|s| s.trim()).filter(|s| !s.is_empty())
}

/// Human-readable system name: `PRETTY_NAME`, else `NAME VERSION`
/// (or `NAME VERSION_ID`), else `NAME`.
pub fn display_name(map: &HashMap<String, String>) -> Option<String> {
    if let Some(pretty) = non_empty(map, "PRETTY_NAME") {
        return Some(pretty.to_string());
    }
    let name = non_empty(map, "NAME")?;
    match non_empty(map, "VERSION").or_else(|| non_empty(map, "VERSION_ID")) {
        Some(version) => Some(format!("{name} {version}")),
        None => Some(name.to_string()),
    }
}

/// Reads the first existing file of `paths` and returns its display name.
pub fn read_display_name_from<P: AsRef<Path>>(paths: &[P]) -> Option<String> {
    paths
        .iter()
        .find_map(|p| std::fs::read_to_string(p).ok())
        .and_then(|content| display_name(&parse(&content)))
}

/// The installed system's display name, if it can be determined.
pub fn system_display_name() -> Option<String> {
    read_display_name_from(&PATHS)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_quoting_styles() {
        let m = parse(
            "# comment\n\nNAME=\"Fedora Linux\"\nID=fedora\nVERSION_ID='41'\n\
             PRETTY_NAME=\"Say \\\"hi\\\" \\$HOME \\\\ \\n\"\n  BAD LINE\n=novalue\n",
        );
        assert_eq!(m["NAME"], "Fedora Linux");
        assert_eq!(m["ID"], "fedora");
        assert_eq!(m["VERSION_ID"], "41");
        assert_eq!(m["PRETTY_NAME"], "Say \"hi\" $HOME \\ \\n");
        assert_eq!(m.len(), 4);
    }

    #[test]
    fn display_name_fallbacks() {
        let m = parse("PRETTY_NAME=\"Ubuntu 24.04 LTS\"\nNAME=Ubuntu\n");
        assert_eq!(display_name(&m).as_deref(), Some("Ubuntu 24.04 LTS"));

        let m = parse("PRETTY_NAME=\"\"\nNAME=Debian\nVERSION=\"12 (bookworm)\"\nVERSION_ID=12\n");
        assert_eq!(display_name(&m).as_deref(), Some("Debian 12 (bookworm)"));

        let m = parse("NAME=Arch\nVERSION_ID=rolling\n");
        assert_eq!(display_name(&m).as_deref(), Some("Arch rolling"));

        let m = parse("NAME=Gentoo\n");
        assert_eq!(display_name(&m).as_deref(), Some("Gentoo"));

        assert_eq!(display_name(&parse("ID=x\n")), None);
        assert_eq!(display_name(&parse("")), None);
    }

    #[test]
    fn reads_first_existing_file() {
        let dir = std::env::temp_dir().join(format!("helix-tour-osr-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let missing = dir.join("missing");
        let fallback = dir.join("os-release");
        std::fs::write(&fallback, "NAME=Helix\nVERSION_ID=1\n").unwrap();
        assert_eq!(read_display_name_from(&[&missing, &fallback]).as_deref(), Some("Helix 1"));
        assert_eq!(read_display_name_from(&[&missing]), None);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
