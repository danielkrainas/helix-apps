//! GTK user interface.

use crate::controller::{self, Command, Event, Status, THUMB_SIZE};
use crate::countdown::{self, Action, Countdown};
use crate::devices::SourceMode;
use crate::encoders::{self, VideoFormat};
use crate::naming::{self, CaptureKind};
use crate::pipeline::{self, Frame};
use crate::qr;
use crate::settings::{self, Settings};
use adw::prelude::*;
use gtk::{gdk, gio, glib};
use helix_app_common::i18n::tr;
use helix_app_common::trf;
use helix_app_common::trnf;
use helix_app_common::Labelled;
use helix_app_common::Prefs;
use helix_app_common::{announce, Urgency};
use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Instant;

pub const APP_ID: &str = "org.helix.Camera";

struct Ui {
    window: adw::ApplicationWindow,
    toasts: adw::ToastOverlay,
    stack: gtk::Stack,
    viewfinder: gtk::Picture,
    status_page: adw::StatusPage,
    countdown_label: gtk::Label,
    flash: gtk::Box,
    rec_label: gtk::Label,
    shutter: gtk::Button,
    gallery: gtk::Button,
    gallery_image: gtk::Picture,
    switcher: gtk::Button,
    photo_toggle: gtk::ToggleButton,
    video_toggle: gtk::ToggleButton,
    qr_revealer: gtk::Revealer,
    qr_label: gtk::Label,
    qr_open: gtk::Button,
}

struct State {
    ui: Ui,
    settings: RefCell<Settings>,
    prefs: Prefs,
    qr: qr::Scanner,
    qr_throttle: Cell<qr::Throttle>,
    qr_tracker: RefCell<qr::Tracker>,
    qr_text: RefCell<Option<String>>,
    handle: RefCell<Option<controller::Handle>>,
    countdown: Cell<Countdown>,
    countdown_timer: RefCell<Option<glib::SourceId>>,
    rec_timer: RefCell<Option<glib::SourceId>>,
    rec_started: Cell<Option<Instant>>,
    recording: Cell<bool>,
    busy: Cell<bool>,
    streaming: Cell<bool>,
    last_capture: RefCell<Option<PathBuf>>,
    formats: Vec<(VideoFormat, bool)>,
    photos_ok: bool,
    silent: bool,
}

impl State {
    fn send(&self, cmd: Command) {
        if let Some(h) = self.handle.borrow().as_ref() {
            h.send(cmd);
        }
    }

    fn save_settings(&self) {
        let _ = self.settings.borrow().save(&self.prefs, &Settings::path());
    }

    fn video_mode(&self) -> bool {
        self.settings.borrow().capture_mode == "video"
    }

    fn toast(&self, msg: &str) {
        self.ui.toasts.add_toast(adw::Toast::new(msg));
    }

    /// Speaks `msg` on behalf of the window.
    fn say(&self, msg: &str, urgency: Urgency) {
        announce(&self.ui.window, msg, urgency);
    }

    /// Whether preview frames should be scanned for QR codes.
    fn scanning(&self) -> bool {
        self.streaming.get() && !self.video_mode() && !self.recording.get() && !self.busy.get()
    }
}

