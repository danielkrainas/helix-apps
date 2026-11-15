//! Mallard (projectmallard.org) to HTML transformer.

use crate::html::{self, escape, escape_attr, LinkItem};
use crate::search;
use crate::uri::DocRef;
use crate::xmlutil;
use helix_app_common::i18n::tr;
use helix_app_common::trf;
use roxmltree::Node;
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

pub const IF_NS: &str = "http://projectmallard.org/if/1.0/";

/// Tokens that are true for `if:test` conditionals.
const TRUE_TOKENS: &[&str] = &[
    "target:html",
    "target:help",
    "platform:gnome",
    "platform:gnome-shell",
    "platform:linux",
    "platform:helix",
];

/// Evaluates an `if:test` expression: comma-separated alternatives (OR),
/// each a whitespace-separated list of tokens (AND), `!` negates a token.
/// Unknown tokens are false.
pub fn test_true(expr: &str) -> bool {
    fn token(t: &str) -> bool {
        match t.strip_prefix('!') {
            Some(n) => !token(n),
            None => TRUE_TOKENS.contains(&t),
        }
    }
    if expr.trim().is_empty() {
        return true;
    }
    expr.split(',').any(|alt| {
        let mut toks = alt.split_whitespace().peekable();
        toks.peek().is_some() && toks.all(token)
    })
}

fn visible(n: Node) -> bool {
    n.attribute((IF_NS, "test")).map(test_true).unwrap_or(true)
}

fn is_if(n: Node, name: &str) -> bool {
    n.is_element() && n.tag_name().namespace() == Some(IF_NS) && n.tag_name().name() == name
}

fn name(n: Node) -> &'static str {
    // Maps a Mallard element to a static name for matching; unknown
    // elements map to "" and are rendered as their children.
    const KNOWN: &[&str] = &[
        "page", "section", "info", "title", "subtitle", "desc", "p", "list", "steps", "terms",
        "tree", "item", "code", "screen", "synopsis", "example", "figure", "listing", "quote",
        "cite", "note", "media", "table", "thead", "tbody", "tfoot", "tr", "td", "th", "col",
        "colgroup", "comment", "links", "link", "gui", "guiseq", "key", "keyseq", "menu", "app",
        "cmd", "sys", "file", "input", "output", "var", "em", "span", "hi", "date", "name",
        "email", "credit", "license", "revision", "include", "fallback", "if", "when", "else",
        "choose", "br", "years", "div", "sym",
    ];
    let n = n.tag_name().name();
    KNOWN.iter().find(|k| **k == n).copied().unwrap_or("")
}

