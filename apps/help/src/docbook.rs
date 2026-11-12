//! DocBook 4/5 to HTML transformer with simple chunking.

use crate::html::{self, escape, escape_attr};
use crate::search;
use crate::uri::DocRef;
use crate::xmlutil;
use roxmltree::{Node, NodeId};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

const XML_NS: &str = "http://www.w3.org/XML/1998/namespace";
const XLINK_NS: &str = "http://www.w3.org/1999/xlink";

#[derive(Debug, Clone)]
pub struct Chunk {
    pub id: String,
    pub title: String,
    pub depth: usize,
    /// Direct sub-sections with ids: `(id, title)`.
    pub sections: Vec<(String, String)>,
}

#[derive(Debug)]
pub struct DocBookDoc {
    pub dir: PathBuf,
    pub entry: PathBuf,
    pub title: String,
    pub chunks: Vec<Chunk>,
    /// Element id → containing chunk id (`index` for the root page).
    pub ids: HashMap<String, String>,
    pub entries: Vec<search::Entry>,
    source: String,
}

fn local<'a>(n: Node<'a, '_>) -> &'a str {
    n.tag_name().name()
}

pub fn node_id<'a>(n: Node<'a, '_>) -> Option<&'a str> {
    n.attribute("id").or_else(|| n.attribute((XML_NS, "id")))
}

const INFO: &[&str] = &[
    "info", "bookinfo", "articleinfo", "chapterinfo", "sectioninfo", "sect1info", "sect2info", "sect3info",
    "sect4info", "sect5info", "prefaceinfo", "appendixinfo", "partinfo", "refentryinfo", "refsect1info",
    "glossaryinfo", "setinfo", "blockinfo", "objectinfo",
];

const SECTIONS: &[&str] = &[
    "chapter", "appendix", "preface", "part", "section", "sect1", "sect2", "sect3", "sect4", "sect5", "simplesect",
    "refentry", "refsect1", "refsect2", "refsect3", "refsection", "glossary", "bibliography", "article", "colophon",
    "dedication", "acknowledgements", "reference", "index", "setindex",
];

/// Title node of an element: `title` child or `*info/title`.
fn title_node<'a, 'i>(n: Node<'a, 'i>) -> Option<Node<'a, 'i>> {
    xmlutil::child(n, "title").or_else(|| {
        n.children()
            .filter(|c| c.is_element() && INFO.contains(&local(*c)))
            .find_map(|i| xmlutil::child(i, "title"))
    })
}

fn title_of(n: Node) -> String {
    title_node(n).map(xmlutil::text_of).unwrap_or_default()
}

fn info_child<'a, 'i>(n: Node<'a, 'i>, name: &str) -> Option<Node<'a, 'i>> {
    xmlutil::child(n, name).or_else(|| {
        n.children()
            .filter(|c| c.is_element() && INFO.contains(&local(*c)))
            .find_map(|i| xmlutil::child(i, name))
    })
}

/// Elements that become their own page.
fn chunk_nodes<'a, 'i>(root: Node<'a, 'i>) -> Vec<(Node<'a, 'i>, usize)> {
    let mut out = Vec::new();
    let is_book = matches!(local(root), "book" | "set");
    for c in root.children().filter(|c| c.is_element()) {
        let name = local(c);
        let chunk = if is_book {
            matches!(name, "chapter" | "appendix" | "preface" | "part" | "glossary" | "bibliography" | "reference" | "article" | "colophon")
        } else {
            matches!(name, "sect1" | "section" | "appendix" | "refentry" | "glossary" | "bibliography")
        };
        if chunk {
            out.push((c, 0));
            if name == "part" {
                for cc in c.children().filter(|cc| cc.is_element() && matches!(local(*cc), "chapter" | "appendix" | "preface")) {
                    out.push((cc, 1));
                }
            }
        }
    }
    out
}

fn chunk_id(n: Node, i: usize) -> String {
    node_id(n).map(str::to_string).unwrap_or_else(|| format!("chunk-{}", i + 1))
}

fn text_outside(n: Node, skip: &HashSet<NodeId>) -> String {
    let mut s = String::new();
    for d in n.descendants().filter(|d| d.is_text()) {
        if d.ancestors().any(|a| skip.contains(&a.id()) || (a.is_element() && INFO.contains(&local(a)))) {
            continue;
        }
        s.push_str(d.text().unwrap_or(""));
        s.push(' ');
    }
    xmlutil::collapse_ws(&s)
}

impl DocBookDoc {
    pub fn load(entry: &Path) -> Result<DocBookDoc, String> {
        let source = xmlutil::read_flat(entry)?;
        let dir = entry.parent().unwrap_or(Path::new(".")).to_path_buf();
        Self::from_source(source, dir, entry.to_path_buf())
    }

