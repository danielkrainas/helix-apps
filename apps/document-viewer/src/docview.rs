//! The continuous document view widget and the shared document model.

use crate::cache::Lru;
use crate::layout::{
    self, fit_zoom, page_px, page_rect_to_content, selection_rect_for_page, tile_rect, tiles_for,
    visible_page_area, Layout, LayoutParams, Rect, ZoomMode, MARGIN,
};
use crate::links::{Link, Target};
use crate::render::{scale_key, Client, DocInfo, JobKey, Renderer, Style, TextPurpose};
use crate::search::Results;
use gtk::prelude::*;
use gtk::subclass::prelude::*;
use gtk::{gdk, glib, graphene, gsk};
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
}

#[derive(Clone, Copy, Debug)]
pub struct SelPoint {
    page: usize,
    pt: (f64, f64),
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
            }
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for DocView {
        const NAME: &'static str = "HelixDocView";
        type Type = super::DocView;
        type ParentType = gtk::Widget;
        type Interfaces = (gtk::Scrollable,);

        fn class_init(klass: &mut Self::Class) {
            klass.set_css_name("docview");
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
        }

        fn dispose(&self) {
            for (adj, id) in self.adj_handlers.take() {
                adj.disconnect(id);
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
        }

        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            self.obj().draw(snapshot);
        }
    }

    impl ScrollableImpl for DocView {}

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
        @implements gtk::Scrollable, gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
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
        *imp.layout.borrow_mut() = Layout::default();
        if let Some(a) = imp.vadj.borrow().as_ref() {
            a.set_value(0.0);
        }
        self.relayout((0.0, 0.0));
        self.update_current();
        self.emit_zoom();
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
        self.imp().sel_anchor.get().is_some() && self.imp().sel_end.get().is_some()
    }

    pub fn selection_style(&self) -> Style {
        self.imp().sel_style.get()
    }

    fn update_selection(&self) {
        let imp = self.imp();
        let Some(model) = self.model() else { return };
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
        if imp.sel_anchor.get().is_none() {
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
                let color = if Some(hi) == current_hit {
                    rgba(1.0, 0.47, 0.0, 0.45)
                } else {
                    rgba(0.96, 0.83, 0.18, 0.40)
                };
                snap.append_color(&color, &grect(&r));
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
                snap.append_color(&rgba(0.21, 0.52, 0.89, 0.35), &grect(&r));
            }
        }
        drop(sel);

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
