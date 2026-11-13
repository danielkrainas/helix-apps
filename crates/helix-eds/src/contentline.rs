//! RFC 6350 / RFC 5545 content lines: the line format shared by vCard and
//! iCalendar (`GROUP.NAME;PARAM=a,b:value`), with folding and escaping.

/// One content line.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Property {
    /// vCard property group (`item1` in `item1.EMAIL`), if any.
    pub group: Option<String>,
    /// Upper-cased property name.
    pub name: String,
    /// Parameters in order; names upper-cased, values unquoted.
    pub params: Vec<(String, Vec<String>)>,
    /// Raw (still escaped) value.
    pub value: String,
}

impl Property {
    pub fn new(name: &str, value: impl Into<String>) -> Self {
        Property { group: None, name: name.to_ascii_uppercase(), params: Vec::new(), value: value.into() }
    }

    /// Builds a property whose value is plain text (escaped here).
    pub fn text(name: &str, text: &str) -> Self {
        Self::new(name, escape_text(text))
    }

    pub fn with_param(mut self, name: &str, value: &str) -> Self {
        self.add_param(name, value);
        self
    }

    pub fn add_param(&mut self, name: &str, value: &str) {
        let name = name.to_ascii_uppercase();
        if let Some((_, vals)) = self.params.iter_mut().find(|(n, _)| *n == name) {
            vals.push(value.to_string());
        } else {
            self.params.push((name, vec![value.to_string()]));
        }
    }

    pub fn set_param(&mut self, name: &str, value: &str) {
        let name = name.to_ascii_uppercase();
        self.params.retain(|(n, _)| *n != name);
        self.params.push((name, vec![value.to_string()]));
    }

    /// First value of a parameter.
    pub fn param(&self, name: &str) -> Option<&str> {
        self.params
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .and_then(|(_, v)| v.first())
            .map(String::as_str)
    }

    /// All values of every occurrence of a parameter (e.g. all TYPE values),
    /// with comma-joined values split.
    pub fn param_values(&self, name: &str) -> Vec<String> {
        self.params
            .iter()
            .filter(|(n, _)| n.eq_ignore_ascii_case(name))
            .flat_map(|(_, v)| v.iter())
            .flat_map(|v| v.split(','))
            .filter(|v| !v.is_empty())
            .map(str::to_string)
            .collect()
    }

    /// The value with text escapes removed.
    pub fn text_value(&self) -> String {
        unescape_text(&self.value)
    }

    /// The value split on unescaped `;` and unescaped.
    pub fn structured(&self) -> Vec<String> {
        split_unescaped(&self.value, ';').iter().map(|s| unescape_text(s)).collect()
    }

    /// The value split on unescaped `,` and unescaped.
    pub fn list(&self) -> Vec<String> {
        split_unescaped(&self.value, ',')
            .iter()
            .map(|s| unescape_text(s))
            .filter(|s| !s.is_empty())
            .collect()
    }

    /// Serializes to a single unfolded line (no line terminator).
    pub fn to_line(&self) -> String {
        let mut out = String::new();
        if let Some(g) = &self.group {
            out.push_str(g);
            out.push('.');
        }
        out.push_str(&self.name);
        for (name, vals) in &self.params {
            out.push(';');
            out.push_str(name);
            out.push('=');
            let joined: Vec<String> = vals.iter().map(|v| quote_param(v)).collect();
            out.push_str(&joined.join(","));
        }
        out.push(':');
        out.push_str(&self.value);
        out
    }
}

fn quote_param(v: &str) -> String {
    let v: String = v.chars().filter(|c| *c != '"' && *c != '\r' && *c != '\n').collect();
    if v.contains([':', ';', ',']) {
        format!("\"{v}\"")
    } else {
        v
    }
}

/// Undoes line folding: CRLF (or LF) followed by a space or tab continues the
/// previous line. Returns logical lines, skipping empty ones.
pub fn unfold(text: &str) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    for raw in text.split('\n') {
        let raw = raw.strip_suffix('\r').unwrap_or(raw);
        if let Some(rest) = raw.strip_prefix([' ', '\t']) {
            if let Some(last) = lines.last_mut() {
                last.push_str(rest);
                continue;
            }
        }
        lines.push(raw.to_string());
    }
    // vCard 2.1 quoted-printable soft line breaks: a line ending in '=' joins
    // the next one when the property is QP-encoded.
    let mut out: Vec<String> = Vec::new();
    let mut joining = false;
    for line in lines {
        if joining {
            if let Some(last) = out.last_mut() {
                last.pop();
                last.push_str(&line);
                joining = last.ends_with('=') && is_qp_line(last);
                continue;
            }
        }
        joining = line.ends_with('=') && is_qp_line(&line);
        out.push(line);
    }
    out.retain(|l| !l.trim().is_empty());
    out
}

fn is_qp_line(line: &str) -> bool {
    let head = line.split(':').next().unwrap_or("").to_ascii_uppercase();
    head.contains("QUOTED-PRINTABLE")
}

