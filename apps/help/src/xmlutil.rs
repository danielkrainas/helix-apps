//! XML helpers shared by the Mallard and DocBook transformers.

use roxmltree::{Document, Node, ParsingOptions};
use std::collections::HashSet;
use std::path::Path;

pub const XI_NS: &str = "http://www.w3.org/2001/XInclude";

/// Named character entities commonly used in DocBook documents that rely on
/// the (unavailable) DocBook DTD.
const ENTITIES: &[(&str, u32)] = &[
    ("nbsp", 0xA0), ("copy", 0xA9), ("reg", 0xAE), ("trade", 0x2122), ("mdash", 0x2014),
    ("ndash", 0x2013), ("hellip", 0x2026), ("lsquo", 0x2018), ("rsquo", 0x2019), ("ldquo", 0x201C),
    ("rdquo", 0x201D), ("laquo", 0xAB), ("raquo", 0xBB), ("bull", 0x2022), ("middot", 0xB7),
    ("times", 0xD7), ("divide", 0xF7), ("deg", 0xB0), ("plusmn", 0xB1), ("micro", 0xB5),
    ("para", 0xB6), ("sect", 0xA7), ("euro", 0x20AC), ("pound", 0xA3), ("yen", 0xA5),
    ("cent", 0xA2), ("larr", 0x2190), ("rarr", 0x2192), ("uarr", 0x2191), ("darr", 0x2193),
    ("harr", 0x2194), ("rArr", 0x21D2), ("lArr", 0x21D0), ("le", 0x2264), ("ge", 0x2265),
    ("ne", 0x2260), ("minus", 0x2212), ("percnt", 0x25), ("num", 0x23), ("dollar", 0x24),
    ("ast", 0x2A), ("commat", 0x40), ("lsqb", 0x5B), ("rsqb", 0x5D), ("lcub", 0x7B),
    ("rcub", 0x7D), ("verbar", 0x7C), ("sol", 0x2F), ("bsol", 0x5C), ("excl", 0x21),
    ("quest", 0x3F), ("iexcl", 0xA1), ("iquest", 0xBF), ("shy", 0xAD), ("zwsp", 0x200B),
    ("thinsp", 0x2009), ("ensp", 0x2002), ("emsp", 0x2003), ("dagger", 0x2020),
    ("Dagger", 0x2021), ("prime", 0x2032), ("Prime", 0x2033), ("frac12", 0xBD),
    ("frac14", 0xBC), ("frac34", 0xBE), ("sup2", 0xB2), ("sup3", 0xB3), ("aacute", 0xE1),
    ("eacute", 0xE9), ("iacute", 0xED), ("oacute", 0xF3), ("uacute", 0xFA), ("agrave", 0xE0),
    ("egrave", 0xE8), ("auml", 0xE4), ("ouml", 0xF6), ("uuml", 0xFC), ("Auml", 0xC4),
    ("Ouml", 0xD6), ("Uuml", 0xDC), ("szlig", 0xDF), ("ntilde", 0xF1), ("ccedil", 0xE7),
    ("check", 0x2713), ("cross", 0x2717),
];

