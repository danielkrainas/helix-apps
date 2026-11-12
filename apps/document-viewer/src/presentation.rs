//! Full-screen presentation mode: one page at a time on black.

use crate::docview::{paint_page, push_invert, Model};
use crate::layout::{rotated_size, Rect};
use crate::render::{Client, JobKey};
use gtk::prelude::*;
use gtk::subclass::prelude::*;
use gtk::{gdk, glib, graphene};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct PresentationView {
        pub model: RefCell<Option<Rc<Model>>>,
        pub page: Cell<usize>,
        pub rotation: Cell<u32>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for PresentationView {
        const NAME: &'static str = "HelixPresentationView";
        type Type = super::PresentationView;
        type ParentType = gtk::Widget;

        fn class_init(klass: &mut Self::Class) {
            klass.set_css_name("presentation");
        }
    }

    impl ObjectImpl for PresentationView {}

    impl WidgetImpl for PresentationView {
        fn snapshot(&self, snap: &gtk::Snapshot) {
            let obj = self.obj();
            let (w, h) = (obj.width() as f64, obj.height() as f64);
            snap.append_color(
                &gdk::RGBA::BLACK,
                &graphene::Rect::new(0.0, 0.0, w as f32, h as f32),
            );
            let Some(model) = self.model.borrow().clone() else {
                return;
            };
            let n = model.n_pages();
            if n == 0 || w <= 0.0 || h <= 0.0 {
                return;
            }
            let page = self.page.get().min(n - 1);
            let rot = self.rotation.get();
            let sf = obj.scale_factor() as f64;
            let mut wants = Vec::new();
            let mut thumbs = Vec::new();
            let mut draw = |p: usize, visible: bool, wants: &mut Vec<JobKey>| {
                let (pw, ph) = model.size(p);
                let (rw, rh) = rotated_size(pw, ph, rot);
                let scale = (w / rw).min(h / rh);
                let dst = Rect::new(
                    ((w - rw * scale) / 2.0).floor(),
                    ((h - rh * scale) / 2.0).floor(),
                    rw * scale,
                    rh * scale,
                );
                let vis = Some(Rect::new(0.0, 0.0, pw, ph));
                if visible {
                    if model.night.get() {
                        push_invert(snap);
                    }
                    paint_page(
                        snap,
                        &model,
                        p,
                        &dst,
                        rot,
                        scale,
                        sf,
                        vis,
                        wants,
                        &mut thumbs,
                    );
                    if model.night.get() {
                        snap.pop();
                    }
                } else {
                    // Prefetch: paint into a throwaway snapshot.
                    let s = gtk::Snapshot::new();
                    paint_page(
                        &s,
                        &model,
                        p,
                        &dst,
                        rot,
                        scale,
                        sf,
                        vis,
                        wants,
                        &mut Vec::new(),
                    );
                }
            };
            draw(page, true, &mut wants);
            if page + 1 < n {
                draw(page + 1, false, &mut wants);
            }
            wants.extend(thumbs);
            model.renderer.set_wants(Client::Presentation, wants);
        }
    }
}