    pub fn from_source(source: String, dir: PathBuf, entry: PathBuf) -> Result<DocBookDoc, String> {
        let (title, chunks, ids, entries) = {
            let doc = xmlutil::parse(&source).map_err(|e| format!("{}: {e}", entry.display()))?;
            let root = doc.root_element();
            let title = title_of(root);
            let nodes = chunk_nodes(root);
            let set: HashSet<NodeId> = nodes.iter().map(|(n, _)| n.id()).collect();
            let mut chunks = Vec::new();
            let mut ids = HashMap::new();
            let mut entries = vec![search::Entry {
                page: None,
                title: title.clone(),
                desc: String::new(),
                body: text_outside(root, &set),
            }];
            for (i, (n, depth)) in nodes.iter().enumerate() {
                let id = chunk_id(*n, i);
                let sections = n
                    .children()
                    .filter(|c| c.is_element() && SECTIONS.contains(&local(*c)) && !set.contains(&c.id()))
                    .filter_map(|c| Some((node_id(c)?.to_string(), title_of(c))))
                    .collect();
                let nested: HashSet<NodeId> = nodes.iter().filter(|(m, _)| m.id() != n.id()).map(|(m, _)| m.id()).collect();
                entries.push(search::Entry {
                    page: Some(id.clone()),
                    title: title_of(*n),
                    desc: String::new(),
                    body: text_outside(*n, &nested),
                });
                chunks.push(Chunk { id, title: title_of(*n), depth: *depth, sections });
            }
            for e in root.descendants().filter(|e| e.is_element()) {
                if let Some(id) = node_id(e) {
                    let owner = e
                        .ancestors()
                        .find_map(|a| nodes.iter().position(|(m, _)| m.id() == a.id()))
                        .map(|i| chunks[i].id.clone())
                        .unwrap_or_else(|| "index".to_string());
                    ids.entry(id.to_string()).or_insert(owner);
                }
            }
            (title, chunks, ids, entries)
        };
        Ok(DocBookDoc { dir, entry, title, chunks, ids, entries, source })
    }

    pub fn quick_title(entry: &Path) -> Option<String> {
        let src = xmlutil::read(entry).ok()?;
        let doc = xmlutil::parse(&src).ok()?;
        Some(title_of(doc.root_element())).filter(|t| !t.is_empty())
    }

    pub fn toc(&self) -> Vec<(usize, String, String)> {
        self.chunks.iter().map(|c| (c.depth, c.id.clone(), c.title.clone())).collect()
    }

    /// URI for an element id: its chunk plus a fragment.
    pub fn link_for(&self, docref: &DocRef, id: &str) -> Option<String> {
        let chunk = self.ids.get(id)?;
        let frag = (chunk != id).then_some(id);
        Some(if chunk == "index" { docref.page_uri(None, frag) } else { docref.page_uri(Some(chunk), frag) })
    }

