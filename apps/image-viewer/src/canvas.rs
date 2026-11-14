//! The image drawing widget: renders a texture with zoom, pan and rotation
//! and handles the pointer/touch gestures that change them.

use gtk::prelude::*;
use gtk::subclass::prelude::*;
use gtk::{gdk, glib, graphene, gsk};

use crate::view::ViewState;

type ZoomCallback = Box<dyn Fn(f64, bool)>;

mod imp {
    use super::*;
    use std::cell::{Cell, RefCell};

    #[derive(Default)]
    pub struct ImageCanvas {
        pub texture: RefCell<Option<gdk::Texture>>,
        pub view: RefCell<ViewState>,
        pub drag_start: Cell<(f64, f64)>,
        pub pinch_start: Cell<f64>,
        pub pointer: Cell<Option<(f64, f64)>>,
        pub on_zoom: RefCell<Option<ZoomCallback>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for ImageCanvas {
        const NAME: &'static str = "HelixImageCanvas";
        type Type = super::ImageCanvas;
        type ParentType = gtk::Widget;
    }

    impl ObjectImpl for ImageCanvas {
        fn constructed(&self) {
            self.parent_constructed();
            let obj = self.obj();
            obj.set_hexpand(true);
            obj.set_vexpand(true);
            obj.set_focusable(true);
            obj.set_overflow(gtk::Overflow::Hidden);
            obj.setup_gestures();
        }
    }

    impl WidgetImpl for ImageCanvas {
        fn measure(&self, _orientation: gtk::Orientation, _for_size: i32) -> (i32, i32, i32, i32) {
            (0, 0, -1, -1)
        }

        fn size_allocate(&self, width: i32, height: i32, _baseline: i32) {
            self.view
                .borrow_mut()
                .set_viewport(width as f64, height as f64);
            self.obj().notify_zoom();
        }

        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            let Some(texture) = self.texture.borrow().clone() else {
                return;
            };
            let v = *self.view.borrow();
            let (x, y, w, h) = v.image_rect();
            let z = v.zoom();
            let filter = if z < 1.0 {
                gsk::ScalingFilter::Trilinear
            } else if z <= 3.0 {
                gsk::ScalingFilter::Linear
            } else {
                gsk::ScalingFilter::Nearest
            };
            snapshot.save();
            // Rotate around the centre of the on-screen image rectangle.
            snapshot.translate(&graphene::Point::new(
                (x + w / 2.0) as f32,
                (y + h / 2.0) as f32,
            ));
            snapshot.rotate(v.rotation as f32);
            let (iw, ih) = (v.image.0 * z, v.image.1 * z);
            let rect =
                graphene::Rect::new((-iw / 2.0) as f32, (-ih / 2.0) as f32, iw as f32, ih as f32);
            snapshot.append_scaled_texture(&texture, filter, &rect);
            snapshot.restore();
        }
    }
}