glib::wrapper! {
    pub struct PresentationView(ObjectSubclass<imp::PresentationView>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl PresentationView {
    pub fn page(&self) -> usize {
        self.imp().page.get()
    }

    fn set_page(&self, p: usize) {
        let n = self
            .imp()
            .model
            .borrow()
            .as_ref()
            .map(|m| m.n_pages())
            .unwrap_or(0);
        if n > 0 {
            self.imp().page.set(p.min(n - 1));
            self.queue_draw();
        }
    }
}

/// Opens a presentation window starting at `page`. `on_close` receives the
/// last presented page.
pub fn present(
    parent: &impl IsA<gtk::Window>,
    model: Rc<Model>,
    page: usize,
    rotation: u32,
    on_close: impl Fn(usize) + 'static,
) -> gtk::Window {
    let view: PresentationView = glib::Object::new();
    *view.imp().model.borrow_mut() = Some(model.clone());
    view.imp().rotation.set(rotation);
    view.set_page(page);
    view.set_focusable(true);

    let win = gtk::Window::builder()
        .transient_for(parent)
        .title("Presentation")
        .default_width(1024)
        .default_height(768)
        .child(&view)
        .build();
    win.add_css_class("presentation-window");

    let next = glib::clone!(
        #[weak]
        view,
        move || view.set_page(view.page() + 1)
    );
    let prev = glib::clone!(
        #[weak]
        view,
        move || view.set_page(view.page().saturating_sub(1))
    );

    let keys = gtk::EventControllerKey::new();
    {
        let (next, prev) = (next.clone(), prev.clone());
        keys.connect_key_pressed(glib::clone!(
            #[weak]
            win,
            #[weak]
            view,
            #[upgrade_or]
            glib::Propagation::Proceed,
            move |_, key, _, _| {
                use gdk::Key;
                match key {
                    Key::Right
                    | Key::Down
                    | Key::Page_Down
                    | Key::space
                    | Key::Return
                    | Key::KP_Enter
                    | Key::n => next(),
                    Key::Left | Key::Up | Key::Page_Up | Key::BackSpace | Key::p => prev(),
                    Key::Home => view.set_page(0),
                    Key::End => view.set_page(usize::MAX),
                    Key::Escape | Key::F5 | Key::q => win.close(),
                    _ => return glib::Propagation::Proceed,
                }
                glib::Propagation::Stop
            }
        ));
    }
    win.add_controller(keys);

    let click = gtk::GestureClick::new();
    click.set_button(0);
    {
        let (next, prev) = (next.clone(), prev.clone());
        click.connect_pressed(move |g, _, _, _| match g.current_button() {
            gdk::BUTTON_PRIMARY => next(),
            gdk::BUTTON_SECONDARY => prev(),
            _ => {}
        });
    }
    view.add_controller(click);

    let scroll = gtk::EventControllerScroll::new(
        gtk::EventControllerScrollFlags::VERTICAL | gtk::EventControllerScrollFlags::DISCRETE,
    );
    scroll.connect_scroll(move |_, _, dy| {
        if dy > 0.0 {
            next();
        } else if dy < 0.0 {
            prev();
        }
        glib::Propagation::Stop
    });
    view.add_controller(scroll);

    // Hide the pointer after 2 s without motion.
    let hide: Rc<RefCell<Option<glib::SourceId>>> = Rc::default();
    let motion = gtk::EventControllerMotion::new();
    let last = Rc::new(Cell::new((f64::NAN, f64::NAN)));
    motion.connect_motion(glib::clone!(
        #[weak]
        view,
        #[strong]
        hide,
        move |_, x, y| {
            if last.get() == (x, y) {
                return;
            }
            last.set((x, y));
            view.set_cursor_from_name(None);
            if let Some(id) = hide.borrow_mut().take() {
                id.remove();
            }
            let h = hide.clone();
            let id = glib::timeout_add_local_once(
                Duration::from_secs(2),
                glib::clone!(
                    #[weak]
                    view,
                    move || {
                        h.borrow_mut().take();
                        view.set_cursor_from_name(Some("none"));
                    }
                ),
            );
            *hide.borrow_mut() = Some(id);
        }
    ));
    view.add_controller(motion);

    win.connect_close_request(glib::clone!(
        #[weak]
        view,
        #[strong]
        model,
        #[upgrade_or]
        glib::Propagation::Proceed,
        move |_| {
            if let Some(id) = hide.borrow_mut().take() {
                id.remove();
            }
            model.renderer.set_wants(Client::Presentation, Vec::new());
            on_close(view.page());
            glib::Propagation::Proceed
        }
    ));

    win.fullscreen();
    win.present();
    view.grab_focus();
    win
}
