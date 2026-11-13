//! A small, tolerant HTML parser producing an arena DOM.
//!
//! It is not a full HTML5 tree builder; it handles what reader mode needs:
//! tags and attributes, comments, raw-text elements, void elements, the
//! common implied end tags (`p`, `li`, `dt`/`dd`, table cells, `option`)
//! and character references. Mismatched end tags never panic: an end tag
//! closes the nearest open element with that name, or is ignored.

pub type NodeId = usize;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NodeKind {
    /// The document root (id 0).
    Root,
    Element {
        tag: String,
        attrs: Vec<(String, String)>,
    },
    Text(String),
}

#[derive(Debug, Clone)]
pub struct Node {
    pub kind: NodeKind,
    pub parent: Option<NodeId>,
    pub children: Vec<NodeId>,
}

#[derive(Debug, Clone)]
pub struct Document {
    pub nodes: Vec<Node>,
}

const VOID: &[&str] = &[
    "area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "param", "source",
    "track", "wbr", "keygen",
];

const RAW_TEXT: &[&str] = &["script", "style", "textarea", "title", "xmp", "noscript", "template"];

/// Start tags that close an open `p`.
const CLOSES_P: &[&str] = &[
    "address", "article", "aside", "blockquote", "details", "div", "dl", "fieldset", "figcaption",
    "figure", "footer", "form", "h1", "h2", "h3", "h4", "h5", "h6", "header", "hr", "main", "menu",
    "nav", "ol", "p", "pre", "section", "table", "ul",
];

impl Document {
    pub const ROOT: NodeId = 0;

    pub fn parse(html: &str) -> Document {
        let mut b = Builder {
            doc: Document {
                nodes: vec![Node {
                    kind: NodeKind::Root,
                    parent: None,
                    children: Vec::new(),
                }],
            },
            stack: vec![0],
        };
        b.run(html);
        b.doc
    }

    pub fn tag(&self, id: NodeId) -> Option<&str> {
        match &self.nodes[id].kind {
            NodeKind::Element { tag, .. } => Some(tag),
            _ => None,
        }
    }

    pub fn is(&self, id: NodeId, name: &str) -> bool {
        self.tag(id) == Some(name)
    }

    pub fn attr(&self, id: NodeId, name: &str) -> Option<&str> {
        match &self.nodes[id].kind {
            NodeKind::Element { attrs, .. } => attrs
                .iter()
                .find(|(k, _)| k == name)
                .map(|(_, v)| v.as_str()),
            _ => None,
        }
    }

    pub fn children(&self, id: NodeId) -> &[NodeId] {
        &self.nodes[id].children
    }

    pub fn parent(&self, id: NodeId) -> Option<NodeId> {
        self.nodes[id].parent
    }

    /// All descendants of `id` in document order (excluding `id`).
    pub fn descendants(&self, id: NodeId) -> Vec<NodeId> {
        let mut out = Vec::new();
        let mut stack: Vec<NodeId> = self.nodes[id].children.iter().rev().copied().collect();
        while let Some(n) = stack.pop() {
            out.push(n);
            stack.extend(self.nodes[n].children.iter().rev());
        }
        out
    }

    /// First element named `name` in document order.
    pub fn find(&self, name: &str) -> Option<NodeId> {
        self.descendants(Self::ROOT)
            .into_iter()
            .find(|&n| self.is(n, name))
    }

    pub fn find_all(&self, name: &str) -> Vec<NodeId> {
        self.descendants(Self::ROOT)
            .into_iter()
            .filter(|&n| self.is(n, name))
            .collect()
    }

    /// Concatenated text of `id` and its descendants.
    pub fn text(&self, id: NodeId) -> String {
        let mut s = String::new();
        self.collect_text(id, &mut s, &|_| false);
        s
    }

    /// Like [`Document::text`] but skipping subtrees for which `skip` is true.
    pub fn collect_text(&self, id: NodeId, out: &mut String, skip: &dyn Fn(NodeId) -> bool) {
        if skip(id) {
            return;
        }
        match &self.nodes[id].kind {
            NodeKind::Text(t) => out.push_str(t),
            NodeKind::Element { tag, .. } if RAW_TEXT.contains(&tag.as_str()) && tag != "title" => {}
            _ => {
                for &c in &self.nodes[id].children {
                    self.collect_text(c, out, skip);
                }
            }
        }
    }
}

struct Builder {
    doc: Document,
    stack: Vec<NodeId>,
}