glib::wrapper! {
    pub struct ImageCanvas(ObjectSubclass<imp::ImageCanvas>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Default for ImageCanvas {
    fn default() -> Self {
        glib::Object::new()
    }
}

impl ImageCanvas {
    /// Shows a new image (resets zoom and rotation when `reset`).
    pub fn set_texture(&self, texture: Option<&gdk::Texture>, reset: bool) {
        let imp = self.imp();
        if reset {
            let mut v = imp.view.borrow_mut();
            let viewport = v.viewport;
            *v = match texture {
                Some(t) => ViewState::new(t.width() as f64, t.height() as f64),
                None => ViewState::default(),
            };
            v.set_viewport(viewport.0, viewport.1);
        }
        imp.texture.replace(texture.cloned());
        self.update_cursor();
        self.queue_draw();
        self.notify_zoom();
    }

    pub fn texture(&self) -> Option<gdk::Texture> {
        self.imp().texture.borrow().clone()
    }

    pub fn view(&self) -> ViewState {
        *self.imp().view.borrow()
    }

    /// Mutates the view state and redraws.
    pub fn update_view(&self, f: impl FnOnce(&mut ViewState)) {
        f(&mut self.imp().view.borrow_mut());
        self.update_cursor();
        self.queue_draw();
        self.notify_zoom();
    }

    /// Called with (zoom, is_best_fit) whenever the zoom may have changed.
    pub fn connect_zoom_changed(&self, f: impl Fn(f64, bool) + 'static) {
        self.imp().on_zoom.replace(Some(Box::new(f)));
    }

    fn notify_zoom(&self) {
        let v = self.view();
        if let Some(f) = self.imp().on_zoom.borrow().as_ref() {
            f(v.zoom(), v.is_best_fit());
        }
    }

    fn update_cursor(&self) {
        let pan = self.imp().texture.borrow().is_some() && self.view().can_pan();
        self.set_cursor_from_name(if pan { Some("grab") } else { None });
    }

    /// Pointer position for anchoring zoom, if the pointer is over the view.
    pub fn pointer(&self) -> Option<(f64, f64)> {
        self.imp().pointer.get()
    }

    fn setup_gestures(&self) {
        // Pan by dragging.
        let drag = gtk::GestureDrag::new();
        drag.set_button(gdk::BUTTON_PRIMARY);
        drag.connect_drag_begin(glib::clone!(
            #[weak(rename_to = canvas)]
            self,
            move |g, _, _| {
                if !canvas.view().can_pan() {
                    g.set_state(gtk::EventSequenceState::Denied);
                    return;
                }
                canvas.imp().drag_start.set(canvas.view().offset);
                canvas.set_cursor_from_name(Some("grabbing"));
            }
        ));
        drag.connect_drag_update(glib::clone!(
            #[weak(rename_to = canvas)]
            self,
            move |_, dx, dy| {
                let (sx, sy) = canvas.imp().drag_start.get();
                canvas.update_view(|v| v.pan_to((sx + dx, sy + dy)));
                canvas.set_cursor_from_name(Some("grabbing"));
            }
        ));
        drag.connect_drag_end(glib::clone!(
            #[weak(rename_to = canvas)]
            self,
            move |_, _, _| canvas.update_cursor()
        ));
        self.add_controller(drag);

        // Double click toggles best fit / 100 %.
        let click = gtk::GestureClick::new();
        click.connect_pressed(glib::clone!(
            #[weak(rename_to = canvas)]
            self,
            move |_, n, x, y| {
                canvas.grab_focus();
                if n == 2 {
                    canvas.update_view(|v| {
                        if v.is_best_fit() && (v.zoom() - 1.0).abs() > 1e-9 {
                            v.zoom_to(1.0, Some((x, y)));
                        } else {
                            v.best_fit();
                        }
                    });
                }
            }
        ));
        self.add_controller(click);

        // Ctrl+scroll zooms; plain scroll pans when zoomed in.
        let scroll = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::BOTH_AXES);
        scroll.connect_scroll(glib::clone!(
            #[weak(rename_to = canvas)]
            self,
            #[upgrade_or]
            glib::Propagation::Proceed,
            move |c, dx, dy| {
                let ctrl = c
                    .current_event_state()
                    .contains(gdk::ModifierType::CONTROL_MASK);
                let wheel = c.unit() == gdk::ScrollUnit::Wheel;
                if ctrl {
                    let anchor = canvas.pointer();
                    let factor = if wheel {
                        crate::view::ZOOM_STEP.powf(-dy)
                    } else {
                        (1.0 - dy / 100.0).max(0.1)
                    };
                    canvas.update_view(|v| {
                        let z = v.zoom() * factor;
                        v.zoom_to(z, anchor);
                    });
                    glib::Propagation::Stop
                } else if canvas.view().can_pan() {
                    let k = if wheel { 60.0 } else { 1.0 };
                    canvas.update_view(|v| v.pan_by(-dx * k, -dy * k));
                    glib::Propagation::Stop
                } else {
                    glib::Propagation::Proceed
                }
            }
        ));
        self.add_controller(scroll);

        // Pinch to zoom.
        let zoom = gtk::GestureZoom::new();
        zoom.connect_begin(glib::clone!(
            #[weak(rename_to = canvas)]
            self,
            move |_, _| canvas.imp().pinch_start.set(canvas.view().zoom())
        ));
        zoom.connect_scale_changed(glib::clone!(
            #[weak(rename_to = canvas)]
            self,
            move |g, scale| {
                let anchor = g.bounding_box_center();
                let start = canvas.imp().pinch_start.get();
                canvas.update_view(|v| v.zoom_to(start * scale, anchor));
            }
        ));
        self.add_controller(zoom);

        let motion = gtk::EventControllerMotion::new();
        motion.connect_motion(glib::clone!(
            #[weak(rename_to = canvas)]
            self,
            move |_, x, y| canvas.imp().pointer.set(Some((x, y)))
        ));
        motion.connect_leave(glib::clone!(
            #[weak(rename_to = canvas)]
            self,
            move |_| canvas.imp().pointer.set(None)
        ));
        self.add_controller(motion);
    }
}
