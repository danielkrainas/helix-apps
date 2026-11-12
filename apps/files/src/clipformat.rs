//! Clipboard formats for file lists.

/// `x-special/gnome-copied-files`: first line `copy` or `cut`, then one URI
/// per line. Returns `(is_cut, uris)`.
pub fn parse_copied_files(text: &str) -> Option<(bool, Vec<String>)> {
    let mut lines = text.lines().map(|l| l.trim_end_matches('\r'));
    let is_cut = match lines.next()?.trim() {
        "cut" => true,
        "copy" => false,
        _ => return None,
    };
    let uris: Vec<String> = lines
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.trim().to_string())
        .collect();
    if uris.is_empty() {
        None
    } else {
        Some((is_cut, uris))
    }
}

pub fn format_copied_files(is_cut: bool, uris: &[String]) -> String {
    let mut out = String::from(if is_cut { "cut" } else { "copy" });
    for u in uris {
        out.push('\n');
        out.push_str(u);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copied_files_round_trip() {
        let uris = vec!["file:///a".to_string(), "file:///b%20c".to_string()];
        let text = format_copied_files(true, &uris);
        assert_eq!(text, "cut\nfile:///a\nfile:///b%20c");
        assert_eq!(parse_copied_files(&text), Some((true, uris.clone())));
        assert_eq!(
            parse_copied_files("copy\r\nfile:///a\r\n"),
            Some((false, vec!["file:///a".into()]))
        );
        assert_eq!(parse_copied_files("move\nfile:///a"), None);
        assert_eq!(parse_copied_files("copy\n"), None);
    }
}
