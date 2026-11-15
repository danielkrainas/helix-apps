//! Reader mode: a readability-style article extractor and the clean page
//! it is shown in.
//!
//! Algorithm (classic "content scoring" heuristic, written from its general
//! description): drop non-content elements and blocks whose class/id look
//! like page furniture, score every paragraph-like block by its text length
//! and commas, propagate the score to its ancestors, discount candidates by
//! link density, take the best candidate plus related siblings, then
//! serialise that subtree through a strict tag/attribute allow-list.

use crate::htmlparse::{Document, NodeId, NodeKind};
use std::collections::HashMap;

/// Minimum amount of article text for reader mode to be offered.
pub const READERABLE_CHARS: usize = 400;
/// Pages larger than this are not analysed.
pub const MAX_HTML_BYTES: usize = 4 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Article {
    pub title: String,
    pub byline: Option<String>,
    pub site_name: Option<String>,
    /// Sanitised article HTML.
    pub content: String,
    /// Length of the article text in characters.
    pub text_length: usize,
}

impl Article {
    pub fn is_readerable(&self) -> bool {
        self.text_length >= READERABLE_CHARS
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FontStyle {
    Sans,
    Serif,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorScheme {
    Light,
    Dark,
}

const REMOVE_TAGS: &[&str] = &[
    "script", "style", "noscript", "template", "iframe", "form", "button", "input", "select",
    "textarea", "svg", "canvas", "object", "embed", "nav", "footer", "aside", "dialog", "link",
    "meta", "head",
];

const UNLIKELY: &[&str] = &[
    "banner",
    "breadcrumb",
    "combx",
    "comment",
    "community",
    "cookie",
    "cover-wrap",
    "disqus",
    "extra",
    "footer",
    "gdpr",
    "header",
    "legends",
    "menu",
    "related",
    "remark",
    "replies",
    "rss",
    "shoutbox",
    "sidebar",
    "skyscraper",
    "social",
    "sponsor",
    "supplemental",
    "ad-break",
    "agegate",
    "pagination",
    "pager",
    "popup",
    "yom-remote",
    "share",
    "newsletter",
    "subscribe",
    "promo",
    "navbar",
    "masthead",
];
const MAYBE: &[&str] = &[
    "and", "article", "body", "column", "content", "main", "shadow", "story", "post", "text",
    "entry",
];
const POSITIVE: &[&str] = &[
    "article",
    "body",
    "content",
    "entry",
    "hentry",
    "h-entry",
    "main",
    "page",
    "pagination",
    "post",
    "text",
    "blog",
    "story",
    "prose",
];
const NEGATIVE: &[&str] = &[
    "-ad-",
    "hidden",
    "banner",
    "combx",
    "comment",
    "com-",
    "contact",
    "foot",
    "footer",
    "footnote",
    "gdpr",
    "masthead",
    "media",
    "meta",
    "outbrain",
    "promo",
    "related",
    "scroll",
    "share",
    "shoutbox",
    "sidebar",
    "skyscraper",
    "sponsor",
    "shopping",
    "tags",
    "tool",
    "widget",
    "byline",
    "author",
    "dateline",
];

const BLOCK_CHILD_TAGS: &[&str] = &[
    "a",
    "blockquote",
    "dl",
    "div",
    "img",
    "ol",
    "p",
    "pre",
    "table",
    "ul",
    "section",
    "article",
    "figure",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
];

fn class_id(doc: &Document, n: NodeId) -> String {
    let mut s = String::new();
    if let Some(c) = doc.attr(n, "class") {
        s.push_str(c);
    }
    s.push(' ');
    if let Some(i) = doc.attr(n, "id") {
        s.push_str(i);
    }
    s.to_ascii_lowercase()
}

fn matches_any(hay: &str, needles: &[&str]) -> bool {
    needles.iter().any(|n| hay.contains(n))
}

fn class_weight(doc: &Document, n: NodeId) -> f64 {
    let ci = class_id(doc, n);
    let mut w = 0.0;
    if matches_any(&ci, NEGATIVE) {
        w -= 25.0;
    }
    if matches_any(&ci, POSITIVE) {
        w += 25.0;
    }
    w
}

fn base_score(doc: &Document, n: NodeId) -> f64 {
    let tag_score = match doc.tag(n).unwrap_or("") {
        "div" | "article" | "section" | "main" => 5.0,
        "pre" | "td" | "blockquote" => 3.0,
        "address" | "ol" | "ul" | "dl" | "dd" | "dt" | "li" | "form" => -3.0,
        "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "th" => -5.0,
        _ => 0.0,
    };
    tag_score + class_weight(doc, n)
}

fn collapse_ws(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

struct Extractor<'a> {
    doc: &'a Document,
    removed: Vec<bool>,
    base: Option<url::Url>,
}

impl Extractor<'_> {
    fn is_removed(&self, n: NodeId) -> bool {
        self.removed[n]
    }

    fn text(&self, n: NodeId) -> String {
        let mut s = String::new();
        self.doc.collect_text(n, &mut s, &|x| self.removed[x]);
        s
    }

    fn text_len(&self, n: NodeId) -> usize {
        collapse_ws(&self.text(n)).chars().count()
    }

    fn link_density(&self, n: NodeId) -> f64 {
        let total = self.text_len(n);
        if total == 0 {
            return 0.0;
        }
        let links: usize = self
            .doc
            .descendants(n)
            .into_iter()
            .filter(|&d| self.doc.is(d, "a") && !self.is_removed(d))
            .map(|a| self.text_len(a))
            .sum();
        links as f64 / total as f64
    }

    fn mark_removed(&mut self) {
        let all = self.doc.descendants(Document::ROOT);
        for n in all {
            let Some(tag) = self.doc.tag(n) else { continue };
            if self.removed[n] {
                continue;
            }
            let hidden = self.doc.attr(n, "hidden").is_some()
                || self.doc.attr(n, "aria-hidden") == Some("true")
                || self.doc.attr(n, "style").is_some_and(|s| {
                    s.replace(' ', "")
                        .to_ascii_lowercase()
                        .contains("display:none")
                });
            let unlikely = !matches!(
                tag,
                "body"
                    | "html"
                    | "article"
                    | "main"
                    | "a"
                    | "table"
                    | "tbody"
                    | "tr"
                    | "td"
                    | "th"
                    | "code"
                    | "pre"
            ) && {
                let ci = class_id(self.doc, n);
                let role = self.doc.attr(n, "role").unwrap_or("");
                (matches_any(&ci, UNLIKELY) && !matches_any(&ci, MAYBE))
                    || matches!(
                        role,
                        "menu"
                            | "menubar"
                            | "complementary"
                            | "navigation"
                            | "alert"
                            | "alertdialog"
                            | "dialog"
                    )
            };
            // Headers at the top of an article usually carry its title and
            // byline; page-level <header> elements are page furniture.
            let page_header = tag == "header" && !self.has_ancestor(n, "article");
            if REMOVE_TAGS.contains(&tag) || hidden || unlikely || page_header {
                self.remove_subtree(n);
            }
        }
    }

    fn has_ancestor(&self, n: NodeId, tag: &str) -> bool {
        let mut p = self.doc.parent(n);
        while let Some(x) = p {
            if self.doc.is(x, tag) {
                return true;
            }
            p = self.doc.parent(x);
        }
        false
    }

    fn remove_subtree(&mut self, n: NodeId) {
        self.removed[n] = true;
        for d in self.doc.descendants(n) {
            self.removed[d] = true;
        }
    }

    /// Paragraph-like nodes whose text is scored.
    fn scorable(&self, root: NodeId) -> Vec<NodeId> {
        self.doc
            .descendants(root)
            .into_iter()
            .filter(|&n| !self.is_removed(n))
            .filter(|&n| match self.doc.tag(n) {
                Some("p" | "pre" | "td" | "blockquote" | "li" | "h2" | "h3") => true,
                // A div without block children acts as a paragraph.
                Some("div" | "section") => !self.doc.children(n).iter().any(|&c| {
                    self.doc
                        .tag(c)
                        .is_some_and(|t| BLOCK_CHILD_TAGS.contains(&t))
                }),
                _ => false,
            })
            .collect()
    }

    fn top_candidate(&self, root: NodeId) -> (NodeId, HashMap<NodeId, f64>) {
        let mut scores: HashMap<NodeId, f64> = HashMap::new();
        for p in self.scorable(root) {
            let text = collapse_ws(&self.text(p));
            let len = text.chars().count();
            if len < 25 {
                continue;
            }
            let commas = text.matches([',', '，', '、']).count() as f64;
            let score = 1.0 + commas + (len as f64 / 100.0).min(3.0);
            let mut ancestor = self.doc.parent(p);
            let mut level = 0;
            while let Some(a) = ancestor {
                if a == Document::ROOT || level > 2 {
                    break;
                }
                let divider = match level {
                    0 => 1.0,
                    1 => 2.0,
                    l => l as f64 * 3.0,
                };
                let entry = scores.entry(a).or_insert_with(|| base_score(self.doc, a));
                *entry += score / divider;
                ancestor = self.doc.parent(a);
                level += 1;
            }
        }
        for (n, s) in scores.iter_mut() {
            *s *= 1.0 - self.link_density(*n);
        }
        let top = scores
            .iter()
            .max_by(|a, b| {
                a.1.partial_cmp(b.1)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then(b.0.cmp(a.0))
            })
            .map(|(n, _)| *n)
            .unwrap_or(root);
        (top, scores)
    }

    /// The top candidate plus siblings that look like part of the article.
    fn gather(&self, top: NodeId, scores: &HashMap<NodeId, f64>) -> Vec<NodeId> {
        let Some(parent) = self.doc.parent(top).filter(|&p| p != Document::ROOT) else {
            return vec![top];
        };
        let top_score = scores.get(&top).copied().unwrap_or(0.0);
        let threshold = (top_score * 0.2).max(10.0);
        let top_class = self.doc.attr(top, "class").unwrap_or("");
        self.doc
            .children(parent)
            .iter()
            .copied()
            .filter(|&s| !self.is_removed(s))
            .filter(|&s| {
                if s == top {
                    return true;
                }
                if self.doc.tag(s).is_none() {
                    return false;
                }
                let mut bonus = 0.0;
                if !top_class.is_empty() && self.doc.attr(s, "class") == Some(top_class) {
                    bonus += top_score * 0.2;
                }
                if scores.get(&s).is_some_and(|sc| sc + bonus >= threshold) {
                    return true;
                }
                if self.doc.is(s, "p") {
                    let text = collapse_ws(&self.text(s));
                    let len = text.chars().count();
                    let ld = self.link_density(s);
                    return (len > 80 && ld < 0.25)
                        || (len > 0 && len <= 80 && ld == 0.0 && text.contains(". "));
                }
                false
            })
            .collect()
    }

    fn absolutize(&self, link: &str) -> Option<String> {
        let link = link.trim();
        if link.is_empty() {
            return None;
        }
        let resolved = match &self.base {
            Some(b) => b.join(link).ok()?.to_string(),
            None => url::Url::parse(link).ok()?.to_string(),
        };
        let lower = resolved.to_ascii_lowercase();
        (lower.starts_with("http://")
            || lower.starts_with("https://")
            || lower.starts_with("mailto:")
            || lower.starts_with("data:image/"))
        .then_some(resolved)
    }

    /// Drops link-heavy or empty containers inside the article.
    fn is_junk(&self, n: NodeId) -> bool {
        let Some(tag) = self.doc.tag(n) else {
            return false;
        };
        match tag {
            "ul" | "ol" | "div" | "section" | "table" | "dl" => {
                let len = self.text_len(n);
                let images = self
                    .doc
                    .descendants(n)
                    .into_iter()
                    .filter(|&d| self.doc.is(d, "img") && !self.is_removed(d))
                    .count();
                if len == 0 && images == 0 {
                    return true;
                }
                let ld = self.link_density(n);
                let list = matches!(tag, "ul" | "ol" | "dl");
                (ld > 0.5 && (list || len < 300)) || (class_weight(self.doc, n) < 0.0 && len < 200)
            }
            _ => false,
        }
    }

    fn serialize(&self, n: NodeId, out: &mut String, title: &str) {
        if self.is_removed(n) {
            return;
        }
        match &self.doc.nodes[n].kind {
            NodeKind::Root => self.serialize_children(n, out, title),
            NodeKind::Text(t) => out.push_str(&escape(t)),
            NodeKind::Element { tag, .. } => {
                if self.is_junk(n) {
                    return;
                }
                let tag = tag.as_str();
                match tag {
                    "p" | "h2" | "h3" | "h4" | "h5" | "h6" | "ul" | "ol" | "li" | "blockquote"
                    | "pre" | "code" | "em" | "strong" | "b" | "i" | "u" | "s" | "sub" | "sup"
                    | "figure" | "figcaption" | "table" | "thead" | "tbody" | "tfoot" | "tr"
                    | "caption" | "dl" | "dt" | "dd" | "kbd" | "samp" | "var" | "mark" | "abbr"
                    | "q" | "cite" | "small" | "del" | "ins" | "time" => {
                        out.push('<');
                        out.push_str(tag);
                        out.push('>');
                        self.serialize_children(n, out, title);
                        out.push_str("</");
                        out.push_str(tag);
                        out.push('>');
                    }
                    "h1" => {
                        // The title is shown separately.
                        let text = collapse_ws(&self.text(n));
                        if text != title {
                            out.push_str("<h2>");
                            self.serialize_children(n, out, title);
                            out.push_str("</h2>");
                        }
                    }
                    "td" | "th" => {
                        out.push('<');
                        out.push_str(tag);
                        for a in ["colspan", "rowspan"] {
                            if let Some(v) = self
                                .doc
                                .attr(n, a)
                                .filter(|v| v.chars().all(|c| c.is_ascii_digit()) && !v.is_empty())
                            {
                                out.push_str(&format!(" {a}=\"{v}\""));
                            }
                        }
                        out.push('>');
                        self.serialize_children(n, out, title);
                        out.push_str("</");
                        out.push_str(tag);
                        out.push('>');
                    }
                    "br" => out.push_str("<br>"),
                    "hr" => out.push_str("<hr>"),
                    "a" => match self.doc.attr(n, "href").and_then(|h| self.absolutize(h)) {
                        Some(href) if !href.starts_with("data:") => {
                            out.push_str(&format!("<a href=\"{}\">", escape_attr(&href)));
                            self.serialize_children(n, out, title);
                            out.push_str("</a>");
                        }
                        _ => self.serialize_children(n, out, title),
                    },
                    "img" => {
                        let src = ["src", "data-src", "data-original"]
                            .iter()
                            .filter_map(|a| self.doc.attr(n, a))
                            .find(|s| !s.trim().is_empty() && !s.starts_with("data:image/gif"))
                            .and_then(|s| self.absolutize(s));
                        if let Some(src) = src {
                            let alt = self.doc.attr(n, "alt").unwrap_or("");
                            out.push_str(&format!(
                                "<img src=\"{}\" alt=\"{}\">",
                                escape_attr(&src),
                                escape_attr(alt)
                            ));
                        }
                    }
                    "div" | "section" | "article" | "main" | "header" => {
                        let blocky = self.doc.children(n).iter().any(|&c| {
                            self.doc.tag(c).is_some_and(|t| {
                                BLOCK_CHILD_TAGS.contains(&t) && t != "a" && t != "img"
                            })
                        });
                        if blocky {
                            self.serialize_children(n, out, title);
                        } else if !collapse_ws(&self.text(n)).is_empty()
                            || self
                                .doc
                                .descendants(n)
                                .iter()
                                .any(|&d| self.doc.is(d, "img"))
                        {
                            out.push_str("<p>");
                            self.serialize_children(n, out, title);
                            out.push_str("</p>");
                        }
                    }
                    // Unknown inline wrappers (span, font, picture…): keep content.
                    _ => self.serialize_children(n, out, title),
                }
            }
        }
    }

    fn serialize_children(&self, n: NodeId, out: &mut String, title: &str) {
        for &c in self.doc.children(n) {
            self.serialize(c, out, title);
        }
    }
}

pub fn escape(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => o.push_str("&amp;"),
            '<' => o.push_str("&lt;"),
            '>' => o.push_str("&gt;"),
            _ => o.push(c),
        }
    }
    o
}