impl Builder {
    fn current(&self) -> NodeId {
        *self.stack.last().expect("root stays on the stack")
    }

    fn append(&mut self, kind: NodeKind) -> NodeId {
        let parent = self.current();
        let id = self.doc.nodes.len();
        self.doc.nodes.push(Node {
            kind,
            parent: Some(parent),
            children: Vec::new(),
        });
        self.doc.nodes[parent].children.push(id);
        id
    }

    fn text(&mut self, t: &str) {
        if t.is_empty() {
            return;
        }
        let t = decode_entities(t);
        let parent = self.current();
        if let Some(&last) = self.doc.nodes[parent].children.last() {
            if let NodeKind::Text(prev) = &mut self.doc.nodes[last].kind {
                prev.push_str(&t);
                return;
            }
        }
        self.append(NodeKind::Text(t));
    }

    fn open_index(&self, name: &str, stop_at: &[&str]) -> Option<usize> {
        for (i, &n) in self.stack.iter().enumerate().rev().take(self.stack.len() - 1) {
            let tag = self.doc.tag(n).unwrap_or("");
            if tag == name {
                return Some(i);
            }
            if stop_at.contains(&tag) {
                return None;
            }
        }
        None
    }

    fn close_to(&mut self, index: usize) {
        self.stack.truncate(index.max(1));
    }

    fn start_tag(&mut self, tag: String, attrs: Vec<(String, String)>, self_closing: bool) {
        // Implied end tags.
        if CLOSES_P.contains(&tag.as_str()) {
            if let Some(i) = self.open_index("p", &["button", "table", "td", "th", "li", "div"]) {
                self.close_to(i);
            }
        }
        match tag.as_str() {
            "li" => {
                if let Some(i) = self.open_index("li", &["ul", "ol", "menu"]) {
                    self.close_to(i);
                }
            }
            "dt" | "dd" => {
                let i = self
                    .open_index("dt", &["dl"])
                    .or_else(|| self.open_index("dd", &["dl"]));
                if let Some(i) = i {
                    self.close_to(i);
                }
            }
            "td" | "th" => {
                let i = self
                    .open_index("td", &["tr", "table"])
                    .or_else(|| self.open_index("th", &["tr", "table"]));
                if let Some(i) = i {
                    self.close_to(i);
                }
            }
            "tr" => {
                if let Some(i) = self.open_index("tr", &["table"]) {
                    self.close_to(i);
                }
            }
            "option" => {
                if let Some(i) = self.open_index("option", &["select"]) {
                    self.close_to(i);
                }
            }
            _ => {}
        }
        let void = VOID.contains(&tag.as_str());
        let id = self.append(NodeKind::Element { tag, attrs });
        if !void && !self_closing {
            self.stack.push(id);
        }
    }

    fn end_tag(&mut self, tag: &str) {
        if let Some(i) = self.open_index(tag, &[]) {
            self.close_to(i);
        }
    }

    fn run(&mut self, html: &str) {
        let bytes = html.as_bytes();
        let mut i = 0;
        let mut text_start = 0;
        while i < bytes.len() {
            if bytes[i] != b'<' {
                i += 1;
                continue;
            }
            let rest = &html[i..];
            let next = bytes.get(i + 1).copied().unwrap_or(0);
            if rest.starts_with("<!--") {
                self.text(&html[text_start..i]);
                let end = rest
                    .strip_prefix("<!--")
                    .and_then(|r| r.find("-->"))
                    .map(|p| i + 4 + p + 3)
                    .unwrap_or(bytes.len());
                i = end;
                text_start = i;
            } else if next == b'!' || next == b'?' {
                self.text(&html[text_start..i]);
                i = rest.find('>').map(|p| i + p + 1).unwrap_or(bytes.len());
                text_start = i;
            } else if next == b'/' && bytes.get(i + 2).is_some_and(|c| c.is_ascii_alphabetic()) {
                self.text(&html[text_start..i]);
                let (name, after) = read_name(html, i + 2);
                i = rest
                    .get(after - i..)
                    .and_then(|r| r.find('>'))
                    .map(|p| after + p + 1)
                    .unwrap_or(bytes.len());
                text_start = i;
                self.end_tag(&name);
            } else if next.is_ascii_alphabetic() {
                self.text(&html[text_start..i]);
                let (name, mut j) = read_name(html, i + 1);
                let (attrs, self_closing, end) = read_attrs(html, j);
                j = end;
                i = j;
                text_start = i;
                let raw = RAW_TEXT.contains(&name.as_str());
                self.start_tag(name.clone(), attrs, self_closing);
                if raw && !self_closing {
                    // Raw text up to the matching end tag.
                    let close = format!("</{name}");
                    let lower = html[i..].to_ascii_lowercase();
                    let stop = lower.find(&close).map(|p| i + p).unwrap_or(bytes.len());
                    let content = &html[i..stop];
                    if !content.is_empty() {
                        let t = if name == "title" || name == "textarea" {
                            decode_entities(content)
                        } else {
                            content.to_string()
                        };
                        self.append(NodeKind::Text(t));
                    }
                    self.end_tag(&name);
                    i = html[stop..]
                        .find('>')
                        .map(|p| stop + p + 1)
                        .unwrap_or(bytes.len());
                    text_start = i;
                }
            } else {
                i += 1;
            }
        }
        self.text(&html[text_start..]);
    }
}

