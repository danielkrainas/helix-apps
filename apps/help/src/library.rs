//! Turns help locations into responses: document loading, caching, home
//! page, search pages and resources. Pure Rust, usable off the main thread.

use crate::discovery::{self, Format, Found};
use crate::docbook::DocBookDoc;
use crate::html::{self, escape, escape_attr};
use crate::mallard::MallardDoc;
use crate::search;
use crate::uri::{self, DocRef, Location};
use helix_app_common::i18n::tr;
use helix_app_common::trf;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

pub enum Content {
    Mallard(MallardDoc),
    DocBook(DocBookDoc),
    Html,
}

pub struct LoadedDoc {
    pub found: Found,
    pub content: Content,
    /// Page to show when none is given (a `.page` file opened directly).
    pub start_page: Option<String>,
}

impl LoadedDoc {
    pub fn title(&self) -> String {
        match &self.content {
            Content::Mallard(m) => m.title(),
            Content::DocBook(d) => d.title.clone(),
            Content::Html => html_title(&self.found.entry).unwrap_or_default(),
        }
    }

    fn search_entries(&self) -> Vec<search::Entry> {
        match &self.content {
            Content::Mallard(m) => m.search_entries(),
            Content::DocBook(d) => d.entries.clone(),
            Content::Html => Vec::new(),
        }
    }

    /// Outline: `(depth, page, title)`, `page == None` for the index.
    pub fn toc(&self) -> Vec<(usize, Option<String>, String)> {
        let mut out = vec![(0, None, self.title())];
        let rest = match &self.content {
            Content::Mallard(m) => m.toc(),
            Content::DocBook(d) => d.toc(),
            Content::Html => Vec::new(),
        };
        out.extend(rest.into_iter().map(|(d, p, t)| (d + 1, Some(p), t)));
        out
    }
}

pub struct Response {
    pub mime: String,
    pub body: Vec<u8>,
}

