//! The continuous document view widget and the shared document model.

use crate::cache::Lru;
use crate::layout::{
    self, fit_zoom, page_px, page_rect_to_content, selection_rect_for_page, tile_rect, tiles_for,
    visible_page_area, Layout, LayoutParams, Rect, ZoomMode, MARGIN,
};
use crate::links::{Link, Target};
use crate::pagetext::{PageText, Unit};
use crate::render::{scale_key, Client, DocInfo, JobKey, Renderer, Style, TextPurpose};
use crate::search::Results;
use gtk::prelude::*;
use gtk::subclass::prelude::*;
use gtk::{gdk, glib, graphene, gsk};
use helix_app_common::trf;
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

const TILE_BUDGET: usize = 256 << 20;
const THUMB_BUDGET: usize = 64 << 20;
pub const THUMB_WIDTH: u32 = 128;

/// State shared by the view, the sidebar and the presentation window.
pub struct Model {
    pub renderer: Renderer,
    pub doc_id: Cell<u64>,
    pub info: RefCell<Option<Rc<DocInfo>>>,
    pub tiles: RefCell<Lru<JobKey, gdk::Texture>>,
    /// Page → thumbnail.
    pub thumbs: RefCell<Lru<u32, gdk::Texture>>,
    pub links: RefCell<HashMap<usize, Vec<Link>>>,
    links_requested: RefCell<HashSet<usize>>,
    /// Page → its text and character boxes (caret, accessibility).
    pub texts: RefCell<HashMap<usize, Rc<PageText>>>,
    texts_requested: RefCell<HashSet<usize>>,
    pub search: RefCell<Results>,
    /// Highlighted selection region per page (page points).
    pub selection: RefCell<Vec<(usize, Vec<Rect>)>>,
    pub night: Cell<bool>,
}

impl Model {
    pub fn new(renderer: Renderer) -> Rc<Self> {
        Rc::new(Model {
            renderer,
            doc_id: Cell::new(0),
            info: RefCell::new(None),
            tiles: RefCell::new(Lru::new(TILE_BUDGET)),
            thumbs: RefCell::new(Lru::new(THUMB_BUDGET)),
            links: RefCell::default(),
            links_requested: RefCell::default(),
            texts: RefCell::default(),
            texts_requested: RefCell::default(),
            search: RefCell::default(),
            selection: RefCell::default(),
            night: Cell::new(false),
        })
    }

    /// Installs a freshly opened document, dropping everything cached.
    pub fn set_document(&self, info: DocInfo) {
        *self.info.borrow_mut() = Some(Rc::new(info));
        self.tiles.borrow_mut().clear();
        self.thumbs.borrow_mut().clear();
        self.links.borrow_mut().clear();
        self.links_requested.borrow_mut().clear();
        self.texts.borrow_mut().clear();
        self.texts_requested.borrow_mut().clear();
        self.search.borrow_mut().clear();
        self.selection.borrow_mut().clear();
    }

    pub fn info(&self) -> Option<Rc<DocInfo>> {
        self.info.borrow().clone()
    }

    pub fn n_pages(&self) -> usize {
        self.info
            .borrow()
            .as_ref()
            .map(|i| i.sizes.len())
            .unwrap_or(0)
    }

    pub fn size(&self, page: usize) -> (f64, f64) {
        self.info
            .borrow()
            .as_ref()
            .and_then(|i| i.sizes.get(page).copied())
            .unwrap_or((612.0, 792.0))
    }

    pub fn label(&self, page: usize) -> String {
        self.info
            .borrow()
            .as_ref()
            .and_then(|i| i.labels.get(page).cloned())
            .unwrap_or_else(|| (page + 1).to_string())
    }

    /// Stores a rendered image; returns the texture.
    pub fn insert_image(&self, key: JobKey, width: u32, height: u32, stride: usize, data: Vec<u8>) {
        let bytes = glib::Bytes::from_owned(data);
        let tex = gdk::MemoryTexture::new(
            width as i32,
            height as i32,
            gdk::MemoryFormat::B8g8r8a8Premultiplied,
            &bytes,
            stride,
        )
        .upcast::<gdk::Texture>();
        let cost = stride * height as usize;
        match key {
            JobKey::Tile { .. } => {
                self.tiles.borrow_mut().insert(key, tex, cost);
            }
            JobKey::Thumb { page, .. } => {
                self.thumbs.borrow_mut().insert(page, tex, cost);
            }
        }
    }

    pub fn request_links(&self, page: usize) {
        if self.links_requested.borrow_mut().insert(page) {
            self.renderer.links(page);
        }
    }

    pub fn request_text(&self, page: usize) {
        if page < self.n_pages() && self.texts_requested.borrow_mut().insert(page) {
            self.renderer.page_text(page);
        }
    }

    pub fn text(&self, page: usize) -> Option<Rc<PageText>> {
        self.texts.borrow().get(&page).cloned()
    }

    pub fn link_at(&self, page: usize, x: f64, y: f64) -> Option<Target> {
        self.links
            .borrow()
            .get(&page)?
            .iter()
            .find(|l| l.rect.contains(x, y))
            .map(|l| l.target.clone())
    }
}

fn rgba(r: f32, g: f32, b: f32, a: f32) -> gdk::RGBA {
    gdk::RGBA::new(r, g, b, a)
}

/// Colours for the selection and search-hit overlays, read from the
/// desktop style at draw time.
struct Highlights {
    /// The selection: the accent colour, translucent.
    selection: gdk::RGBA,
    hit_fill: gdk::RGBA,
    /// Other hits get an outline only in high contrast.
    hit_outline: Option<(gdk::RGBA, f32)>,
    current_fill: gdk::RGBA,
    /// The current hit is outlined, so it stands out without colour.
    current_outline: (gdk::RGBA, f32),
}

impl Highlights {
    fn current(night: bool) -> Self {
        Self::new(
            helix_app_common::style::accent_rgba(),
            helix_app_common::style::is_high_contrast(),
            night,
        )
    }

    fn new(accent: gdk::RGBA, high_contrast: bool, night: bool) -> Self {
        // The page is white (black in night mode); outlines use the
        // opposite colour.
        let ink = if night { 1.0 } else { 0.0 };
        let ink = rgba(ink, ink, ink, 1.0);
        let (sel_a, hit_a, cur_a) = if high_contrast {
            (0.55, 0.6, 0.7)
        } else {
            (0.35, 0.4, 0.45)
        };
        Highlights {
            selection: rgba(accent.red(), accent.green(), accent.blue(), sel_a),
            hit_fill: rgba(0.96, 0.83, 0.18, hit_a),
            hit_outline: high_contrast.then_some((ink, 1.0)),
            current_fill: rgba(1.0, 0.47, 0.0, cur_a),
            current_outline: (ink, if high_contrast { 3.0 } else { 2.0 }),
        }
    }
}

fn grect(r: &Rect) -> graphene::Rect {
    graphene::Rect::new(r.x as f32, r.y as f32, r.w as f32, r.h as f32)
}

/// Pushes the night-mode colour inversion.
pub fn push_invert(snap: &gtk::Snapshot) {
    let m = graphene::Matrix::from_float([
        -1.0, 0.0, 0.0, 0.0, //
        0.0, -1.0, 0.0, 0.0, //
        0.0, 0.0, -1.0, 0.0, //
        0.0, 0.0, 0.0, 1.0,
    ]);
    snap.push_color_matrix(&m, &graphene::Vec4::new(1.0, 1.0, 1.0, 0.0));
}

/// Draws page `page` into `dst` (widget coordinates, rotated size) at
/// `scale` px per pt. Missing tiles inside `vis` (unrotated page points)
/// are appended to `wants`, a missing thumbnail to `thumbs`.
#[allow(clippy::too_many_arguments)]
pub fn paint_page(
    snap: &gtk::Snapshot,
    model: &Model,
    page: usize,
    dst: &Rect,
    rotation: u32,
    scale: f64,
    sf: f64,
    vis: Option<Rect>,
    wants: &mut Vec<JobKey>,
    thumbs: &mut Vec<JobKey>,
) {
    let (pw, ph) = model.size(page);
    snap.save();
    snap.translate(&graphene::Point::new(dst.x as f32, dst.y as f32));
    match rotation {
        90 => snap.translate(&graphene::Point::new(dst.w as f32, 0.0)),
        180 => snap.translate(&graphene::Point::new(dst.w as f32, dst.h as f32)),
        270 => snap.translate(&graphene::Point::new(0.0, dst.h as f32)),
        _ => {}
    }
    if rotation != 0 {
        snap.rotate(rotation as f32);
    }
    let full = Rect::new(0.0, 0.0, pw * scale, ph * scale);
    snap.append_color(&rgba(1.0, 1.0, 1.0, 1.0), &grect(&full));
    match model.thumbs.borrow_mut().get(&(page as u32)) {
        Some(t) => snap.append_texture(t, &grect(&full)),
        None => thumbs.push(JobKey::Thumb {
            page: page as u32,
            width: (THUMB_WIDTH as f64 * sf) as u32,
        }),
    }
    let ds = scale * sf;
    let sk = scale_key(ds);
    let (wpx, hpx) = page_px(pw, ph, ds);
    if let Some(v) = vis {
        let vd = Rect::new(v.x * ds, v.y * ds, v.w * ds, v.h * ds);
        let mut tiles = model.tiles.borrow_mut();
        for (tx, ty) in tiles_for(wpx, hpx, &vd) {
            let key = JobKey::Tile {
                page: page as u32,
                scale_micro: sk,
                tx,
                ty,
            };
            match tiles.get(&key) {
                Some(tex) => {
                    let (x, y, w, h) = tile_rect(wpx, hpx, tx, ty);
                    let r = Rect::new(x as f64 / sf, y as f64 / sf, w as f64 / sf, h as f64 / sf);
                    snap.append_texture(tex, &grect(&r));
                }
                None => wants.push(key),
            }
        }
    }
    snap.restore();
}