fn read_name(html: &str, start: usize) -> (String, usize) {
    let bytes = html.as_bytes();
    let mut j = start;
    while j < bytes.len() && !bytes[j].is_ascii_whitespace() && bytes[j] != b'>' && bytes[j] != b'/'
    {
        j += 1;
    }
    (html[start..j].to_ascii_lowercase(), j)
}

/// Parses attributes from `start` up to and including the closing `>`.
fn read_attrs(html: &str, start: usize) -> (Vec<(String, String)>, bool, usize) {
    let bytes = html.as_bytes();
    let mut attrs: Vec<(String, String)> = Vec::new();
    let mut j = start;
    let mut self_closing = false;
    loop {
        while j < bytes.len() && bytes[j].is_ascii_whitespace() {
            j += 1;
        }
        if j >= bytes.len() {
            return (attrs, self_closing, j);
        }
        match bytes[j] {
            b'>' => return (attrs, self_closing, j + 1),
            b'/' => {
                self_closing = bytes.get(j + 1) == Some(&b'>');
                j += 1;
                continue;
            }
            _ => {}
        }
        let ns = j;
        while j < bytes.len()
            && !bytes[j].is_ascii_whitespace()
            && !matches!(bytes[j], b'=' | b'>')
            && !(bytes[j] == b'/' && bytes.get(j + 1) == Some(&b'>'))
        {
            j += 1;
        }
        let name = html[ns..j].to_ascii_lowercase();
        while j < bytes.len() && bytes[j].is_ascii_whitespace() {
            j += 1;
        }
        let mut value = String::new();
        if bytes.get(j) == Some(&b'=') {
            j += 1;
            while j < bytes.len() && bytes[j].is_ascii_whitespace() {
                j += 1;
            }
            match bytes.get(j) {
                Some(&q) if q == b'"' || q == b'\'' => {
                    let vs = j + 1;
                    let ve = html[vs..].find(q as char).map(|p| vs + p).unwrap_or(bytes.len());
                    value = decode_entities(&html[vs..ve]);
                    j = (ve + 1).min(bytes.len());
                }
                _ => {
                    let vs = j;
                    while j < bytes.len() && !bytes[j].is_ascii_whitespace() && bytes[j] != b'>' {
                        j += 1;
                    }
                    value = decode_entities(&html[vs..j]);
                }
            }
        }
        if !name.is_empty() && !attrs.iter().any(|(k, _)| *k == name) {
            attrs.push((name, value));
        }
    }
}

const ENTITIES: &[(&str, &str)] = &[
    ("amp", "&"),
    ("lt", "<"),
    ("gt", ">"),
    ("quot", "\""),
    ("apos", "'"),
    ("nbsp", "\u{a0}"),
    ("mdash", "—"),
    ("ndash", "–"),
    ("hellip", "…"),
    ("lsquo", "‘"),
    ("rsquo", "’"),
    ("ldquo", "“"),
    ("rdquo", "”"),
    ("laquo", "«"),
    ("raquo", "»"),
    ("bull", "•"),
    ("middot", "·"),
    ("copy", "©"),
    ("reg", "®"),
    ("trade", "™"),
    ("times", "×"),
    ("deg", "°"),
    ("euro", "€"),
    ("pound", "£"),
    ("shy", "\u{ad}"),
    ("zwj", "\u{200d}"),
    ("zwnj", "\u{200c}"),
    ("thinsp", "\u{2009}"),
    ("ensp", "\u{2002}"),
    ("emsp", "\u{2003}"),
    ("eacute", "é"),
    ("egrave", "è"),
    ("aacute", "á"),
    ("agrave", "à"),
    ("uuml", "ü"),
    ("ouml", "ö"),
    ("auml", "ä"),
    ("szlig", "ß"),
    ("ccedil", "ç"),
    ("ntilde", "ñ"),
];

