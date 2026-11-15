//! The render thread: owns the poppler document and does all work with it.

use crate::layout::{page_px, tile_rect, Rect};
use crate::links::{DocCtx, Link, OutlineItem};
use crate::pagetext::PageText;
use crate::search::{flip_rect, snippet, Hit};
use gtk::{gio, glib};
use std::collections::VecDeque;
use std::sync::{Arc, Condvar, Mutex};

/// What to render.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum JobKey {
    /// Tile `(tx, ty)` of `page` at `scale_micro / 1e6` device px per pt.
    Tile {
        page: u32,
        scale_micro: u64,
        tx: u32,
        ty: u32,
    },
    /// Whole page, `width` device pixels wide.
    Thumb { page: u32, width: u32 },
}

pub fn scale_key(scale: f64) -> u64 {
    (scale * 1e6).round().max(1.0) as u64
}

/// Render request lists, highest priority first.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Client {
    Presentation = 0,
    View = 1,
    Thumbs = 2,
}
const CLIENTS: usize = 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextPurpose {
    Clipboard,
    Primary,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Style {
    Glyph,
    Word,
    Line,
}

impl Style {
    fn poppler(self) -> poppler::SelectionStyle {
        match self {
            Style::Glyph => poppler::SelectionStyle::Glyph,
            Style::Word => poppler::SelectionStyle::Word,
            Style::Line => poppler::SelectionStyle::Line,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct DocInfo {
    pub uri: String,
    /// Unrotated page sizes in points.
    pub sizes: Vec<(f64, f64)>,
    /// Page label, or the 1-based number when the page has none.
    pub labels: Vec<String>,
    /// Whether any label differs from the page number.
    pub has_labels: bool,
    pub title: Option<String>,
    pub author: Option<String>,
    pub subject: Option<String>,
    pub keywords: Option<String>,
    pub producer: Option<String>,
    pub creator: Option<String>,
    pub created: Option<i64>,
    pub modified: Option<i64>,
    pub pdf_version: Option<String>,
    pub permissions: u32,
    pub linearized: bool,
    pub encrypted: bool,
    pub outline: Vec<OutlineItem>,
    pub wants_outline: bool,
}

pub enum Reply {
    Opened {
        id: u64,
        info: Box<DocInfo>,
    },
    NeedPassword {
        id: u64,
        uri: String,
        wrong: bool,
    },
    OpenFailed {
        id: u64,
        uri: String,
        message: String,
    },
    Image {
        id: u64,
        key: JobKey,
        width: u32,
        height: u32,
        stride: usize,
        data: Vec<u8>,
    },
    Links {
        id: u64,
        page: usize,
        links: Vec<Link>,
    },
    /// The text of a page with its character boxes.
    PageText {
        id: u64,
        page: usize,
        text: PageText,
    },
    Selection {
        id: u64,
        gen: u64,
        regions: Vec<(usize, Vec<Rect>)>,
    },
    Text {
        id: u64,
        purpose: TextPurpose,
        text: String,
    },
    SearchHits {
        id: u64,
        gen: u64,
        page: usize,
        hits: Vec<Hit>,
    },
    SearchProgress {
        id: u64,
        gen: u64,
        page: usize,
    },
    SearchDone {
        id: u64,
        gen: u64,
    },
}

enum Cmd {
    Open {
        id: u64,
        uri: String,
        password: Option<String>,
    },
    Links {
        page: usize,
    },
    PageText {
        page: usize,
    },
    Region {
        gen: u64,
        rects: Vec<(usize, Rect)>,
        style: Style,
    },
    Text {
        rects: Vec<(usize, Rect)>,
        style: Style,
        purpose: TextPurpose,
    },
}

#[derive(Clone)]
struct SearchState {
    gen: u64,
    query: String,
    flags: poppler::FindFlags,
    next: usize,
}

#[derive(Default)]
struct Queue {
    cmds: VecDeque<Cmd>,
    region: Option<Cmd>,
    wants: [VecDeque<JobKey>; CLIENTS],
    in_progress: Option<JobKey>,
    search: Option<SearchState>,
    search_gen: u64,
    quit: bool,
}

struct Shared {
    q: Mutex<Queue>,
    cv: Condvar,
}

/// Main-thread handle of a render thread.
pub struct Renderer {
    shared: Arc<Shared>,
    /// Joined on drop, so poppler objects are never freed while the process
    /// tears down poppler's globals.
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Renderer {
    pub fn spawn() -> (Renderer, async_channel::Receiver<Reply>) {
        let shared = Arc::new(Shared {
            q: Mutex::new(Queue::default()),
            cv: Condvar::new(),
        });
        let (tx, rx) = async_channel::unbounded();
        let s = shared.clone();
        let thread = std::thread::Builder::new()
            .name("pdf-render".into())
            .spawn(move || {
                Worker {
                    shared: s,
                    tx,
                    doc: None,
                }
                .run()
            })
            .expect("spawn render thread");
        (
            Renderer {
                shared,
                thread: Some(thread),
            },
            rx,
        )
    }

    fn with<R>(&self, f: impl FnOnce(&mut Queue) -> R) -> R {
        let mut q = self.shared.q.lock().unwrap_or_else(|e| e.into_inner());
        let r = f(&mut q);
        self.shared.cv.notify_one();
        r
    }

    pub fn open(&self, id: u64, uri: &str, password: Option<String>) {
        self.with(|q| {
            q.cmds.clear();
            q.region = None;
            q.wants.iter_mut().for_each(|w| w.clear());
            q.search = None;
            q.search_gen += 1;
            q.cmds.push_back(Cmd::Open {
                id,
                uri: uri.to_string(),
                password,
            });
        });
    }

    /// Replaces the pending render list of `client`.
    pub fn set_wants(&self, client: Client, jobs: Vec<JobKey>) {
        let mut q = self.shared.q.lock().unwrap_or_else(|e| e.into_inner());
        let ip = q.in_progress;
        let new: VecDeque<JobKey> = jobs.into_iter().filter(|j| Some(*j) != ip).collect();
        if q.wants[client as usize] != new {
            q.wants[client as usize] = new;
            self.shared.cv.notify_one();
        }
    }

    pub fn links(&self, page: usize) {
        self.with(|q| q.cmds.push_back(Cmd::Links { page }));
    }

    pub fn page_text(&self, page: usize) {
        self.with(|q| q.cmds.push_back(Cmd::PageText { page }));
    }

    /// Requests the highlighted region of a selection (latest request wins).
    pub fn select_region(&self, gen: u64, rects: Vec<(usize, Rect)>, style: Style) {
        self.with(|q| q.region = Some(Cmd::Region { gen, rects, style }));
    }

    pub fn selected_text(&self, rects: Vec<(usize, Rect)>, style: Style, purpose: TextPurpose) {
        self.with(|q| {
            q.cmds.push_back(Cmd::Text {
                rects,
                style,
                purpose,
            })
        });
    }

    pub fn search(&self, gen: u64, query: &str, case_sensitive: bool, whole_words: bool) {
        let mut flags = poppler::FindFlags::DEFAULT;
        if case_sensitive {
            flags |= poppler::FindFlags::CASE_SENSITIVE;
        }
        if whole_words {
            flags |= poppler::FindFlags::WHOLE_WORDS_ONLY;
        }
        self.with(|q| {
            q.search_gen = gen;
            q.search = (!query.is_empty()).then(|| SearchState {
                gen,
                query: query.to_string(),
                flags,
                next: 0,
            });
        });
    }

    pub fn cancel_search(&self) {
        self.with(|q| {
            q.search = None;
            q.search_gen += 1;
        });
    }
}

impl Drop for Renderer {
    fn drop(&mut self) {
        self.with(|q| q.quit = true);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

enum Work {
    Cmd(Cmd),
    Job(JobKey),
    Search(SearchState),
}

/// Document id, document and page sizes.
type LoadedDoc = (u64, poppler::Document, Vec<(f64, f64)>);

struct Worker {
    shared: Arc<Shared>,
    tx: async_channel::Sender<Reply>,
    doc: Option<LoadedDoc>,
}

impl Worker {
    fn run(mut self) {
        loop {
            let work = {
                let mut q = self.shared.q.lock().unwrap_or_else(|e| e.into_inner());
                q.in_progress = None;
                loop {
                    if q.quit {
                        return;
                    }
                    if let Some(c) = q.cmds.pop_front() {
                        break Work::Cmd(c);
                    }
                    if let Some(c) = q.region.take() {
                        break Work::Cmd(c);
                    }
                    if let Some(j) = q.wants.iter_mut().find_map(|w| w.pop_front()) {
                        q.in_progress = Some(j);
                        break Work::Job(j);
                    }
                    if let Some(s) = q.search.clone() {
                        break Work::Search(s);
                    }
                    q = self.shared.cv.wait(q).unwrap_or_else(|e| e.into_inner());
                }
            };
            let alive = match work {
                Work::Cmd(c) => self.command(c),
                Work::Job(j) => self.render(j),
                Work::Search(s) => self.search_step(s),
            };
            if !alive {
                return;
            }
        }
    }

    fn send(&self, r: Reply) -> bool {
        self.tx.send_blocking(r).is_ok()
    }

    fn id(&self) -> u64 {
        self.doc.as_ref().map(|d| d.0).unwrap_or(0)
    }

    fn command(&mut self, c: Cmd) -> bool {
        match c {
            Cmd::Open { id, uri, password } => {
                // The previous document stays loaded until the new one
                // opens, so a failed or cancelled open keeps the old view.
                let file = gio::File::for_uri(&uri);
                match poppler::Document::from_gfile(
                    &file,
                    password.as_deref(),
                    None::<&gio::Cancellable>,
                ) {
                    Ok(doc) => {
                        let info = doc_info(&doc, &uri, password.is_some());
                        let sizes = info.sizes.clone();
                        self.doc = Some((id, doc, sizes));
                        self.send(Reply::Opened {
                            id,
                            info: Box::new(info),
                        })
                    }
                    Err(e) if e.matches(poppler::Error::Encrypted) => {
                        self.send(Reply::NeedPassword {
                            id,
                            uri,
                            wrong: password.is_some(),
                        })
                    }
                    Err(e) => self.send(Reply::OpenFailed {
                        id,
                        uri,
                        message: e.message().to_string(),
                    }),
                }
            }
            Cmd::Links { page } => {
                let Some((id, doc, sizes)) = &self.doc else {
                    return true;
                };
                let heights: Vec<f64> = sizes.iter().map(|s| s.1).collect();
                let links = doc
                    .page(page as i32)
                    .map(|p| {
                        let mut links = DocCtx {
                            doc,
                            heights: &heights,
                        }
                        .page_links(&p);
                        for l in &mut links {
                            l.text = p
                                .text_for_area(&mut to_poppler_rect(&l.rect))
                                .map(|t| t.split_whitespace().collect::<Vec<_>>().join(" "))
                                .unwrap_or_default();
                        }
                        // Reading order, for keyboard focus.
                        links.sort_by(|a, b| {
                            (a.rect.y, a.rect.x)
                                .partial_cmp(&(b.rect.y, b.rect.x))
                                .unwrap_or(std::cmp::Ordering::Equal)
                        });
                        links
                    })
                    .unwrap_or_default();
                self.send(Reply::Links {
                    id: *id,
                    page,
                    links,
                })
            }
            Cmd::PageText { page } => {
                let Some((id, doc, _)) = &self.doc else {
                    return true;
                };
                let text = doc
                    .page(page as i32)
                    .map(|p| {
                        let s = p.text().map(|t| t.to_string()).unwrap_or_default();
                        PageText::new(&s, text_layout(&p))
                    })
                    .unwrap_or_default();
                self.send(Reply::PageText {
                    id: *id,
                    page,
                    text,
                })
            }
            Cmd::Region { gen, rects, style } => {
                let Some((id, doc, _)) = &self.doc else {
                    return true;
                };
                // Poppler rounds the region to integer units of `scale`;
                // ask at 4× and scale back for sub-point precision.
                const K: f64 = 4.0;
                let regions = rects
                    .into_iter()
                    .filter_map(|(i, r)| {
                        let page = doc.page(i as i32)?;
                        let mut sel = to_poppler_rect(&r);
                        let region = page.selected_region(K, style.poppler(), &mut sel)?;
                        let rs = (0..region.num_rectangles())
                            .map(|n| {
                                let x = region.rectangle(n);
                                Rect::new(
                                    x.x() as f64 / K,
                                    x.y() as f64 / K,
                                    x.width() as f64 / K,
                                    x.height() as f64 / K,
                                )
                            })
                            .collect();
                        Some((i, rs))
                    })
                    .collect();
                self.send(Reply::Selection {
                    id: *id,
                    gen,
                    regions,
                })
            }
            Cmd::Text {
                rects,
                style,
                purpose,
            } => {
                let Some((id, doc, _)) = &self.doc else {
                    return true;
                };
                let text = rects
                    .into_iter()
                    .filter_map(|(i, r)| {
                        let page = doc.page(i as i32)?;
                        page.selected_text(style.poppler(), &mut to_poppler_rect(&r))
                            .map(|s| s.to_string())
                    })
                    .filter(|s| !s.is_empty())
                    .collect::<Vec<_>>()
                    .join("\n");
                self.send(Reply::Text {
                    id: *id,
                    purpose,
                    text,
                })
            }
        }
    }

    fn render(&mut self, key: JobKey) -> bool {
        let Some((id, doc, sizes)) = &self.doc else {
            return true;
        };
        let (page_i, scale, rect) = match key {
            JobKey::Tile {
                page,
                scale_micro,
                tx,
                ty,
            } => {
                let Some(&(pw, ph)) = sizes.get(page as usize) else {
                    return true;
                };
                let s = scale_micro as f64 / 1e6;
                let (w, h) = page_px(pw, ph, s);
                (page, s, tile_rect(w, h, tx, ty))
            }
            JobKey::Thumb { page, width } => {
                let Some(&(pw, ph)) = sizes.get(page as usize) else {
                    return true;
                };
                let s = width as f64 / pw.max(1.0);
                let (w, h) = page_px(pw, ph, s);
                (page, s, (0, 0, w, h))
            }
        };
        let (x, y, w, h) = rect;
        if w == 0 || h == 0 {
            return true;
        }
        let Some(page) = doc.page(page_i as i32) else {
            return true;
        };
        let Some((stride, data)) = render_area(&page, scale, x, y, w, h) else {
            return true;
        };
        self.send(Reply::Image {
            id: *id,
            key,
            width: w,
            height: h,
            stride,
            data,
        })
    }

    fn search_step(&mut self, mut s: SearchState) -> bool {
        let id = self.id();
        let Some((_, doc, sizes)) = &self.doc else {
            let mut q = self.shared.q.lock().unwrap_or_else(|e| e.into_inner());
            q.search = None;
            return true;
        };
        let n = sizes.len();
        let mut ok = true;
        if s.next < n {
            let i = s.next;
            if let Some(page) = doc.page(i as i32) {
                let (pw, ph) = sizes[i];
                let hits: Vec<Hit> = page
                    .find_text_with_options(&s.query, s.flags)
                    .iter()
                    .map(|r| {
                        let rect = flip_rect(r.x1(), r.y1(), r.x2(), r.y2(), ph);
                        let mid = rect.y + rect.h / 2.0;
                        let mut line = to_poppler_rect(&Rect::new(0.0, mid, pw, 0.0));
                        let ctx = page
                            .text_for_area(&mut line)
                            .map(|t| t.to_string())
                            .unwrap_or_default();
                        let (context, matched) = snippet(&ctx, &s.query, 60);
                        Hit {
                            page: i,
                            rect,
                            context,
                            matched,
                        }
                    })
                    .collect();
                if !hits.is_empty() {
                    ok = self.send(Reply::SearchHits {
                        id,
                        gen: s.gen,
                        page: i,
                        hits,
                    });
                }
            }
            s.next += 1;
            if s.next.is_multiple_of(10) {
                ok &= self.send(Reply::SearchProgress {
                    id,
                    gen: s.gen,
                    page: s.next,
                });
            }
        }
        let done = s.next >= n;
        let mut q = self.shared.q.lock().unwrap_or_else(|e| e.into_inner());
        if q.search_gen == s.gen && q.search.is_some() {
            q.search = if done { None } else { Some(s.clone()) };
            drop(q);
            if done {
                ok &= self.send(Reply::SearchDone { id, gen: s.gen });
            }
        }
        ok
    }
}

/// Poppler's box for every character of `page.text()` (top-left-origin
/// page points). `poppler-rs` does not wrap `poppler_page_get_text_layout`.
fn text_layout(page: &poppler::Page) -> Vec<Rect> {
    use gtk::glib::translate::ToGlibPtr;
    let mut rects: *mut poppler::ffi::PopplerRectangle = std::ptr::null_mut();
    let mut n: std::os::raw::c_uint = 0;
    // SAFETY: poppler fills `rects` with `n` rectangles allocated with
    // g_malloc, which are copied out and freed here.
    unsafe {
        let ok =
            poppler::ffi::poppler_page_get_text_layout(page.to_glib_none().0, &mut rects, &mut n);
        if ok == 0 || rects.is_null() {
            return Vec::new();
        }
        let out = std::slice::from_raw_parts(rects, n as usize)
            .iter()
            .map(|r| Rect::from_corners(r.x1, r.y1, r.x2, r.y2))
            .collect();
        glib::ffi::g_free(rects as *mut _);
        out
    }
}

pub fn to_poppler_rect(r: &Rect) -> poppler::Rectangle {
    let mut p = poppler::Rectangle::new();
    p.set_x1(r.x);
    p.set_y1(r.y);
    p.set_x2(r.right());
    p.set_y2(r.bottom());
    p
}

/// Renders the `w`×`h` device-pixel area at `(x, y)` of `page` at `scale`
/// on white. Returns `(stride, premultiplied BGRA bytes)`.
pub fn render_area(
    page: &poppler::Page,
    scale: f64,
    x: u32,
    y: u32,
    w: u32,
    h: u32,
) -> Option<(usize, Vec<u8>)> {
    let mut surf = cairo::ImageSurface::create(cairo::Format::ARgb32, w as i32, h as i32).ok()?;
    {
        let cr = cairo::Context::new(&surf).ok()?;
        cr.set_source_rgb(1.0, 1.0, 1.0);
        cr.paint().ok()?;
        cr.translate(-(x as f64), -(y as f64));
        cr.scale(scale, scale);
        page.render(&cr);
    }
    surf.flush();
    let stride = surf.stride() as usize;
    let data = surf.data().ok()?.to_vec();
    Some((stride, data))
}

fn opt(s: Option<glib::GString>) -> Option<String> {
    s.map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

fn doc_info(doc: &poppler::Document, uri: &str, encrypted: bool) -> DocInfo {
    let n = doc.n_pages().max(0) as usize;
    let mut sizes = Vec::with_capacity(n);
    let mut labels = Vec::with_capacity(n);
    let mut has_labels = false;
    for i in 0..n {
        match doc.page(i as i32) {
            Some(p) => {
                let (w, h) = p.size();
                sizes.push((w.max(1.0), h.max(1.0)));
                let num = (i + 1).to_string();
                let label = opt(p.label()).unwrap_or_else(|| num.clone());
                has_labels |= label != num;
                labels.push(label);
            }
            None => {
                sizes.push((612.0, 792.0));
                labels.push((i + 1).to_string());
            }
        }
    }
    let heights: Vec<f64> = sizes.iter().map(|s| s.1).collect();
    let outline = DocCtx {
        doc,
        heights: &heights,
    }
    .outline();
    DocInfo {
        uri: uri.to_string(),
        sizes,
        labels,
        has_labels,
        title: opt(doc.title()),
        author: opt(doc.author()),
        subject: opt(doc.subject()),
        keywords: opt(doc.keywords()),
        producer: opt(doc.producer()),
        creator: opt(doc.creator()),
        created: doc.creation_date_time().map(|d| d.to_unix()),
        modified: doc.modification_date_time().map(|d| d.to_unix()),
        pdf_version: opt(doc.pdf_version_string()),
        permissions: doc.permissions().bits(),
        linearized: doc.is_linearized(),
        encrypted,
        wants_outline: doc.page_mode() == poppler::PageMode::UseOutlines && !outline.is_empty(),
        outline,
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use crate::links::Target;
    use gtk::prelude::*;
    use std::time::Duration;

    /// Writes a small PDF with cairo: 3 pages, text and a link on page 1,
    /// an outline pointing at page 3.
    pub fn make_pdf(path: &std::path::Path) {
        let surf = cairo::PdfSurface::new(200.0, 300.0, path).unwrap();
        surf.set_metadata(cairo::PdfMetadata::Title, "Test Document")
            .unwrap();
        surf.set_metadata(cairo::PdfMetadata::Author, "Helix")
            .unwrap();
        let cr = cairo::Context::new(&surf).unwrap();
        cr.select_font_face("Sans", cairo::FontSlant::Normal, cairo::FontWeight::Normal);
        cr.set_font_size(12.0);
        // Page 1
        cr.move_to(20.0, 40.0);
        cr.show_text("Hello needle world").unwrap();
        cr.tag_begin("Link", "uri='https://example.org/'");
        cr.move_to(20.0, 80.0);
        cr.show_text("example link").unwrap();
        cr.tag_end("Link");
        cr.tag_begin("Link", "dest='target'");
        cr.move_to(20.0, 120.0);
        cr.show_text("internal").unwrap();
        cr.tag_end("Link");
        surf.add_outline(
            cairo::PDF_OUTLINE_ROOT,
            "Third",
            "page=3 pos=[0 50]",
            cairo::PdfOutline::OPEN,
        )
        .unwrap();
        cr.show_page().unwrap();
        // Page 2 (landscape)
        surf.set_size(300.0, 200.0).unwrap();
        cr.move_to(20.0, 40.0);
        cr.show_text("Second page").unwrap();
        cr.show_page().unwrap();
        // Page 3
        surf.set_size(200.0, 300.0).unwrap();
        cr.tag_begin("cairo.dest", "name='target' x=0 y=100");
        cr.tag_end("cairo.dest");
        cr.move_to(20.0, 250.0);
        cr.show_text("another needle here").unwrap();
        cr.show_page().unwrap();
        drop(cr);
        surf.finish();
    }

    fn recv(rx: &async_channel::Receiver<Reply>) -> Reply {
        for _ in 0..500 {
            if let Ok(r) = rx.try_recv() {
                return r;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("render thread timed out");
    }

    #[test]
    fn worker_end_to_end() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.pdf");
        make_pdf(&path);
        let uri = gio::File::for_path(&path).uri().to_string();
        let (r, rx) = Renderer::spawn();
        r.open(1, &uri, None);
        let info = match recv(&rx) {
            Reply::Opened { id: 1, info } => info,
            _ => panic!("expected Opened"),
        };
        assert_eq!(
            info.sizes,
            vec![(200.0, 300.0), (300.0, 200.0), (200.0, 300.0)]
        );
        assert_eq!(info.title.as_deref(), Some("Test Document"));
        assert_eq!(info.author.as_deref(), Some("Helix"));
        assert!(!info.has_labels);
        assert_eq!(info.outline.len(), 1);
        assert_eq!(info.outline[0].title, "Third");
        match &info.outline[0].target {
            Some(Target::Page {
                page: 2,
                top: Some(top),
                ..
            }) => assert!((top - 50.0).abs() < 1.0, "top {top}"),
            t => panic!("outline target {t:?}"),
        }

        // Search: rectangles come back flipped to top-left coordinates.
        r.search(7, "needle", false, false);
        let mut hits = Vec::new();
        loop {
            match recv(&rx) {
                Reply::SearchHits {
                    gen: 7, hits: h, ..
                } => hits.extend(h),
                Reply::SearchDone { gen: 7, .. } => break,
                _ => {}
            }
        }
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].page, 0);
        assert!(
            hits[0].rect.y < 40.0 && hits[0].rect.bottom() > 30.0,
            "{:?}",
            hits[0].rect
        );
        assert!(
            hits[0].context.contains("Hello needle world"),
            "{}",
            hits[0].context
        );
        assert_eq!(hits[1].page, 2);
        assert!(
            hits[1].rect.y > 230.0 && hits[1].rect.y < 250.0,
            "{:?}",
            hits[1].rect
        );

        // Links.
        r.links(0);
        let links = match recv(&rx) {
            Reply::Links { page: 0, links, .. } => links,
            _ => panic!("expected links"),
        };
        let uri_link = links
            .iter()
            .find(|l| matches!(l.target, Target::Uri(_)))
            .expect("uri link");
        assert_eq!(uri_link.target, Target::Uri("https://example.org/".into()));
        assert!(
            uri_link.rect.y < 80.0 && uri_link.rect.bottom() > 70.0,
            "{:?}",
            uri_link.rect
        );
        let internal = links
            .iter()
            .find(|l| matches!(l.target, Target::Page { .. }))
            .expect("internal link");
        match internal.target {
            Target::Page { page, top, .. } => {
                assert_eq!(page, 2);
                assert!((top.unwrap() - 100.0).abs() < 1.0);
            }
            _ => unreachable!(),
        }

        // Text selection.
        r.selected_text(
            vec![(0, Rect::new(0.0, 25.0, 200.0, 20.0))],
            Style::Glyph,
            TextPurpose::Clipboard,
        );
        match recv(&rx) {
            Reply::Text { text, .. } => assert!(text.contains("needle"), "{text}"),
            _ => panic!("expected text"),
        }
        r.select_region(
            3,
            vec![(0, Rect::new(0.0, 25.0, 200.0, 20.0))],
            Style::Glyph,
        );
        match recv(&rx) {
            Reply::Selection {
                gen: 3, regions, ..
            } => {
                assert_eq!(regions[0].0, 0);
                assert!(!regions[0].1.is_empty());
            }
            _ => panic!("expected selection"),
        }

        // Tile rendering.
        let key = JobKey::Tile {
            page: 1,
            scale_micro: scale_key(2.0),
            tx: 0,
            ty: 0,
        };
        r.set_wants(Client::View, vec![key]);
        match recv(&rx) {
            Reply::Image {
                key: k,
                width,
                height,
                stride,
                data,
                ..
            } => {
                assert_eq!(k, key);
                assert_eq!((width, height), (600, 400));
                assert_eq!(data.len(), stride * height as usize);
                // Background is white.
                assert_eq!(&data[0..4], &[255, 255, 255, 255]);
            }
            _ => panic!("expected image"),
        }
    }

    #[test]
    fn missing_file_fails() {
        let (r, rx) = Renderer::spawn();
        r.open(2, "file:///nonexistent/helix-test.pdf", None);
        assert!(matches!(recv(&rx), Reply::OpenFailed { id: 2, .. }));
    }
}