/// Tiles of `page` intersecting `vis` that are not cached.
fn missing_tiles(
    model: &Model,
    page: usize,
    scale: f64,
    sf: f64,
    vis: &Rect,
    out: &mut Vec<JobKey>,
) {
    let (pw, ph) = model.size(page);
    let ds = scale * sf;
    let sk = scale_key(ds);
    let (wpx, hpx) = page_px(pw, ph, ds);
    let vd = Rect::new(vis.x * ds, vis.y * ds, vis.w * ds, vis.h * ds);
    let tiles = model.tiles.borrow();
    for (tx, ty) in tiles_for(wpx, hpx, &vd) {
        let key = JobKey::Tile {
            page: page as u32,
            scale_micro: sk,
            tx,
            ty,
        };
        if !tiles.contains(&key) && !out.contains(&key) {
            out.push(key);
        }
    }
}

/// Page, top and left of a deferred navigation.
pub type GoTo = (usize, Option<f64>, Option<f64>);

#[derive(Default)]
pub struct Callbacks {
    pub page_changed: Option<Box<dyn Fn(usize)>>,
    pub zoom_changed: Option<Box<dyn Fn(f64, ZoomMode)>>,
    pub link_activated: Option<Box<dyn Fn(Target)>>,
    pub selection_changed: Option<Box<dyn Fn(bool)>>,
    pub caret_mode_changed: Option<Box<dyn Fn(bool)>>,
}

/// A caret position: page and character offset in its text.
pub type Caret = (usize, usize);

#[derive(Clone, Copy, Debug)]
pub struct SelPoint {
    page: usize,
    pt: (f64, f64),
}

mod spot_imp {
    use super::*;

    #[derive(Default)]
    pub struct LinkSpot {
        pub page: Cell<usize>,
        pub index: Cell<usize>,
        pub link: RefCell<Option<Link>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for LinkSpot {
        const NAME: &'static str = "HelixDocViewLink";
        type Type = super::LinkSpot;
        type ParentType = gtk::Widget;

        fn class_init(klass: &mut Self::Class) {
            klass.set_css_name("doclink");
            klass.set_accessible_role(gtk::AccessibleRole::Link);
        }
    }

    impl ObjectImpl for LinkSpot {
        fn constructed(&self) {
            self.parent_constructed();
            let obj = self.obj();
            obj.set_focusable(true);
            // Pointer input stays with the view (selection, link clicks).
            obj.set_can_target(false);
        }
    }