/// Decodes named (common subset) and numeric character references.
pub fn decode_entities(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(p) = rest.find('&') {
        out.push_str(&rest[..p]);
        rest = &rest[p..];
        let end = rest[1..]
            .char_indices()
            .take(32)
            .find(|(_, c)| !c.is_ascii_alphanumeric() && *c != '#')
            .map(|(i, _)| i + 1);
        let (name, consumed) = match end {
            Some(e) if rest[e..].starts_with(';') => (&rest[1..e], e + 1),
            Some(e) => (&rest[1..e], e),
            None => (&rest[1..], rest.len()),
        };
        let decoded = if let Some(num) = name.strip_prefix('#') {
            let code = match num.strip_prefix(['x', 'X']) {
                Some(hex) => u32::from_str_radix(hex, 16).ok(),
                None => num.parse::<u32>().ok(),
            };
            code.map(|c| char::from_u32(c).unwrap_or('\u{fffd}').to_string())
        } else {
            ENTITIES
                .iter()
                .find(|(n, _)| *n == name)
                .map(|(_, v)| v.to_string())
        };
        match decoded {
            Some(d) if !name.is_empty() => {
                out.push_str(&d);
                rest = &rest[consumed..];
            }
            _ => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic_tree() {
        let d = Document::parse("<html><body><p class=a id='x'>Hi <b>there</b></p></body></html>");
        let p = d.find("p").unwrap();
        assert_eq!(d.attr(p, "class"), Some("a"));
        assert_eq!(d.attr(p, "id"), Some("x"));
        assert_eq!(d.text(p), "Hi there");
        assert!(d.is(d.parent(p).unwrap(), "body"));
    }

    #[test]
    fn implied_ends_and_voids() {
        let d = Document::parse("<ul><li>a<li>b</ul><p>one<p>two<br>three<img src=x.png>");
        let lis = d.find_all("li");
        assert_eq!(lis.len(), 2);
        assert_eq!(d.parent(lis[1]), d.find("ul"));
        let ps = d.find_all("p");
        assert_eq!(ps.len(), 2);
        assert_eq!(d.text(ps[0]), "one");
        assert_eq!(d.text(ps[1]), "twothree");
        assert_eq!(d.parent(d.find("img").unwrap()), Some(ps[1]));
        let d = Document::parse("<p>para<div>block</div>");
        assert_eq!(d.parent(d.find("div").unwrap()), Some(Document::ROOT));
    }

    #[test]
    fn raw_text_and_comments() {
        let d = Document::parse(
            "<title>A &amp; B</title><script>if (a < b) { x = '</p>'; }</script><!-- <p>no</p> --><p>yes</p>",
        );
        assert_eq!(d.text(d.find("title").unwrap()), "A & B");
        assert_eq!(d.find_all("p").len(), 1);
        assert_eq!(d.text(d.find("script").unwrap()), "");
        assert!(d.text(0).contains("yes"));
    }

    #[test]
    fn stray_end_tags_and_garbage() {
        let d = Document::parse("</div><p>a</span>b</p>< c <<d> &unknown; &#x41;&#66;&lt");
        assert_eq!(d.text(d.find("p").unwrap()), "ab");
        let t = d.text(0);
        assert!(t.contains("< c <"));
        assert!(t.contains("&unknown; AB<"));
        // Unterminated constructs do not panic.
        Document::parse("<p class=\"unterminated");
        Document::parse("<!-- open");
        Document::parse("<script>never closed");
        Document::parse("<a href=x");
    }

    #[test]
    fn entities() {
        assert_eq!(decode_entities("a&nbsp;b &mdash; &#8212; &#x2014;"), "a\u{a0}b — — —");
        assert_eq!(decode_entities("AT&T & co"), "AT&T & co");
        assert_eq!(decode_entities("&amp"), "&");
    }

    #[test]
    fn non_ascii_text() {
        let d = Document::parse("<p>Grüße — 日本語</p>");
        assert_eq!(d.text(d.find("p").unwrap()), "Grüße — 日本語");
    }
}