/// Parses one unfolded content line.
pub fn parse_line(line: &str) -> Option<Property> {
    // Name and params end at the first ':' that is not inside quotes.
    let mut in_quote = false;
    let mut colon = None;
    for (i, c) in line.char_indices() {
        match c {
            '"' => in_quote = !in_quote,
            ':' if !in_quote => {
                colon = Some(i);
                break;
            }
            _ => {}
        }
    }
    let colon = colon?;
    let head = &line[..colon];
    let value = &line[colon + 1..];
    let parts = split_params(head);
    let mut iter = parts.into_iter();
    let name_part = iter.next()?;
    let (group, name) = match name_part.rsplit_once('.') {
        Some((g, n)) => (Some(g.to_string()), n.to_string()),
        None => (None, name_part),
    };
    if name.is_empty() {
        return None;
    }
    let mut params = Vec::new();
    for p in iter {
        let (pname, pval) = match p.split_once('=') {
            Some((n, v)) => (n.trim().to_ascii_uppercase(), v.to_string()),
            // vCard 2.1 bare parameters (`TEL;HOME;VOICE:`) are TYPE values.
            None => ("TYPE".to_string(), p.trim().to_string()),
        };
        let vals = split_param_values(&pval);
        params.push((pname, vals));
    }
    let mut prop = Property { group, name: name.to_ascii_uppercase(), params, value: value.to_string() };
    if prop.param("ENCODING").is_some_and(|e| e.eq_ignore_ascii_case("QUOTED-PRINTABLE")) {
        prop.value = decode_quoted_printable(&prop.value);
        prop.params.retain(|(n, _)| n != "ENCODING");
    }
    Some(prop)
}

fn split_params(head: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_quote = false;
    for c in head.chars() {
        match c {
            '"' => {
                in_quote = !in_quote;
                cur.push(c);
            }
            ';' if !in_quote => out.push(std::mem::take(&mut cur)),
            _ => cur.push(c),
        }
    }
    out.push(cur);
    out
}

fn split_param_values(v: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_quote = false;
    for c in v.chars() {
        match c {
            '"' => in_quote = !in_quote,
            ',' if !in_quote => out.push(std::mem::take(&mut cur)),
            _ => cur.push(c),
        }
    }
    out.push(cur);
    out
}

fn decode_quoted_printable(v: &str) -> String {
    let bytes = v.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'=' && i + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok().and_then(|h| u8::from_str_radix(h, 16).ok());
            if let Some(b) = hex {
                out.push(b);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Escapes a TEXT value (`\`, `;`, `,`, newline).
pub fn escape_text(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            ';' => out.push_str("\\;"),
            ',' => out.push_str("\\,"),
            '\n' => out.push_str("\\n"),
            '\r' => {}
            _ => out.push(c),
        }
    }
    out
}

/// Reverses [`escape_text`].
pub fn unescape_text(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('n') | Some('N') => out.push('\n'),
                Some(other) => out.push(other),
                None => out.push('\\'),
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Splits on `sep` where not preceded by a backslash escape. Components stay
/// escaped.
pub fn split_unescaped(s: &str, sep: char) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            cur.push(c);
            if let Some(n) = chars.next() {
                cur.push(n);
            }
        } else if c == sep {
            out.push(std::mem::take(&mut cur));
        } else {
            cur.push(c);
        }
    }
    out.push(cur);
    out
}

/// Folds a logical line to at most 75 octets per physical line without
/// splitting UTF-8 sequences, joined with CRLF + space.
pub fn fold(line: &str) -> String {
    let mut out = String::with_capacity(line.len() + line.len() / 70 * 3);
    let mut width = 0;
    for c in line.chars() {
        let len = c.len_utf8();
        if width + len > 75 {
            out.push_str("\r\n ");
            width = 1;
        }
        out.push(c);
        width += len;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unfold_joins_continuations() {
        let lines = unfold("NOTE:hello\r\n  world\r\nFN:x\r\n");
        assert_eq!(lines, vec!["NOTE:hello world", "FN:x"]);
    }

    #[test]
    fn parse_group_params_and_quotes() {
        let p = parse_line("item1.EMAIL;TYPE=work,pref;X-LABEL=\"a:b;c\":me@x.org").unwrap();
        assert_eq!(p.group.as_deref(), Some("item1"));
        assert_eq!(p.name, "EMAIL");
        assert_eq!(p.param_values("type"), vec!["work", "pref"]);
        assert_eq!(p.param("X-LABEL"), Some("a:b;c"));
        assert_eq!(p.value, "me@x.org");
    }

    #[test]
    fn bare_v21_params_become_types() {
        let p = parse_line("TEL;HOME;VOICE:123").unwrap();
        assert_eq!(p.param_values("TYPE"), vec!["HOME", "VOICE"]);
    }

    #[test]
    fn quoted_printable_is_decoded() {
        let lines = unfold("NOTE;ENCODING=QUOTED-PRINTABLE:caf=C3=A9 =\r\nau lait\r\n");
        let p = parse_line(&lines[0]).unwrap();
        assert_eq!(p.value, "café au lait");
        assert!(p.param("ENCODING").is_none());
    }

    #[test]
    fn escape_round_trip() {
        let s = "a;b,c\\d\nnext";
        assert_eq!(unescape_text(&escape_text(s)), s);
        assert_eq!(escape_text("a;b"), "a\\;b");
    }

    #[test]
    fn structured_split_respects_escapes() {
        let p = parse_line("ADR:;;1 Main St\\, Apt 2;Town;;;").unwrap();
        let s = p.structured();
        assert_eq!(s[2], "1 Main St, Apt 2");
        assert_eq!(s[3], "Town");
        assert_eq!(s.len(), 7);
    }

    #[test]
    fn fold_respects_utf8_and_width() {
        let line = format!("NOTE:{}", "é".repeat(80));
        let folded = fold(&line);
        for physical in folded.split("\r\n") {
            assert!(physical.len() <= 75, "{} octets", physical.len());
        }
        assert_eq!(unfold(&folded), vec![line]);
    }

    #[test]
    fn to_line_quotes_when_needed() {
        let p = Property::new("X", "v").with_param("LABEL", "a,b").with_param("TYPE", "home");
        assert_eq!(p.to_line(), "X;LABEL=\"a,b\";TYPE=home:v");
    }
}