pub fn escape_attr(s: &str) -> String {
    escape(s).replace('"', "&quot;").replace('\'', "&#39;")
}

fn meta(doc: &Document, keys: &[&str]) -> Option<String> {
    for m in doc.find_all("meta") {
        let key = doc
            .attr(m, "property")
            .or_else(|| doc.attr(m, "name"))
            .unwrap_or("")
            .to_ascii_lowercase();
        if keys.contains(&key.as_str()) {
            if let Some(c) = doc
                .attr(m, "content")
                .map(collapse_ws)
                .filter(|c| !c.is_empty())
            {
                return Some(c);
            }
        }
    }
    None
}

/// Strips a " | Site" / " - Site" suffix when the remaining title is
/// substantial.
pub fn clean_title(raw: &str) -> String {
    let raw = collapse_ws(raw);
    for sep in [" | ", " – ", " — ", " - ", " :: ", " » "] {
        if let Some(idx) = raw.rfind(sep) {
            let head = raw[..idx].trim();
            if head.split_whitespace().count() >= 3 {
                return head.to_string();
            }
        }
    }
    raw
}

/// Extracts the main article of `html`. `base_url` resolves relative links.
pub fn extract(html: &str, base_url: &str) -> Option<Article> {
    if html.len() > MAX_HTML_BYTES {
        return None;
    }
    let doc = Document::parse(html);
    let base_href = doc.find("base").and_then(|b| doc.attr(b, "href"));
    let page = url::Url::parse(base_url).ok();
    let base = match (base_href, &page) {
        (Some(h), Some(p)) => p.join(h).ok().or(page.clone()),
        _ => page,
    };

    let title = meta(&doc, &["og:title", "twitter:title"])
        .or_else(|| doc.find("title").map(|t| clean_title(&doc.text(t))))
        .filter(|t| !t.is_empty())
        .or_else(|| doc.find("h1").map(|h| collapse_ws(&doc.text(h))))
        .unwrap_or_default();
    let mut byline = meta(&doc, &["author", "article:author", "byl"]);
    if byline.as_deref().is_some_and(|b| b.starts_with("http")) {
        byline = None;
    }
    if byline.is_none() {
        byline = doc
            .descendants(Document::ROOT)
            .into_iter()
            .find(|&n| {
                doc.attr(n, "rel") == Some("author")
                    || doc.attr(n, "itemprop") == Some("author")
                    || class_id(&doc, n)
                        .split_whitespace()
                        .any(|c| c == "byline" || c == "author")
            })
            .map(|n| collapse_ws(&doc.text(n)))
            .filter(|b| !b.is_empty() && b.chars().count() < 100);
    }
    let site_name = meta(&doc, &["og:site_name", "application-name"]);

    let mut ex = Extractor {
        doc: &doc,
        removed: vec![false; doc.nodes.len()],
        base,
    };
    ex.mark_removed();
    let root = doc.find("body").unwrap_or(Document::ROOT);
    let (top, scores) = ex.top_candidate(root);
    let parts = ex.gather(top, &scores);

    let mut content = String::new();
    for p in &parts {
        ex.serialize(*p, &mut content, &title);
    }
    // Link text does not count as article text.
    let text_length: usize = parts
        .iter()
        .map(|&p| (ex.text_len(p) as f64 * (1.0 - ex.link_density(p))).round() as usize)
        .sum();
    if text_length == 0 {
        return None;
    }
    Some(Article {
        title,
        byline,
        site_name,
        content,
        text_length,
    })
}