/// Rewrites a document so that roxmltree (which has no DTD loading) can
/// parse it: references to undeclared named entities become numeric
/// references (or vanish when unknown); references to external `SYSTEM`
/// entities become `xi:include`s of the referenced file.
pub fn preprocess(src: &str) -> String {
    let mut declared: HashSet<String> = ["lt", "gt", "amp", "apos", "quot"].iter().map(|s| s.to_string()).collect();
    let mut external: Vec<(String, String)> = Vec::new();
    let mut out_src = src.to_string();
    if let Some(dtd_start) = src.find("<!DOCTYPE") {
        let mut search = dtd_start;
        while let Some(off) = src[search..].find("<!ENTITY") {
            let start = search + off;
            let Some(end_off) = src[start..].find('>') else { break };
            let decl = &src[start..start + end_off + 1];
            let mut words = decl["<!ENTITY".len()..].split_whitespace();
            if let Some(name) = words.next().filter(|n| *n != "%") {
                if matches!(words.next(), Some("SYSTEM") | Some("PUBLIC")) {
                    let quoted: Vec<&str> = decl.split(['"', '\'']).collect();
                    // SYSTEM "file" → index 1; PUBLIC "id" "file" → index 3.
                    let file = if decl.contains("PUBLIC") { quoted.get(3) } else { quoted.get(1) };
                    if let Some(f) = file {
                        external.push((name.to_string(), f.to_string()));
                    }
                } else {
                    declared.insert(name.to_string());
                }
            }
            search = start + end_off + 1;
        }
        // Remove external declarations; roxmltree can't resolve them.
        for (name, _) in &external {
            if let Some(pos) = out_src.find(&format!("<!ENTITY {name} ")) {
                if let Some(end) = out_src[pos..].find('>') {
                    out_src.replace_range(pos..pos + end + 1, "");
                }
            }
        }
    }
    let src = out_src;
    let mut out = String::with_capacity(src.len());
    let mut rest = src.as_str();
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        let after = &rest[i + 1..];
        let end = after.find(';').filter(|&e| e > 0 && e < 40);
        let name = end.map(|e| &after[..e]).filter(|n| {
            !n.starts_with('#') && n.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_')
        });
        match name {
            Some(n) if declared.contains(n) => {
                out.push('&');
                out.push_str(n);
                out.push(';');
            }
            Some(n) => {
                if let Some((_, file)) = external.iter().find(|(e, _)| e == n) {
                    out.push_str(&format!("<xi:include xmlns:xi=\"{XI_NS}\" href=\"{}\"/>", escape_attr(file)));
                } else if let Some((_, cp)) = ENTITIES.iter().find(|(e, _)| *e == n) {
                    out.push_str(&format!("&#{cp};"));
                }
            }
            None => {
                out.push('&');
                rest = after;
                continue;
            }
        }
        rest = &after[n_len(name) + 1..];
    }
    out.push_str(rest);
    out
}

fn n_len(name: Option<&str>) -> usize {
    name.map(str::len).unwrap_or(0)
}

pub fn options() -> ParsingOptions {
    ParsingOptions { allow_dtd: true, ..ParsingOptions::default() }
}

/// Parses already-preprocessed text.
pub fn parse(text: &str) -> Result<Document<'_>, String> {
    Document::parse_with_options(text, options()).map_err(|e| e.to_string())
}

/// Reads and preprocesses a file.
pub fn read(path: &Path) -> Result<String, String> {
    let raw = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(preprocess(&String::from_utf8_lossy(&raw)))
}

pub fn is_xinclude(n: Node) -> bool {
    n.is_element() && n.tag_name().name() == "include" && n.tag_name().namespace() == Some(XI_NS)
}

/// All descendant text, whitespace-collapsed.
pub fn text_of(n: Node) -> String {
    let mut s = String::new();
    for d in n.descendants().filter(|d| d.is_text()) {
        s.push_str(d.text().unwrap_or(""));
    }
    collapse_ws(&s)
}

pub fn collapse_ws(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub fn child<'a, 'i>(n: Node<'a, 'i>, name: &str) -> Option<Node<'a, 'i>> {
    n.children().find(|c| c.is_element() && c.tag_name().name() == name)
}

pub fn children<'a, 'i>(n: Node<'a, 'i>, name: &'static str) -> impl Iterator<Item = Node<'a, 'i>> {
    n.children().filter(move |c| c.is_element() && c.tag_name().name() == name)
}

pub fn escape(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '<' => o.push_str("&lt;"),
            '>' => o.push_str("&gt;"),
            '&' => o.push_str("&amp;"),
            _ => o.push(c),
        }
    }
    o
}

pub fn escape_attr(s: &str) -> String {
    escape(s).replace('"', "&quot;")
}

/// Resolves an `xi:include href` relative to `base_dir`, refusing anything
/// that escapes it. Only `parse="xml"` (the default) is supported; text
/// includes return the file content escaped.
pub enum Included {
    Xml(String),
    Text(String),
}

pub fn load_include(n: Node, base_dir: &Path) -> Option<Included> {
    let href = n.attribute("href")?;
    if href.contains("://") {
        return None;
    }
    let path = base_dir.join(href);
    let canon = path.canonicalize().ok()?;
    let base = base_dir.canonicalize().ok()?;
    if !canon.starts_with(&base) {
        return None;
    }
    if n.attribute("parse") == Some("text") {
        return std::fs::read_to_string(&canon).ok().map(Included::Text);
    }
    read(&canon).ok().map(Included::Xml)
}