    pub fn render(&self, docref: &DocRef, page: Option<&str>) -> Option<(String, String)> {
        let doc = xmlutil::parse(&self.source).ok()?;
        let root = doc.root_element();
        let nodes = chunk_nodes(root);
        let chunk_set: HashSet<NodeId> = nodes.iter().map(|(n, _)| n.id()).collect();
        let by_id: HashMap<&str, Node> =
            root.descendants().filter(|n| n.is_element()).filter_map(|n| Some((node_id(n)?, n))).collect();
        let mut r = Renderer { doc: self, docref, chunk_set, by_id };
        let page = page.filter(|p| *p != "index");
        let mut body = String::new();
        let title;
        match page {
            None => {
                title = self.title.clone();
                body.push_str(&format!("<h1>{}</h1>", title_node(root).map(|t| r.inlines(t)).unwrap_or_default()));
                if let Some(st) = info_child(root, "subtitle") {
                    body.push_str(&format!("<div class=\"subtitle\">{}</div>", r.inlines(st)));
                }
                let authors = authors(root);
                if !authors.is_empty() {
                    body.push_str(&format!("<p class=\"credits\">{}</p>", escape(&authors.join(", "))));
                }
                if let Some(a) = info_child(root, "abstract") {
                    body.push_str(&format!("<div class=\"abstract\">{}</div>", r.children(a, 2)));
                }
                body.push_str(&r.content(root, 2));
                if !self.chunks.is_empty() {
                    body.push_str("<div class=\"toc\"><h2>Contents</h2><ul>");
                    for c in &self.chunks {
                        let pad = if c.depth > 0 { " style=\"margin-left:1.2em\"" } else { "" };
                        body.push_str(&format!(
                            "<li{pad}><a href=\"{}\">{}</a>",
                            escape_attr(&docref.page_uri(Some(&c.id), None)),
                            escape(&c.title)
                        ));
                        if !c.sections.is_empty() {
                            body.push_str("<ul>");
                            for (sid, st) in &c.sections {
                                body.push_str(&format!(
                                    "<li><a href=\"{}\">{}</a></li>",
                                    escape_attr(&docref.page_uri(Some(&c.id), Some(sid))),
                                    escape(st)
                                ));
                            }
                            body.push_str("</ul>");
                        }
                        body.push_str("</li>");
                    }
                    body.push_str("</ul></div>");
                }
                if let Some(l) = info_child(root, "legalnotice") {
                    body.push_str(&format!("<div class=\"legal\">{}</div>", r.children(l, 3)));
                }
            }
            Some(p) => {
                let idx = self.chunks.iter().position(|c| c.id == p)?;
                let (n, depth) = nodes[idx];
                title = self.chunks[idx].title.clone();
                let mut trail = vec![(docref.page_uri(None, None), self.title.clone())];
                if depth > 0 {
                    if let Some(parent) = self.chunks[..idx].iter().rev().find(|c| c.depth < depth) {
                        trail.push((docref.page_uri(Some(&parent.id), None), parent.title.clone()));
                    }
                }
                body.push_str(&html::trail(&trail));
                body.push_str(&format!(
                    "<h1 id=\"{}\">{}</h1>",
                    escape_attr(p),
                    title_node(n).map(|t| r.inlines(t)).unwrap_or_default()
                ));
                body.push_str(&r.content(n, 2));
                // Child chunks (chapters of a part).
                let kids: Vec<&Chunk> =
                    self.chunks[idx + 1..].iter().take_while(|c| c.depth > depth).collect();
                if !kids.is_empty() {
                    body.push_str("<div class=\"toc\"><ul>");
                    for c in kids {
                        body.push_str(&format!(
                            "<li><a href=\"{}\">{}</a></li>",
                            escape_attr(&docref.page_uri(Some(&c.id), None)),
                            escape(&c.title)
                        ));
                    }
                    body.push_str("</ul></div>");
                }
                body.push_str("<nav class=\"series\"><span>");
                let prev = if idx == 0 { Some((docref.page_uri(None, None), self.title.clone())) } else {
                    let c = &self.chunks[idx - 1];
                    Some((docref.page_uri(Some(&c.id), None), c.title.clone()))
                };
                if let Some((u, t)) = prev {
                    body.push_str(&format!("<a href=\"{}\">\u{2190} {}</a>", escape_attr(&u), escape(&t)));
                }
                body.push_str("</span><span>");
                if let Some(c) = self.chunks.get(idx + 1) {
                    body.push_str(&format!(
                        "<a href=\"{}\">{} \u{2192}</a>",
                        escape_attr(&docref.page_uri(Some(&c.id), None)),
                        escape(&c.title)
                    ));
                }
                body.push_str("</span></nav>");
            }
        }
        Some((title.clone(), html::page(&title, &body)))
    }
}

fn authors(root: Node) -> Vec<String> {
    let mut out = Vec::new();
    let info = root.children().find(|c| c.is_element() && INFO.contains(&local(*c)));
    if let Some(info) = info {
        for a in info.descendants().filter(|d| d.is_element() && local(*d) == "author") {
            let name = ["honorific", "firstname", "givenname", "othername", "surname"]
                .iter()
                .filter_map(|p| a.descendants().find(|d| d.is_element() && local(*d) == *p).map(xmlutil::text_of))
                .collect::<Vec<_>>()
                .join(" ");
            let name = if name.is_empty() {
                a.descendants()
                    .find(|d| d.is_element() && local(*d) == "personname")
                    .map(xmlutil::text_of)
                    .unwrap_or_default()
            } else {
                name
            };
            if !name.is_empty() && !out.contains(&name) {
                out.push(name);
            }
        }
    }
    out
}

const BLOCKS: &[&str] = &[
    "itemizedlist", "orderedlist", "variablelist", "simplelist", "programlisting", "screen", "literallayout",
    "note", "tip", "important", "warning", "caution", "figure", "informalfigure", "example", "informalexample",
    "table", "informaltable", "blockquote", "mediaobject", "procedure", "para", "simpara", "synopsis", "address",
    "glosslist", "segmentedlist", "qandaset", "sidebar", "formalpara", "equation", "informalequation", "cmdsynopsis",
];

struct Renderer<'d, 'a, 'i> {
    doc: &'d DocBookDoc,
    docref: &'d DocRef,
    chunk_set: HashSet<NodeId>,
    by_id: HashMap<&'a str, Node<'a, 'i>>,
}