impl Response {
    fn html(s: String) -> Response {
        Response {
            mime: "text/html".into(),
            body: s.into_bytes(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct DocSummary {
    pub id: String,
    pub title: String,
    pub uri: String,
}

pub struct Library {
    dirs: Vec<PathBuf>,
    locales: Vec<String>,
    cache: Mutex<HashMap<DocRef, Arc<LoadedDoc>>>,
}

fn html_title(path: &Path) -> Option<String> {
    let s = std::fs::read_to_string(path).ok()?;
    let lower = s.to_ascii_lowercase();
    let a = lower.find("<title")?;
    let a = a + lower[a..].find('>')? + 1;
    let b = a + lower[a..].find("</title")?;
    Some(crate::xmlutil::collapse_ws(&s[a..b])).filter(|t| !t.is_empty())
}

pub fn file_uri(path: &Path) -> String {
    format!(
        "file://{}",
        uri::percent_encode(&path.to_string_lossy(), "/")
    )
}

impl Library {
    pub fn new(dirs: Vec<PathBuf>, locales: Vec<String>) -> Library {
        Library {
            dirs,
            locales,
            cache: Mutex::default(),
        }
    }

    pub fn from_env() -> Library {
        Library::new(discovery::help_dirs(), crate::locale::from_env())
    }

    pub fn resolve(&self, doc: &DocRef) -> Option<Found> {
        match doc {
            DocRef::Id(id) => discovery::find(id, &self.dirs, &self.locales),
            DocRef::Path(p) => discovery::detect_path(p),
        }
    }

    pub fn load(&self, doc: &DocRef) -> Result<Arc<LoadedDoc>, String> {
        if let Some(d) = self.cache.lock().unwrap().get(doc) {
            return Ok(d.clone());
        }
        let found = self.resolve(doc).ok_or_else(|| match doc {
            DocRef::Id(id) => trf!(
                "The help document \u{201C}{id}\u{201D} is not installed.",
                id = id
            ),
            DocRef::Path(p) => trf!("No help document was found at {}.", p.display()),
        })?;
        let mut start_page = None;
        let content = match found.format {
            Format::Mallard => {
                // Pages missing from a translation come from the next
                // locale's copy (in the end the untranslated one).
                let dirs: Vec<PathBuf> = match doc {
                    DocRef::Id(id) => discovery::find_all(id, &self.dirs, &self.locales)
                        .into_iter()
                        .filter(|f| f.format == Format::Mallard)
                        .map(|f| f.dir)
                        .collect(),
                    DocRef::Path(_) => vec![found.dir.clone()],
                };
                let m = MallardDoc::load_dirs(&dirs)?;
                if let DocRef::Path(p) = doc {
                    if p.is_file() {
                        start_page = m
                            .pages
                            .values()
                            .find(|pg| pg.path == *p)
                            .map(|pg| pg.id.clone());
                    }
                }
                Content::Mallard(m)
            }
            Format::DocBook => Content::DocBook(DocBookDoc::load(&found.entry)?),
            Format::Html => Content::Html,
        };
        let loaded = Arc::new(LoadedDoc {
            found,
            content,
            start_page,
        });
        self.cache
            .lock()
            .unwrap()
            .insert(doc.clone(), loaded.clone());
        Ok(loaded)
    }

    /// `file://` URI when `doc` is a plain HTML document.
    pub fn html_entry(&self, doc: &DocRef) -> Option<String> {
        let f = self.resolve(doc)?;
        (f.format == Format::Html).then(|| file_uri(&f.entry))
    }

    pub fn documents(&self) -> Vec<DocSummary> {
        let mut docs: Vec<DocSummary> = discovery::list_ids(&self.dirs, &self.locales)
            .into_iter()
            .filter_map(|id| {
                let f = discovery::find(&id, &self.dirs, &self.locales)?;
                let title = match f.format {
                    Format::Mallard => MallardDoc::quick_title(&f.dir),
                    Format::DocBook => DocBookDoc::quick_title(&f.entry),
                    Format::Html => html_title(&f.entry),
                }
                .unwrap_or_else(|| id.clone());
                let uri = if f.format == Format::Html {
                    file_uri(&f.entry)
                } else {
                    DocRef::Id(id.clone()).page_uri(None, None)
                };
                Some(DocSummary { id, title, uri })
            })
            .collect();
        docs.sort_by_cached_key(|d| (d.title.to_lowercase(), d.id.clone()));
        docs
    }

    pub fn respond(&self, loc: &Location) -> Response {
        match loc {
            Location::Home { search: None } => Response::html(self.home()),
            Location::Home { search: Some(q) } => Response::html(self.search_all(q)),
            Location::Doc {
                doc, page, search, ..
            } => match self.load(doc) {
                Err(e) => Response::html(html::error_page(tr("Document Not Found"), &e)),
                Ok(d) => match search {
                    Some(q) => Response::html(search_page(doc, &d, q)),
                    None => Response::html(self.render(doc, &d, page.as_deref())),
                },
            },
            Location::Resource { doc, path } => match self.resource(doc, path) {
                Some(r) => r,
                None => Response {
                    mime: "text/plain".into(),
                    body: b"Not found".to_vec(),
                },
            },
            Location::External(u) => Response::html(html::error_page(tr("External Link"), u)),
        }
    }

    fn render(&self, docref: &DocRef, d: &LoadedDoc, page: Option<&str>) -> String {
        let page = page.or(d.start_page.as_deref());
        let rendered = match &d.content {
            Content::Mallard(m) => match page {
                None if !m.pages.contains_key("index") => Some(mallard_listing(docref, m)),
                p => m.render(docref, p.unwrap_or("index")),
            },
            Content::DocBook(db) => db.render(docref, page),
            Content::Html => {
                let uri = file_uri(&d.found.entry);
                Some((
                    d.title(),
                    html::ui_page(
                        &d.title(),
                        &format!(
                            "<meta http-equiv=\"refresh\" content=\"0;url={0}\"><p><a href=\"{0}\">{1}</a></p>",
                            escape_attr(&uri),
                            escape(&trf!("Open {title}", title = d.title()))
                        ),
                    ),
                ))
            }
        };
        match rendered {
            Some((_, h)) => h,
            None => html::error_page(
                tr("Page Not Found"),
                &trf!(
                    "The page \u{201C}{page}\u{201D} was not found in this document.",
                    page = page.unwrap_or("index")
                ),
            ),
        }
    }

    fn resource(&self, doc: &DocRef, rel: &str) -> Option<Response> {
        // A figure missing from a translation comes from the next copy, as
        // pages do. Only figures and media are served, from inside the
        // document's directory.
        let copies = match doc {
            DocRef::Id(id) => discovery::find_all(id, &self.dirs, &self.locales),
            DocRef::Path(_) => self.resolve(doc).into_iter().collect(),
        };
        copies.iter().find_map(|f| {
            let mime = crate::policy::resource_mime(rel)?;
            let path = crate::policy::confined(&f.dir, rel)?;
            let body = std::fs::read(&path).ok()?;
            Some(Response {
                mime: mime.to_string(),
                body,
            })
        })
    }

    fn home(&self) -> String {
        let docs = self.documents();
        let mut body = format!("<h1>{}</h1>", escape(tr("Help")));
        if docs.is_empty() {
            body.push_str(&format!(
                "<p>{}</p>",
                escape(tr("No help documents are installed."))
            ));
        } else {
            body.push_str("<ul class=\"doclist\">");
            for d in &docs {
                body.push_str(&format!(
                    "<li><a href=\"{}\">{}<span class=\"doc-id\">{}</span></a></li>",
                    escape_attr(&d.uri),
                    escape(&d.title),
                    escape(&d.id)
                ));
            }
            body.push_str("</ul>");
        }
        html::ui_page(tr("Help"), &body)
    }

    fn search_all(&self, q: &str) -> String {
        let mut all: Vec<(String, DocRef, search::Entry)> = Vec::new();
        for d in self.documents() {
            let docref = DocRef::Id(d.id.clone());
            if let Ok(loaded) = self.load(&docref) {
                for e in loaded.search_entries() {
                    all.push((d.title.clone(), docref.clone(), e));
                }
            }
        }
        let entries: Vec<search::Entry> = all.iter().map(|(_, _, e)| e.clone()).collect();
        let hits = search::rank(&entries, q);
        let items: Vec<String> = hits
            .iter()
            .take(100)
            .map(|h| {
                let (doc_title, docref, e) = &all[h.index];
                result_item(docref, e, q, Some(doc_title))
            })
            .collect();
        results_page(q, &items, None)
    }
}

fn mallard_listing(docref: &DocRef, m: &MallardDoc) -> (String, String) {
    let items: Vec<html::LinkItem> = m
        .pages
        .values()
        .map(|p| html::LinkItem {
            uri: docref.page_uri(Some(&p.id), None),
            title: p.meta.title.clone(),
            desc: Some(p.meta.desc.clone()),
        })
        .collect();
    let body = format!(
        "<h1>{}</h1>{}",
        escape(tr("Pages")),
        html::link_list("links-topic", None, &items)
    );
    (tr("Pages").into(), html::ui_page(tr("Pages"), &body))
}

fn result_item(docref: &DocRef, e: &search::Entry, q: &str, doc_title: Option<&str>) -> String {
    let uri = match &e.page {
        Some(p) if p != "index" => docref.page_uri(Some(p), None),
        _ => docref.page_uri(None, None),
    };
    let snip = if e.desc.is_empty() {
        search::snippet(&e.body, q, 160)
    } else {
        e.desc.clone()
    };
    let doc = doc_title
        .map(|t| format!(" <span class=\"doc-name\">\u{2014} {}</span>", escape(t)))
        .unwrap_or_default();
    format!(
        "<li><a href=\"{}\">{}</a>{doc}<span class=\"snippet\">{}</span></li>",
        escape_attr(&uri),
        escape(&e.title),
        escape(&snip)
    )
}

fn results_page(q: &str, items: &[String], back: Option<(String, String)>) -> String {
    let mut body = String::new();
    if let Some((uri, title)) = back {
        body.push_str(&html::trail(&[(uri, title)]));
    }
    body.push_str(&format!(
        "<h1>{}</h1>",
        escape(&trf!("Search results for \u{201C}{q}\u{201D}", q = q))
    ));
    if items.is_empty() {
        body.push_str(&format!(
            "<p>{}</p>",
            escape(tr("No matching pages were found."))
        ));
    } else {
        body.push_str(&format!(
            "<ul class=\"links results\">{}</ul>",
            items.concat()
        ));
    }
    html::ui_page(&trf!("Search: {q}", q = q), &body)
}

fn search_page(docref: &DocRef, d: &LoadedDoc, q: &str) -> String {
    let entries = d.search_entries();
    let hits = search::rank(&entries, q);
    let items: Vec<String> = hits
        .iter()
        .take(100)
        .map(|h| result_item(docref, &entries[h.index], q, None))
        .collect();
    results_page(q, &items, Some((docref.page_uri(None, None), d.title())))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    const NS: &str = "xmlns=\"http://projectmallard.org/1.0/\"";

    fn setup() -> (PathBuf, Library) {
        let root = std::env::temp_dir().join(format!("helix-help-lib-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let doc = root.join("help/C/fixture");
        fs::create_dir_all(doc.join("figures")).unwrap();
        fs::write(doc.join("figures/a.png"), b"PNG").unwrap();
        fs::write(root.join("help/C/secret.txt"), b"SECRET").unwrap();
        fs::write(
            doc.join("index.page"),
            format!("<page {NS} id=\"index\" type=\"guide\"><title>Fixture Help</title></page>"),
        )
        .unwrap();
        fs::write(
            doc.join("wifi.page"),
            format!(
                "<page {NS} id=\"wifi\"><info><link type=\"guide\" xref=\"index\"/><desc>Connect to wireless</desc></info>\
                 <title>Wireless networks</title><p>Enter the network password.</p></page>"
            ),
        )
        .unwrap();
        fs::write(
            doc.join("print.page"),
            format!("<page {NS} id=\"print\"><info><link type=\"guide\" xref=\"index\"/></info><title>Printing</title><p>Forgot the password? wireless</p></page>"),
        )
        .unwrap();
        let web = root.join("help/C/web");
        fs::create_dir_all(&web).unwrap();
        fs::write(
            web.join("index.html"),
            "<html><head><title>Web Manual</title></head></html>",
        )
        .unwrap();
        let lib = Library::new(vec![root.join("help")], vec!["C".into()]);
        (root, lib)
    }

    fn body(r: Response) -> String {
        String::from_utf8(r.body).unwrap()
    }

    #[test]
    fn pages_state_their_language() {
        let root = std::env::temp_dir().join(format!("helix-help-lang-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let page = |dir: &Path, id: &str, attrs: &str, title: &str| {
            fs::create_dir_all(dir).unwrap();
            fs::write(
                dir.join(format!("{id}.page")),
                format!(
                    "<page {NS} id=\"{id}\"{attrs}><info><link type=\"guide\" xref=\"index\"/></info>\
                     <title>{title}</title></page>"
                ),
            )
            .unwrap();
        };
        let de = root.join("help/de/fixture");
        let c = root.join("help/C/fixture");
        page(&de, "index", "", "Hilfe");
        page(&de, "wifi", "", "Drahtlose Netzwerke");
        page(&de, "quoted", " xml:lang=\"de-CH\"", "Zitat");
        page(&c, "index", "", "Help");
        page(&c, "wifi", "", "Wireless networks");
        page(&c, "print", "", "Printing");
        let db = root.join("help/fr/manual");
        fs::create_dir_all(&db).unwrap();
        fs::write(
            db.join("index.docbook"),
            "<book><title>Manuel</title><chapter id=\"a\"><title>Un</title><para>x</para></chapter></book>",
        )
        .unwrap();
        let lib = Library::new(
            vec![root.join("help")],
            vec!["de".into(), "fr".into(), "C".into()],
        );
        let cwd = Path::new("/");
        let get = |u: &str| body(lib.respond(&uri::parse(u, cwd)));
        let lang_of = |h: &str| {
            let a = h.find("<html lang=\"").expect("page states its language") + 12;
            h[a..a + h[a..].find('"').unwrap()].to_string()
        };

        // Translated pages say so; a page the translation lacks falls back
        // to the untranslated copy and says it is English.
        let index = get("help:fixture");
        assert!(index.contains("Hilfe"));
        assert_eq!(lang_of(&index), "de");
        let wifi = get("help:fixture/wifi");
        assert!(wifi.contains("Drahtlose Netzwerke"));
        assert_eq!(lang_of(&wifi), "de");
        let print = get("help:fixture/print");
        assert!(print.contains("Printing"));
        assert_eq!(lang_of(&print), "en");
        // A page's own xml:lang wins.
        assert_eq!(lang_of(&get("help:fixture/quoted")), "de-CH");
        // DocBook: the language of its directory.
        assert_eq!(lang_of(&get("help:manual")), "fr");
        assert_eq!(lang_of(&get("help:manual/a")), "fr");
        // Help's own pages are in the language of its interface.
        let ui = crate::locale::ui_lang();
        assert_eq!(lang_of(&get("help:")), ui);
        assert_eq!(lang_of(&get("help:nope")), ui);
        assert_eq!(lang_of(&get("help:?search=printing")), ui);
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn end_to_end() {
        let (root, lib) = setup();
        let docs = lib.documents();
        assert_eq!(
            docs.iter().map(|d| d.title.as_str()).collect::<Vec<_>>(),
            ["Fixture Help", "Web Manual"]
        );
        assert!(docs[1].uri.starts_with("file://"));
        let cwd = Path::new("/");
        let home = body(lib.respond(&uri::parse("help:", cwd)));
        assert!(home.contains("href=\"help:fixture\""));

        let idx = body(lib.respond(&uri::parse("help:fixture", cwd)));
        assert!(idx.contains("help:fixture/wifi") && idx.contains("Connect to wireless"));

        let res = lib.respond(&uri::parse("help:fixture/figures/a.png", cwd));
        assert_eq!(res.mime, "image/png");
        assert_eq!(res.body, b"PNG");
        let esc = lib.respond(&Location::Resource {
            doc: DocRef::Id("fixture".into()),
            path: "../secret.txt".into(),
        });
        assert_ne!(esc.body, b"SECRET");
        // Only figures and media are served from the document directory.
        for name in ["page.html", "page.xhtml", "x.js", "notes.txt", "wifi.page"] {
            let file = root.join("help/C/fixture").join(name);
            if !file.exists() {
                fs::write(file, b"<p>").unwrap();
            }
            let r = lib.respond(&Location::Resource {
                doc: DocRef::Id("fixture".into()),
                path: name.into(),
            });
            assert_eq!(r.mime, "text/plain", "{name}");
            assert_eq!(r.body, b"Not found", "{name}");
        }
        assert!(idx.contains("http-equiv=\"Content-Security-Policy\""));

        let s = body(lib.respond(&uri::parse("help:fixture?search=wireless%20password", cwd)));
        let w = s.find("help:fixture/wifi").unwrap();
        let p = s.find("help:fixture/print").unwrap();
        assert!(w < p, "title match ranks first");

        let all = body(lib.respond(&uri::parse("help:?search=printing", cwd)));
        assert!(all.contains("help:fixture/print") && all.contains("Fixture Help"));

        let missing = body(lib.respond(&uri::parse("help:nope", cwd)));
        assert!(missing.contains("not installed"));
        let nopage = body(lib.respond(&uri::parse("help:fixture/nopage", cwd)));
        assert!(nopage.contains("Page Not Found"));

        // A .page file opened directly starts at that page.
        let direct = body(lib.respond(&uri::parse(
            &root.join("help/C/fixture/wifi.page").to_string_lossy(),
            cwd,
        )));
        assert!(direct.contains("<h1>Wireless networks</h1>"));
        let d = lib.load(&DocRef::Id("fixture".into())).unwrap();
        let toc: Vec<Option<String>> = d.toc().into_iter().map(|t| t.1).collect();
        assert_eq!(toc, [None, Some("print".into()), Some("wifi".into())]);
        assert_eq!(
            lib.html_entry(&DocRef::Id("web".into())).unwrap(),
            file_uri(&root.join("help/C/web/index.html"))
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn fixture_documents_by_path() {
        let fx = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
        let lib = Library::new(vec![], vec!["C".into()]);
        let cwd = Path::new("/");
        let demo = fx.join("demo");
        let idx = body(lib.respond(&uri::parse(&format!("ghelp:{}", demo.display()), cwd)));
        let docref = DocRef::Path(demo.clone());
        assert!(idx.contains(&escape_attr(&docref.resource_uri("figures/logo.svg"))));
        assert!(
            idx.contains(&escape_attr(&docref.page_uri(Some("start"), None))),
            "{idx}"
        );
        let start = body(lib.respond(&uri::parse(&docref.page_uri(Some("start"), None), cwd)));
        assert!(start.contains("On GNOME and Helix") && !start.contains("Unity only"));
        let logo = lib.respond(&uri::parse(&docref.resource_uri("figures/logo.svg"), cwd));
        assert_eq!(logo.mime, "image/svg+xml");

        let manual = fx.join("manual/index.docbook");
        let mref = DocRef::Path(manual.clone());
        let usage = body(lib.respond(&uri::parse(&mref.page_uri(Some("usage"), None), cwd)));
        assert!(usage.contains(&escape_attr(&mref.page_uri(Some("opts"), None))));
        assert!(usage.contains("<span class=\"gui\">File</span><span class=\"arrow\">"));
        let idx = body(lib.respond(&uri::parse(&manual.to_string_lossy(), cwd)));
        assert!(idx.contains("Demo Manual") && idx.contains("\u{2014} a demo"));
    }

    /// Renders every page of every installed document (read-only).
    /// Run with `cargo test -p helix-help -- --ignored installed_docs --nocapture`.
    #[test]
    #[ignore]
    fn installed_docs() {
        let lib = Library::new(vec![PathBuf::from("/usr/share/help")], vec!["C".into()]);
        let mut pages = 0;
        let mut figures = 0;
        let mut missing = 0;
        let mut failures = Vec::new();
        let start = std::time::Instant::now();
        for d in lib.documents() {
            let docref = DocRef::Id(d.id.clone());
            let Ok(loaded) = lib.load(&docref) else {
                if !d.uri.starts_with("file:") {
                    failures.push(format!("{}: load failed", d.id));
                }
                continue;
            };
            let ids: Vec<Option<String>> = match &loaded.content {
                Content::Mallard(m) => m.pages.keys().cloned().map(Some).collect(),
                Content::DocBook(db) => std::iter::once(None)
                    .chain(db.chunks.iter().map(|c| Some(c.id.clone())))
                    .collect(),
                Content::Html => vec![],
            };
            for p in ids {
                pages += 1;
                let h = lib.render(&docref, &loaded, p.as_deref());
                if h.contains("Page Not Found") {
                    failures.push(format!("{}/{:?}", d.id, p));
                }
                // Every figure and media file a page shows is of a served
                // type and comes from the help schemes.
                for src in h
                    .split("src=\"")
                    .skip(1)
                    .filter_map(|s| s.split('"').next())
                {
                    figures += 1;
                    let src = src.replace("&amp;", "&");
                    match uri::parse(&src, Path::new("/")) {
                        Location::Resource { path, .. } => {
                            if crate::policy::resource_mime(&path).is_none() {
                                failures.push(format!("{}/{:?}: refused {src}", d.id, p));
                            } else if lib.respond(&uri::parse(&src, Path::new("/"))).mime
                                == "text/plain"
                            {
                                missing += 1;
                            }
                        }
                        _ => failures.push(format!("{}/{:?}: not a help resource {src}", d.id, p)),
                    }
                }
            }
            println!("{:30} {:5} pages  {}", d.id, loaded.toc().len(), d.title);
        }
        println!(
            "{pages} pages ({figures} figures, {missing} not installed) rendered in {:?}; failures: {failures:?}",
            start.elapsed()
        );
        assert!(failures.is_empty());
    }
}