/// A link target: a page or a section with an id.
#[derive(Debug, Clone, Default)]
pub struct Target {
    /// `page` or `page#section`.
    pub xref: String,
    pub title: String,
    pub sort_title: String,
    pub desc: String,
    pub links: Vec<InfoLink>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InfoLink {
    pub kind: String,
    /// Normalised: `page` or `page#section`.
    pub xref: String,
    pub group: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Page {
    pub id: String,
    pub path: PathBuf,
    pub meta: Target,
    pub sections: Vec<Target>,
    pub body: String,
    pub source: String,
}

#[derive(Debug, Default)]
pub struct MallardDoc {
    pub dir: PathBuf,
    pub pages: BTreeMap<String, Page>,
}

fn normalize_xref(page_id: &str, xref: &str) -> String {
    match xref.strip_prefix('#') {
        Some(s) => format!("{page_id}#{s}"),
        None => xref.to_string(),
    }
}

fn extract_target(n: Node, page_id: &str, xref: String) -> Target {
    let info = xmlutil::child(n, "info");
    let info_title = |ty: &str| {
        info.and_then(|i| {
            i.children()
                .find(|c| {
                    c.is_element()
                        && c.tag_name().name() == "title"
                        && c.attribute("type") == Some(ty)
                        && visible(*c)
                })
                .map(xmlutil::text_of)
        })
    };
    let main_title = xmlutil::child(n, "title")
        .map(xmlutil::text_of)
        .unwrap_or_default();
    let title = info_title("link")
        .filter(|t| !t.is_empty())
        .unwrap_or_else(|| main_title.clone());
    let sort_title = info_title("sort").unwrap_or_else(|| title.clone());
    let desc = info
        .and_then(|i| xmlutil::child(i, "desc"))
        .map(xmlutil::text_of)
        .unwrap_or_default();
    let links = info
        .map(|i| {
            xmlutil::children(i, "link")
                .filter(|l| visible(*l))
                .filter_map(|l| {
                    Some(InfoLink {
                        kind: l.attribute("type")?.to_string(),
                        xref: normalize_xref(page_id, l.attribute("xref")?),
                        group: l.attribute("group").map(str::to_string),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    Target {
        xref,
        title,
        sort_title,
        desc,
        links,
    }
}

fn body_text(root: Node) -> String {
    let mut s = String::new();
    for d in root.descendants().filter(|d| d.is_text()) {
        if d.ancestors()
            .any(|a| a.is_element() && matches!(a.tag_name().name(), "info" | "comment"))
        {
            continue;
        }
        s.push_str(d.text().unwrap_or(""));
        s.push(' ');
    }
    xmlutil::collapse_ws(&s)
}

/// Extracts a page's metadata from flattened source.
pub fn parse_page(source: String, path: PathBuf) -> Result<Page, String> {
    let doc = xmlutil::parse(&source).map_err(|e| format!("{}: {e}", path.display()))?;
    let root = doc.root_element();
    if root.tag_name().name() != "page" {
        return Err(trf!("{}: not a Mallard page", path.display()));
    }
    let id = root
        .attribute("id")
        .map(str::to_string)
        .or_else(|| path.file_stem().map(|s| s.to_string_lossy().into_owned()))
        .unwrap_or_default();
    let meta = extract_target(root, &id, id.clone());
    let sections = root
        .descendants()
        .filter(|n| n.is_element() && n.tag_name().name() == "section" && visible(*n))
        .filter_map(|s| {
            let sid = s.attribute("id")?;
            Some(extract_target(s, &id, format!("{id}#{sid}")))
        })
        .collect();
    let body = body_text(root);
    drop(doc);
    Ok(Page {
        id,
        path,
        meta,
        sections,
        body,
        source,
    })
}

impl MallardDoc {
    /// Loads every `*.page` file in `dir`.
    pub fn load(dir: &Path) -> Result<MallardDoc, String> {
        let mut doc = MallardDoc {
            dir: dir.to_path_buf(),
            pages: BTreeMap::new(),
        };
        let rd = std::fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let mut files: Vec<PathBuf> = rd
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e == "page"))
            .collect();
        files.sort();
        for f in files {
            match xmlutil::read_flat(&f).and_then(|src| parse_page(src, f.clone())) {
                Ok(p) => {
                    doc.pages.entry(p.id.clone()).or_insert(p);
                }
                Err(e) => eprintln!("helix-help: {e}"),
            }
        }
        if doc.pages.is_empty() {
            return Err(trf!("{}: no Mallard pages", dir.display()));
        }
        Ok(doc)
    }

    /// Reads only `index.page`'s title (for the document list).
    pub fn quick_title(dir: &Path) -> Option<String> {
        let src = xmlutil::read(&dir.join("index.page")).ok()?;
        let doc = xmlutil::parse(&src).ok()?;
        let root = doc.root_element();
        let t = extract_target(root, "index", "index".into());
        let info = xmlutil::child(root, "info");
        // Prefer an explicit text title (used for window titles).
        let text_title = info.and_then(|i| {
            i.children()
                .find(|c| {
                    c.is_element()
                        && c.tag_name().name() == "title"
                        && c.attribute("type") == Some("text")
                })
                .map(xmlutil::text_of)
        });
        Some(text_title.unwrap_or(t.title)).filter(|t| !t.is_empty())
    }

    pub fn title(&self) -> String {
        self.pages
            .get("index")
            .map(|p| p.meta.title.clone())
            .unwrap_or_default()
    }

    pub fn target(&self, xref: &str) -> Option<&Target> {
        match xref.split_once('#') {
            Some((p, _)) => self.pages.get(p)?.sections.iter().find(|s| s.xref == xref),
            None => self.pages.get(xref).map(|p| &p.meta),
        }
    }

    fn all_targets(&self) -> impl Iterator<Item = &Target> {
        self.pages
            .values()
            .flat_map(|p| std::iter::once(&p.meta).chain(p.sections.iter()))
    }

    /// Topic links of a guide page or section (unsorted, de-duplicated),
    /// as `(group, xref)`.
    pub fn topic_links(&self, xref: &str) -> Vec<(Option<String>, String)> {
        let mut out: Vec<(Option<String>, String)> = Vec::new();
        let mut push = |g: Option<String>, x: String| {
            if x != xref && !out.iter().any(|(_, o)| *o == x) {
                out.push((g, x));
            }
        };
        if let Some(t) = self.target(xref) {
            for l in t.links.iter().filter(|l| l.kind == "topic") {
                if self.target(&l.xref).is_some() {
                    push(l.group.clone(), l.xref.clone());
                }
            }
        }
        for t in self.all_targets() {
            for l in t
                .links
                .iter()
                .filter(|l| l.kind == "guide" && l.xref == xref)
            {
                push(l.group.clone(), t.xref.clone());
            }
        }
        out
    }

    /// Guides of a page or section: its own guide links plus every target
    /// listing it as a topic.
    pub fn guide_links(&self, xref: &str) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        if let Some(t) = self.target(xref) {
            for l in t.links.iter().filter(|l| l.kind == "guide") {
                if self.target(&l.xref).is_some() && !out.contains(&l.xref) && l.xref != xref {
                    out.push(l.xref.clone());
                }
            }
        }
        for t in self.all_targets() {
            if t.links.iter().any(|l| l.kind == "topic" && l.xref == xref)
                && !out.contains(&t.xref)
                && t.xref != xref
            {
                out.push(t.xref.clone());
            }
        }
        out
    }

    /// See-also links are reciprocal.
    pub fn seealso_links(&self, xref: &str) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        if let Some(t) = self.target(xref) {
            for l in t.links.iter().filter(|l| l.kind == "seealso") {
                if self.target(&l.xref).is_some() && !out.contains(&l.xref) && l.xref != xref {
                    out.push(l.xref.clone());
                }
            }
        }
        for t in self.all_targets() {
            if t.links
                .iter()
                .any(|l| l.kind == "seealso" && l.xref == xref)
                && !out.contains(&t.xref)
                && t.xref != xref
            {
                out.push(t.xref.clone());
            }
        }
        out
    }

    fn sort_key(&self, xref: &str) -> String {
        self.target(xref)
            .map(|t| t.sort_title.to_lowercase())
            .unwrap_or_default()
    }

    /// Orders links into groups (`first`, the listed groups, `default`,
    /// `last`), sorting by sort title within each group.
    pub fn group_sort(
        &self,
        links: Vec<(Option<String>, String)>,
        groups: Option<&str>,
    ) -> Vec<String> {
        let mut order: Vec<String> = groups
            .unwrap_or("")
            .split_whitespace()
            .map(str::to_string)
            .collect();
        if !order.iter().any(|g| g == "default") {
            order.push("default".into());
        }
        if !order.iter().any(|g| g == "first") {
            order.insert(0, "first".into());
        }
        if !order.iter().any(|g| g == "last") {
            order.push("last".into());
        }
        let mut buckets: Vec<Vec<String>> = vec![Vec::new(); order.len()];
        let default_idx = order.iter().position(|g| g == "default").unwrap_or(0);
        for (g, x) in links {
            let idx = g
                .and_then(|g| order.iter().position(|o| *o == g))
                .unwrap_or(default_idx);
            buckets[idx].push(x);
        }
        let mut out = Vec::new();
        for mut b in buckets {
            b.sort_by_cached_key(|x| (self.sort_key(x), x.clone()));
            out.extend(b);
        }
        out
    }

    fn sorted_by_title(&self, mut xrefs: Vec<String>) -> Vec<String> {
        xrefs.sort_by_cached_key(|x| (self.sort_key(x), x.clone()));
        xrefs
    }

    /// Chain of guide pages from `index` down to (excluding) `page`.
    pub fn trail(&self, page: &str) -> Vec<String> {
        let mut chain = Vec::new();
        let mut seen: HashSet<String> = HashSet::from([page.to_string()]);
        let mut cur = page.to_string();
        while cur != "index" && chain.len() < 12 {
            let guides = self.guide_links(&cur);
            let next = guides
                .iter()
                .map(|g| g.split('#').next().unwrap_or(g).to_string())
                .find(|g| !seen.contains(g));
            match next {
                Some(g) => {
                    seen.insert(g.clone());
                    chain.push(g.clone());
                    cur = g;
                }
                None => break,
            }
        }
        chain.reverse();
        chain
    }

    /// Outline of the document following topic links from `index`:
    /// `(depth, xref, title)`.
    pub fn toc(&self) -> Vec<(usize, String, String)> {
        let mut out = Vec::new();
        let mut seen = HashSet::new();
        if self.pages.contains_key("index") {
            seen.insert("index".to_string());
            self.toc_rec("index", 0, &mut seen, &mut out);
        }
        out
    }

    fn toc_rec(
        &self,
        page: &str,
        depth: usize,
        seen: &mut HashSet<String>,
        out: &mut Vec<(usize, String, String)>,
    ) {
        if depth > 8 {
            return;
        }
        let Some(p) = self.pages.get(page) else {
            return;
        };
        let mut links = self.group_sort(self.topic_links(page), None);
        for s in &p.sections {
            links.extend(self.group_sort(self.topic_links(&s.xref), None));
        }
        for x in links {
            let pid = x.split('#').next().unwrap_or(&x).to_string();
            if !seen.insert(pid.clone()) {
                continue;
            }
            let title = self.target(&x).map(|t| t.title.clone()).unwrap_or_default();
            out.push((depth, pid.clone(), title));
            self.toc_rec(&pid, depth + 1, seen, out);
        }
    }

    pub fn search_entries(&self) -> Vec<search::Entry> {
        self.pages
            .values()
            .map(|p| search::Entry {
                page: Some(p.id.clone()),
                title: p.meta.title.clone(),
                desc: p.meta.desc.clone(),
                body: p.body.clone(),
            })
            .collect()
    }

    fn link_item(&self, docref: &DocRef, xref: &str) -> Option<LinkItem> {
        let t = self.target(xref)?;
        let (page, frag) = match xref.split_once('#') {
            Some((p, s)) => (p, Some(s)),
            None => (xref, None),
        };
        Some(LinkItem {
            uri: page_uri(docref, page, frag),
            title: t.title.clone(),
            desc: Some(t.desc.clone()),
        })
    }

    /// Renders a page to a full HTML document; returns `(title, html)`.
    pub fn render(&self, docref: &DocRef, page_id: &str) -> Option<(String, String)> {
        let page = self.pages.get(page_id)?;
        let doc = xmlutil::parse(&page.source).ok()?;
        let root = doc.root_element();
        let mut r = Renderer {
            doc: self,
            docref,
            page,
            placed: HashSet::new(),
        };
        let mut body = String::new();
        let trail: Vec<(String, String)> = self
            .trail(page_id)
            .iter()
            .filter_map(|p| {
                Some((
                    page_uri(docref, p, None),
                    self.pages.get(p)?.meta.title.clone(),
                ))
            })
            .collect();
        body.push_str(&html::trail(&trail));
        body.push_str(&r.container(root, 1, page_id));
        let seealso = self.sorted_by_title(self.seealso_links(page_id));
        if !r.placed.contains(&format!("seealso:{page_id}")) {
            let items: Vec<LinkItem> = seealso
                .iter()
                .filter_map(|x| self.link_item(docref, x))
                .collect();
            body.push_str(&html::link_list("seealso", Some(tr("See Also")), &items));
        }
        if !r.placed.contains(&format!("guide:{page_id}")) {
            let guides = self.sorted_by_title(self.guide_links(page_id));
            let items: Vec<LinkItem> = guides
                .iter()
                .filter_map(|x| {
                    self.link_item(docref, x)
                        .map(|i| LinkItem { desc: None, ..i })
                })
                .collect();
            body.push_str(&html::link_list(
                "guides",
                Some(tr("More Information")),
                &items,
            ));
        }
        let title = xmlutil::child(root, "title")
            .map(xmlutil::text_of)
            .unwrap_or_else(|| page.meta.title.clone());
        Some((title.clone(), html::page(&title, &body)))
    }
}

fn page_uri(docref: &DocRef, page: &str, frag: Option<&str>) -> String {
    if page == "index" {
        docref.page_uri(None, frag)
    } else {
        docref.page_uri(Some(page), frag)
    }
}

const INLINE_PARENTS: &[&str] = &[
    "p",
    "title",
    "desc",
    "subtitle",
    "cite",
    "em",
    "span",
    "link",
    "gui",
    "guiseq",
    "key",
    "keyseq",
    "menu",
    "app",
    "cmd",
    "sys",
    "file",
    "input",
    "output",
    "var",
    "code",
    "screen",
    "hi",
    "date",
    "name",
    "email",
    "synopsis_inline",
];

fn in_inline_context(n: Node) -> bool {
    let mut p = n.parent();
    // Skip conditional wrappers.
    while let Some(x) = p {
        if x.is_element() && x.tag_name().namespace() == Some(IF_NS) {
            p = x.parent();
        } else {
            break;
        }
    }
    p.map(|p| p.is_element() && INLINE_PARENTS.contains(&p.tag_name().name()))
        .unwrap_or(false)
}

struct Renderer<'d> {
    doc: &'d MallardDoc,
    docref: &'d DocRef,
    page: &'d Page,
    placed: HashSet<String>,
}

impl Renderer<'_> {
    fn resource(&self, src: &str) -> String {
        if src.contains("://") || src.starts_with("help:") || src.starts_with("ghelp:") {
            return src.to_string();
        }
        // Paths are relative to the page file; express them relative to the doc dir.
        let page_dir = self.page.path.parent().unwrap_or(&self.doc.dir);
        let rel = page_dir
            .strip_prefix(&self.doc.dir)
            .unwrap_or(Path::new(""));
        let joined = rel.join(src);
        self.docref.resource_uri(&joined.to_string_lossy())
    }

    /// Renders a page or section: title, content and automatic links.
    fn container(&mut self, n: Node, level: usize, xref: &str) -> String {
        let mut s = String::new();
        let has = |ty: &str| {
            n.children().any(|c| {
                c.is_element() && c.tag_name().name() == "links" && c.attribute("type") == Some(ty)
            })
        };
        let explicit_topic = has("topic");
        for c in n.children() {
            if c.is_element() && c.tag_name().name() == "title" {
                if c.attribute("type").is_none() && visible(c) {
                    let h = level.min(6);
                    s.push_str(&format!("<h{h}>{}</h{h}>", self.inlines(c)));
                }
                continue;
            }
            if c.is_element() && c.tag_name().name() == "subtitle" {
                s.push_str(&format!(
                    "<div class=\"subtitle\">{}</div>",
                    self.inlines(c)
                ));
                continue;
            }
            s.push_str(&self.node(c, level, xref));
        }
        if !explicit_topic {
            let links = self.doc.topic_links(xref);
            if !links.is_empty() {
                s.push_str(&self.topic_list(xref, None, None));
            }
        }
        s
    }

    fn topic_list(&self, xref: &str, groups: Option<&str>, heading: Option<String>) -> String {
        let links = self.doc.group_sort(self.doc.topic_links(xref), groups);
        let items: Vec<LinkItem> = links
            .iter()
            .filter_map(|x| self.doc.link_item(self.docref, x))
            .collect();
        html::link_list("links-topic", heading.as_deref(), &items)
    }

    fn children(&mut self, n: Node, level: usize, xref: &str) -> String {
        n.children().map(|c| self.node(c, level, xref)).collect()
    }

    fn inlines(&mut self, n: Node) -> String {
        let xref = self.page.id.clone();
        n.children().map(|c| self.node(c, 6, &xref)).collect()
    }

    fn block_title(&mut self, n: Node) -> String {
        xmlutil::child(n, "title")
            .filter(|t| visible(*t))
            .map(|t| format!("<div class=\"block-title\">{}</div>", self.inlines(t)))
            .unwrap_or_default()
    }

    fn non_title_children(&mut self, n: Node, level: usize, xref: &str) -> String {
        n.children()
            .filter(|c| {
                !(c.is_element() && matches!(c.tag_name().name(), "title" | "desc" | "info"))
            })
            .map(|c| self.node(c, level, xref))
            .collect()
    }

    fn node(&mut self, n: Node, level: usize, xref: &str) -> String {
        if n.is_text() {
            return escape(n.text().unwrap_or(""));
        }
        if !n.is_element() || !visible(n) {
            return String::new();
        }
        if n.tag_name().namespace() == Some(IF_NS) {
            return match n.tag_name().name() {
                "if" => {
                    if n.attribute("test").map(test_true).unwrap_or(true) {
                        self.children(n, level, xref)
                    } else {
                        String::new()
                    }
                }
                "choose" => {
                    for c in n.children().filter(|c| c.is_element()) {
                        if is_if(c, "when") && c.attribute("test").map(test_true).unwrap_or(false) {
                            return self.children(c, level, xref);
                        }
                        if is_if(c, "else") {
                            return self.children(c, level, xref);
                        }
                    }
                    String::new()
                }
                _ => String::new(),
            };
        }
        let id_attr = n
            .attribute("id")
            .map(|i| format!(" id=\"{}\"", escape_attr(i)))
            .unwrap_or_default();
        match name(n) {
            "info" | "comment" | "col" | "colgroup" | "credit" | "revision" | "license" => {
                String::new()
            }
            "section" => {
                let sxref = n
                    .attribute("id")
                    .map(|i| format!("{}#{i}", self.page.id))
                    .unwrap_or_default();
                let inner = if sxref.is_empty() {
                    let tmp = format!("{}#", self.page.id);
                    self.container(n, level + 1, &tmp)
                } else {
                    self.container(n, level + 1, &sxref)
                };
                format!("<section{id_attr}>{inner}</section>")
            }
            "p" => format!("<p{id_attr}>{}</p>", self.inlines(n)),
            "title" => format!("<div class=\"block-title\">{}</div>", self.inlines(n)),
            "subtitle" => format!("<div class=\"subtitle\">{}</div>", self.inlines(n)),
            "desc" => format!("<div class=\"desc\">{}</div>", self.inlines(n)),
            "list" => {
                let tag = if n.attribute("type") == Some("numbered") {
                    "ol"
                } else {
                    "ul"
                };
                let title = self.block_title(n);
                let items: String = xmlutil::children(n, "item")
                    .filter(|i| visible(*i))
                    .map(|i| format!("<li>{}</li>", self.children(i, level, xref)))
                    .collect();
                format!("<div class=\"list\"{id_attr}>{title}<{tag}>{items}</{tag}></div>")
            }
            "steps" => {
                let title = self.block_title(n);
                let items: String = xmlutil::children(n, "item")
                    .filter(|i| visible(*i))
                    .map(|i| format!("<li>{}</li>", self.children(i, level, xref)))
                    .collect();
                format!("<div class=\"steps\"{id_attr}>{title}<ol>{items}</ol></div>")
            }
            "terms" => {
                let title = self.block_title(n);
                let mut items = String::new();
                for i in xmlutil::children(n, "item").filter(|i| visible(*i)) {
                    for t in xmlutil::children(i, "title") {
                        items.push_str(&format!("<dt>{}</dt>", self.inlines(t)));
                    }
                    let body: String = i
                        .children()
                        .filter(|c| !(c.is_element() && c.tag_name().name() == "title"))
                        .map(|c| self.node(c, level, xref))
                        .collect();
                    items.push_str(&format!("<dd>{body}</dd>"));
                }
                format!(
                    "<div class=\"terms\"{id_attr}>{title}<dl class=\"terms\">{items}</dl></div>"
                )
            }
            "tree" => {
                let title = self.block_title(n);
                format!(
                    "<div class=\"tree\"{id_attr}>{title}<ul class=\"tree\">{}</ul></div>",
                    self.tree_items(n)
                )
            }
            "code" | "screen" if !in_inline_context(n) => {
                let cls = name(n);
                let mut inner = self.inlines(n);
                if inner.starts_with('\n') {
                    inner.remove(0);
                }
                format!("<pre class=\"{cls}\"{id_attr}>{}</pre>", inner.trim_end())
            }
            "code" => format!("<code>{}</code>", self.inlines(n)),
            "screen" => format!("<code class=\"screen\">{}</code>", self.inlines(n)),
            "synopsis" | "example" | "listing" | "figure" => {
                let cls = name(n);
                let title = self.block_title(n);
                let desc = xmlutil::child(n, "desc")
                    .map(|d| format!("<div class=\"desc\">{}</div>", self.inlines(d)))
                    .unwrap_or_default();
                let body = self.non_title_children(n, level, xref);
                format!("<div class=\"{cls}\"{id_attr}>{title}{body}{desc}</div>")
            }
            "quote" => {
                let title = self.block_title(n);
                let cite = xmlutil::child(n, "cite")
                    .map(|c| format!("<div class=\"cite\">\u{2014} {}</div>", self.inlines(c)))
                    .unwrap_or_default();
                let body: String = n
                    .children()
                    .filter(|c| {
                        !(c.is_element() && matches!(c.tag_name().name(), "title" | "cite"))
                    })
                    .map(|c| self.node(c, level, xref))
                    .collect();
                format!("<blockquote{id_attr}>{title}{body}{cite}</blockquote>")
            }
            "note" => {
                let style = n.attribute("style").unwrap_or("");
                let known = [
                    "tip",
                    "important",
                    "warning",
                    "caution",
                    "advanced",
                    "bug",
                    "danger",
                    "sidebar",
                ];
                let cls = style
                    .split_whitespace()
                    .find(|s| known.contains(s))
                    .map(|s| format!(" note-{s}"))
                    .unwrap_or_default();
                let title = self.block_title(n);
                let body = self.non_title_children(n, level, xref);
                format!("<div class=\"note{cls}\"{id_attr}>{title}{body}</div>")
            }
            "media" => self.media(n),
            "table" => self.table(n, level, xref),
            "links" => self.links_element(n, xref),
            "link" => self.link(n),
            "gui" => format!("<span class=\"gui\">{}</span>", self.inlines(n)),
            "menu" => format!("<span class=\"menu\">{}</span>", self.inlines(n)),
            "guiseq" => self.seq(n, "guiseq", "<span class=\"arrow\">\u{25B8}</span>"),
            "keyseq" => {
                let sep = if n.attribute("type") == Some("sequence") {
                    " "
                } else {
                    "<span class=\"plus\">+</span>"
                };
                self.seq(n, "keyseq", sep)
            }
            "key" => format!("<kbd>{}</kbd>", self.inlines(n)),
            "app" => format!("<span class=\"app\">{}</span>", self.inlines(n)),
            "cmd" | "sys" | "file" | "output" | "var" | "input" => {
                format!("<span class=\"{}\">{}</span>", name(n), self.inlines(n))
            }
            "em" => {
                if n.attribute("style").is_some_and(|s| s.contains("strong")) {
                    format!("<strong>{}</strong>", self.inlines(n))
                } else {
                    format!("<em>{}</em>", self.inlines(n))
                }
            }
            "span" | "hi" | "date" | "name" | "sym" | "years" => {
                format!("<span>{}</span>", self.inlines(n))
            }
            "email" => {
                let t = xmlutil::text_of(n);
                format!("<a href=\"mailto:{}\">{}</a>", escape_attr(&t), escape(&t))
            }
            "br" => "<br>".into(),
            "item" => format!(
                "<div class=\"item\">{}</div>",
                self.children(n, level, xref)
            ),
            "thead" | "tbody" | "tfoot" | "tr" | "td" | "th" => self.children(n, level, xref),
            "div" => format!("<div{id_attr}>{}</div>", self.children(n, level, xref)),
            _ => self.children(n, level, xref),
        }
    }

    fn seq(&mut self, n: Node, cls: &str, sep: &str) -> String {
        let elems: Vec<Node> = n
            .children()
            .filter(|c| c.is_element() && visible(*c))
            .collect();
        if elems.is_empty() {
            return format!("<span class=\"{cls}\">{}</span>", self.inlines(n));
        }
        let parts: Vec<String> = elems
            .iter()
            .map(|c| self.node(*c, 6, &self.page.id.clone()))
            .collect();
        format!("<span class=\"{cls}\">{}</span>", parts.join(sep))
    }

    fn tree_items(&mut self, n: Node) -> String {
        let mut s = String::new();
        for i in xmlutil::children(n, "item").filter(|i| visible(*i)) {
            let label: String = i
                .children()
                .filter(|c| !(c.is_element() && c.tag_name().name() == "item"))
                .map(|c| self.node(c, 6, &self.page.id.clone()))
                .collect();
            let sub = if xmlutil::child(i, "item").is_some() {
                format!("<ul>{}</ul>", self.tree_items(i))
            } else {
                String::new()
            };
            s.push_str(&format!("<li>{}{sub}</li>", label.trim()));
        }
        s
    }

    fn media(&mut self, n: Node) -> String {
        let Some(src) = n.attribute("src") else {
            return String::new();
        };
        let uri = escape_attr(&self.resource(src));
        let ty = n.attribute("type").unwrap_or("image");
        let alt = escape_attr(&xmlutil::text_of(n));
        let size = ["width", "height"]
            .iter()
            .filter_map(|a| {
                n.attribute(*a)
                    .map(|v| format!(" {a}=\"{}\"", escape_attr(v)))
            })
            .collect::<String>();
        let inline = in_inline_context(n);
        let el = match ty {
            "video" => format!("<video controls preload=\"metadata\" src=\"{uri}\"{size}></video>"),
            "audio" => format!("<audio controls preload=\"metadata\" src=\"{uri}\"></audio>"),
            _ => {
                let cls = if inline {
                    " class=\"media-inline\""
                } else {
                    ""
                };
                format!("<img src=\"{uri}\" alt=\"{alt}\"{size}{cls}>")
            }
        };
        if inline {
            el
        } else {
            format!("<div class=\"media-block\">{el}</div>")
        }
    }

    fn table(&mut self, n: Node, level: usize, xref: &str) -> String {
        let mut s = String::from("<table>");
        if let Some(t) = xmlutil::child(n, "title") {
            s.push_str(&format!("<caption>{}</caption>", self.inlines(t)));
        }
        let rows = |s: &mut String, parent: Node, me: &mut Self, head: bool| {
            for tr in xmlutil::children(parent, "tr").filter(|r| visible(*r)) {
                s.push_str("<tr>");
                for cell in tr
                    .children()
                    .filter(|c| c.is_element() && matches!(c.tag_name().name(), "td" | "th"))
                {
                    let tag = if head || cell.tag_name().name() == "th" {
                        "th"
                    } else {
                        "td"
                    };
                    let span = ["colspan", "rowspan"]
                        .iter()
                        .filter_map(|a| {
                            cell.attribute(*a)
                                .map(|v| format!(" {a}=\"{}\"", escape_attr(v)))
                        })
                        .collect::<String>();
                    s.push_str(&format!(
                        "<{tag}{span}>{}</{tag}>",
                        me.children(cell, level, xref)
                    ));
                }
                s.push_str("</tr>");
            }
        };
        for part in n.children().filter(|c| c.is_element()) {
            match part.tag_name().name() {
                "thead" => {
                    s.push_str("<thead>");
                    rows(&mut s, part, self, true);
                    s.push_str("</thead>");
                }
                "tbody" | "tfoot" => {
                    s.push_str("<tbody>");
                    rows(&mut s, part, self, false);
                    s.push_str("</tbody>");
                }
                _ => {}
            }
        }
        if xmlutil::child(n, "tr").is_some() {
            s.push_str("<tbody>");
            rows(&mut s, n, self, false);
            s.push_str("</tbody>");
        }
        s.push_str("</table>");
        s
    }

    fn links_element(&mut self, n: Node, xref: &str) -> String {
        let heading = xmlutil::child(n, "title").map(xmlutil::text_of);
        let page_level = !xref.contains('#');
        match n.attribute("type") {
            Some("topic") => self.topic_list(xref, n.attribute("groups"), heading),
            Some("section") => {
                // Sections that are children of the containing element.
                let container = n.parent().unwrap_or(n);
                let items: Vec<LinkItem> = xmlutil::children(container, "section")
                    .filter(|s| visible(*s))
                    .filter_map(|s| {
                        let id = s.attribute("id")?;
                        let x = format!("{}#{id}", self.page.id);
                        self.doc.link_item(self.docref, &x)
                    })
                    .collect();
                html::link_list("links-section", heading.as_deref(), &items)
            }
            Some("seealso") => {
                if page_level {
                    self.placed.insert(format!("seealso:{xref}"));
                }
                let items: Vec<LinkItem> = self
                    .doc
                    .sorted_by_title(self.doc.seealso_links(xref))
                    .iter()
                    .filter_map(|x| self.doc.link_item(self.docref, x))
                    .collect();
                html::link_list(
                    "seealso",
                    Some(heading.as_deref().unwrap_or(tr("See Also"))),
                    &items,
                )
            }
            Some("guide") => {
                if page_level {
                    self.placed.insert(format!("guide:{xref}"));
                }
                let items: Vec<LinkItem> = self
                    .doc
                    .sorted_by_title(self.doc.guide_links(xref))
                    .iter()
                    .filter_map(|x| self.doc.link_item(self.docref, x))
                    .collect();
                html::link_list(
                    "guides",
                    Some(heading.as_deref().unwrap_or(tr("More Information"))),
                    &items,
                )
            }
            _ => String::new(),
        }
    }

    fn link(&mut self, n: Node) -> String {
        let content = self.inlines(n);
        let has_content = !xmlutil::text_of(n).is_empty() || n.children().any(|c| c.is_element());
        if let Some(x) = n.attribute("xref") {
            let x = normalize_xref(&self.page.id, x);
            if let Some(t) = self.doc.target(&x) {
                let (page, frag) = match x.split_once('#') {
                    Some((p, s)) => (p, Some(s)),
                    None => (x.as_str(), None),
                };
                let uri = page_uri(self.docref, page, frag);
                let text = if has_content {
                    content
                } else {
                    escape(&t.title)
                };
                return format!("<a href=\"{}\">{text}</a>", escape_attr(&uri));
            }
            // A section link without metadata (no id'd section found) or a
            // link into an element id: point at the page anyway.
            let (page, frag) = x.split_once('#').unwrap_or((x.as_str(), ""));
            if self.doc.pages.contains_key(page) {
                let uri = page_uri(self.docref, page, Some(frag));
                let text = if has_content { content } else { escape(&x) };
                return format!("<a href=\"{}\">{text}</a>", escape_attr(&uri));
            }
            if n.attribute("href").is_none() {
                return if has_content { content } else { escape(&x) };
            }
        }
        if let Some(h) = n.attribute("href") {
            let internal = h.starts_with("help:") || h.starts_with("ghelp:") || h.starts_with('#');
            let cls = if internal { "" } else { " class=\"external\"" };
            let text = if has_content { content } else { escape(h) };
            return format!("<a href=\"{}\"{cls}>{text}</a>", escape_attr(h));
        }
        content
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page(id: &str, ty: &str, info: &str, body: &str) -> Page {
        let src = format!(
            "<page xmlns=\"http://projectmallard.org/1.0/\" xmlns:if=\"{IF_NS}\" id=\"{id}\" type=\"{ty}\">\
             <info>{info}</info>{body}</page>"
        );
        parse_page(src, PathBuf::from(format!("/doc/{id}.page"))).unwrap()
    }

    fn fixture() -> MallardDoc {
        let mut d = MallardDoc {
            dir: "/doc".into(),
            pages: BTreeMap::new(),
        };
        for p in [
            page("index", "guide", "<title type=\"text\">Fixture Help</title>", "<title>Fixture</title><section id=\"more\"><title>More</title></section>"),
            page("net", "guide", "<link type=\"guide\" xref=\"index\"/><desc>Networking</desc>", "<title>Network</title>"),
            page(
                "wifi",
                "topic",
                "<link type=\"guide\" xref=\"net\" group=\"first\"/><link type=\"seealso\" xref=\"vpn\"/><desc>Wireless</desc>",
                "<title>Wi-Fi</title>",
            ),
            page("vpn", "topic", "<link type=\"guide\" xref=\"net\"/><title type=\"sort\">0 vpn</title>", "<title>VPN</title>"),
            page("eth", "topic", "<link type=\"guide\" xref=\"net\"/>", "<title>Ethernet</title>"),
            page("about", "topic", "<link type=\"guide\" xref=\"index#more\"/>", "<title>About</title>"),
            page("legal", "topic", "", "<title>Legal</title>"),
            page("credits", "topic", "<link type=\"guide\" xref=\"index\" group=\"last\"/>", "<title>Credits</title>"),
        ] {
            d.pages.insert(p.id.clone(), p);
        }
        // index explicitly lists legal as a topic.
        d.pages.get_mut("index").unwrap().meta.links.push(InfoLink {
            kind: "topic".into(),
            xref: "legal".into(),
            group: None,
        });
        d
    }

    #[test]
    fn conditionals() {
        assert!(test_true("target:html"));
        assert!(test_true("platform:gnome-classic, platform:gnome"));
        assert!(!test_true("platform:unity"));
        assert!(test_true("!platform:unity"));
        assert!(!test_true("target:html platform:unity"));
        assert!(test_true("target:html !platform:unity"));
        assert!(test_true(""));
    }

    #[test]
    fn guide_and_topic_links() {
        let d = fixture();
        let net = d.group_sort(d.topic_links("net"), None);
        // first group, then default sorted by sort title ("0 vpn" < "ethernet").
        assert_eq!(net, ["wifi", "vpn", "eth"]);
        let idx = d.group_sort(d.topic_links("index"), None);
        assert_eq!(idx, ["legal", "net", "credits"]);
        assert_eq!(d.topic_links("index#more"), [(None, "about".to_string())]);
        assert_eq!(d.guide_links("wifi"), ["net"]);
        assert_eq!(d.guide_links("legal"), ["index"]);
        assert_eq!(d.guide_links("about"), ["index#more"]);
        // groups attribute ordering
        let custom = d.group_sort(
            vec![
                (Some("b".into()), "eth".into()),
                (Some("a".into()), "vpn".into()),
                (None, "wifi".into()),
            ],
            Some("a b"),
        );
        assert_eq!(custom, ["vpn", "eth", "wifi"]);
    }

    #[test]
    fn seealso_is_reciprocal() {
        let d = fixture();
        assert_eq!(d.seealso_links("wifi"), ["vpn"]);
        assert_eq!(d.seealso_links("vpn"), ["wifi"]);
        assert!(d.seealso_links("eth").is_empty());
    }

    #[test]
    fn trail_and_toc() {
        let d = fixture();
        assert_eq!(d.trail("wifi"), ["index", "net"]);
        assert_eq!(d.trail("about"), ["index"]);
        assert!(d.trail("index").is_empty());
        let toc: Vec<(usize, String)> = d.toc().into_iter().map(|(dep, x, _)| (dep, x)).collect();
        let expect: Vec<(usize, String)> = [
            (0, "legal"),
            (0, "net"),
            (1, "wifi"),
            (1, "vpn"),
            (1, "eth"),
            (0, "credits"),
            (0, "about"),
        ]
        .into_iter()
        .map(|(a, b)| (a, b.to_string()))
        .collect();
        assert_eq!(toc, expect);
    }

    #[test]
    fn renders_guide_page() {
        let d = fixture();
        let docref = DocRef::Id("fx".into());
        let (title, html) = d.render(&docref, "net").unwrap();
        assert_eq!(title, "Network");
        assert!(html.contains("<h1>Network</h1>"));
        assert!(
            html.contains(
                "href=\"help:fx/wifi\">Wi-Fi</a><span class=\"link-desc\">Wireless</span>"
            ),
            "{html}"
        );
        let wifi = html.find("help:fx/wifi").unwrap();
        let eth = html.find("help:fx/eth").unwrap();
        assert!(wifi < eth);
        assert!(html.contains("<nav class=\"trail\"><a href=\"help:fx\">Fixture</a>"));
        let (_, idx) = d.render(&docref, "index").unwrap();
        assert!(idx.contains("<section id=\"more\"><h2>More</h2>"));
        assert!(idx.contains("help:fx/about"));
        let (_, wifi) = d.render(&docref, "wifi").unwrap();
        assert!(wifi.contains("See Also") && wifi.contains("help:fx/vpn"));
        assert!(wifi.contains("More Information") && wifi.contains("help:fx/net"));
    }

    #[test]
    fn renders_blocks_and_inlines() {
        let body = r##"<title>Blocks</title>
            <p>Press <keyseq><key>Ctrl</key><key>C</key></keyseq> then choose
               <guiseq><gui>File</gui><gui>Save</gui></guiseq> in <app>Files</app>.
               See <link xref="other">this</link>, <link xref="other"/>, <link xref="#s1"/> and
               <link href="https://example.org">the web</link>. <media type="image" src="figures/i.png">icon</media></p>
            <steps><item><p>One</p></item><item><p>Two</p></item></steps>
            <list type="numbered"><item><p>A</p></item></list>
            <terms><item><title>Term</title><p>Def</p></item></terms>
            <tree><item>root<item>child</item></item></tree>
            <code>let x = 1 &lt; 2;</code>
            <screen><output>$ </output><input>ls</input></screen>
            <note style="warning"><p>Careful</p></note>
            <media type="image" src="figures/big.png" width="100"/>
            <if:if test="platform:unity"><p>UNITY</p></if:if>
            <if:choose><if:when test="platform:unity"><p>NO</p></if:when><if:else><p>ELSE</p></if:else></if:choose>
            <p if:test="!platform:gnome">HIDDEN</p>
            <comment><p>SECRET</p></comment>
            <table><thead><tr><td><p>H</p></td></tr></thead><tbody><tr><td><p>C</p></td></tr></tbody></table>
            <section id="s1"><title>Sec One</title><p>x</p></section>"##;
        let mut d = MallardDoc {
            dir: "/doc".into(),
            pages: BTreeMap::new(),
        };
        let p = page("blocks", "topic", "", body);
        d.pages.insert("blocks".into(), p);
        let o = page("other", "topic", "", "<title>Other Page</title>");
        d.pages.insert("other".into(), o);
        let (_, h) = d.render(&DocRef::Id("fx".into()), "blocks").unwrap();
        for needle in [
            "<span class=\"keyseq\"><kbd>Ctrl</kbd><span class=\"plus\">+</span><kbd>C</kbd></span>",
            "<span class=\"gui\">File</span><span class=\"arrow\">\u{25B8}</span><span class=\"gui\">Save</span>",
            "<span class=\"app\">Files</span>",
            "<a href=\"help:fx/other\">this</a>",
            "<a href=\"help:fx/other\">Other Page</a>",
            "<a href=\"help:fx/blocks#s1\">Sec One</a>",
            "<a href=\"https://example.org\" class=\"external\">the web</a>",
            "<img src=\"help:fx/figures/i.png\" alt=\"icon\" class=\"media-inline\">",
            "<div class=\"steps\"><ol><li><p>One</p></li><li><p>Two</p></li></ol></div>",
            "<ol><li><p>A</p></li></ol>",
            "<dt>Term</dt><dd><p>Def</p></dd>",
            "<ul class=\"tree\"><li>root<ul><li>child</li></ul></li></ul>",
            "<pre class=\"code\">let x = 1 &lt; 2;</pre>",
            "<pre class=\"screen\"><span class=\"output\">$ </span><span class=\"input\">ls</span></pre>",
            "<div class=\"note note-warning\"><p>Careful</p></div>",
            "<div class=\"media-block\"><img src=\"help:fx/figures/big.png\" alt=\"\" width=\"100\"></div>",
            "<p>ELSE</p>",
            "<thead><tr><th><p>H</p></th></tr></thead><tbody><tr><td><p>C</p></td></tr></tbody>",
        ] {
            assert!(h.contains(needle), "missing {needle}\n{h}");
        }
        for absent in ["UNITY", "<p>NO</p>", "HIDDEN", "SECRET"] {
            assert!(!h.contains(absent), "unexpected {absent}");
        }
    }

    #[test]
    fn search_text_excludes_info() {
        let p = page(
            "x",
            "topic",
            "<desc>DESC</desc><credit><name>Bob</name></credit>",
            "<title>T</title><p>Body words</p>",
        );
        assert_eq!(p.body, "T Body words");
        assert_eq!(p.meta.desc, "DESC");
    }
}