/// `m:ss` (or `h:mm:ss`) for the recording timer.
pub fn format_elapsed(secs: u64) -> String {
    let (h, m, s) = (secs / 3600, (secs / 60) % 60, secs % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

pub fn run() -> glib::ExitCode {
    let app = adw::Application::builder().application_id(APP_ID).build();
    helix_app_common::install_standard_actions(&app, tr("Camera"), tr("Take pictures and videos"));
    app.connect_startup(|_| {
        load_css();
        adw::StyleManager::default().set_color_scheme(adw::ColorScheme::ForceDark);
    });
    app.connect_activate(|app| {
        if let Some(w) = app.active_window() {
            w.present();
            return;
        }
        let state = build(app);
        state.ui.window.present();
    });
    helix_app_common::shortcuts::set_accels(&app, crate::shortcuts::ACCELS);
    app.run_with_args(&[std::env::args().next().unwrap_or_default()])
}

fn load_css() {
    let css = gtk::CssProvider::new();
    // Black and white here belong to the viewfinder and the controls drawn
    // over the camera picture (the same in every style); recording uses
    // the style's destructive colour.
    css.load_from_string(
        ".viewfinder { background: black; }
         .shutter { min-width: 64px; min-height: 64px; border-radius: 9999px; padding: 0;
                    background: white; border: 4px solid alpha(white, 0.4); background-clip: padding-box; }
         .shutter:disabled { opacity: 0.4; }
         .shutter.video { background: @destructive_bg_color; }
         .shutter.recording { background: @destructive_bg_color; }
         .shutter image { color: black; }
         .shutter.video image, .shutter.recording image { color: @destructive_fg_color; }
         .gallery { min-width: 48px; min-height: 48px; border-radius: 9999px; padding: 0; }
         .gallery picture { border-radius: 9999px; }
         .countdown { font-size: 8.7em; font-weight: 800; color: white;
                      text-shadow: 0 2px 8px alpha(black, 0.6); }
         .flash { background: white; }
         .rec-label { background: @destructive_bg_color; color: @destructive_fg_color; border-radius: 9999px;
                      padding: 2px 10px; font-weight: bold; }
         .controls { padding: 12px 18px; }
         .qr-banner { margin: 12px; padding: 6px 6px 6px 14px; border-radius: 12px;
                      background: alpha(black, 0.75); color: white; }",
    );
    gtk::style_context_add_provider_for_display(
        &gdk::Display::default().expect("display"),
        &css,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
}

fn menu_model(formats: &[(VideoFormat, bool)]) -> gio::Menu {
    let menu = gio::Menu::new();
    let timer = gio::Menu::new();
    timer.append(Some(tr("Off")), Some("win.countdown::0"));
    for d in countdown::DELAYS.iter().skip(1) {
        timer.append(
            Some(&trnf!("{n} Second", "{n} Seconds", *d)),
            Some(&format!("win.countdown::{d}")),
        );
    }
    let prefs = gio::Menu::new();
    prefs.append_submenu(Some(tr("_Countdown")), &timer);
    let fmt = gio::Menu::new();
    for (f, _) in formats {
        fmt.append(Some(f.label()), Some(&format!("win.format-{}", f.id())));
    }
    prefs.append_submenu(Some(tr("Video _Format")), &fmt);
    prefs.append(Some(tr("Play Shutter _Sound")), Some("win.shutter-sound"));
    menu.append_section(None, &prefs);
    let misc = gio::Menu::new();
    misc.append(
        Some(helix_app_common::i18n::tr(
            helix_app_common::shortcuts::MENU_LABEL,
        )),
        Some("win.show-help-overlay"),
    );
    misc.append(Some(tr("_About Camera")), Some("app.about"));
    menu.append_section(None, &misc);
    menu
}

fn build(app: &adw::Application) -> Rc<State> {
    gst::init().ok();
    let formats = encoders::availability(&encoders::gst_has);
    let photos_ok = encoders::can_take_photos(&encoders::gst_has);
    let prefs = Prefs::new(settings::SCHEMA);
    let settings = Settings::load(&prefs, &Settings::path());
    let mode = SourceMode::from_env();

    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title(tr("Camera"))
        .default_width(800)
        .default_height(640)
        .width_request(helix_app_common::MIN_WIDTH)
        .height_request(helix_app_common::MIN_HEIGHT)
        .build();
    {
        use helix_app_common::window_state::{persist, SizeKeys, Store};
        persist(
            &window,
            Store::settings(
                &prefs,
                SizeKeys::Split("window-width", "window-height"),
                "is-maximized",
                "helix-camera",
            ),
        );
    }

    // Viewfinder area.
    let viewfinder = gtk::Picture::builder()
        .content_fit(gtk::ContentFit::Contain)
        .hexpand(true)
        .vexpand(true)
        .build();
    let status_page = adw::StatusPage::builder()
        .icon_name("camera-web-symbolic")
        .build();
    let stack = gtk::Stack::builder()
        .transition_type(gtk::StackTransitionType::Crossfade)
        .build();
    stack.add_named(&viewfinder, Some("viewfinder"));
    stack.add_named(&status_page, Some("status"));
    stack.add_css_class("viewfinder");

    let overlay = gtk::Overlay::builder().child(&stack).vexpand(true).build();
    let countdown_label = gtk::Label::builder()
        .halign(gtk::Align::Center)
        .valign(gtk::Align::Center)
        .visible(false)
        .css_classes(["countdown"])
        .build();
    let flash = gtk::Box::builder()
        .css_classes(["flash"])
        .opacity(0.0)
        .can_target(false)
        .build();
    let rec_label = gtk::Label::builder()
        .halign(gtk::Align::Center)
        .valign(gtk::Align::Start)
        .margin_top(12)
        .visible(false)
        .css_classes(["rec-label", "numeric"])
        .build();
    countdown_label.set_can_target(false);
    rec_label.set_can_target(false);
    overlay.add_overlay(&flash);
    overlay.add_overlay(&countdown_label);
    overlay.add_overlay(&rec_label);

    // QR-code banner at the bottom of the viewfinder.
    let qr_label = gtk::Label::builder()
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .max_width_chars(40)
        .hexpand(true)
        .xalign(0.0)
        .selectable(true)
        .build();
    let qr_icon = gtk::Image::from_gicon(&gio::ThemedIcon::from_names(&[
        "qr-code-symbolic",
        "insert-link-symbolic",
    ]));
    let qr_copy = gtk::Button::builder()
        .icon_name("edit-copy-symbolic")
        .tooltip_text(tr("Copy"))
        .action_name("win.qr-copy")
        .css_classes(["flat", "circular"])
        .build()
        .labelled(tr("Copy"));
    let qr_open = gtk::Button::builder()
        .label(tr("Open"))
        .action_name("win.qr-open")
        .css_classes(["suggested-action", "pill"])
        .build();
    let qr_close = gtk::Button::builder()
        .icon_name("window-close-symbolic")
        .tooltip_text(tr("Dismiss"))
        .action_name("win.qr-dismiss")
        .css_classes(["flat", "circular"])
        .build()
        .labelled(tr("Dismiss"));
    let qr_box = gtk::Box::builder()
        .spacing(8)
        .css_classes(["qr-banner"])
        .build();
    qr_box.append(&qr_icon);
    qr_box.append(&qr_label);
    qr_box.append(&qr_copy);
    qr_box.append(&qr_open);
    qr_box.append(&qr_close);
    let qr_revealer = gtk::Revealer::builder()
        .child(&qr_box)
        .transition_type(gtk::RevealerTransitionType::SlideUp)
        .valign(gtk::Align::End)
        .halign(gtk::Align::Center)
        .build();
    overlay.add_overlay(&qr_revealer);

    // Controls.
    let gallery_image = gtk::Picture::builder()
        .content_fit(gtk::ContentFit::Cover)
        .width_request(48)
        .height_request(48)
        .build();
    let gallery = gtk::Button::builder()
        .child(&gallery_image)
        .tooltip_text(tr("Open Last Capture"))
        .action_name("win.open-gallery")
        .valign(gtk::Align::Center)
        .css_classes(["gallery", "circular"])
        .build();
    helix_app_common::set_accessible_label(&gallery, tr("Open Last Capture"));
    let shutter = gtk::Button::builder()
        .icon_name("camera-photo-symbolic")
        .tooltip_text(tr("Take Photo"))
        .action_name("win.shutter")
        .halign(gtk::Align::Center)
        .css_classes(["shutter"])
        .build()
        .labelled(tr("Take Photo"));
    let switcher = gtk::Button::builder()
        .icon_name("camera-switch-symbolic")
        .tooltip_text(tr("Switch Camera"))
        .action_name("win.next-camera")
        .valign(gtk::Align::Center)
        .visible(false)
        .css_classes(["circular", "flat"])
        .build()
        .labelled(tr("Switch Camera"));
    let photo_toggle = gtk::ToggleButton::builder()
        .label(tr("Photo"))
        .action_name("win.mode")
        .action_target(&"photo".to_variant())
        .build();
    let video_toggle = gtk::ToggleButton::builder()
        .label(tr("Video"))
        .action_name("win.mode")
        .action_target(&"video".to_variant())
        .build();
    let mode_box = gtk::Box::builder()
        .css_classes(["linked"])
        .halign(gtk::Align::Center)
        .build();
    mode_box.append(&photo_toggle);
    mode_box.append(&video_toggle);
    let center = gtk::Box::new(gtk::Orientation::Vertical, 8);
    center.append(&shutter);
    center.append(&mode_box);
    let controls = gtk::CenterBox::builder().css_classes(["controls"]).build();
    controls.set_start_widget(Some(&gallery));
    controls.set_center_widget(Some(&center));
    controls.set_end_widget(Some(&switcher));

    let header = adw::HeaderBar::new();
    let menu_button = gtk::MenuButton::builder()
        .icon_name("open-menu-symbolic")
        .tooltip_text(tr("Main Menu"))
        .menu_model(&menu_model(&formats))
        .primary(true)
        .build()
        .labelled(tr("Main Menu"));
    header.pack_end(&menu_button);

    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&header);
    toolbar.set_content(Some(&overlay));
    toolbar.add_bottom_bar(&controls);
    let toasts = adw::ToastOverlay::new();
    toasts.set_child(Some(&toolbar));
    window.set_content(Some(&toasts));

    window.set_help_overlay(Some(&helix_app_common::shortcuts::window(
        crate::shortcuts::GROUPS,
    )));

    let handle = controller::spawn(controller::Config {
        mode,
        output_dir: naming::output_dir(),
        remembered_camera: settings.last_camera.clone(),
    });
    let frames = handle.frames.clone();
    let events = handle.events.clone();

    let state = Rc::new(State {
        ui: Ui {
            window,
            toasts,
            stack,
            viewfinder,
            status_page,
            countdown_label,
            flash,
            rec_label,
            shutter,
            gallery,
            gallery_image,
            switcher,
            photo_toggle,
            video_toggle,
            qr_revealer,
            qr_label,
            qr_open,
        },
        settings: RefCell::new(settings),
        prefs,
        qr: qr::Scanner::spawn(),
        qr_throttle: Cell::new(qr::Throttle::default()),
        qr_tracker: RefCell::new(qr::Tracker::default()),
        qr_text: RefCell::new(None),
        handle: RefCell::new(Some(handle)),
        countdown: Cell::new(Countdown::Idle),
        countdown_timer: RefCell::new(None),
        rec_timer: RefCell::new(None),
        rec_started: Cell::new(None),
        recording: Cell::new(false),
        busy: Cell::new(false),
        streaming: Cell::new(false),
        last_capture: RefCell::new(None),
        formats,
        photos_ok,
        silent: mode != SourceMode::Real || std::env::var_os("HELIX_CAMERA_SILENT").is_some(),
    });

    install_actions(&state);
    load_latest_capture(&state);
    update_controls(&state);
    show_status(&state, &Status::Starting);

    // Frames → viewfinder.
    let weak = Rc::downgrade(&state);
    glib::spawn_future_local(async move {
        while let Ok(frame) = frames.recv().await {
            let Some(state) = weak.upgrade() else { break };
            state.ui.viewfinder.set_paintable(Some(&texture(&frame)));
            if state.scanning() {
                let mut t = state.qr_throttle.get();
                if t.ready(Instant::now(), qr::SCAN_INTERVAL) {
                    state.qr.offer(frame);
                }
                state.qr_throttle.set(t);
            }
        }
    });
    // QR scan results.
    let weak = Rc::downgrade(&state);
    let results = state.qr.results.clone();
    glib::spawn_future_local(async move {
        while let Ok(result) = results.recv().await {
            let Some(state) = weak.upgrade() else { break };
            if !state.scanning() {
                continue;
            }
            let change = state.qr_tracker.borrow_mut().update(result);
            apply_qr(&state, change);
        }
    });
    // Worker events.
    let weak = Rc::downgrade(&state);
    glib::spawn_future_local(async move {
        while let Ok(ev) = events.recv().await {
            let Some(state) = weak.upgrade() else { break };
            handle_event(&state, ev);
        }
    });

    if mode == SourceMode::Test && std::env::var_os("HELIX_CAMERA_SMOKE").is_some() {
        smoke_script(&state);
    }

    let weak = Rc::downgrade(&state);
    state.ui.window.connect_close_request(move |_| {
        if let Some(state) = weak.upgrade() {
            state.save_settings();
            stop_countdown(&state);
            // Joins the worker, which finalizes any running recording.
            state.handle.borrow_mut().take();
        }
        glib::Propagation::Proceed
    });
    // The application keeps the state alive until shutdown (everything else
    // holds weak references). Ctrl+Q skips close-request, so recordings are
    // also finalized here.
    let keep = state.clone();
    app.connect_shutdown(move |_| {
        keep.handle.borrow_mut().take();
    });
    state
}

/// Scripted walk through the UI for headless smoke tests (test source only):
/// photo, 3 s countdown photo, a short video, camera switch, quit.
fn smoke_script(state: &Rc<State>) {
    let steps: &[(u64, &str, Option<&str>)] = &[
        (2000, "win.shutter", None),
        (2500, "win.countdown", Some("3")),
        (2600, "win.shutter", None),
        (6500, "win.mode", Some("video")),
        (7000, "win.shutter", None),
        (9000, "win.shutter", None),
        (10000, "smoke:qr", None),
        (10300, "win.qr-copy", None),
        (10600, "win.qr-dismiss", None),
        (11000, "win.next-camera", None),
        (12500, "app.quit", None),
    ];
    for (ms, action, target) in steps.iter().copied() {
        let weak = Rc::downgrade(state);
        glib::timeout_add_local_once(std::time::Duration::from_millis(ms), move || {
            let Some(s) = weak.upgrade() else { return };
            eprintln!("smoke: {action} {target:?}");
            if action == "smoke:qr" {
                let change = s
                    .qr_tracker
                    .borrow_mut()
                    .update(Some("https://example.org/helix".into()));
                apply_qr(&s, change);
                return;
            }
            let arg = target.map(|t| t.to_variant());
            let _ = WidgetExt::activate_action(&s.ui.window, action, arg.as_ref());
        });
    }
}

fn apply_qr(state: &State, change: Option<qr::Change>) {
    match change {
        None => {}
        Some(qr::Change::Show(text)) => {
            let uri = qr::openable_uri(&text);
            state.ui.qr_label.set_label(&text);
            state.ui.qr_label.set_tooltip_text(Some(&text));
            state.ui.qr_open.set_visible(uri.is_some());
            state.ui.qr_revealer.set_reveal_child(true);
            state.say(
                &trf!("QR code detected: {text}", text = text),
                Urgency::Polite,
            );
            *state.qr_text.borrow_mut() = Some(text);
        }
        Some(qr::Change::Hide) => {
            state.ui.qr_revealer.set_reveal_child(false);
            state.qr_text.borrow_mut().take();
        }
    }
}

/// Hides the QR banner and forgets the current code (mode change, camera
/// switch, errors).
fn reset_qr(state: &State) {
    let change = state.qr_tracker.borrow_mut().reset();
    apply_qr(state, change);
}

fn texture(frame: &Frame) -> gdk::Texture {
    gdk::MemoryTexture::new(
        frame.width,
        frame.height,
        gdk::MemoryFormat::R8g8b8a8,
        &frame.data,
        frame.stride,
    )
    .upcast()
}

fn install_actions(state: &Rc<State>) {
    let win = &state.ui.window;

    let qr_copy = gio::SimpleAction::new("qr-copy", None);
    let weak = Rc::downgrade(state);
    qr_copy.connect_activate(move |_, _| {
        let Some(s) = weak.upgrade() else { return };
        let Some(text) = s.qr_text.borrow().clone() else {
            return;
        };
        WidgetExt::display(&s.ui.window).clipboard().set_text(&text);
        s.toast(tr("Copied to clipboard"));
    });
    win.add_action(&qr_copy);

    let qr_open = gio::SimpleAction::new("qr-open", None);
    let weak = Rc::downgrade(state);
    qr_open.connect_activate(move |_, _| {
        let Some(s) = weak.upgrade() else { return };
        let Some(uri) = s.qr_text.borrow().as_deref().and_then(qr::openable_uri) else {
            return;
        };
        let launcher = gtk::UriLauncher::new(&uri);
        let weak = Rc::downgrade(&s);
        launcher.launch(Some(&s.ui.window), None::<&gio::Cancellable>, move |res| {
            if let (Err(e), Some(s)) = (res, weak.upgrade()) {
                if !e.matches(gtk::DialogError::Dismissed) {
                    s.toast(&trf!("Could not open link: {}", e.message()));
                }
            }
        });
    });
    win.add_action(&qr_open);

    let qr_dismiss = gio::SimpleAction::new("qr-dismiss", None);
    let weak = Rc::downgrade(state);
    qr_dismiss.connect_activate(move |_, _| {
        let Some(s) = weak.upgrade() else { return };
        s.qr_tracker.borrow_mut().dismiss();
        apply_qr(&s, Some(qr::Change::Hide));
    });
    win.add_action(&qr_dismiss);

    let shutter = gio::SimpleAction::new("shutter", None);
    let weak = Rc::downgrade(state);
    shutter.connect_activate(move |_, _| {
        if let Some(s) = weak.upgrade() {
            on_shutter(&s);
        }
    });
    win.add_action(&shutter);

    let cancel = gio::SimpleAction::new("cancel-countdown", None);
    let weak = Rc::downgrade(state);
    cancel.connect_activate(move |_, _| {
        if let Some(s) = weak.upgrade() {
            let mut c = s.countdown.get();
            if c.cancel() == Action::Cancelled {
                s.countdown.set(c);
                stop_countdown(&s);
            }
        }
    });
    win.add_action(&cancel);

    let initial_mode = state.settings.borrow().capture_mode.clone();
    let mode = gio::SimpleAction::new_stateful(
        "mode",
        Some(glib::VariantTy::STRING),
        &initial_mode.to_variant(),
    );
    let weak = Rc::downgrade(state);
    mode.connect_change_state(move |a, v| {
        let Some(s) = weak.upgrade() else { return };
        let Some(m) = v.and_then(|v| v.get::<String>()) else {
            return;
        };
        if s.recording.get() || (m == "video" && !s.formats.iter().any(|(_, ok)| *ok)) {
            return;
        }
        a.set_state(&m.to_variant());
        let mut c = s.countdown.get();
        if c.cancel() == Action::Cancelled {
            s.countdown.set(c);
            stop_countdown(&s);
        }
        s.settings.borrow_mut().capture_mode = m;
        s.save_settings();
        if s.video_mode() {
            reset_qr(&s);
        }
        update_controls(&s);
    });
    win.add_action(&mode);

    let next = gio::SimpleAction::new("next-camera", None);
    let weak = Rc::downgrade(state);
    next.connect_activate(move |_, _| {
        if let Some(s) = weak.upgrade() {
            if !s.recording.get() {
                reset_qr(&s);
                s.send(Command::NextCamera);
            }
        }
    });
    win.add_action(&next);

    let gallery = gio::SimpleAction::new("open-gallery", None);
    let weak = Rc::downgrade(state);
    gallery.connect_activate(move |_, _| {
        let Some(s) = weak.upgrade() else { return };
        let Some(path) = s.last_capture.borrow().clone() else {
            return;
        };
        let launcher = gtk::FileLauncher::new(Some(&gio::File::for_path(&path)));
        let weak = Rc::downgrade(&s);
        launcher.launch(Some(&s.ui.window), None::<&gio::Cancellable>, move |res| {
            if let (Err(e), Some(s)) = (res, weak.upgrade()) {
                if !e.matches(gtk::DialogError::Dismissed) {
                    s.toast(&trf!("Could not open capture: {}", e.message()));
                }
            }
        });
    });
    win.add_action(&gallery);

    let delay = state.settings.borrow().countdown.to_string();
    let countdown = gio::SimpleAction::new_stateful(
        "countdown",
        Some(glib::VariantTy::STRING),
        &delay.to_variant(),
    );
    let weak = Rc::downgrade(state);
    countdown.connect_change_state(move |a, v| {
        let Some(s) = weak.upgrade() else { return };
        let Some(d) = v.and_then(|v| v.get::<String>()) else {
            return;
        };
        let secs = countdown::normalize_delay(d.parse().unwrap_or(0));
        a.set_state(&secs.to_string().to_variant());
        s.settings.borrow_mut().countdown = secs;
        s.save_settings();
    });
    win.add_action(&countdown);

    let sound = gio::SimpleAction::new_stateful(
        "shutter-sound",
        None,
        &state.settings.borrow().play_shutter_sound.to_variant(),
    );
    let weak = Rc::downgrade(state);
    sound.connect_change_state(move |a, v| {
        let Some(s) = weak.upgrade() else { return };
        let Some(on) = v.and_then(|v| v.get::<bool>()) else {
            return;
        };
        a.set_state(&on.to_variant());
        s.settings.borrow_mut().play_shutter_sound = on;
        s.save_settings();
    });
    win.add_action(&sound);

    let current = state.settings.borrow().video_format();
    let mut format_actions = Vec::new();
    for (f, ok) in state.formats.iter().copied() {
        let a = gio::SimpleAction::new_stateful(
            &format!("format-{}", f.id()),
            None,
            &(f == current).to_variant(),
        );
        a.set_enabled(ok);
        win.add_action(&a);
        format_actions.push((f, a));
    }
    let format_actions = Rc::new(format_actions);
    for (f, a) in format_actions.iter() {
        let f = *f;
        let all = Rc::downgrade(&format_actions);
        let weak = Rc::downgrade(state);
        a.connect_change_state(move |_, _| {
            let (Some(s), Some(all)) = (weak.upgrade(), all.upgrade()) else {
                return;
            };
            for (g, b) in all.iter() {
                b.set_state(&(*g == f).to_variant());
            }
            s.settings.borrow_mut().video_format = f.id().into();
            s.save_settings();
        });
    }
}

fn on_shutter(state: &Rc<State>) {
    if !state.streaming.get() || state.busy.get() {
        return;
    }
    if state.video_mode() {
        if state.recording.get() {
            state.busy.set(true);
            state.send(Command::StopRecording);
        } else {
            state.busy.set(true);
            state.send(Command::StartRecording(
                state.settings.borrow().video_format(),
            ));
        }
        update_controls(state);
        return;
    }
    if !state.photos_ok {
        state.toast(tr(
            "Taking photos is not supported: a JPEG encoder is missing",
        ));
        return;
    }
    let delay = state.settings.borrow().countdown;
    let mut c = state.countdown.get();
    let action = c.press(delay);
    state.countdown.set(c);
    apply_countdown(state, action);
}

fn apply_countdown(state: &Rc<State>, action: Action) {
    match action {
        Action::None => {}
        Action::Capture => {
            stop_countdown(state);
            capture_photo(state);
        }
        Action::Cancelled => {
            stop_countdown(state);
            state.say(tr("Countdown cancelled"), Urgency::Polite);
        }
        Action::Show(n) => {
            state.ui.countdown_label.set_label(&n.to_string());
            state.ui.countdown_label.set_visible(true);
            // Each tick is spoken so the self-timer is not visual-only.
            state.say(&n.to_string(), Urgency::Polite);
            if state.countdown_timer.borrow().is_none() {
                let weak = Rc::downgrade(state);
                let id = glib::timeout_add_seconds_local(1, move || {
                    let Some(s) = weak.upgrade() else {
                        return glib::ControlFlow::Break;
                    };
                    let mut c = s.countdown.get();
                    let action = c.tick();
                    s.countdown.set(c);
                    if matches!(action, Action::Show(_)) {
                        apply_countdown(&s, action);
                        glib::ControlFlow::Continue
                    } else {
                        // The timer ends itself; forget its id first.
                        s.countdown_timer.borrow_mut().take();
                        apply_countdown(&s, action);
                        glib::ControlFlow::Break
                    }
                });
                *state.countdown_timer.borrow_mut() = Some(id);
            }
        }
    }
    update_controls(state);
}

fn stop_countdown(state: &State) {
    if let Some(id) = state.countdown_timer.borrow_mut().take() {
        id.remove();
    }
    state.ui.countdown_label.set_visible(false);
}

/// Whether the desktop allows animations (the flash is skipped otherwise).
fn animations_enabled() -> bool {
    gtk::Settings::default().is_none_or(|s| s.is_gtk_enable_animations())
}

/// Confirms a saved capture: a toast with an Open button when the flash
/// cannot be seen (animations off), otherwise a spoken message (the toast
/// would be spoken too, so only one of them is used).
fn confirm_capture(state: &State, message: &str) {
    if animations_enabled() {
        state.say(message, Urgency::Polite);
    } else {
        let toast = adw::Toast::builder()
            .title(message)
            .button_label(tr("_Open"))
            .action_name("win.open-gallery")
            .build();
        state.ui.toasts.add_toast(toast);
    }
}

fn capture_photo(state: &Rc<State>) {
    let anim = adw::TimedAnimation::new(
        &state.ui.flash,
        0.8,
        0.0,
        350,
        adw::PropertyAnimationTarget::new(&state.ui.flash, "opacity"),
    );
    anim.play();
    if state.settings.borrow().play_shutter_sound {
        pipeline::play_shutter_sound(state.silent);
    }
    state.send(Command::TakePhoto);
}

fn handle_event(state: &Rc<State>, ev: Event) {
    match ev {
        Event::Cameras { names, active, .. } => {
            state.ui.switcher.set_visible(names.len() > 1);
            if let Some(name) = active.and_then(|i| names.get(i)) {
                let changed = state.settings.borrow().last_camera.as_deref() != Some(name.as_str());
                if changed && names.len() > 1 {
                    state.settings.borrow_mut().last_camera = Some(name.clone());
                    state.save_settings();
                }
                state
                    .ui
                    .switcher
                    .set_tooltip_text(Some(&trf!("Switch Camera (current: {name})", name = name)));
            }
        }
        Event::Status(st) => show_status(state, &st),
        Event::PhotoSaved { path, thumb } => {
            set_gallery(state, path, thumb.as_ref().map(texture), CaptureKind::Photo);
            confirm_capture(state, tr("Photo saved"));
        }
        Event::RecordingStarted(path) => {
            state.busy.set(false);
            state
                .ui
                .rec_label
                .set_tooltip_text(path.file_name().and_then(|n| n.to_str()));
            state.recording.set(true);
            state.rec_started.set(Some(Instant::now()));
            state.ui.rec_label.set_label(&format_elapsed(0));
            state.ui.rec_label.set_visible(true);
            state.say(tr("Recording started"), Urgency::Polite);
            let weak = Rc::downgrade(state);
            let id = glib::timeout_add_local(std::time::Duration::from_millis(500), move || {
                let Some(s) = weak.upgrade() else {
                    return glib::ControlFlow::Break;
                };
                if let Some(t) = s.rec_started.get() {
                    s.ui.rec_label
                        .set_label(&format_elapsed(t.elapsed().as_secs()));
                }
                glib::ControlFlow::Continue
            });
            *state.rec_timer.borrow_mut() = Some(id);
        }
        Event::RecordingSaved { path, thumb } => {
            let secs = state.rec_started.get().map(|t| t.elapsed().as_secs());
            end_recording(state);
            state.say(&recording_stopped_message(secs), Urgency::Polite);
            let tex = thumb.as_ref().map(texture);
            set_gallery(state, path, tex, CaptureKind::Video);
            if !animations_enabled() {
                confirm_capture(state, tr("Video saved"));
            }
        }
        Event::Failed(msg) => {
            if state.busy.get() || state.recording.get() {
                end_recording(state);
            }
            state.toast(&msg);
        }
    }
    update_controls(state);
}

/// "Recording stopped, 0:12" (just "Recording stopped" without a start time).
pub fn recording_stopped_message(secs: Option<u64>) -> String {
    match secs {
        Some(s) => trf!("Recording stopped, {}", format_elapsed(s)),
        None => tr("Recording stopped").to_string(),
    }
}

fn end_recording(state: &State) {
    state.busy.set(false);
    state.recording.set(false);
    state.rec_started.set(None);
    state.ui.rec_label.set_visible(false);
    if let Some(id) = state.rec_timer.borrow_mut().take() {
        id.remove();
    }
}

fn show_status(state: &Rc<State>, st: &Status) {
    let page = &state.ui.status_page;
    state.streaming.set(matches!(st, Status::Streaming));
    match st {
        Status::Starting | Status::Streaming => {
            state.ui.stack.set_visible_child_name("viewfinder");
            if matches!(st, Status::Starting) {
                state.ui.viewfinder.set_paintable(None::<&gdk::Paintable>);
            }
            return;
        }
        Status::NoCamera => {
            page.set_icon_name(Some("camera-disabled-symbolic"));
            page.set_title(tr("No Camera Found"));
            page.set_description(Some(tr("Connect a camera device")));
        }
        Status::Denied => {
            page.set_icon_name(Some("camera-disabled-symbolic"));
            page.set_title(tr("Permission Denied"));
            page.set_description(Some(tr(
                "Camera access was not granted. It can be allowed in Settings → Apps → Camera.",
            )));
        }
        Status::Error(e) => {
            page.set_icon_name(Some("dialog-error-symbolic"));
            page.set_title(tr("Camera Error"));
            page.set_description(Some(&glib::markup_escape_text(e)));
        }
    }
    if state.recording.get() || state.busy.get() {
        end_recording(state);
    }
    reset_qr(state);
    stop_countdown(state);
    state.countdown.set(Countdown::Idle);
    state.ui.stack.set_visible_child_name("status");
    // The status page replaces the viewfinder: say why.
    let detail = match st {
        Status::Error(e) => e.clone(),
        _ => page
            .description()
            .map(|d| d.to_string())
            .unwrap_or_default(),
    };
    state.say(&format!("{}. {detail}", page.title()), Urgency::Assertive);
}

fn update_controls(state: &State) {
    let ui = &state.ui;
    let video = state.video_mode();
    let streaming = state.streaming.get();
    let recording = state.recording.get();
    ui.shutter.set_sensitive(streaming && !state.busy.get());
    for c in ["video", "recording"] {
        ui.shutter.remove_css_class(c);
    }
    let (icon, tip) = if recording {
        ui.shutter.add_css_class("recording");
        ("media-playback-stop-symbolic", tr("Stop Recording"))
    } else if video {
        ui.shutter.add_css_class("video");
        ("media-record-symbolic", tr("Start Recording"))
    } else if state.countdown.get().is_running() {
        ("process-stop-symbolic", tr("Cancel Countdown"))
    } else {
        ("camera-photo-symbolic", tr("Take Photo"))
    };
    ui.shutter.set_icon_name(icon);
    ui.shutter.set_tooltip_text(Some(tip));
    helix_app_common::set_accessible_label(&ui.shutter, tip);
    ui.video_toggle
        .set_sensitive(!recording && state.formats.iter().any(|(_, ok)| *ok));
    ui.photo_toggle.set_sensitive(!recording);
    ui.switcher.set_sensitive(!recording && !state.busy.get());
    ui.gallery
        .set_sensitive(state.last_capture.borrow().is_some());
}

fn set_gallery(state: &State, path: PathBuf, tex: Option<gdk::Texture>, kind: CaptureKind) {
    match tex {
        Some(t) => state.ui.gallery_image.set_paintable(Some(&t)),
        None => {
            let icon = match kind {
                CaptureKind::Photo => "image-x-generic-symbolic",
                CaptureKind::Video => "video-x-generic-symbolic",
            };
            let theme = gtk::IconTheme::for_display(&WidgetExt::display(&state.ui.window));
            let paintable = theme.lookup_icon(
                icon,
                &[],
                32,
                1,
                gtk::TextDirection::None,
                gtk::IconLookupFlags::empty(),
            );
            state.ui.gallery_image.set_paintable(Some(&paintable));
        }
    }
    let tip = trf!(
        "Open {}",
        path.file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    );
    state.ui.gallery.set_tooltip_text(Some(&tip));
    helix_app_common::set_accessible_label(&state.ui.gallery, &tip);
    *state.last_capture.borrow_mut() = Some(path);
}

/// Shows the newest existing capture in the gallery button (decoded off the
/// main thread).
fn load_latest_capture(state: &Rc<State>) {
    let dir = naming::output_dir();
    let weak = Rc::downgrade(state);
    glib::spawn_future_local(async move {
        let found = gio::spawn_blocking(move || {
            let (path, kind) = naming::latest_capture(&dir)?;
            // Pixbufs are not Send: hand back the raw pixels.
            let pixels = match kind {
                CaptureKind::Photo => {
                    gtk::gdk_pixbuf::Pixbuf::from_file_at_scale(&path, THUMB_SIZE, THUMB_SIZE, true)
                        .ok()
                        .map(|p| {
                            (
                                p.width(),
                                p.height(),
                                p.rowstride() as usize,
                                p.has_alpha(),
                                p.read_pixel_bytes(),
                            )
                        })
                }
                CaptureKind::Video => None,
            };
            Some((path, kind, pixels))
        })
        .await
        .ok()
        .flatten();
        let (Some(state), Some((path, kind, pixels))) = (weak.upgrade(), found) else {
            return;
        };
        if state.last_capture.borrow().is_some() {
            return;
        }
        let tex = pixels.map(|(w, h, stride, alpha, bytes)| {
            let format = if alpha {
                gdk::MemoryFormat::R8g8b8a8
            } else {
                gdk::MemoryFormat::R8g8b8
            };
            gdk::MemoryTexture::new(w, h, format, &bytes, stride).upcast::<gdk::Texture>()
        });
        set_gallery(&state, path, tex, kind);
        update_controls(&state);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn elapsed_formatting() {
        assert_eq!(format_elapsed(0), "0:00");
        assert_eq!(format_elapsed(65), "1:05");
        assert_eq!(format_elapsed(3725), "1:02:05");
    }

    #[test]
    fn recording_stopped_includes_duration() {
        assert_eq!(
            recording_stopped_message(Some(12)),
            "Recording stopped, 0:12"
        );
        assert_eq!(recording_stopped_message(None), "Recording stopped");
    }
}