/// Strips the XML declaration, doctype and leading comments of an included
/// document so its root element can be spliced into another document.
fn strip_prolog(text: &str) -> &str {
    let mut s = text;
    loop {
        let t = s.trim_start();
        if t.starts_with("<?") {
            s = t.find("?>").map(|i| &t[i + 2..]).unwrap_or("");
        } else if t.starts_with("<!--") {
            s = t.find("-->").map(|i| &t[i + 3..]).unwrap_or("");
        } else if t.starts_with("<!DOCTYPE") {
            // Skip an internal subset if present.
            let end = match (t.find('['), t.find('>')) {
                (Some(b), Some(g)) if b < g => t.find("]>").map(|i| i + 2),
                (_, Some(g)) => Some(g + 1),
                _ => None,
            };
            s = end.map(|i| &t[i..]).unwrap_or("");
        } else {
            return t;
        }
    }
}

/// Reads `path`, preprocesses it, and splices in `xi:include`d XML (and
/// text) files relative to its directory, recursively (depth-limited).
pub fn read_flat(path: &Path) -> Result<String, String> {
    let text = read(path)?;
    let dir = path.parent().unwrap_or(Path::new("."));
    Ok(flatten(text, dir, 0))
}

fn flatten(text: String, dir: &Path, depth: u32) -> String {
    if depth > 4 || !text.contains(XI_NS) {
        return text;
    }
    let replacements: Vec<(std::ops::Range<usize>, String)> = {
        let Ok(doc) = parse(&text) else { return text };
        doc.descendants()
            .filter(|n| is_xinclude(*n) && !n.ancestors().skip(1).any(is_xinclude))
            .map(|n| {
                let content = match load_include(n, dir) {
                    Some(Included::Xml(x)) => {
                        let inner = flatten(x, dir, depth + 1);
                        strip_prolog(&inner).to_string()
                    }
                    Some(Included::Text(t)) => escape(&t),
                    None => n
                        .children()
                        .find(|c| c.is_element() && c.tag_name().name() == "fallback")
                        .and_then(|f| {
                            let r = f.range();
                            let inner = &text[r.clone()];
                            let open_end = inner.find('>')? + 1;
                            let close = inner.rfind("</")?;
                            (open_end <= close).then(|| inner[open_end..close].to_string())
                        })
                        .unwrap_or_default(),
                };
                (n.range(), content)
            })
            .collect()
    };
    let mut out = text;
    for (range, content) in replacements.into_iter().rev() {
        out.replace_range(range, &content);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xinclude_flattening() {
        let d = std::env::temp_dir().join(format!("helix-help-xi-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("legal.xml"), "<?xml version=\"1.0\"?>\n<!-- c -->\n<license><p>CC &mdash; BY</p></license>").unwrap();
        std::fs::write(
            d.join("main.xml"),
            format!(
                "<page xmlns:xi=\"{XI_NS}\"><xi:include href=\"legal.xml\"/>\
                 <xi:include href=\"missing.xml\"><xi:fallback><p>gone</p></xi:fallback></xi:include>\
                 <xi:include href=\"../../etc/passwd\" parse=\"text\"/></page>"
            ),
        )
        .unwrap();
        let flat = read_flat(&d.join("main.xml")).unwrap();
        let doc = parse(&flat).unwrap();
        assert_eq!(text_of(doc.root_element()), "CC \u{2014} BYgone");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn entities_are_rewritten() {
        let src = "<!DOCTYPE a [\n<!ENTITY app \"Foo\">\n<!ENTITY legal SYSTEM \"legal.xml\">\n]>\n<a>&app; &mdash; &amp; &#169; &bogus;&legal;</a>";
        let pre = preprocess(src);
        let doc = parse(&pre).unwrap();
        let root = doc.root_element();
        assert_eq!(text_of(root), "Foo \u{2014} & \u{a9}");
        let inc = root.descendants().find(|n| is_xinclude(*n)).unwrap();
        assert_eq!(inc.attribute("href"), Some("legal.xml"));
    }

    #[test]
    fn no_doctype_untouched() {
        let src = "<p>a &lt; b &copy;</p>";
        let pre = preprocess(src);
        assert_eq!(text_of(parse(&pre).unwrap().root_element()), "a < b \u{a9}");
    }
}