const READER_CSS: &str = r#"
:root { color-scheme: light; --fg: #1e1e1e; --bg: #fafafa; --muted: #5e5c64; --link: #1c71d8; --rule: #deddda; --code: #f0f0f0; }
:root.dark { color-scheme: dark; --fg: #e8e8e8; --bg: #1e1e1e; --muted: #a8a8a8; --link: #78aeed; --rule: #3d3d3d; --code: #2a2a2a; }
:root.hc { --fg: #000000; --bg: #ffffff; --muted: #1e1e1e; --link: #0b4fa8; --rule: #1e1e1e; }
:root.hc.dark { --fg: #ffffff; --bg: #000000; --muted: #e8e8e8; --link: #a8cdff; --rule: #e8e8e8; }
html { background: var(--bg); color: var(--fg); }
body { margin: 0 auto; max-width: 42em; padding: 2.5em 1.25em 4em; line-height: 1.6; font-size: 1.15rem; }
body.sans { font-family: "Adwaita Sans", "Cantarell", "Inter", system-ui, sans-serif; }
body.serif { font-family: "Noto Serif", "DejaVu Serif", Georgia, serif; }
header.reader { border-bottom: 1px solid var(--rule); margin-bottom: 1.5em; padding-bottom: 1em; }
header.reader h1 { font-size: 2em; line-height: 1.2; margin: 0 0 .4em; }
header.reader .meta { color: var(--muted); font-size: .9em; }
header.reader .meta a { color: var(--muted); }
a { color: var(--link); }
img, video, figure { max-width: 100%; height: auto; }
figure { margin: 1.5em 0; }
figcaption { color: var(--muted); font-size: .85em; }
pre, code { font-family: "Adwaita Mono", "Source Code Pro", monospace; font-size: .9em; background: var(--code); border-radius: 4px; }
pre { padding: .8em; overflow-x: auto; }
pre code { background: none; }
blockquote { margin: 1em 0; padding-left: 1em; border-left: 3px solid var(--rule); color: var(--muted); }
table { border-collapse: collapse; }
td, th { border: 1px solid var(--rule); padding: .3em .5em; }
h2, h3, h4 { line-height: 1.3; }
"#;

/// The complete reader page. Scripts are blocked by a strict CSP, so the
/// page is inert even if the source contained something unexpected.
/// The reader's palette is its own (it imitates paper, not the window);
/// `high_contrast` switches it to black and white with full-strength
/// secondary text and rules.
pub fn render(
    article: &Article,
    source_url: &str,
    font: FontStyle,
    colors: ColorScheme,
    high_contrast: bool,
) -> String {
    let host = url::Url::parse(source_url)
        .ok()
        .and_then(|u| u.host_str().map(str::to_string))
        .unwrap_or_default();
    let mut meta = Vec::new();
    if let Some(b) = &article.byline {
        meta.push(escape(b));
    }
    let site = article.site_name.clone().unwrap_or(host);
    if !site.is_empty() {
        meta.push(format!(
            "<a href=\"{}\">{}</a>",
            escape_attr(source_url),
            escape(&site)
        ));
    }
    format!(
        "<!DOCTYPE html><html class=\"{scheme}\"><head><meta charset=\"utf-8\">\
         <meta http-equiv=\"Content-Security-Policy\" content=\"default-src 'none'; img-src * data:; media-src *; style-src 'unsafe-inline'\">\
         <meta name=\"viewport\" content=\"width=device-width\">\
         <title>{title}</title><style>{css}</style></head>\
         <body class=\"{font}\"><header class=\"reader\"><h1>{title}</h1><div class=\"meta\">{meta}</div></header>\
         <article>{content}</article></body></html>",
        scheme = match (colors == ColorScheme::Dark, high_contrast) {
            (true, false) => "dark",
            (false, false) => "light",
            (true, true) => "dark hc",
            (false, true) => "light hc",
        },
        title = escape(&article.title),
        css = READER_CSS,
        font = if font == FontStyle::Serif { "serif" } else { "sans" },
        meta = meta.join(" · "),
        content = article.content,
    )
}

/// JavaScript (run in an isolated world) returning the page's HTML.
pub const GET_HTML_JS: &str = "document.documentElement ? document.documentElement.outerHTML : ''";

#[cfg(test)]
mod tests {
    use super::*;

    fn para(n: usize) -> String {
        (0..n)
            .map(|i| format!("Sentence number {i} of the article, with some commas, and words. "))
            .collect()
    }

    fn sample() -> String {
        format!(
            r#"<!doctype html><html><head><title>Rust Reader Mode Works Well | Example News</title>
            <meta name="author" content="Ada Lovelace"><meta property="og:site_name" content="Example News">
            <script>var x = "<p>not content</p>";</script><style>p {{ color: red }}</style></head>
            <body><header><nav><a href="/">Home</a> <a href="/about">About</a></nav></header>
            <div class="sidebar"><p>{side}</p></div>
            <div id="main"><article class="post-content">
              <h1>Rust Reader Mode Works Well</h1>
              <p>{p1}</p><p>{p2} <a href="/more" onclick="evil()">relative link</a></p>
              <img src="/img/photo.jpg" alt="A photo" onerror="evil()">
              <ul class="share-buttons"><li><a href="https://x/">Share</a></li><li><a href="https://y/">Tweet</a></li></ul>
              <pre><code>fn main() {{ println!("&lt;hi&gt;"); }}</code></pre>
              <p style="display:none">{hidden}</p>
              <iframe src="https://ads.example/"></iframe>
            </article></div>
            <footer><p>{foot}</p></footer>
            <div class="comments"><p>{comment}</p></div></body></html>"#,
            side = para(3).replace("article", "sidebar"),
            p1 = para(6),
            p2 = para(5),
            hidden = "HIDDEN TEXT ".repeat(20),
            foot = "Footer text, copyright, and more. ".repeat(10),
            comment = "Comment text, I disagree, strongly. ".repeat(10),
        )
    }

    #[test]
    fn extracts_main_article() {
        let a = extract(&sample(), "https://news.example/2024/story.html").unwrap();
        assert_eq!(a.title, "Rust Reader Mode Works Well");
        assert_eq!(a.byline.as_deref(), Some("Ada Lovelace"));
        assert_eq!(a.site_name.as_deref(), Some("Example News"));
        assert!(a.is_readerable());
        assert!(a.content.contains("Sentence number 5 of the article"));
        assert!(a
            .content
            .contains("<a href=\"https://news.example/more\">relative link</a>"));
        assert!(a
            .content
            .contains("<img src=\"https://news.example/img/photo.jpg\" alt=\"A photo\">"));
        assert!(
            a.content.contains(
                "<pre><code>fn main() { println!(&quot;&lt;hi&gt;&quot;); }</code></pre>"
            ) || a
                .content
                .contains("<pre><code>fn main() { println!(\"&lt;hi&gt;\"); }</code></pre>")
        );
        for absent in [
            "not content",
            "Footer text",
            "Comment text",
            "HIDDEN",
            "sidebar",
            "Tweet",
            "onclick",
            "onerror",
            "evil",
            "iframe",
            "Home",
            "color: red",
        ] {
            assert!(
                !a.content.contains(absent),
                "{absent} leaked into {}",
                a.content
            );
        }
        // The duplicated h1 is dropped; the title is rendered separately.
        assert!(!a.content.contains("<h1>"));
    }

    #[test]
    fn short_pages_are_not_readerable() {
        let a = extract(
            "<html><body><p>Just a short note here, nothing more.</p></body></html>",
            "https://e/",
        );
        assert!(a.is_none_or(|a| !a.is_readerable()));
        assert!(extract("<html><body></body></html>", "https://e/").is_none());
        assert!(extract("", "https://e/").is_none());
    }

    #[test]
    fn link_lists_are_not_articles() {
        let links: String = (0..40)
            .map(|i| {
                format!("<li><a href=\"/p{i}\">A fairly long link title number {i}, see</a></li>")
            })
            .collect();
        let html = format!("<body><div class=\"content\"><ul>{links}</ul></div></body>");
        let a = extract(&html, "https://e/");
        assert!(a.is_none_or(|a| !a.is_readerable()));
    }

    #[test]
    fn sanitizes_urls_and_markup() {
        let html = format!(
            "<body><article><p>{} <a href=\"javascript:alert(1)\">bad</a> <img src=\"javascript:x\"> \
             <b onmouseover=\"x()\">bold</b> &lt;script&gt;</p></article></body>",
            para(8)
        );
        let a = extract(&html, "https://e/").unwrap();
        assert!(!a.content.contains("javascript"));
        assert!(!a.content.contains("onmouseover"));
        assert!(a.content.contains("<b>bold</b>"));
        assert!(a.content.contains("&lt;script&gt;"));
        assert!(!a.content.contains("<script"));
    }

    #[test]
    fn titles() {
        assert_eq!(
            clean_title("A Long Story About Things - Site"),
            "A Long Story About Things"
        );
        assert_eq!(clean_title("Short | Site"), "Short | Site");
        assert_eq!(clean_title("  Plain   title "), "Plain title");
    }

    #[test]
    fn render_is_escaped_and_styled() {
        let a = Article {
            title: "<Title>".into(),
            byline: Some("Me & You".into()),
            site_name: None,
            content: "<p>x</p>".into(),
            text_length: 1,
        };
        let html = render(
            &a,
            "https://site.example/a?b=\"c\"",
            FontStyle::Serif,
            ColorScheme::Dark,
            false,
        );
        assert!(html.contains("<h1>&lt;Title&gt;</h1>"));
        assert!(html.contains("Me &amp; You"));
        assert!(html.contains("site.example</a>"));
        assert!(html.contains("&quot;c&quot;"));
        assert!(html.contains("class=\"serif\""));
        assert!(html.contains("<html class=\"dark\">"));
        assert!(html.contains("default-src 'none'"));
    }
}