impl Renderer<'_, '_, '_> {
    fn children(&mut self, n: Node, level: usize) -> String {
        n.children().map(|c| self.node(c, level)).collect()
    }

    fn inlines(&mut self, n: Node) -> String {
        self.children(n, 6)
    }

    /// Body of a section-like element: everything but its title/info, with
    /// nested chunks left out.
    fn content(&mut self, n: Node, level: usize) -> String {
        let kids: Vec<Node> = n
            .children()
            .filter(|c| {
                !(c.is_element()
                    && (matches!(local(*c), "title" | "subtitle" | "titleabbrev")
                        || INFO.contains(&local(*c))
                        || self.chunk_set.contains(&c.id())))
            })
            .collect();
        kids.into_iter().map(|c| self.node(c, level)).collect()
    }

    fn block_title(&mut self, n: Node) -> String {
        title_node(n).map(|t| format!("<div class=\"block-title\">{}</div>", self.inlines(t))).unwrap_or_default()
    }

    fn resource(&self, src: &str) -> String {
        if src.contains("://") {
            return src.to_string();
        }
        let base = self.doc.entry.parent().unwrap_or(&self.doc.dir);
        let rel = base.strip_prefix(&self.doc.dir).unwrap_or(Path::new("")).join(src);
        self.docref.resource_uri(&rel.to_string_lossy())
    }

    fn target_text(&self, id: &str) -> Option<String> {
        let n = self.by_id.get(id)?;
        n.attribute("xreflabel").map(str::to_string).or_else(|| Some(title_of(*n)).filter(|t| !t.is_empty()))
    }

    fn internal_link(&mut self, n: Node, linkend: &str, xref: bool) -> String {
        let content = if xref { String::new() } else { self.inlines(n) };
        let text = if content.trim().is_empty() {
            escape(&self.target_text(linkend).unwrap_or_else(|| linkend.to_string()))
        } else {
            content
        };
        match self.doc.link_for(self.docref, linkend) {
            Some(uri) => format!("<a href=\"{}\">{text}</a>", escape_attr(&uri)),
            None => text,
        }
    }

    fn external_link(&mut self, n: Node, url: &str) -> String {
        let content = self.inlines(n);
        let text = if content.trim().is_empty() { escape(url) } else { content };
        let internal = url.starts_with("help:") || url.starts_with("ghelp:");
        let cls = if internal { "" } else { " class=\"external\"" };
        format!("<a href=\"{}\"{cls}>{text}</a>", escape_attr(url))
    }

    fn media(&mut self, n: Node, inline: bool) -> String {
        let data = n.descendants().find(|d| d.is_element() && local(*d) == "imagedata");
        let alt = n
            .descendants()
            .find(|d| d.is_element() && local(*d) == "textobject")
            .map(xmlutil::text_of)
            .unwrap_or_default();
        let caption = xmlutil::child(n, "caption").map(|c| self.children(c, 6));
        let Some(src) = data.and_then(|d| d.attribute("fileref")) else {
            return if alt.is_empty() { String::new() } else { escape(&alt) };
        };
        let size = data
            .and_then(|d| d.attribute("width").or_else(|| d.attribute("contentwidth")))
            .map(|w| format!(" width=\"{}\"", escape_attr(w)))
            .unwrap_or_default();
        let img = format!(
            "<img src=\"{}\" alt=\"{}\"{size}{}>",
            escape_attr(&self.resource(src)),
            escape_attr(&alt),
            if inline { " class=\"media-inline\"" } else { "" }
        );
        if inline {
            img
        } else {
            let cap = caption.map(|c| format!("<div class=\"caption\">{c}</div>")).unwrap_or_default();
            format!("<div class=\"media-block\">{img}{cap}</div>")
        }
    }

    fn table(&mut self, n: Node) -> String {
        let mut s = String::from("<table>");
        if let Some(t) = title_node(n) {
            s.push_str(&format!("<caption>{}</caption>", self.inlines(t)));
        }
        let groups: Vec<Node> = {
            let g: Vec<Node> = xmlutil::children(n, "tgroup").collect();
            if g.is_empty() { vec![n] } else { g }
        };
        for g in groups {
            for part in g.children().filter(|c| c.is_element()) {
                let (open, close, head) = match local(part) {
                    "thead" => ("<thead>", "</thead>", true),
                    "tbody" => ("<tbody>", "</tbody>", false),
                    "tfoot" => ("<tfoot>", "</tfoot>", false),
                    "row" | "tr" => {
                        s.push_str(&self.row(part, false));
                        continue;
                    }
                    _ => continue,
                };
                s.push_str(open);
                for row in part.children().filter(|r| r.is_element() && matches!(local(*r), "row" | "tr")) {
                    s.push_str(&self.row(row, head));
                }
                s.push_str(close);
            }
        }
        s.push_str("</table>");
        s
    }

    fn row(&mut self, row: Node, head: bool) -> String {
        let mut s = String::from("<tr>");
        for cell in row.children().filter(|c| c.is_element() && matches!(local(*c), "entry" | "td" | "th")) {
            let tag = if head || local(cell) == "th" { "th" } else { "td" };
            let mut attrs = String::new();
            if let Some(m) = cell.attribute("morerows").and_then(|m| m.parse::<u32>().ok()) {
                attrs.push_str(&format!(" rowspan=\"{}\"", m + 1));
            }
            for a in ["colspan", "rowspan"] {
                if let Some(v) = cell.attribute(a) {
                    attrs.push_str(&format!(" {a}=\"{}\"", escape_attr(v)));
                }
            }
            s.push_str(&format!("<{tag}{attrs}>{}</{tag}>", self.children(cell, 6)));
        }
        s.push_str("</tr>");
        s
    }

    fn list_items(&mut self, n: Node, item: &'static str, level: usize) -> String {
        xmlutil::children(n, item).map(|i| format!("<li>{}</li>", self.children(i, level))).collect()
    }

    fn node(&mut self, n: Node, level: usize) -> String {
        if n.is_text() {
            return escape(n.text().unwrap_or(""));
        }
        if !n.is_element() || self.chunk_set.contains(&n.id()) {
            return String::new();
        }
        let name = local(n);
        if INFO.contains(&name) {
            return String::new();
        }
        let id_attr = node_id(n).map(|i| format!(" id=\"{}\"", escape_attr(i))).unwrap_or_default();
        if SECTIONS.contains(&name) {
            let h = level.min(6);
            let title = title_node(n).map(|t| self.inlines(t)).unwrap_or_default();
            let body = self.content(n, level + 1);
            return format!("<section{id_attr}><h{h}>{title}</h{h}>{body}</section>");
        }
        match name {
            "title" | "subtitle" | "titleabbrev" | "indexterm" | "remark" | "revhistory" | "refmeta" | "toc" | "lot" => {
                String::new()
            }
            "para" | "simpara" => {
                let has_block = n.children().any(|c| c.is_element() && BLOCKS.contains(&local(c)));
                if has_block {
                    format!("<div class=\"para\"{id_attr}>{}</div>", self.children(n, level))
                } else {
                    format!("<p{id_attr}>{}</p>", self.inlines(n))
                }
            }
            "formalpara" => {
                let t = self.block_title(n);
                let body = self.content(n, level);
                format!("<div class=\"formalpara\"{id_attr}>{t}{body}</div>")
            }
            "bridgehead" => format!("<h4{id_attr}>{}</h4>", self.inlines(n)),
            "partintro" | "abstract" | "legalnotice" => format!("<div class=\"{name}\">{}</div>", self.content(n, level)),
            "refnamediv" => {
                let names: Vec<String> = xmlutil::children(n, "refname").map(xmlutil::text_of).collect();
                let purpose = xmlutil::child(n, "refpurpose").map(xmlutil::text_of).unwrap_or_default();
                format!("<p><strong>{}</strong> \u{2014} {}</p>", escape(&names.join(", ")), escape(&purpose))
            }
            "itemizedlist" => {
                let t = self.block_title(n);
                format!("<div class=\"list\"{id_attr}>{t}<ul>{}</ul></div>", self.list_items(n, "listitem", level))
            }
            "orderedlist" => {
                let t = self.block_title(n);
                format!("<div class=\"list\"{id_attr}>{t}<ol>{}</ol></div>", self.list_items(n, "listitem", level))
            }
            "procedure" => {
                let t = self.block_title(n);
                format!("<div class=\"steps\"{id_attr}>{t}<ol>{}</ol></div>", self.list_items(n, "step", level))
            }
            "substeps" => format!("<ol>{}</ol>", self.list_items(n, "step", level)),
            "simplelist" => {
                if n.attribute("type") == Some("inline") {
                    let items: Vec<String> = xmlutil::children(n, "member").map(|m| self.inlines(m)).collect();
                    items.join(", ")
                } else {
                    format!("<ul>{}</ul>", self.list_items(n, "member", level))
                }
            }
            "variablelist" | "glosslist" | "glossdiv" => {
                let t = self.block_title(n);
                let mut items = String::new();
                for e in n.children().filter(|c| c.is_element() && matches!(local(*c), "varlistentry" | "glossentry")) {
                    for term in e.children().filter(|c| c.is_element() && matches!(local(*c), "term" | "glossterm")) {
                        items.push_str(&format!("<dt>{}</dt>", self.inlines(term)));
                    }
                    let id = node_id(e).map(|i| format!(" id=\"{}\"", escape_attr(i))).unwrap_or_default();
                    let defs: String = e
                        .children()
                        .filter(|c| c.is_element() && matches!(local(*c), "listitem" | "glossdef" | "glosssee"))
                        .map(|c| self.children(c, level))
                        .collect();
                    items.push_str(&format!("<dd{id}>{defs}</dd>"));
                }
                format!("<div class=\"terms\"{id_attr}>{t}<dl class=\"terms\">{items}</dl></div>")
            }
            "programlisting" | "screen" | "literallayout" | "synopsis" | "address" | "funcsynopsis" | "cmdsynopsis" => {
                let mut inner = self.inlines(n);
                if inner.starts_with('\n') {
                    inner.remove(0);
                }
                let cls = if name == "screen" { "screen" } else { "code" };
                format!("<pre class=\"{cls}\"{id_attr}>{}</pre>", inner.trim_end())
            }
            "note" | "tip" | "important" | "warning" | "caution" => {
                let t = self.block_title(n);
                let body = self.content(n, level);
                format!("<div class=\"note note-{name}\"{id_attr}>{t}{body}</div>")
            }
            "sidebar" => {
                let t = self.block_title(n);
                let body = self.content(n, level);
                format!("<div class=\"note note-sidebar\"{id_attr}>{t}{body}</div>")
            }
            "figure" | "informalfigure" | "example" | "informalexample" | "equation" | "informalequation" => {
                let cls = if name.contains("figure") { "figure" } else { "example" };
                let t = self.block_title(n);
                let body = self.content(n, level);
                format!("<div class=\"{cls}\"{id_attr}>{t}{body}</div>")
            }
            "mediaobject" | "screenshot" => {
                if name == "screenshot" {
                    return self.children(n, level);
                }
                self.media(n, false)
            }
            "inlinemediaobject" | "inlinegraphic" | "graphic" => {
                if let Some(src) = n.attribute("fileref") {
                    return format!("<img src=\"{}\" alt=\"\">", escape_attr(&self.resource(src)));
                }
                self.media(n, true)
            }
            "table" | "informaltable" => self.table(n),
            "blockquote" | "epigraph" => {
                let attr = xmlutil::child(n, "attribution")
                    .map(|a| format!("<div class=\"cite\">\u{2014} {}</div>", self.inlines(a)))
                    .unwrap_or_default();
                let body: String = n
                    .children()
                    .filter(|c| !(c.is_element() && matches!(local(*c), "attribution" | "title")))
                    .map(|c| self.node(c, level))
                    .collect();
                format!("<blockquote{id_attr}>{body}{attr}</blockquote>")
            }
            "qandaset" | "qandadiv" | "qandaentry" | "answer" => format!("<div class=\"{name}\">{}</div>", self.content(n, level)),
            "question" => format!("<div class=\"question\"><strong>{}</strong></div>", self.children(n, level)),
            "footnote" => format!("<span class=\"footnote\"> [{}]</span>", self.children(n, 6)),
            "anchor" => format!("<a{id_attr}></a>"),
            // Inline markup
            "emphasis" => {
                if n.attribute("role").is_some_and(|r| r == "bold" || r == "strong") {
                    format!("<strong>{}</strong>", self.inlines(n))
                } else {
                    format!("<em>{}</em>", self.inlines(n))
                }
            }
            "literal" | "code" | "function" | "varname" | "parameter" | "option" | "constant" | "classname" | "type"
            | "returnvalue" | "structname" | "structfield" | "symbol" | "token" | "envar" | "errorcode" | "systemitem"
            | "markup" | "sgmltag" | "tag" | "methodname" | "property" => format!("<code>{}</code>", self.inlines(n)),
            "filename" => format!("<span class=\"file\">{}</span>", self.inlines(n)),
            "command" => format!("<span class=\"cmd\">{}</span>", self.inlines(n)),
            "userinput" => format!("<span class=\"input\">{}</span>", self.inlines(n)),
            "computeroutput" | "prompt" => format!("<span class=\"output\">{}</span>", self.inlines(n)),
            "replaceable" => format!("<span class=\"var\">{}</span>", self.inlines(n)),
            "application" | "productname" => format!("<span class=\"app\">{}</span>", self.inlines(n)),
            "guilabel" | "guibutton" | "guiicon" | "guimenu" | "guisubmenu" | "guimenuitem" | "interface"
            | "menuitem" => format!("<span class=\"gui\">{}</span>", self.inlines(n)),
            "menuchoice" => {
                let parts: Vec<String> = n
                    .children()
                    .filter(|c| c.is_element() && local(*c) != "shortcut")
                    .map(|c| self.node(c, 6))
                    .collect();
                format!("<span class=\"guiseq\">{}</span>", parts.join("<span class=\"arrow\">\u{25B8}</span>"))
            }
            "keycombo" => {
                let parts: Vec<String> = n.children().filter(|c| c.is_element()).map(|c| self.node(c, 6)).collect();
                format!("<span class=\"keyseq\">{}</span>", parts.join("<span class=\"plus\">+</span>"))
            }
            "keycap" | "keysym" | "key" => format!("<kbd>{}</kbd>", self.inlines(n)),
            "quote" => format!("\u{201C}{}\u{201D}", self.inlines(n)),
            "firstterm" | "glossterm" | "citetitle" | "foreignphrase" | "wordasword" => format!("<em>{}</em>", self.inlines(n)),
            "subscript" => format!("<sub>{}</sub>", self.inlines(n)),
            "superscript" => format!("<sup>{}</sup>", self.inlines(n)),
            "trademark" => format!("{}\u{2122}", self.inlines(n)),
            "email" => {
                let t = xmlutil::text_of(n);
                format!("<a href=\"mailto:{}\">{}</a>", escape_attr(&t), escape(&t))
            }
            "ulink" => {
                let url = n.attribute("url").unwrap_or("").to_string();
                self.external_link(n, &url)
            }
            "link" => {
                if let Some(l) = n.attribute("linkend") {
                    let l = l.to_string();
                    self.internal_link(n, &l, false)
                } else if let Some(h) = n.attribute((XLINK_NS, "href")).or_else(|| n.attribute("url")) {
                    let h = h.to_string();
                    self.external_link(n, &h)
                } else {
                    self.inlines(n)
                }
            }
            "xref" => match n.attribute("linkend") {
                Some(l) => {
                    let l = l.to_string();
                    self.internal_link(n, &l, true)
                }
                None => String::new(),
            },
            "uri" => {
                let t = xmlutil::text_of(n);
                format!("<a href=\"{}\" class=\"external\">{}</a>", escape_attr(&t), escape(&t))
            }
            _ => self.children(n, level),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BOOK: &str = r#"<?xml version="1.0"?>
<!DOCTYPE book PUBLIC "-//OASIS//DTD DocBook XML V4.5//EN" "http://www.oasis-open.org/docbook/xml/4.5/docbookx.dtd" [
  <!ENTITY app "Widget">
]>
<book id="index">
  <bookinfo><title>&app; Manual</title><author><firstname>Ada</firstname><surname>Lovelace</surname></author>
  <legalnotice><para>Copyright &copy; nobody</para></legalnotice></bookinfo>
  <preface><title>Intro</title><para>Welcome &mdash; read on.</para></preface>
  <chapter id="start"><title>Getting Started</title>
    <para>Use <guimenu>File</guimenu> and <keycombo><keycap>Ctrl</keycap><keycap>O</keycap></keycombo>.
      See <xref linkend="adv-opts"/> or <link linkend="start">here</link> or <ulink url="https://example.org">site</ulink>.</para>
    <itemizedlist><listitem><para>One</para></listitem></itemizedlist>
    <orderedlist><listitem><para>First</para></listitem></orderedlist>
    <variablelist><varlistentry><term>Term</term><listitem><para>Def</para></listitem></varlistentry></variablelist>
    <programlisting>int x = 1 &lt; 2;</programlisting>
    <screen><prompt>$ </prompt><userinput>widget --help</userinput></screen>
    <note><para>Heads up</para></note>
    <warning><para>Danger</para></warning>
    <sect1 id="start-more"><title>More</title><para>Nested</para>
      <figure><title>Shot</title><mediaobject><imageobject><imagedata fileref="figures/shot.png"/></imageobject><textobject><phrase>A shot</phrase></textobject></mediaobject></figure>
    </sect1>
  </chapter>
  <chapter id="advanced"><title>Advanced</title>
    <section id="adv-opts"><title>Options</title>
      <informaltable><tgroup cols="2"><thead><row><entry>Name</entry><entry>Value</entry></row></thead>
        <tbody><row><entry>a</entry><entry><para>b</para></entry></row></tbody></tgroup></informaltable>
    </section>
  </chapter>
</book>"#;

    fn load() -> DocBookDoc {
        let src = xmlutil::preprocess(BOOK);
        DocBookDoc::from_source(src, "/d".into(), "/d/index.docbook".into()).unwrap()
    }

    #[test]
    fn chunks_and_ids() {
        let d = load();
        assert_eq!(d.title, "Widget Manual");
        let ids: Vec<&str> = d.chunks.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids, ["chunk-1", "start", "advanced"]);
        assert_eq!(d.chunks[1].sections, [("start-more".to_string(), "More".to_string())]);
        assert_eq!(d.ids["adv-opts"], "advanced");
        assert_eq!(d.ids["index"], "index");
        let r = DocRef::Id("w".into());
        assert_eq!(d.link_for(&r, "adv-opts").unwrap(), "help:w/advanced#adv-opts");
        assert_eq!(d.link_for(&r, "start").unwrap(), "help:w/start");
        assert_eq!(d.link_for(&r, "index").unwrap(), "help:w");
    }

    #[test]
    fn renders_index_with_toc() {
        let d = load();
        let (title, h) = d.render(&DocRef::Id("w".into()), None).unwrap();
        assert_eq!(title, "Widget Manual");
        assert!(h.contains("<h1>Widget Manual</h1>"));
        assert!(h.contains("Ada Lovelace"));
        assert!(h.contains("<a href=\"help:w/start\">Getting Started</a><ul><li><a href=\"help:w/start#start-more\">More</a>"));
        assert!(h.contains("Copyright \u{a9} nobody"));
        assert!(!h.contains("Welcome"), "chunk content stays on its own page");
    }

    #[test]
    fn renders_chapter() {
        let d = load();
        let (title, h) = d.render(&DocRef::Id("w".into()), Some("start")).unwrap();
        assert_eq!(title, "Getting Started");
        for needle in [
            "<span class=\"gui\">File</span>",
            "<span class=\"keyseq\"><kbd>Ctrl</kbd><span class=\"plus\">+</span><kbd>O</kbd></span>",
            "<a href=\"help:w/advanced#adv-opts\">Options</a>",
            "<a href=\"help:w/start\">here</a>",
            "<a href=\"https://example.org\" class=\"external\">site</a>",
            "<ul><li><p>One</p></li></ul>",
            "<ol><li><p>First</p></li></ol>",
            "<dt>Term</dt><dd><p>Def</p></dd>",
            "<pre class=\"code\">int x = 1 &lt; 2;</pre>",
            "<pre class=\"screen\"><span class=\"output\">$ </span><span class=\"input\">widget --help</span></pre>",
            "<div class=\"note note-note\"><p>Heads up</p></div>",
            "<div class=\"note note-warning\"><p>Danger</p></div>",
            "<section id=\"start-more\"><h2>More</h2>",
            "<img src=\"help:w/figures/shot.png\" alt=\"A shot\">",
            "<nav class=\"trail\"><a href=\"help:w\">Widget Manual</a>",
            "\u{2190} Intro",
            "Advanced \u{2192}",
        ] {
            assert!(h.contains(needle), "missing {needle}\n{h}");
        }
        let (_, adv) = d.render(&DocRef::Id("w".into()), Some("advanced")).unwrap();
        assert!(adv.contains("<thead><tr><th>Name</th><th>Value</th></tr></thead><tbody><tr><td>a</td><td><p>b</p></td></tr></tbody>"), "{adv}");
        assert!(d.render(&DocRef::Id("w".into()), Some("nope")).is_none());
    }

    #[test]
    fn article_chunks_sections_and_docbook5() {
        let src = r#"<article xmlns="http://docbook.org/ns/docbook" xmlns:xlink="http://www.w3.org/1999/xlink" xml:id="index">
            <info><title>Five</title></info>
            <para>Top <link xlink:href="https://x.org/">x</link></para>
            <section xml:id="s1"><title>S1</title><para>a</para><section xml:id="s1a"><title>Sub</title></section></section>
            <section><title>S2</title><para>b <link linkend="s1a"/></para></section>
        </article>"#;
        let d = DocBookDoc::from_source(src.into(), "/d".into(), "/d/x.xml".into()).unwrap();
        assert_eq!(d.title, "Five");
        assert_eq!(d.toc().iter().map(|c| c.1.as_str()).collect::<Vec<_>>(), ["s1", "chunk-2"]);
        let r = DocRef::Id("five".into());
        let (_, idx) = d.render(&r, None).unwrap();
        assert!(idx.contains("<a href=\"https://x.org/\" class=\"external\">x</a>"));
        let (_, s2) = d.render(&r, Some("chunk-2")).unwrap();
        assert!(s2.contains("<a href=\"help:five/s1#s1a\">Sub</a>"), "{s2}");
        assert_eq!(d.entries.len(), 3);
        assert!(d.entries[1].body.contains("Sub"));
    }
}