    impl WidgetImpl for LinkSpot {}
}

glib::wrapper! {
    /// An invisible, focusable stand-in for one link of a visible page: it
    /// gives the link a keyboard focus ring and an accessible object
    /// (role link, named by its text, described by its target).
    pub struct LinkSpot(ObjectSubclass<spot_imp::LinkSpot>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl LinkSpot {
    fn new() -> Self {
        glib::Object::new()
    }

    fn key(&self) -> (usize, usize) {
        (self.imp().page.get(), self.imp().index.get())
    }

    fn set_link(&self, page: usize, index: usize, link: &Link, labels: &[String]) {
        let imp = self.imp();
        imp.page.set(page);
        imp.index.set(index);
        if imp.link.borrow().as_ref() == Some(link) {
            return;
        }
        let target = link.target.describe(labels);
        let name = if link.text.is_empty() {
            target.clone()
        } else {
            link.text.clone()
        };
        self.update_property(&[
            gtk::accessible::Property::Label(&name),
            gtk::accessible::Property::Description(&target),
        ]);
        *imp.link.borrow_mut() = Some(link.clone());
    }

    /// The link's page and area (page points).
    pub fn page_rect(&self) -> Option<(usize, Rect)> {
        let link = self.imp().link.borrow();
        link.as_ref().map(|l| (self.imp().page.get(), l.rect))
    }

    pub fn target(&self) -> Option<Target> {
        self.imp().link.borrow().as_ref().map(|l| l.target.clone())
    }
}

mod imp {
    use super::*;
    use std::sync::OnceLock;

    pub struct DocView {
        pub model: RefCell<Option<Rc<Model>>>,
        pub hadj: RefCell<Option<gtk::Adjustment>>,
        pub vadj: RefCell<Option<gtk::Adjustment>>,
        pub adj_handlers: RefCell<Vec<(gtk::Adjustment, glib::SignalHandlerId)>>,
        pub hpolicy: Cell<gtk::ScrollablePolicy>,
        pub vpolicy: Cell<gtk::ScrollablePolicy>,
        pub layout: RefCell<Layout>,
        pub zoom: Cell<f64>,
        pub mode: Cell<ZoomMode>,
        pub rotation: Cell<u32>,
        pub dual: Cell<bool>,
        pub current: Cell<usize>,
        pub pending_goto: Cell<Option<GoTo>>,
        pub in_goto: Cell<bool>,
        pub sel_anchor: Cell<Option<SelPoint>>,
        pub sel_end: Cell<Option<SelPoint>>,
        pub sel_style: Cell<Style>,
        pub sel_gen: Cell<u64>,
        pub click_count: Cell<i32>,
        pub press_link: RefCell<Option<Target>>,
        pub dragging: Cell<bool>,
        pub pan_start: Cell<(f64, f64)>,
        pub pinch_start: Cell<f64>,
        pub pointer: Cell<Option<(f64, f64)>>,
        pub callbacks: RefCell<Callbacks>,
        /// Focusable stand-ins for the links on visible pages.
        pub spots: RefCell<Vec<LinkSpot>>,
        pub spots_queued: Cell<bool>,
        pub caret_mode: Cell<bool>,
        pub caret: Cell<Option<Caret>>,
        /// Remembered horizontal position for Up / Down (page points).
        pub caret_x: Cell<Option<f64>>,
        /// Where a Shift+arrow selection started.
        pub caret_anchor: Cell<Option<Caret>>,
        /// Keyboard selection, ordered start..end.
        pub kbd_sel: Cell<Option<(Caret, Caret)>>,
        /// The page whose text is exposed to assistive technology.
        pub a11y_page: Cell<usize>,
    }

    impl Default for DocView {
        fn default() -> Self {
            DocView {
                model: RefCell::new(None),
                hadj: RefCell::new(None),
                vadj: RefCell::new(None),
                adj_handlers: RefCell::default(),
                hpolicy: Cell::new(gtk::ScrollablePolicy::Minimum),
                vpolicy: Cell::new(gtk::ScrollablePolicy::Minimum),
                layout: RefCell::default(),
                zoom: Cell::new(1.0),
                mode: Cell::new(ZoomMode::Automatic),
                rotation: Cell::new(0),
                dual: Cell::new(false),
                current: Cell::new(0),
                pending_goto: Cell::new(None),
                in_goto: Cell::new(false),
                sel_anchor: Cell::new(None),
                sel_end: Cell::new(None),
                sel_style: Cell::new(Style::Glyph),
                sel_gen: Cell::new(0),
                click_count: Cell::new(1),
                press_link: RefCell::new(None),
                dragging: Cell::new(false),
                pan_start: Cell::new((0.0, 0.0)),
                pinch_start: Cell::new(1.0),
                pointer: Cell::new(None),
                callbacks: RefCell::default(),
                spots: RefCell::default(),
                spots_queued: Cell::new(false),
                caret_mode: Cell::new(false),
                caret: Cell::new(None),
                caret_x: Cell::new(None),
                caret_anchor: Cell::new(None),
                kbd_sel: Cell::new(None),
                a11y_page: Cell::new(usize::MAX),
            }
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for DocView {
        const NAME: &'static str = "HelixDocView";
        type Type = super::DocView;
        type ParentType = gtk::Widget;
        type Interfaces = (gtk::Scrollable, gtk::AccessibleText);

        fn class_init(klass: &mut Self::Class) {
            klass.set_css_name("docview");
            klass.set_accessible_role(gtk::AccessibleRole::Document);
        }
    }

    impl ObjectImpl for DocView {
        fn properties() -> &'static [glib::ParamSpec] {
            static PROPS: OnceLock<Vec<glib::ParamSpec>> = OnceLock::new();
            PROPS.get_or_init(|| {
                vec![
                    glib::ParamSpecOverride::for_interface::<gtk::Scrollable>("hadjustment"),
                    glib::ParamSpecOverride::for_interface::<gtk::Scrollable>("vadjustment"),
                    glib::ParamSpecOverride::for_interface::<gtk::Scrollable>("hscroll-policy"),
                    glib::ParamSpecOverride::for_interface::<gtk::Scrollable>("vscroll-policy"),
                ]
            })
        }

        fn set_property(&self, _id: usize, value: &glib::Value, pspec: &glib::ParamSpec) {
            match pspec.name() {
                "hadjustment" => self.set_adjustment(true, value.get().ok().flatten()),
                "vadjustment" => self.set_adjustment(false, value.get().ok().flatten()),
                "hscroll-policy" => self
                    .hpolicy
                    .set(value.get().unwrap_or(gtk::ScrollablePolicy::Minimum)),
                "vscroll-policy" => self
                    .vpolicy
                    .set(value.get().unwrap_or(gtk::ScrollablePolicy::Minimum)),
                _ => {}
            }
        }

        fn property(&self, _id: usize, pspec: &glib::ParamSpec) -> glib::Value {
            match pspec.name() {
                "hadjustment" => self.hadj.borrow().to_value(),
                "vadjustment" => self.vadj.borrow().to_value(),
                "hscroll-policy" => self.hpolicy.get().to_value(),
                "vscroll-policy" => self.vpolicy.get().to_value(),
                _ => unreachable!(),
            }
        }

        fn constructed(&self) {
            self.parent_constructed();
            let obj = self.obj();
            obj.set_focusable(true);
            obj.set_overflow(gtk::Overflow::Hidden);
            obj.setup_input();
            // Highlights follow the accent colour and high-contrast style.
            helix_app_common::style::redraw_on_style_change(&*obj);
        }

        fn dispose(&self) {
            for (adj, id) in self.adj_handlers.take() {
                adj.disconnect(id);
            }
            self.spots.take();
            while let Some(child) = self.obj().first_child() {
                child.unparent();
            }
        }
    }

    impl WidgetImpl for DocView {
        fn size_allocate(&self, width: i32, height: i32, baseline: i32) {
            self.parent_size_allocate(width, height, baseline);
            self.obj()
                .relayout_with(width as f64, height as f64, (0.0, 0.0));
            if let Some((p, top, left)) = self.pending_goto.take() {
                self.obj().go_to(p, top, left);
            }
            self.obj().allocate_spots();
        }

        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            let obj = self.obj();
            obj.draw(snapshot);
            for spot in self.spots.borrow().iter() {
                obj.snapshot_child(spot, snapshot);
            }
        }

        /// Tab moves from the document through the links on the visible
        /// pages, then out; Shift+Tab goes back the same way.
        fn focus(&self, direction: gtk::DirectionType) -> bool {
            let obj = self.obj();
            let spots = self.spots.borrow().clone();
            let on_self = obj.has_focus();
            let cur = spots.iter().position(|s| s.has_focus());
            let next = match direction {
                gtk::DirectionType::TabForward => match (on_self, cur) {
                    (true, _) if spots.is_empty() => return false,
                    (true, _) => Some(0),
                    (false, Some(i)) if i + 1 < spots.len() => Some(i + 1),
                    (false, Some(_)) => return false,
                    (false, None) => None,
                },
                gtk::DirectionType::TabBackward => match (on_self, cur) {
                    (true, _) => return false,
                    (false, Some(0)) => None,
                    (false, Some(i)) => Some(i - 1),
                    (false, None) if spots.is_empty() => None,
                    (false, None) => Some(spots.len() - 1),
                },
                _ if on_self || cur.is_some() => return false,
                _ => None,
            };
            match next {
                Some(i) => {
                    let spot = &spots[i];
                    spot.grab_focus();
                    if let Some((page, rect)) = spot.page_rect() {
                        obj.show_rect(page, &rect);
                    }
                }
                None => {
                    obj.grab_focus();
                }
            }
            true
        }
    }

    impl ScrollableImpl for DocView {}

    impl AccessibleTextImpl for DocView {
        fn contents(&self, start: u32, end: u32) -> Option<glib::Bytes> {
            let text = self.obj().a11y_text();
            let s = text
                .map(|t| t.slice(start as usize, end as usize))
                .unwrap_or_default();
            Some(nul_terminated(s))
        }

        fn contents_at(
            &self,
            offset: u32,
            granularity: gtk::AccessibleTextGranularity,
        ) -> Option<(u32, u32, glib::Bytes)> {
            let text = self.obj().a11y_text()?;
            let unit = match granularity {
                gtk::AccessibleTextGranularity::Word => Unit::Word,
                gtk::AccessibleTextGranularity::Sentence => Unit::Sentence,
                gtk::AccessibleTextGranularity::Line => Unit::Line,
                gtk::AccessibleTextGranularity::Paragraph => Unit::Paragraph,
                _ => Unit::Char,
            };
            let (s, e) = text.range_at(offset as usize, unit);
            Some((s as u32, e as u32, nul_terminated(text.slice(s, e))))
        }

        fn caret_position(&self) -> u32 {
            match self.caret.get() {
                Some((p, o)) if p == self.a11y_page.get() => o as u32,
                _ => 0,
            }
        }

        fn selection(&self) -> Vec<gtk::AccessibleTextRange> {
            let page = self.a11y_page.get();
            let Some(((ap, ao), (bp, bo))) = self.kbd_sel.get() else {
                return Vec::new();
            };
            if page < ap || page > bp {
                return Vec::new();
            }
            let len = self.obj().a11y_text().map_or(0, |t| t.len());
            let s = if page == ap { ao } else { 0 };
            let e = if page == bp { bo } else { len };
            if e <= s {
                return Vec::new();
            }
            vec![gtk::AccessibleTextRange::new(s, e - s)]
        }

        fn attributes(
            &self,
            _offset: u32,
        ) -> Vec<(gtk::AccessibleTextRange, glib::GString, glib::GString)> {
            Vec::new()
        }

        fn default_attributes(&self) -> Vec<(glib::GString, glib::GString)> {
            Vec::new()
        }
    }

    /// GTK reads accessible text contents as a C string.
    fn nul_terminated(s: String) -> glib::Bytes {
        let mut v = s.into_bytes();
        v.push(0);
        glib::Bytes::from_owned(v)
    }

    impl DocView {
        fn set_adjustment(&self, horizontal: bool, adj: Option<gtk::Adjustment>) {
            let slot = if horizontal { &self.hadj } else { &self.vadj };
            if let Some(old) = slot.borrow().as_ref() {
                let mut hs = self.adj_handlers.borrow_mut();
                if let Some(pos) = hs.iter().position(|(a, _)| a == old) {
                    let (a, id) = hs.remove(pos);
                    a.disconnect(id);
                }
            }
            if let Some(adj) = &adj {
                let obj = self.obj().downgrade();
                let id = adj.connect_value_changed(move |_| {
                    if let Some(obj) = obj.upgrade() {
                        obj.on_scrolled();
                    }
                });
                self.adj_handlers.borrow_mut().push((adj.clone(), id));
            }
            *slot.borrow_mut() = adj;
            self.obj().queue_allocate();
        }
    }
}

glib::wrapper! {
    pub struct DocView(ObjectSubclass<imp::DocView>)
        @extends gtk::Widget,
        @implements gtk::Scrollable, gtk::AccessibleText, gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Default for DocView {
    fn default() -> Self {
        glib::Object::new()
    }
}

impl DocView {
    pub fn set_model(&self, model: Rc<Model>) {
        *self.imp().model.borrow_mut() = Some(model);
    }

    fn model(&self) -> Option<Rc<Model>> {
        self.imp().model.borrow().clone()
    }

    pub fn callbacks(&self) -> std::cell::RefMut<'_, Callbacks> {
        self.imp().callbacks.borrow_mut()
    }

    fn scroll(&self) -> (f64, f64) {
        let imp = self.imp();
        let h = imp.hadj.borrow().as_ref().map(|a| a.value()).unwrap_or(0.0);
        let v = imp.vadj.borrow().as_ref().map(|a| a.value()).unwrap_or(0.0);
        (h, v)
    }

    fn viewport(&self) -> Rect {
        let (h, v) = self.scroll();
        Rect::new(h, v, self.width() as f64, self.height() as f64)
    }

    pub fn zoom(&self) -> f64 {
        self.imp().zoom.get()
    }

    pub fn zoom_mode(&self) -> ZoomMode {
        self.imp().mode.get()
    }

    pub fn rotation(&self) -> u32 {
        self.imp().rotation.get()
    }

    pub fn dual(&self) -> bool {
        self.imp().dual.get()
    }

    pub fn current_page(&self) -> usize {
        self.imp().current.get()
    }

    /// Resets the view for a newly loaded document.
    pub fn document_changed(&self, mode: ZoomMode, zoom: f64, rotation: u32, dual: bool) {
        let imp = self.imp();
        imp.mode.set(mode);
        imp.zoom.set(layout::clamp_zoom(zoom));
        imp.rotation.set(layout::norm_rotation(rotation as i32));
        imp.dual.set(dual);
        imp.current.set(usize::MAX);
        imp.sel_anchor.set(None);
        imp.sel_end.set(None);
        imp.kbd_sel.set(None);
        imp.caret_anchor.set(None);
        imp.caret.set(None);
        imp.caret_x.set(None);
        imp.a11y_page.set(usize::MAX);
        *imp.layout.borrow_mut() = Layout::default();
        if let Some(a) = imp.vadj.borrow().as_ref() {
            a.set_value(0.0);
        }
        self.relayout((0.0, 0.0));
        self.update_current();
        self.emit_zoom();
        self.queue_sync_links();
        self.refresh_a11y_page();
        if imp.caret_mode.get() {
            self.place_caret();
        }
    }

    fn emit_zoom(&self) {
        let imp = self.imp();
        if let Some(cb) = imp.callbacks.borrow().zoom_changed.as_ref() {
            cb(imp.zoom.get(), imp.mode.get());
        }
    }

    fn relayout(&self, anchor: (f64, f64)) {
        self.relayout_with(self.width() as f64, self.height() as f64, anchor);
    }

    /// Recomputes the layout keeping the document point under `anchor`
    /// (viewport coordinates) in place.
    fn relayout_with(&self, w: f64, h: f64, anchor: (f64, f64)) {
        let imp = self.imp();
        let Some(model) = self.model() else { return };
        let Some(info) = model.info() else {
            *imp.layout.borrow_mut() = Layout::default();
            self.configure_adjustments(w, h, 0.0, 0.0, 0.0, 0.0);
            return;
        };
        let (sx, sy) = self.scroll();
        let old = imp.layout.borrow().clone();
        let anchored = (!old.pages.is_empty()).then(|| {
            let (cx, cy) = (sx + anchor.0, sy + anchor.1);
            let p = old
                .page_at(cx, cy)
                .or_else(|| old.nearest_page(cx, cy))
                .unwrap_or(0);
            let (pw, ph) = info.sizes[p];
            (p, old.content_to_page_unclamped(p, cx, cy, pw, ph))
        });
        if imp.mode.get() != ZoomMode::Free && w > 0.0 {
            let z = fit_zoom(
                imp.mode.get(),
                &info.sizes,
                imp.rotation.get(),
                imp.dual.get(),
                w,
                h,
                imp.zoom.get(),
            );
            if (z - imp.zoom.get()).abs() > 1e-9 {
                imp.zoom.set(z);
                self.emit_zoom();
            }
        }
        let new = Layout::compute(&LayoutParams {
            pages: &info.sizes,
            rotation: imp.rotation.get(),
            dual: imp.dual.get(),
            scale: layout::zoom_to_scale(imp.zoom.get()),
            viewport_w: w,
        });
        let (nx, ny) = match anchored {
            Some((p, (px, py))) if p < new.pages.len() => {
                let (pw, ph) = info.sizes[p];
                let (cx, cy) = new.page_to_content(p, px, py, pw, ph);
                (cx - anchor.0, cy - anchor.1)
            }
            _ => (sx, sy),
        };
        let (cw, ch) = (new.width, new.height);
        *imp.layout.borrow_mut() = new;
        self.configure_adjustments(w, h, cw, ch, nx, ny);
        self.queue_draw();
    }

    fn configure_adjustments(&self, w: f64, h: f64, cw: f64, ch: f64, x: f64, y: f64) {
        let imp = self.imp();
        if let Some(a) = imp.hadj.borrow().as_ref() {
            let x = x.clamp(0.0, (cw - w).max(0.0));
            a.configure(x, 0.0, cw.max(w), 40.0, w * 0.9, w);
        }
        if let Some(a) = imp.vadj.borrow().as_ref() {
            let y = y.clamp(0.0, (ch - h).max(0.0));
            a.configure(y, 0.0, ch.max(h), 40.0, (h - 40.0).max(h * 0.5), h);
        }
    }

    fn on_scrolled(&self) {
        self.queue_draw();
        // Link stand-ins follow the pages.
        self.queue_allocate();
        self.queue_sync_links();
        if !self.imp().in_goto.get() {
            self.update_current();
        }
    }

    fn update_current(&self) {
        let imp = self.imp();
        let l = imp.layout.borrow();
        if l.pages.is_empty() {
            return;
        }
        let p = l.current_page(&self.viewport());
        drop(l);
        self.set_current(p);
    }

    fn set_current(&self, p: usize) {
        let imp = self.imp();
        if imp.current.get() != p {
            imp.current.set(p);
            if let Some(cb) = imp.callbacks.borrow().page_changed.as_ref() {
                cb(p);
            }
            self.refresh_a11y_page();
        }
    }

    // ----- zoom / layout controls -----

    pub fn set_zoom_mode(&self, mode: ZoomMode) {
        self.imp().mode.set(mode);
        self.relayout((self.width() as f64 / 2.0, 0.0));
        self.emit_zoom();
    }

    /// Sets an explicit zoom around `anchor` (viewport point; centre when
    /// `None`).
    pub fn set_zoom(&self, zoom: f64, anchor: Option<(f64, f64)>) {
        let imp = self.imp();
        imp.mode.set(ZoomMode::Free);
        imp.zoom.set(layout::clamp_zoom(zoom));
        let a = anchor.unwrap_or((self.width() as f64 / 2.0, self.height() as f64 / 2.0));
        self.relayout(a);
        self.emit_zoom();
    }

    pub fn zoom_in(&self, anchor: Option<(f64, f64)>) {
        self.set_zoom(layout::zoom_in_step(self.zoom()), anchor);
    }

    pub fn zoom_out(&self, anchor: Option<(f64, f64)>) {
        self.set_zoom(layout::zoom_out_step(self.zoom()), anchor);
    }

    pub fn set_rotation(&self, rotation: u32) {
        let p = self.current_page();
        self.imp()
            .rotation
            .set(layout::norm_rotation(rotation as i32));
        self.relayout((0.0, 0.0));
        self.go_to(p, None, None);
    }

    pub fn set_dual(&self, dual: bool) {
        let p = self.current_page();
        self.imp().dual.set(dual);
        self.relayout((0.0, 0.0));
        self.go_to(p, None, None);
    }

    // ----- navigation -----

    /// Scrolls to `page`, optionally to a top/left position (page points).
    pub fn go_to(&self, page: usize, top: Option<f64>, left: Option<f64>) {
        let imp = self.imp();
        let Some(model) = self.model() else { return };
        let n = model.n_pages();
        if n == 0 {
            return;
        }
        let page = page.min(n - 1);
        if self.width() <= 0 || imp.layout.borrow().pages.is_empty() {
            imp.pending_goto.set(Some((page, top, left)));
            imp.current.set(page);
            return;
        }
        let l = imp.layout.borrow().clone();
        let r = l.pages[page];
        let (pw, ph) = model.size(page);
        let (sx, _) = self.scroll();
        let (x, y) = match top {
            Some(t) => {
                let (cx, cy) = l.page_to_content(page, left.unwrap_or(0.0), t, pw, ph);
                let x = if left.is_some() { cx - MARGIN } else { sx };
                (x, cy - MARGIN)
            }
            None => (sx, r.y - MARGIN),
        };
        imp.in_goto.set(true);
        if let Some(a) = imp.hadj.borrow().as_ref() {
            a.set_value(x);
        }
        if let Some(a) = imp.vadj.borrow().as_ref() {
            a.set_value(y);
        }
        imp.in_goto.set(false);
        imp.current.set(usize::MAX);
        self.set_current(page);
        self.queue_draw();
    }

    /// Makes a rectangle (page points) of `page` visible.
    pub fn show_rect(&self, page: usize, rect: &Rect) {
        let imp = self.imp();
        let Some(model) = self.model() else { return };
        if imp.layout.borrow().pages.len() <= page || self.width() <= 0 {
            self.go_to(page, Some(rect.y), None);
            return;
        }
        let l = imp.layout.borrow().clone();
        let (pw, ph) = model.size(page);
        let r = page_rect_to_content(rect, &l.pages[page], pw, ph, l.rotation, l.scale);
        let view = self.viewport();
        imp.in_goto.set(true);
        if r.y < view.y || r.bottom() > view.bottom() {
            if let Some(a) = imp.vadj.borrow().as_ref() {
                a.set_value(r.y - view.h / 3.0);
            }
        }
        if r.x < view.x || r.right() > view.right() {
            if let Some(a) = imp.hadj.borrow().as_ref() {
                a.set_value(r.x - view.w / 3.0);
            }
        }
        imp.in_goto.set(false);
        imp.current.set(usize::MAX);
        self.set_current(page);
        self.queue_draw();
    }

    pub fn scroll_by(&self, dx: f64, dy: f64) {
        let imp = self.imp();
        if dx != 0.0 {
            if let Some(a) = imp.hadj.borrow().as_ref() {
                a.set_value(a.value() + dx);
            }
        }
        if dy != 0.0 {
            if let Some(a) = imp.vadj.borrow().as_ref() {
                a.set_value(a.value() + dy);
            }
        }
    }

    pub fn scroll_pages(&self, n: f64) {
        let inc = self
            .imp()
            .vadj
            .borrow()
            .as_ref()
            .map(|a| a.page_increment())
            .unwrap_or(0.0);
        self.scroll_by(0.0, n * inc);
    }

    pub fn next_page(&self) {
        let step = if self.dual() { 2 } else { 1 };
        let p = self.current_page() + step;
        if let Some(m) = self.model() {
            if p < m.n_pages() {
                self.go_to(p, None, None);
            }
        }
    }

    pub fn prev_page(&self) {
        let step = if self.dual() { 2 } else { 1 };
        self.go_to(self.current_page().saturating_sub(step), None, None);
    }

    // ----- selection -----

    fn sel_point(&self, vx: f64, vy: f64) -> Option<SelPoint> {
        let model = self.model()?;
        let l = self.imp().layout.borrow();
        let (sx, sy) = self.scroll();
        let (cx, cy) = (vx + sx, vy + sy);
        let page = l.page_at(cx, cy).or_else(|| l.nearest_page(cx, cy))?;
        let (pw, ph) = model.size(page);
        Some(SelPoint {
            page,
            pt: l.content_to_page(page, cx, cy, pw, ph),
        })
    }

    /// Selection rectangles per page (page points).
    pub fn selection_rects(&self) -> Vec<(usize, Rect)> {
        let imp = self.imp();
        let (Some(a), Some(b), Some(model)) =
            (imp.sel_anchor.get(), imp.sel_end.get(), self.model())
        else {
            return Vec::new();
        };
        (a.page.min(b.page)..=a.page.max(b.page))
            .filter_map(|i| {
                selection_rect_for_page(i, a.page, a.pt, b.page, b.pt, model.size(i))
                    .map(|r| (i, r))
            })
            .collect()
    }

    pub fn has_selection(&self) -> bool {
        let imp = self.imp();
        (imp.sel_anchor.get().is_some() && imp.sel_end.get().is_some())
            || imp.kbd_sel.get().is_some()
    }

    pub fn selection_style(&self) -> Style {
        self.imp().sel_style.get()
    }

    fn update_selection(&self) {
        let imp = self.imp();
        let Some(model) = self.model() else { return };
        // A pointer selection replaces a keyboard one.
        imp.kbd_sel.set(None);
        imp.caret_anchor.set(None);
        imp.sel_gen.set(imp.sel_gen.get() + 1);
        let rects = self.selection_rects();
        if rects.is_empty() {
            model.selection.borrow_mut().clear();
            self.queue_draw();
        } else {
            model
                .renderer
                .select_region(imp.sel_gen.get(), rects, imp.sel_style.get());
        }
        if let Some(cb) = imp.callbacks.borrow().selection_changed.as_ref() {
            cb(self.has_selection());
        }
    }

    pub fn selection_gen(&self) -> u64 {
        self.imp().sel_gen.get()
    }

    pub fn clear_selection(&self) {
        let imp = self.imp();
        imp.caret_anchor.set(None);
        if imp.sel_anchor.get().is_none() {
            if imp.kbd_sel.get().is_some() {
                self.update_selection();
            }
            return;
        }
        imp.sel_anchor.set(None);
        imp.sel_end.set(None);
        self.update_selection();
    }

    pub fn select_all(&self) {
        let Some(model) = self.model() else { return };
        let n = model.n_pages();
        if n == 0 {
            return;
        }
        let imp = self.imp();
        imp.sel_style.set(Style::Glyph);
        imp.sel_anchor.set(Some(SelPoint {
            page: 0,
            pt: (0.0, 0.0),
        }));
        imp.sel_end.set(Some(SelPoint {
            page: n - 1,
            pt: model.size(n - 1),
        }));
        self.update_selection();
    }

    /// Asks the render thread for the selected text.
    pub fn request_selected_text(&self, purpose: TextPurpose) {
        if let Some(text) = self.keyboard_selection_text() {
            if !text.is_empty() {
                match purpose {
                    TextPurpose::Clipboard => self.clipboard().set_text(&text),
                    TextPurpose::Primary => self.primary_clipboard().set_text(&text),
                }
            }
            return;
        }
        let rects = self.selection_rects();
        if let (false, Some(model)) = (rects.is_empty(), self.model()) {
            model
                .renderer
                .selected_text(rects, self.selection_style(), purpose);
        }
    }

    // ----- input -----

    fn setup_input(&self) {
        // Selection / link clicks.
        let drag = gtk::GestureDrag::new();
        drag.set_button(gdk::BUTTON_PRIMARY);
        drag.connect_drag_begin(glib::clone!(
            #[weak(rename_to = view)]
            self,
            move |_, x, y| {
                view.grab_focus();
                let imp = view.imp();
                imp.dragging.set(false);
                *imp.press_link.borrow_mut() = view.link_at_view(x, y);
            }
        ));
        drag.connect_drag_update(glib::clone!(
            #[weak(rename_to = view)]
            self,
            move |g, dx, dy| {
                let imp = view.imp();
                let Some((x0, y0)) = g.start_point() else {
                    return;
                };
                if !imp.dragging.get() {
                    if dx.hypot(dy) < 4.0 {
                        return;
                    }
                    imp.dragging.set(true);
                    imp.press_link.replace(None);
                    imp.sel_style.set(Style::Glyph);
                    imp.sel_anchor.set(view.sel_point(x0, y0));
                }
                imp.sel_end.set(view.sel_point(x0 + dx, y0 + dy));
                view.update_selection();
            }
        ));
        drag.connect_drag_end(glib::clone!(
            #[weak(rename_to = view)]
            self,
            move |_, _, _| {
                let imp = view.imp();
                if imp.dragging.get() {
                    imp.dragging.set(false);
                    view.request_selected_text(TextPurpose::Primary);
                    return;
                }
                if let Some(target) = imp.press_link.take() {
                    if let Some(cb) = imp.callbacks.borrow().link_activated.as_ref() {
                        cb(target);
                    }
                    return;
                }
                if imp.click_count.get() < 2 {
                    view.clear_selection();
                }
            }
        ));
        self.add_controller(drag);

        // Word / line selection on double / triple click.
        let click = gtk::GestureClick::new();
        click.set_button(gdk::BUTTON_PRIMARY);
        click.connect_pressed(glib::clone!(
            #[weak(rename_to = view)]
            self,
            move |_, n, x, y| {
                let imp = view.imp();
                imp.click_count.set(n);
                if n >= 2 {
                    if let Some(p) = view.sel_point(x, y) {
                        imp.sel_style
                            .set(if n == 2 { Style::Word } else { Style::Line });
                        imp.sel_anchor.set(Some(p));
                        imp.sel_end.set(Some(p));
                        view.update_selection();
                        view.request_selected_text(TextPurpose::Primary);
                    }
                }
            }
        ));
        self.add_controller(click);

        // Middle-button panning.
        let pan = gtk::GestureDrag::new();
        pan.set_button(gdk::BUTTON_MIDDLE);
        pan.connect_drag_begin(glib::clone!(
            #[weak(rename_to = view)]
            self,
            move |_, _, _| {
                view.imp().pan_start.set(view.scroll());
                view.set_cursor_from_name(Some("grabbing"));
            }
        ));
        pan.connect_drag_update(glib::clone!(
            #[weak(rename_to = view)]
            self,
            move |_, dx, dy| {
                let (h, v) = view.imp().pan_start.get();
                let (sh, sv) = view.scroll();
                view.scroll_by(h - dx - sh, v - dy - sv);
            }
        ));
        pan.connect_drag_end(glib::clone!(
            #[weak(rename_to = view)]
            self,
            move |_, _, _| view.set_cursor_from_name(None)
        ));
        self.add_controller(pan);

        // Hover: cursor and link tooltips.
        let motion = gtk::EventControllerMotion::new();
        motion.connect_motion(glib::clone!(
            #[weak(rename_to = view)]
            self,
            move |_, x, y| {
                view.imp().pointer.set(Some((x, y)));
                view.update_hover(x, y)
            }
        ));
        motion.connect_leave(glib::clone!(
            #[weak(rename_to = view)]
            self,
            move |_| view.imp().pointer.set(None)
        ));
        self.add_controller(motion);

        // Ctrl+scroll zoom.
        let scroll = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::BOTH_AXES);
        scroll.connect_scroll(glib::clone!(
            #[weak(rename_to = view)]
            self,
            #[upgrade_or]
            glib::Propagation::Proceed,
            move |c, _dx, dy| {
                if !c
                    .current_event_state()
                    .contains(gdk::ModifierType::CONTROL_MASK)
                {
                    return glib::Propagation::Proceed;
                }
                let anchor = view.imp().pointer.get();
                if c.unit() == gdk::ScrollUnit::Wheel {
                    if dy < 0.0 {
                        view.zoom_in(anchor);
                    } else if dy > 0.0 {
                        view.zoom_out(anchor);
                    }
                } else {
                    view.set_zoom(view.zoom() * (-dy / 100.0).exp(), anchor);
                }
                glib::Propagation::Stop
            }
        ));
        self.add_controller(scroll);

        // Pinch zoom.
        let pinch = gtk::GestureZoom::new();
        pinch.connect_begin(glib::clone!(
            #[weak(rename_to = view)]
            self,
            move |_, _| view.imp().pinch_start.set(view.zoom())
        ));
        pinch.connect_scale_changed(glib::clone!(
            #[weak(rename_to = view)]
            self,
            move |g, scale| {
                let anchor = g.bounding_box_center();
                view.set_zoom(view.imp().pinch_start.get() * scale, anchor);
            }
        ));
        self.add_controller(pinch);

        // Navigation keys.
        let keys = gtk::EventControllerKey::new();
        keys.connect_key_pressed(glib::clone!(
            #[weak(rename_to = view)]
            self,
            #[upgrade_or]
            glib::Propagation::Proceed,
            move |_, key, _, state| view.on_key(key, state)
        ));
        self.add_controller(keys);
    }

    fn on_key(&self, key: gdk::Key, state: gdk::ModifierType) -> glib::Propagation {
        use gdk::Key;
        let ctrl = state.contains(gdk::ModifierType::CONTROL_MASK);
        let shift = state.contains(gdk::ModifierType::SHIFT_MASK);
        let alt = state.contains(gdk::ModifierType::ALT_MASK);
        if alt {
            return glib::Propagation::Proceed;
        }
        let n = self.model().map(|m| m.n_pages()).unwrap_or(0);
        if matches!(key, Key::Return | Key::KP_Enter | Key::ISO_Enter) && !ctrl {
            if let Some(target) = self.focused_link() {
                if let Some(cb) = self.imp().callbacks.borrow().link_activated.as_ref() {
                    cb(target);
                }
                return glib::Propagation::Stop;
            }
        }
        if self.imp().caret_mode.get() && self.caret_key(key, ctrl, shift) {
            return glib::Propagation::Stop;
        }
        match key {
            Key::Page_Down | Key::KP_Page_Down if ctrl => self.next_page(),
            Key::Page_Up | Key::KP_Page_Up if ctrl => self.prev_page(),
            Key::Page_Down | Key::KP_Page_Down => self.scroll_pages(1.0),
            Key::Page_Up | Key::KP_Page_Up => self.scroll_pages(-1.0),
            Key::space | Key::KP_Space if !ctrl => {
                self.scroll_pages(if shift { -1.0 } else { 1.0 })
            }
            Key::BackSpace if !ctrl => self.scroll_pages(-1.0),
            Key::Home | Key::KP_Home => self.go_to(0, None, None),
            Key::End | Key::KP_End => self.go_to(n.saturating_sub(1), None, None),
            Key::n if !ctrl => self.next_page(),
            Key::p if !ctrl => self.prev_page(),
            Key::d if !ctrl => {
                let _ = self.activate_action("win.dual", None);
            }
            Key::Down | Key::KP_Down if !ctrl => self.scroll_by(0.0, 40.0),
            Key::Up | Key::KP_Up if !ctrl => self.scroll_by(0.0, -40.0),
            Key::Left | Key::KP_Left if !ctrl => self.scroll_by(-40.0, 0.0),
            Key::Right | Key::KP_Right if !ctrl => self.scroll_by(40.0, 0.0),
            Key::Escape if self.has_selection() => self.clear_selection(),
            _ => return glib::Propagation::Proceed,
        }
        glib::Propagation::Stop
    }

    fn link_at_view(&self, x: f64, y: f64) -> Option<Target> {
        let model = self.model()?;
        let l = self.imp().layout.borrow();
        let (sx, sy) = self.scroll();
        let (cx, cy) = (x + sx, y + sy);
        let page = l.page_at(cx, cy)?;
        let (pw, ph) = model.size(page);
        let (px, py) = l.content_to_page(page, cx, cy, pw, ph);
        model.link_at(page, px, py)
    }

    fn update_hover(&self, x: f64, y: f64) {
        let Some(model) = self.model() else { return };
        if self.imp().dragging.get() {
            return;
        }
        let on_page = {
            let l = self.imp().layout.borrow();
            let (sx, sy) = self.scroll();
            l.page_at(x + sx, y + sy).is_some()
        };
        match self.link_at_view(x, y) {
            Some(t) => {
                self.set_cursor_from_name(Some("pointer"));
                let labels = model.info().map(|i| i.labels.clone()).unwrap_or_default();
                let tip = t.describe(&labels);
                if self.tooltip_text().as_deref() != Some(tip.as_str()) {
                    self.set_tooltip_text(Some(&tip));
                }
            }
            None => {
                self.set_cursor_from_name(if on_page { Some("text") } else { None });
                if self.tooltip_text().is_some() {
                    self.set_tooltip_text(None);
                }
            }
        }
    }

    // ----- drawing -----

    fn draw(&self, snap: &gtk::Snapshot) {
        let imp = self.imp();
        let Some(model) = self.model() else { return };
        let Some(info) = model.info() else { return };
        let l = imp.layout.borrow();
        if l.pages.is_empty() {
            return;
        }
        let view = self.viewport();
        let sf = self.scale_factor() as f64;
        let rot = l.rotation;
        let mut wants = Vec::new();
        let mut thumbs = Vec::new();
        let visible = l.visible(&view);
        let night = model.night.get();

        for &i in &visible {
            let r = l.pages[i];
            let dst = Rect::new(r.x - view.x, r.y - view.y, r.w, r.h);
            let rr = gsk::RoundedRect::from_rect(grect(&dst), 0.0);
            snap.append_outset_shadow(&rr, &rgba(0.0, 0.0, 0.0, 0.25), 0.0, 1.0, 0.0, 3.0);
            if night {
                push_invert(snap);
            }
            let (pw, ph) = info.sizes[i];
            let vis = visible_page_area(&l, i, &view, pw, ph);
            paint_page(
                snap,
                &model,
                i,
                &dst,
                rot,
                l.scale,
                sf,
                vis,
                &mut wants,
                &mut thumbs,
            );
            if night {
                snap.pop();
            }
            model.request_links(i);
        }

        // Overlays: search hits and selection.
        let hl = Highlights::current(night);
        let search = model.search.borrow();
        let current_hit = search.current();
        for &i in &visible {
            let (pw, ph) = info.sizes[i];
            let page_rect = l.pages[i];
            for hi in search.on_page(i) {
                let h = &search.hits()[hi];
                let mut r = page_rect_to_content(&h.rect, &page_rect, pw, ph, rot, l.scale);
                r.x -= view.x;
                r.y -= view.y;
                let (fill, outline) = if Some(hi) == current_hit {
                    (hl.current_fill, Some(hl.current_outline))
                } else {
                    (hl.hit_fill, hl.hit_outline)
                };
                snap.append_color(&fill, &grect(&r));
                if let Some((color, width)) = outline {
                    // Grow the box by the line width so the outline does
                    // not cover the matched text.
                    let o = Rect::new(
                        r.x - width as f64,
                        r.y - width as f64,
                        r.w + 2.0 * width as f64,
                        r.h + 2.0 * width as f64,
                    );
                    let rr = gsk::RoundedRect::from_rect(grect(&o), 0.0);
                    snap.append_border(&rr, &[width; 4], &[color; 4]);
                }
            }
        }
        drop(search);
        let sel = model.selection.borrow();
        for (i, rects) in sel.iter() {
            if !visible.contains(i) {
                continue;
            }
            let (pw, ph) = info.sizes[*i];
            for pr in rects {
                let mut r = page_rect_to_content(pr, &l.pages[*i], pw, ph, rot, l.scale);
                r.x -= view.x;
                r.y -= view.y;
                snap.append_color(&hl.selection, &grect(&r));
            }
        }
        drop(sel);

        // The caret, in the text colour of the page.
        if let (true, Some((p, o))) = (imp.caret_mode.get(), imp.caret.get()) {
            if let (true, Some(text)) = (visible.contains(&p), model.text(p)) {
                if let Some(cr) = text.caret_rect(o) {
                    let (pw, ph) = info.sizes[p];
                    let mut r = page_rect_to_content(&cr, &l.pages[p], pw, ph, rot, l.scale);
                    r.x -= view.x;
                    r.y -= view.y;
                    // The caret box is empty along the text direction;
                    // make it two pixels thick whichever way it runs.
                    r.w = r.w.max(2.0);
                    r.h = r.h.max(2.0);
                    let c = if night { 1.0 } else { 0.0 };
                    snap.append_color(&rgba(c, c, c, 1.0), &grect(&r));
                }
            }
        }

        // Prefetch one viewport above and below.
        let ext = Rect::new(view.x, view.y - view.h, view.w, view.h * 3.0);
        for i in l.visible(&ext) {
            let (pw, ph) = info.sizes[i];
            if let Some(v) = visible_page_area(&l, i, &ext, pw, ph) {
                missing_tiles(&model, i, l.scale, sf, &v, &mut wants);
            }
        }
        wants.extend(thumbs);
        model.renderer.set_wants(Client::View, wants);
    }
}

// ----- links, caret and accessible text -----

impl DocView {
    /// Called when the links of a page have arrived.
    pub fn links_changed(&self) {
        self.queue_sync_links();
    }

    /// Called when the text of `page` has arrived.
    pub fn text_arrived(&self, page: usize) {
        let imp = self.imp();
        if page == imp.a11y_page.get() && self.has_atspi() {
            if let Some(t) = self.a11y_text().filter(|t| !t.is_empty()) {
                self.update_contents(gtk::AccessibleTextContentChange::Insert, 0, t.len() as u32);
            }
        }
        if imp.caret_mode.get() && imp.caret.get().map(|c| c.0) == Some(page) {
            self.queue_draw();
        }
    }

    /// Whether accessibility notifications can be sent (GTK's fallback
    /// accessibility context crashes on some of them).
    fn has_atspi(&self) -> bool {
        self.at_context().type_().name() == "GtkAtSpiContext"
    }

    /// The page exposed as accessible text: the caret's page in caret
    /// mode, otherwise the current page.
    fn a11y_target_page(&self) -> usize {
        let imp = self.imp();
        match (imp.caret_mode.get(), imp.caret.get()) {
            (true, Some((p, _))) => p,
            _ => imp.current.get(),
        }
    }

    fn a11y_text(&self) -> Option<Rc<PageText>> {
        self.model()?.text(self.imp().a11y_page.get())
    }

    /// Switches the accessible text to the target page, telling assistive
    /// technology that the text changed, and describes the position.
    fn refresh_a11y_page(&self) {
        let imp = self.imp();
        let Some(model) = self.model() else { return };
        let n = model.n_pages();
        let p = self.a11y_target_page();
        if p >= n {
            return;
        }
        model.request_text(p);
        if imp.a11y_page.get() != p {
            let atspi = self.has_atspi();
            // Removal is reported while the old text is still exposed.
            if let (true, Some(old)) = (atspi, self.a11y_text()) {
                if !old.is_empty() {
                    self.update_contents(
                        gtk::AccessibleTextContentChange::Remove,
                        0,
                        old.len() as u32,
                    );
                }
            }
            imp.a11y_page.set(p);
            if let (true, Some(new)) = (atspi, self.a11y_text()) {
                if !new.is_empty() {
                    self.update_contents(
                        gtk::AccessibleTextContentChange::Insert,
                        0,
                        new.len() as u32,
                    );
                }
            }
        }
        self.update_property(&[gtk::accessible::Property::Description(&page_description(
            &model.label(p),
            p,
            n,
        ))]);
    }

    // ----- link stand-ins -----

    fn queue_sync_links(&self) {
        let imp = self.imp();
        if imp.spots_queued.replace(true) {
            return;
        }
        let weak = self.downgrade();
        glib::idle_add_local_once(move || {
            if let Some(view) = weak.upgrade() {
                view.imp().spots_queued.set(false);
                view.sync_links();
            }
        });
    }

    /// Makes the link stand-ins match the links of the visible pages, in
    /// reading order, keeping existing ones (and their focus).
    fn sync_links(&self) {
        let imp = self.imp();
        let Some(model) = self.model() else { return };
        let visible = self.visible_pages();
        let want: Vec<(usize, usize, Link)> = {
            let links = model.links.borrow();
            visible
                .iter()
                .filter_map(|p| links.get(p).map(|v| (*p, v)))
                .flat_map(|(p, v)| v.iter().enumerate().map(move |(i, l)| (p, i, l.clone())))
                .collect()
        };
        let same = {
            let spots = imp.spots.borrow();
            spots.len() == want.len()
                && spots.iter().zip(&want).all(|(s, (p, i, l))| {
                    s.key() == (*p, *i) && s.imp().link.borrow().as_ref() == Some(l)
                })
        };
        if same {
            return;
        }
        let labels = model.info().map(|i| i.labels.clone()).unwrap_or_default();
        let old = imp.spots.take();
        let mut new = Vec::with_capacity(want.len());
        for (p, i, link) in &want {
            let spot = old
                .iter()
                .find(|s| s.key() == (*p, *i))
                .cloned()
                .unwrap_or_else(|| {
                    let s = LinkSpot::new();
                    s.set_parent(self);
                    s
                });
            spot.set_link(*p, *i, link, &labels);
            new.push(spot);
        }
        for s in old.iter().filter(|s| !new.contains(s)) {
            if s.has_focus() {
                self.grab_focus();
            }
            s.unparent();
        }
        for s in &new {
            s.insert_before(self, None::<&gtk::Widget>);
        }
        *imp.spots.borrow_mut() = new;
        self.queue_allocate();
    }

    fn visible_pages(&self) -> Vec<usize> {
        let l = self.imp().layout.borrow();
        if l.pages.is_empty() {
            Vec::new()
        } else {
            l.visible(&self.viewport())
        }
    }

    /// Places the link stand-ins over their links.
    fn allocate_spots(&self) {
        let imp = self.imp();
        let Some(model) = self.model() else { return };
        let l = imp.layout.borrow();
        let (sx, sy) = self.scroll();
        for spot in imp.spots.borrow().iter() {
            let Some((p, rect)) = spot.page_rect() else {
                continue;
            };
            let Some(page) = l.pages.get(p) else { continue };
            let (pw, ph) = model.size(p);
            let r = page_rect_to_content(&rect, page, pw, ph, l.rotation, l.scale);
            spot.measure(gtk::Orientation::Horizontal, -1);
            spot.size_allocate(
                &gtk::Allocation::new(
                    (r.x - sx).round() as i32,
                    (r.y - sy).round() as i32,
                    r.w.round().max(1.0) as i32,
                    r.h.round().max(1.0) as i32,
                ),
                -1,
            );
        }
    }

    /// The target of the link that has keyboard focus.
    pub fn focused_link(&self) -> Option<Target> {
        self.imp()
            .spots
            .borrow()
            .iter()
            .find(|s| s.has_focus())
            .and_then(|s| s.target())
    }

    /// Where a keyboard-opened context menu points: the focused link, the
    /// caret, or the middle of the view (view coordinates).
    pub fn context_point(&self) -> (f64, f64) {
        let imp = self.imp();
        if let Some(spot) = imp.spots.borrow().iter().find(|s| s.has_focus()) {
            if let Some(b) = spot.compute_bounds(self) {
                return (
                    (b.x() + b.width() / 2.0) as f64,
                    (b.y() + b.height()) as f64,
                );
            }
        }
        if let (true, Some((p, o)), Some(model)) =
            (imp.caret_mode.get(), imp.caret.get(), self.model())
        {
            if let Some(cr) = model.text(p).and_then(|t| t.caret_rect(o)) {
                let l = imp.layout.borrow();
                if let Some(page) = l.pages.get(p) {
                    let (pw, ph) = model.size(p);
                    let r = page_rect_to_content(&cr, page, pw, ph, l.rotation, l.scale);
                    let (sx, sy) = self.scroll();
                    return (r.x - sx, r.bottom() - sy);
                }
            }
        }
        (self.width() as f64 / 2.0, self.height() as f64 / 2.0)
    }

    // ----- caret navigation -----

    pub fn caret_mode(&self) -> bool {
        self.imp().caret_mode.get()
    }

    /// Turns caret navigation on or off. With it on, the arrow keys move
    /// a text caret instead of scrolling, and Shift extends a selection.
    pub fn set_caret_mode(&self, on: bool) {
        let imp = self.imp();
        if imp.caret_mode.replace(on) == on {
            return;
        }
        if on {
            self.place_caret();
            self.grab_focus();
        }
        self.refresh_a11y_page();
        if let Some(cb) = imp.callbacks.borrow().caret_mode_changed.as_ref() {
            cb(on);
        }
        self.queue_draw();
    }

    /// Puts the caret at the start of the current page unless it is
    /// already on a visible page.
    fn place_caret(&self) {
        let imp = self.imp();
        let Some(model) = self.model() else { return };
        if model.n_pages() == 0 {
            imp.caret.set(None);
            return;
        }
        let visible = self.visible_pages();
        if imp.caret.get().is_some_and(|(p, _)| visible.contains(&p)) {
            return;
        }
        let p = imp.current.get().min(model.n_pages() - 1);
        imp.caret.set(Some((p, 0)));
        imp.caret_x.set(None);
        self.prefetch_text_around(p);
        self.refresh_a11y_page();
    }

    fn prefetch_text_around(&self, p: usize) {
        if let Some(model) = self.model() {
            model.request_text(p);
            model.request_text(p + 1);
            if p > 0 {
                model.request_text(p - 1);
            }
        }
    }

    /// Handles a key in caret mode; returns whether it was used.
    fn caret_key(&self, key: gdk::Key, ctrl: bool, shift: bool) -> bool {
        use gdk::Key;
        let imp = self.imp();
        let Some(model) = self.model() else {
            return false;
        };
        let Some((p, o)) = imp.caret.get() else {
            return false;
        };
        let Some(text) = model.text(p) else {
            // Text not here yet; swallow movement keys until it is.
            model.request_text(p);
            return matches!(
                key,
                Key::Left | Key::Right | Key::Up | Key::Down | Key::Home | Key::End
            );
        };
        let n = model.n_pages();
        let prev_end = |p: usize| -> Option<Caret> {
            let p = p.checked_sub(1)?;
            model.text(p).map(|t| (p, t.len()))
        };
        let next_start = |p: usize| -> Option<Caret> {
            (p + 1 < n && model.text(p + 1).is_some()).then_some((p + 1, 0))
        };
        let x = imp.caret_x.get().or_else(|| text.caret_x(o)).unwrap_or(0.0);
        let (to, keep_x) = match key {
            Key::Left | Key::KP_Left if ctrl => {
                let w = text.prev_word(o);
                (if w == o { prev_end(p) } else { Some((p, w)) }, false)
            }
            Key::Right | Key::KP_Right if ctrl => {
                let w = text.next_word(o);
                (if w == o { next_start(p) } else { Some((p, w)) }, false)
            }
            Key::Left | Key::KP_Left => (
                if o == 0 {
                    prev_end(p)
                } else {
                    Some((p, o - 1))
                },
                false,
            ),
            Key::Right | Key::KP_Right => (
                if o >= text.len() {
                    next_start(p)
                } else {
                    Some((p, o + 1))
                },
                false,
            ),
            Key::Up | Key::KP_Up if !ctrl => {
                let to = text.vertical(o, x, true).map(|o| (p, o)).or_else(|| {
                    let q = p.checked_sub(1)?;
                    let t = model.text(q)?;
                    let (s, e) = t.line_bounds(t.len());
                    Some((q, t.closest_on_line(s, e, x)))
                });
                (to, true)
            }
            Key::Down | Key::KP_Down if !ctrl => {
                let to = text.vertical(o, x, false).map(|o| (p, o)).or_else(|| {
                    let t = model.text(p + 1).filter(|_| p + 1 < n)?;
                    let (s, e) = t.line_bounds(0);
                    Some((p + 1, t.closest_on_line(s, e, x)))
                });
                (to, true)
            }
            Key::Home | Key::KP_Home if !ctrl => (Some((p, text.line_bounds(o).0)), false),
            Key::End | Key::KP_End if !ctrl => (Some((p, text.line_bounds(o).1)), false),
            _ => return false,
        };
        match to {
            Some(c) => self.move_caret(c, shift, keep_x.then_some(x)),
            None => {
                // At a page edge whose neighbour is not loaded yet.
                self.prefetch_text_around(p);
                self.error_bell();
            }
        }
        true
    }

    /// Moves the caret, extending the keyboard selection when `extend`.
    pub fn move_caret(&self, to: Caret, extend: bool, keep_x: Option<f64>) {
        let imp = self.imp();
        let Some(model) = self.model() else { return };
        let from = imp.caret.get();
        if extend {
            if imp.caret_anchor.get().is_none() {
                // Continue an existing keyboard selection from its far end.
                let anchor = match (imp.kbd_sel.get(), from) {
                    (Some((a, b)), Some(f)) if b == f => Some(a),
                    (Some((a, b)), Some(f)) if a == f => Some(b),
                    _ => from,
                };
                imp.caret_anchor.set(anchor);
            }
        } else if self.has_selection() {
            self.clear_selection();
        }
        imp.caret.set(Some(to));
        imp.caret_x.set(keep_x);
        self.prefetch_text_around(to.0);
        if let Some(r) = model.text(to.0).and_then(|t| t.caret_rect(to.1)) {
            let pad = Rect::new(r.x - 20.0, r.y - 10.0, r.w + 40.0, r.h + 20.0);
            self.show_rect(to.0, &pad);
        }
        if extend {
            let anchor = imp.caret_anchor.get().unwrap_or(to);
            self.set_keyboard_selection(anchor, to);
        }
        self.refresh_a11y_page();
        if self.has_atspi() {
            self.update_caret_position();
            if extend {
                self.update_selection_bound();
            }
        }
        self.queue_draw();
    }

    fn set_keyboard_selection(&self, a: Caret, b: Caret) {
        let imp = self.imp();
        let Some(model) = self.model() else { return };
        let (s, e) = if a <= b { (a, b) } else { (b, a) };
        imp.sel_anchor.set(None);
        imp.sel_end.set(None);
        imp.sel_gen.set(imp.sel_gen.get() + 1);
        imp.kbd_sel.set((s != e).then_some((s, e)));
        let regions: Vec<(usize, Vec<Rect>)> = (s.0..=e.0)
            .filter_map(|p| {
                let t = model.text(p)?;
                let from = if p == s.0 { s.1 } else { 0 };
                let to = if p == e.0 { e.1 } else { t.len() };
                Some((p, t.range_rects(from, to)))
            })
            .collect();
        *model.selection.borrow_mut() = regions;
        if let Some(cb) = imp.callbacks.borrow().selection_changed.as_ref() {
            cb(self.has_selection());
        }
        self.request_selected_text(TextPurpose::Primary);
    }

    fn keyboard_selection_text(&self) -> Option<String> {
        let (s, e) = self.imp().kbd_sel.get()?;
        let model = self.model()?;
        Some(
            (s.0..=e.0)
                .filter_map(|p| {
                    let t = model.text(p)?;
                    let from = if p == s.0 { s.1 } else { 0 };
                    let to = if p == e.0 { e.1 } else { t.len() };
                    Some(t.slice(from, to))
                })
                .collect::<Vec<_>>()
                .join("\n"),
        )
    }

    /// Moves the caret by a word. Ctrl+Left / Ctrl+Right are also the
    /// rotate shortcuts; in caret mode the rotate actions call this
    /// (Ctrl+Shift+arrows reach the view directly).
    pub fn caret_word(&self, forward: bool) {
        let state = gdk::ModifierType::CONTROL_MASK;
        let key = if forward {
            gdk::Key::Right
        } else {
            gdk::Key::Left
        };
        self.on_key(key, state);
    }
}

/// "Page iv (4 of 120)" or "Page 4 of 120".
pub fn page_description(label: &str, page: usize, n: usize) -> String {
    let num = (page + 1).to_string();
    if label == num {
        trf!("Page {num} of {n}", num = num, n = n)
    } else {
        trf!(
            "Page {label} ({num} of {n})",
            label = label,
            num = num,
            n = n
        )
    }
}

#[cfg(test)]
mod tests {
    use super::{page_description, Highlights};
    use gtk::gdk;

    #[test]
    fn page_descriptions() {
        assert_eq!(page_description("4", 3, 120), "Page 4 of 120");
        assert_eq!(page_description("iv", 3, 120), "Page iv (4 of 120)");
    }

    #[test]
    fn highlights_follow_accent_and_contrast() {
        let purple = gdk::RGBA::new(0.57, 0.25, 0.68, 1.0);
        let h = Highlights::new(purple, false, false);
        assert_eq!(
            (h.selection.red(), h.selection.green(), h.selection.blue()),
            (0.57, 0.25, 0.68)
        );
        // The current hit has an outline; other hits none.
        assert!(h.current_outline.1 >= 2.0);
        assert!(h.hit_outline.is_none());
        let hc = Highlights::new(purple, true, false);
        assert!(hc.selection.alpha() > h.selection.alpha());
        assert!(hc.hit_fill.alpha() > h.hit_fill.alpha());
        assert!(hc.current_outline.1 > h.current_outline.1);
        assert!(hc.hit_outline.is_some());
        // Night mode: light outline on the dark page.
        let night = Highlights::new(purple, false, true);
        assert_eq!(night.current_outline.0.red(), 1.0);
        assert_eq!(h.current_outline.0.red(), 0.0);
    }
}
