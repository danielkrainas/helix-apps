//! Window, controls and glue between GStreamer, MPRIS and GTK.

use crate::mpris::{self, Command, Mpris, Prop};
use crate::paintable::VideoPaintable;
use crate::player::{self, Player};
use crate::resume::{self, ResumeStore};
use crate::sink::Frame;
use crate::{screenshot, subtitles, timefmt};
use adw::prelude::*;
use gtk::{gdk, gio, glib};
use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, Instant};

pub const APP_ID: &str = "org.helix.VideoPlayer";
const APP_NAME: &str = "Video Player";
const HIDE_DELAY: Duration = Duration::from_secs(2);
const SEEK_STEP: f64 = 10.0;
const VOLUME_STEP: f64 = 0.05;
const SAVE_INTERVAL: Duration = Duration::from_secs(10);
const SPEEDS: &[(&str, &str)] =
    &[("0.5", "0.5×"), ("0.75", "0.75×"), ("1", "Normal"), ("1.25", "1.25×"), ("1.5", "1.5×"), ("2", "2×")];

pub fn run(target: Option<String>) -> glib::ExitCode {
    if let Err(err) = gst::init() {
        eprintln!("helix-video-player: cannot initialise GStreamer: {err}");
        return glib::ExitCode::FAILURE;
    }
    let app = adw::Application::builder().application_id(APP_ID).flags(gio::ApplicationFlags::NON_UNIQUE).build();
    helix_app_common::install_standard_actions(&app, APP_NAME, "Play videos");
    app.connect_startup(|app| {
        load_css();
        for (action, accels) in ACCELS {
            app.set_accels_for_action(action, accels);
        }
    });
    let initial = RefCell::new(Some(target));
    app.connect_activate(move |app| {
        if let Some(win) = app.active_window() {
            win.present();
            return;
        }
        let ui = match Ui::new(app) {
            Ok(ui) => ui,
            Err(err) => {
                eprintln!("helix-video-player: {err}");
                app.quit();
                return;
            }
        };
        if let Some(Some(uri)) = initial.borrow_mut().take() {
            ui.open_uri(&uri);
        }
        ui.window.present();
        // Keep the UI alive as long as the window (handlers are dropped
        // when the window is destroyed, breaking the cycle).
        let keep = RefCell::new(Some(ui.clone()));
        ui.window.connect_destroy(move |_| {
            keep.borrow_mut().take();
        });
    });
    app.run_with_args(&[std::env::args().next().unwrap_or_default()])
}

const ACCELS: &[(&str, &[&str])] = &[
    ("win.open", &["<Control>o"]),
    ("win.play-pause", &["space", "KP_Space", "AudioPlay", "AudioPause"]),
    ("win.seek-back", &["Left", "KP_Left"]),
    ("win.seek-forward", &["Right", "KP_Right"]),
    ("win.volume-up", &["Up", "KP_Up"]),
    ("win.volume-down", &["Down", "KP_Down"]),
    ("win.toggle-mute", &["m", "AudioMute"]),
    ("win.fullscreen", &["F11", "f"]),
    ("win.leave-fullscreen", &["Escape"]),
    ("win.frame-prev", &["comma"]),
    ("win.frame-next", &["period"]),
    ("win.screenshot", &["<Control><Alt>s"]),
];

fn load_css() {
    let css = gtk::CssProvider::new();
    css.load_from_string(
        ".vp-video { background: black; }
         .vp-controls { margin: 12px; padding: 4px 8px; border-radius: 12px; }
         .vp-header { background: linear-gradient(to bottom, alpha(black, 0.65), transparent); color: white; box-shadow: none; }
         .vp-time { font-feature-settings: \"tnum\"; min-width: 4.5em; }
         .vp-seek { min-width: 120px; }",
    );
    if let Some(display) = gdk::Display::default() {
        gtk::style_context_add_provider_for_display(&display, &css, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);
    }
}

#[derive(Default)]
struct St {
    uri: Option<String>,
    name: String,
    duration: Option<f64>,
    prerolled: bool,
    showing_video: bool,
    want_play: bool,
    eos: bool,
    error: bool,
    pending_seek: Option<f64>,
    missing: Vec<String>,
    missing_toasted: bool,
    frame_duration: f64,
    hide_source: Option<glib::SourceId>,
    accurate_seek_source: Option<glib::SourceId>,
    inhibit: Option<u32>,
    over_controls: bool,
    last_pointer: Option<(f64, f64)>,
    last_save: Option<Instant>,
    track: u64,
    playing_state: bool,
}

pub struct Ui {
    app: adw::Application,
    pub window: adw::ApplicationWindow,
    toasts: adw::ToastOverlay,
    stack: gtk::Stack,
    error_page: adw::StatusPage,
    video_page: gtk::Overlay,
    paintable: VideoPaintable,
    title: adw::WindowTitle,
    header_revealer: gtk::Revealer,
    controls_revealer: gtk::Revealer,
    spinner: gtk::Spinner,
    play_button: gtk::Button,
    elapsed: gtk::Label,
    remaining: gtk::Label,
    seek: gtk::Scale,
    seek_popover: gtk::Popover,
    seek_label: gtk::Label,
    volume_button: gtk::MenuButton,
    volume_scale: gtk::Scale,
    mute_button: gtk::ToggleButton,
    speed_button: gtk::MenuButton,
    tracks_button: gtk::MenuButton,
    menu_buttons: Vec<gtk::MenuButton>,
    fullscreen_button: gtk::Button,
    player: Player,
    resume: RefCell<ResumeStore>,
    mpris: Mpris,
    bus_watch: RefCell<Option<gst::bus::BusWatchGuard>>,
    st: RefCell<St>,
    updating: Cell<bool>,
    me: RefCell<std::rc::Weak<Ui>>,
}

fn icon_button(icon: &str, tooltip: &str, action: &str) -> gtk::Button {
    gtk::Button::builder()
        .icon_name(icon)
        .tooltip_text(tooltip)
        .action_name(action)
        .focus_on_click(false)
        .focusable(false)
        .css_classes(["flat"])
        .build()
}

fn primary_menu() -> gio::Menu {
    let menu = gio::Menu::new();
    let s1 = gio::Menu::new();
    s1.append(Some("_Open…"), Some("win.open"));
    s1.append(Some("Take _Screenshot"), Some("win.screenshot"));
    menu.append_section(None, &s1);
    let s2 = gio::Menu::new();
    s2.append(Some("_About Video Player"), Some("app.about"));
    menu.append_section(None, &s2);
    menu
}

fn header(menu: &gio::Menu) -> (adw::HeaderBar, gtk::MenuButton) {
    let header = adw::HeaderBar::new();
    let open = gtk::Button::builder().icon_name("document-open-symbolic").tooltip_text("Open…").action_name("win.open").build();
    header.pack_start(&open);
    let menu_button = gtk::MenuButton::builder()
        .icon_name("open-menu-symbolic")
        .tooltip_text("Main Menu")
        .menu_model(menu)
        .primary(true)
        .build();
    header.pack_end(&menu_button);
    (header, menu_button)
}

fn status_view(menu: &gio::Menu, page: &adw::StatusPage) -> (adw::ToolbarView, gtk::MenuButton) {
    let view = adw::ToolbarView::new();
    let (hb, mb) = header(menu);
    view.add_top_bar(&hb);
    view.set_content(Some(page));
    (view, mb)
}

impl Ui {
    fn new(app: &adw::Application) -> Result<Rc<Ui>, glib::BoolError> {
        let (frame_tx, frame_rx) = async_channel::bounded::<()>(1);
        let player = Player::new(move || {
            let _ = frame_tx.try_send(());
        })?;
        let (cmd_tx, cmd_rx) = async_channel::unbounded::<Command>();
        let mpris = Mpris::start(cmd_tx);

        let window = adw::ApplicationWindow::builder()
            .application(app)
            .title(APP_NAME)
            .default_width(960)
            .default_height(600)
            .build();
        let menu = primary_menu();
        let mut menu_buttons = Vec::new();

        // Empty state.
        let open_btn = gtk::Button::builder()
            .label("_Open…")
            .use_underline(true)
            .action_name("win.open")
            .halign(gtk::Align::Center)
            .css_classes(["pill", "suggested-action"])
            .build();
        let empty_page = adw::StatusPage::builder()
            .icon_name("video-x-generic-symbolic")
            .title("Watch Videos")
            .description("Drag and drop videos here")
            .child(&open_btn)
            .build();
        let (empty_view, mb) = status_view(&menu, &empty_page);
        menu_buttons.push(mb);

        // Error state.
        let err_open = gtk::Button::builder()
            .label("_Open Another File…")
            .use_underline(true)
            .action_name("win.open")
            .halign(gtk::Align::Center)
            .css_classes(["pill"])
            .build();
        let error_page = adw::StatusPage::builder()
            .icon_name("dialog-error-symbolic")
            .title("Cannot Play File")
            .child(&err_open)
            .build();
        let (error_view, mb) = status_view(&menu, &error_page);
        menu_buttons.push(mb);

        // Video page.
        let paintable = VideoPaintable::default();
        let picture = gtk::Picture::builder()
            .paintable(&paintable)
            .content_fit(gtk::ContentFit::Contain)
            .can_shrink(true)
            .hexpand(true)
            .vexpand(true)
            .build();
        let video_page = gtk::Overlay::builder().child(&picture).css_classes(["vp-video"]).build();

        let title = adw::WindowTitle::new(APP_NAME, "");
        let (video_header, mb) = header(&menu);
        menu_buttons.push(mb);
        video_header.set_title_widget(Some(&title));
        video_header.add_css_class("vp-header");
        let header_revealer = gtk::Revealer::builder()
            .transition_type(gtk::RevealerTransitionType::Crossfade)
            .reveal_child(true)
            .valign(gtk::Align::Start)
            .child(&video_header)
            .build();
        video_page.add_overlay(&header_revealer);

        let spinner = gtk::Spinner::builder()
            .width_request(48)
            .height_request(48)
            .halign(gtk::Align::Center)
            .valign(gtk::Align::Center)
            .can_target(false)
            .build();
        video_page.add_overlay(&spinner);

        let play_button = icon_button("media-playback-start-symbolic", "Play", "win.play-pause");
        let elapsed = gtk::Label::builder().label("0:00").css_classes(["vp-time", "numeric"]).xalign(1.0).build();
        let remaining = gtk::Label::builder().label("--:--").css_classes(["vp-time", "numeric"]).xalign(0.0).build();
        let seek = gtk::Scale::builder()
            .orientation(gtk::Orientation::Horizontal)
            .adjustment(&gtk::Adjustment::new(0.0, 0.0, 1.0, 1.0, 10.0, 0.0))
            .hexpand(true)
            .draw_value(false)
            .focusable(false)
            .css_classes(["vp-seek"])
            .build();
        let seek_label = gtk::Label::builder().css_classes(["numeric"]).build();
        let seek_popover = gtk::Popover::builder()
            .autohide(false)
            .has_arrow(false)
            .can_focus(false)
            .can_target(false)
            .position(gtk::PositionType::Top)
            .child(&seek_label)
            .build();
        seek_popover.set_parent(&seek);
        {
            let p = seek_popover.clone();
            seek.connect_destroy(move |_| p.unparent());
        }

        let mute_button = gtk::ToggleButton::builder()
            .icon_name("audio-volume-muted-symbolic")
            .tooltip_text("Mute")
            .build();
        let volume_scale = gtk::Scale::builder()
            .orientation(gtk::Orientation::Horizontal)
            .adjustment(&gtk::Adjustment::new(1.0, 0.0, 1.0, 0.05, 0.1, 0.0))
            .width_request(160)
            .draw_value(false)
            .build();
        let vol_box = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        vol_box.append(&mute_button);
        vol_box.append(&volume_scale);
        let volume_button = gtk::MenuButton::builder()
            .icon_name("audio-volume-high-symbolic")
            .tooltip_text("Volume")
            .popover(&gtk::Popover::builder().child(&vol_box).build())
            .focus_on_click(false)
            .css_classes(["flat"])
            .direction(gtk::ArrowType::Up)
            .build();

        let speed_menu = gio::Menu::new();
        for (value, label) in SPEEDS {
            let item = gio::MenuItem::new(Some(label), None);
            item.set_action_and_target_value(Some("win.speed"), Some(&value.to_variant()));
            speed_menu.append_item(&item);
        }
        let speed_button = gtk::MenuButton::builder()
            .label("1×")
            .tooltip_text("Playback Speed")
            .menu_model(&speed_menu)
            .focus_on_click(false)
            .css_classes(["flat"])
            .direction(gtk::ArrowType::Up)
            .build();
        let tracks_button = gtk::MenuButton::builder()
            .icon_name("media-view-subtitles-symbolic")
            .tooltip_text("Audio and Subtitles")
            .focus_on_click(false)
            .css_classes(["flat"])
            .direction(gtk::ArrowType::Up)
            .build();
        let fullscreen_button = icon_button("view-fullscreen-symbolic", "Fullscreen", "win.fullscreen");

        let controls = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(6)
            .css_classes(["osd", "toolbar", "vp-controls"])
            .build();
        for w in [
            play_button.upcast_ref::<gtk::Widget>(),
            elapsed.upcast_ref(),
            seek.upcast_ref(),
            remaining.upcast_ref(),
            volume_button.upcast_ref(),
            speed_button.upcast_ref(),
            tracks_button.upcast_ref(),
            fullscreen_button.upcast_ref(),
        ] {
            controls.append(w);
        }
        let controls_revealer = gtk::Revealer::builder()
            .transition_type(gtk::RevealerTransitionType::Crossfade)
            .reveal_child(true)
            .valign(gtk::Align::End)
            .child(&controls)
            .build();
        video_page.add_overlay(&controls_revealer);

        let stack = gtk::Stack::builder().transition_type(gtk::StackTransitionType::Crossfade).build();
        stack.add_named(&empty_view, Some("empty"));
        stack.add_named(&error_view, Some("error"));
        stack.add_named(&video_page, Some("video"));
        stack.set_visible_child_name("empty");
        let toasts = adw::ToastOverlay::new();
        toasts.set_child(Some(&stack));
        window.set_content(Some(&toasts));

        let resume_path = helix_app_common::data_dir("helix-video-player").join("positions.json");
        let ui = Rc::new(Ui {
            app: app.clone(),
            window,
            toasts,
            stack,
            error_page,
            video_page,
            paintable,
            title,
            header_revealer,
            controls_revealer,
            spinner,
            play_button,
            elapsed,
            remaining,
            seek,
            seek_popover,
            seek_label,
            volume_button,
            volume_scale,
            mute_button,
            speed_button,
            tracks_button,
            menu_buttons,
            fullscreen_button,
            player,
            resume: RefCell::new(ResumeStore::load(&resume_path)),
            mpris,
            bus_watch: RefCell::new(None),
            st: RefCell::new(St { frame_duration: 1.0 / 25.0, ..Default::default() }),
            updating: Cell::new(false),
            me: RefCell::new(std::rc::Weak::new()),
        });
        ui.me.replace(Rc::downgrade(&ui));
        ui.install_actions();
        ui.connect_signals(&picture, &controls);
        ui.rebuild_tracks_menu();
        ui.sync_volume_ui();

        // Frames.
        glib::spawn_future_local(glib::clone!(
            #[weak]
            ui,
            async move {
                while frame_rx.recv().await.is_ok() {
                    ui.on_frame();
                }
            }
        ));
        // MPRIS commands.
        glib::spawn_future_local(glib::clone!(
            #[weak]
            ui,
            async move {
                while let Ok(cmd) = cmd_rx.recv().await {
                    ui.on_mpris(cmd);
                }
            }
        ));
        // Bus.
        let watch = ui
            .player
            .bus()
            .add_watch_local(glib::clone!(
                #[weak]
                ui,
                #[upgrade_or]
                glib::ControlFlow::Break,
                move |_, msg| {
                    ui.on_bus(msg);
                    glib::ControlFlow::Continue
                }
            ))
            .expect("bus watch");
        ui.bus_watch.replace(Some(watch));
        // Position ticker.
        glib::timeout_add_local(
            Duration::from_millis(250),
            glib::clone!(
                #[weak]
                ui,
                #[upgrade_or]
                glib::ControlFlow::Break,
                move || {
                    ui.tick();
                    glib::ControlFlow::Continue
                }
            ),
        );
        Ok(ui)
    }

    // ---- actions --------------------------------------------------------

    fn install_actions(self: &Rc<Self>) {
        let w = &self.window;
        let simple = |name: &str, f: fn(&Rc<Ui>)| {
            let a = gio::SimpleAction::new(name, None);
            let weak = Rc::downgrade(self);
            a.connect_activate(move |_, _| {
                if let Some(ui) = weak.upgrade() {
                    f(&ui);
                }
            });
            w.add_action(&a);
        };
        simple("open", |ui| ui.open_dialog());
        simple("play-pause", |ui| ui.toggle_play());
        simple("seek-back", |ui| ui.seek_relative(-SEEK_STEP));
        simple("seek-forward", |ui| ui.seek_relative(SEEK_STEP));
        simple("volume-up", |ui| ui.set_volume(ui.player.volume() + VOLUME_STEP));
        simple("volume-down", |ui| ui.set_volume(ui.player.volume() - VOLUME_STEP));
        simple("toggle-mute", |ui| ui.set_muted(!ui.player.muted()));
        simple("fullscreen", |ui| ui.window.set_fullscreened(!ui.window.is_fullscreen()));
        simple("leave-fullscreen", |ui| ui.window.set_fullscreened(false));
        simple("frame-next", |ui| ui.frame_step(true));
        simple("frame-prev", |ui| ui.frame_step(false));
        simple("screenshot", |ui| ui.screenshot());
        simple("load-subtitle", |ui| ui.subtitle_dialog());

        let speed = gio::SimpleAction::new_stateful("speed", Some(glib::VariantTy::STRING), &"1".to_variant());
        let weak = Rc::downgrade(self);
        speed.connect_activate(move |_, param| {
            let (Some(ui), Some(v)) = (weak.upgrade(), param.and_then(|p| p.get::<String>())) else { return };
            if let Ok(rate) = v.parse::<f64>() {
                ui.set_rate(rate);
            }
        });
        w.add_action(&speed);

        let audio = gio::SimpleAction::new_stateful("audio-track", Some(glib::VariantTy::INT32), &0i32.to_variant());
        let weak = Rc::downgrade(self);
        audio.connect_activate(move |a, param| {
            let (Some(ui), Some(i)) = (weak.upgrade(), param.and_then(|p| p.get::<i32>())) else { return };
            ui.player.set_audio(i);
            a.set_state(&i.to_variant());
        });
        w.add_action(&audio);

        let text = gio::SimpleAction::new_stateful("subtitle-track", Some(glib::VariantTy::INT32), &(-1i32).to_variant());
        let weak = Rc::downgrade(self);
        text.connect_activate(move |a, param| {
            let (Some(ui), Some(i)) = (weak.upgrade(), param.and_then(|p| p.get::<i32>())) else { return };
            ui.player.set_text(i);
            a.set_state(&i.to_variant());
        });
        w.add_action(&text);
        self.update_action_states();
    }

    fn action(&self, name: &str) -> Option<gio::SimpleAction> {
        self.window.lookup_action(name).and_downcast::<gio::SimpleAction>()
    }

    fn update_action_states(&self) {
        let has_media = {
            let st = self.st.borrow();
            st.uri.is_some() && !st.error
        };
        for name in ["play-pause", "seek-back", "seek-forward", "frame-next", "frame-prev", "speed", "load-subtitle"] {
            if let Some(a) = self.action(name) {
                a.set_enabled(has_media);
            }
        }
        if let Some(a) = self.action("screenshot") {
            a.set_enabled(has_media && self.paintable.texture().is_some());
        }
        if let Some(a) = self.action("leave-fullscreen") {
            a.set_enabled(self.window.is_fullscreen());
        }
    }

    // ---- signal wiring --------------------------------------------------

    fn connect_signals(self: &Rc<Self>, picture: &gtk::Picture, controls: &gtk::Box) {
        // Window lifecycle.
        self.window.connect_close_request(glib::clone!(
            #[weak(rename_to = ui)]
            self,
            #[upgrade_or]
            glib::Propagation::Proceed,
            move |_| {
                ui.save_position();
                ui.player.stop();
                ui.set_inhibit(false);
                ui.bus_watch.replace(None);
                glib::Propagation::Proceed
            }
        ));
        self.window.connect_fullscreened_notify(glib::clone!(
            #[weak(rename_to = ui)]
            self,
            move |w| {
                let fs = w.is_fullscreen();
                ui.fullscreen_button.set_icon_name(if fs { "view-restore-symbolic" } else { "view-fullscreen-symbolic" });
                ui.fullscreen_button.set_tooltip_text(Some(if fs { "Leave Fullscreen" } else { "Fullscreen" }));
                ui.update_action_states();
                ui.mpris.update(|s| s.fullscreen = fs);
                ui.mpris.changed(&[Prop::Fullscreen]);
                ui.show_controls();
            }
        ));

        // Auto-hide: pointer motion over the video page.
        let motion = gtk::EventControllerMotion::new();
        motion.connect_motion(glib::clone!(
            #[weak(rename_to = ui)]
            self,
            move |_, x, y| {
                // Some backends repeat motion events without movement.
                let moved = ui.st.borrow_mut().last_pointer.replace((x, y)) != Some((x, y));
                if moved {
                    ui.show_controls();
                }
            }
        ));
        motion.connect_leave(glib::clone!(
            #[weak(rename_to = ui)]
            self,
            move |_| ui.schedule_hide()
        ));
        self.video_page.add_controller(motion);
        for w in [controls.upcast_ref::<gtk::Widget>(), self.header_revealer.upcast_ref()] {
            let m = gtk::EventControllerMotion::new();
            m.connect_enter(glib::clone!(
                #[weak(rename_to = ui)]
                self,
                move |_, _, _| {
                    ui.st.borrow_mut().over_controls = true;
                }
            ));
            m.connect_leave(glib::clone!(
                #[weak(rename_to = ui)]
                self,
                move |_| {
                    ui.st.borrow_mut().over_controls = false;
                    ui.schedule_hide();
                }
            ));
            w.add_controller(m);
        }
        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        keys.connect_key_pressed(glib::clone!(
            #[weak(rename_to = ui)]
            self,
            #[upgrade_or]
            glib::Propagation::Proceed,
            move |_, _, _, _| {
                ui.show_controls();
                glib::Propagation::Proceed
            }
        ));
        self.window.add_controller(keys);
        for b in self.menu_buttons.iter().chain([&self.volume_button, &self.speed_button, &self.tracks_button]) {
            b.connect_active_notify(glib::clone!(
                #[weak(rename_to = ui)]
                self,
                move |b| {
                    if b.is_active() {
                        ui.show_controls();
                    } else {
                        ui.schedule_hide();
                    }
                }
            ));
        }

        // Clicks on the video: double-click toggles fullscreen.
        let click = gtk::GestureClick::new();
        click.connect_pressed(glib::clone!(
            #[weak(rename_to = ui)]
            self,
            move |_, n, _, _| {
                ui.show_controls();
                if n == 2 {
                    ui.window.set_fullscreened(!ui.window.is_fullscreen());
                }
            }
        ));
        picture.add_controller(click);

        // Seek bar.
        self.seek.connect_change_value(glib::clone!(
            #[weak(rename_to = ui)]
            self,
            #[upgrade_or]
            glib::Propagation::Proceed,
            move |scale, _, value| {
                let upper = scale.adjustment().upper();
                let value = value.clamp(0.0, upper);
                ui.user_seek(value);
                glib::Propagation::Proceed
            }
        ));
        let hover = gtk::EventControllerMotion::new();
        hover.connect_motion(glib::clone!(
            #[weak(rename_to = ui)]
            self,
            move |_, x, _| ui.seek_hover(x)
        ));
        hover.connect_leave(glib::clone!(
            #[weak(rename_to = ui)]
            self,
            move |_| ui.seek_popover.popdown()
        ));
        self.seek.add_controller(hover);

        // Volume.
        self.volume_scale.connect_change_value(glib::clone!(
            #[weak(rename_to = ui)]
            self,
            #[upgrade_or]
            glib::Propagation::Proceed,
            move |_, _, value| {
                let v = value.clamp(0.0, 1.0);
                ui.set_volume(v * v * v);
                glib::Propagation::Proceed
            }
        ));
        self.mute_button.connect_toggled(glib::clone!(
            #[weak(rename_to = ui)]
            self,
            move |b| {
                if !ui.updating.get() {
                    ui.set_muted(b.is_active());
                }
            }
        ));

        // Drag and drop.
        let drop = gtk::DropTarget::new(gdk::FileList::static_type(), gdk::DragAction::COPY);
        drop.connect_drop(glib::clone!(
            #[weak(rename_to = ui)]
            self,
            #[upgrade_or]
            false,
            move |_, value, _, _| {
                let Ok(list) = value.get::<gdk::FileList>() else { return false };
                match list.files().first() {
                    Some(file) => {
                        ui.open_uri(&file.uri());
                        true
                    }
                    None => false,
                }
            }
        ));
        self.toasts.add_controller(drop);
    }

    // ---- opening --------------------------------------------------------

    fn open_dialog(self: &Rc<Self>) {
        let videos = gtk::FileFilter::new();
        videos.set_name(Some("Videos"));
        videos.add_mime_type("video/*");
        let all = gtk::FileFilter::new();
        all.set_name(Some("All Files"));
        all.add_pattern("*");
        let filters = gio::ListStore::new::<gtk::FileFilter>();
        filters.append(&videos);
        filters.append(&all);
        let dialog = gtk::FileDialog::builder().title("Open Video").modal(true).filters(&filters).build();
        dialog.open(
            Some(&self.window),
            None::<&gio::Cancellable>,
            glib::clone!(
                #[weak(rename_to = ui)]
                self,
                move |res| {
                    if let Ok(file) = res {
                        ui.open_uri(&file.uri());
                    }
                }
            ),
        );
    }

    fn subtitle_dialog(self: &Rc<Self>) {
        let subs = gtk::FileFilter::new();
        subs.set_name(Some("Subtitles"));
        for ext in subtitles::EXTENSIONS {
            subs.add_suffix(ext);
        }
        let filters = gio::ListStore::new::<gtk::FileFilter>();
        filters.append(&subs);
        let dialog = gtk::FileDialog::builder().title("Load Subtitle File").modal(true).filters(&filters).build();
        dialog.open(
            Some(&self.window),
            None::<&gio::Cancellable>,
            glib::clone!(
                #[weak(rename_to = ui)]
                self,
                move |res| {
                    if let Ok(file) = res {
                        ui.load_subtitle(&file.uri());
                    }
                }
            ),
        );
    }

    fn load_subtitle(&self, uri: &str) {
        let pos = self.player.set_suburi(uri);
        let mut st = self.st.borrow_mut();
        st.pending_seek = pos;
        st.prerolled = false;
    }

    pub fn open_uri(self: &Rc<Self>, uri: &str) {
        self.save_position();
        let file = gio::File::for_uri(uri);
        let path = file.path();
        let name = file
            .basename()
            .map(|b| b.to_string_lossy().into_owned())
            .unwrap_or_else(|| uri.rsplit('/').next().unwrap_or(uri).to_string());
        let suburi: Option<String> = path
            .as_deref()
            .and_then(subtitles::discover)
            .and_then(|p| glib::filename_to_uri(&p, None).ok())
            .map(|u| u.to_string());
        {
            let mut st = self.st.borrow_mut();
            st.uri = Some(uri.to_string());
            st.name = name.clone();
            st.duration = None;
            st.prerolled = false;
            st.showing_video = false;
            st.want_play = true;
            st.eos = false;
            st.error = false;
            st.pending_seek = None;
            st.missing.clear();
            st.missing_toasted = false;
            st.track += 1;
            st.last_save = Some(Instant::now());
        }
        self.paintable.clear();
        self.window.set_title(Some(&name));
        self.title.set_title(&name);
        self.title.set_subtitle("");
        self.stack.set_visible_child_name("video");
        self.spinner.set_spinning(true);
        self.spinner.set_visible(true);
        self.set_rate_ui(1.0);
        self.update_time_labels(0.0);
        self.player.load(uri, suburi.as_deref(), true);
        self.show_controls();
        self.update_action_states();
        let track = self.st.borrow().track;
        let _ = path; // local path only used for subtitle discovery
        self.mpris.update(|s| {
            s.has_media = true;
            s.track = track;
            s.title = name.clone();
            s.url = uri.to_string();
            s.length_us = None;
            s.position_us = 0;
            s.rate = 1.0;
        });
        self.mpris.changed(&[Prop::Metadata, Prop::Rate]);
    }

    fn show_error(&self, detail: &str) {
        let missing = self.st.borrow().missing.clone();
        let description = if missing.is_empty() {
            glib::markup_escape_text(detail).to_string()
        } else {
            format!(
                "The required codec is not installed: <b>{}</b>",
                glib::markup_escape_text(&missing.join(", "))
            )
        };
        self.error_page.set_description(Some(&description));
        self.st.borrow_mut().error = true;
        self.st.borrow_mut().want_play = false;
        self.player.stop();
        self.paintable.clear();
        self.spinner.set_spinning(false);
        self.stack.set_visible_child_name("error");
        self.set_inhibit(false);
        self.update_action_states();
        self.mpris.update(|s| {
            s.has_media = false;
            s.status = "Stopped";
        });
        self.mpris.changed(&[Prop::Metadata, Prop::PlaybackStatus]);
    }

    // ---- frames / bus / ticks -------------------------------------------

    fn on_frame(&self) {
        let Some(sample) = self.player.slot.take() else { return };
        let Some(frame) = Frame::from_sample(&sample) else { return };
        self.paintable.set_frame(&frame);
        let first = {
            let mut st = self.st.borrow_mut();
            st.frame_duration = frame.frame_duration();
            !std::mem::replace(&mut st.showing_video, true)
        };
        if first {
            self.spinner.set_spinning(false);
            self.spinner.set_visible(false);
            self.update_action_states();
        }
    }

    fn on_bus(self: &Rc<Self>, msg: &gst::Message) {
        use gst::MessageView as M;
        let from_playbin = msg.src().is_some_and(|s| s == self.player.playbin.upcast_ref::<gst::Object>());
        match msg.view() {
            M::Element(_) => {
                if let Ok(m) = gst_pbutils::MissingPluginMessage::parse(msg) {
                    let d = m.description().to_string();
                    eprintln!("helix-video-player: missing plugin: {d}");
                    let mut st = self.st.borrow_mut();
                    if !st.missing.contains(&d) {
                        st.missing.push(d);
                    }
                }
            }
            M::Error(e) => {
                if self.st.borrow().error {
                    return;
                }
                eprintln!("helix-video-player: {} ({:?})", e.error(), e.debug());
                self.show_error(&e.error().to_string());
            }
            M::Eos(_) => {
                {
                    let mut st = self.st.borrow_mut();
                    st.eos = true;
                    st.want_play = false;
                }
                self.player.set_state(false);
                if let Some(uri) = self.st.borrow().uri.clone() {
                    let mut store = self.resume.borrow_mut();
                    if store.forget(&uri) {
                        let _ = store.save();
                    }
                }
                self.show_controls();
            }
            M::StateChanged(sc) if from_playbin => {
                let playing = sc.current() == gst::State::Playing;
                if self.st.borrow().playing_state != playing {
                    self.st.borrow_mut().playing_state = playing;
                    self.on_playing_changed(playing);
                }
            }
            M::AsyncDone(_) => self.on_prerolled(),
            M::DurationChanged(_) => {
                self.refresh_duration();
            }
            M::Application(a) => {
                if a.structure().is_some_and(|s| s.name() == player::TRACKS_CHANGED) {
                    self.rebuild_tracks_menu();
                }
            }
            M::Buffering(b) => {
                let pct = b.percent();
                let want = self.st.borrow().want_play;
                if pct < 100 {
                    self.player.set_state(false);
                    self.spinner.set_visible(true);
                    self.spinner.set_spinning(true);
                } else {
                    self.spinner.set_visible(false);
                    self.spinner.set_spinning(false);
                    self.player.set_state(want);
                }
            }
            _ => {}
        }
    }

    fn on_prerolled(self: &Rc<Self>) {
        let first = !std::mem::replace(&mut self.st.borrow_mut().prerolled, true);
        if !first {
            return;
        }
        self.refresh_duration();
        self.rebuild_tracks_menu();
        let seekable = self.player.is_seekable();
        self.mpris.update(|s| s.seekable = seekable);
        self.mpris.changed(&[Prop::CanSeek]);
        let pending = self.st.borrow_mut().pending_seek.take();
        if let Some(pos) = pending {
            self.player.seek(pos, true);
            return;
        }
        // A file whose video cannot be decoded at all is unsupported, even if
        // its audio plays; name the missing codec on the error page.
        let no_video = self.player.playbin.property::<i32>("n-video") == 0;
        if no_video && !self.st.borrow().missing.is_empty() {
            self.show_error("No decodable video stream");
            return;
        }
        // Missing codec for a secondary stream (e.g. audio): warn once.
        let missing = {
            let mut st = self.st.borrow_mut();
            (!st.missing.is_empty() && !std::mem::replace(&mut st.missing_toasted, true)).then(|| st.missing.join(", "))
        };
        if let Some(m) = missing {
            self.toasts.add_toast(adw::Toast::new(&format!("Missing codec: {m}")));
        }
        let uri = self.st.borrow().uri.clone();
        if let Some(pos) = uri.and_then(|u| self.resume.borrow().lookup(&u)) {
            let toast = adw::Toast::builder()
                .title(format!("Resume from {}?", timefmt::format_secs(pos, false)))
                .button_label("_Resume")
                .use_markup(false)
                .timeout(10)
                .build();
            toast.connect_button_clicked(glib::clone!(
                #[weak(rename_to = ui)]
                self,
                move |_| {
                    ui.player.seek(pos, true);
                    ui.update_time_labels(pos);
                }
            ));
            self.toasts.add_toast(toast);
        }
    }

    fn refresh_duration(&self) {
        let d = self.player.duration();
        self.st.borrow_mut().duration = d;
        self.updating.set(true);
        self.seek.adjustment().set_upper(d.unwrap_or(1.0));
        self.updating.set(false);
        self.mpris.update(|s| s.length_us = d.map(|d| (d * 1e6) as i64));
        self.mpris.changed(&[Prop::Metadata]);
    }

    fn on_playing_changed(&self, playing: bool) {
        self.play_button.set_icon_name(if playing { "media-playback-pause-symbolic" } else { "media-playback-start-symbolic" });
        self.play_button.set_tooltip_text(Some(if playing { "Pause" } else { "Play" }));
        self.set_inhibit(playing);
        let has_media = self.st.borrow().uri.is_some() && !self.st.borrow().error;
        let status = match (playing, has_media) {
            (true, _) => "Playing",
            (false, true) => "Paused",
            (false, false) => "Stopped",
        };
        self.mpris.update(|s| s.status = status);
        self.mpris.changed(&[Prop::PlaybackStatus]);
        if playing {
            self.schedule_hide();
        } else {
            self.show_controls();
        }
    }

    fn tick(&self) {
        if self.st.borrow().uri.is_none() || self.st.borrow().error {
            return;
        }
        let Some(pos) = self.player.position() else { return };
        if self.st.borrow().duration.is_none() {
            self.refresh_duration();
        }
        if self.st.borrow().accurate_seek_source.is_none() {
            self.updating.set(true);
            self.seek.set_value(pos);
            self.updating.set(false);
            self.update_time_labels(pos);
        }
        self.mpris.update(|s| s.position_us = (pos * 1e6) as i64);
        let due = self.player.is_playing()
            && self.st.borrow().last_save.is_none_or(|t| t.elapsed() >= SAVE_INTERVAL);
        if due {
            self.save_position();
        }
    }

    fn update_time_labels(&self, pos: f64) {
        let d = self.st.borrow().duration;
        self.elapsed.set_label(&timefmt::elapsed(pos, d));
        self.remaining.set_label(&timefmt::remaining(pos, d));
    }

    fn save_position(&self) {
        let (uri, d, eos) = {
            let st = self.st.borrow();
            (st.uri.clone(), st.duration, st.eos)
        };
        let Some(uri) = uri else { return };
        if eos || self.st.borrow().error {
            return;
        }
        let Some(pos) = self.player.position() else { return };
        self.st.borrow_mut().last_save = Some(Instant::now());
        let mut store = self.resume.borrow_mut();
        if store.record(&uri, pos, d, resume::now_unix()) {
            if let Err(err) = store.save() {
                eprintln!("helix-video-player: cannot save positions: {err}");
            }
        }
    }

    // ---- playback control -----------------------------------------------

    fn toggle_play(&self) {
        if self.st.borrow().uri.is_none() {
            return;
        }
        if self.player.is_playing() {
            self.pause();
        } else {
            self.play();
        }
    }

    fn play(&self) {
        if self.st.borrow().uri.is_none() || self.st.borrow().error {
            return;
        }
        let eos = std::mem::replace(&mut self.st.borrow_mut().eos, false);
        self.st.borrow_mut().want_play = true;
        if eos {
            self.player.seek(0.0, true);
        }
        self.player.set_state(true);
    }

    fn pause(&self) {
        self.st.borrow_mut().want_play = false;
        self.player.set_state(false);
        self.save_position();
    }

    fn stop(&self) {
        self.save_position();
        self.st.borrow_mut().want_play = false;
        self.player.set_state(false);
        self.player.seek(0.0, true);
    }

    fn seek_to(&self, pos: f64, accurate: bool) {
        if self.st.borrow().uri.is_none() {
            return;
        }
        let pos = match self.st.borrow().duration {
            Some(d) => pos.clamp(0.0, d),
            None => pos.max(0.0),
        };
        self.st.borrow_mut().eos = false;
        self.player.seek(pos, accurate);
        self.updating.set(true);
        self.seek.set_value(pos);
        self.updating.set(false);
        self.update_time_labels(pos);
        self.mpris.update(|s| s.position_us = (pos * 1e6) as i64);
        self.mpris.seeked((pos * 1e6) as i64);
    }

    fn seek_relative(&self, delta: f64) {
        if let Some(pos) = self.player.position() {
            self.seek_to(pos + delta, true);
        }
    }

    /// Dragging the seek bar: fast key-unit seeks, then one accurate seek
    /// once the value stops changing.
    fn user_seek(self: &Rc<Self>, value: f64) {
        if self.updating.get() {
            return;
        }
        self.seek_to(value, false);
        if let Some(src) = self.st.borrow_mut().accurate_seek_source.take() {
            src.remove();
        }
        let src = glib::timeout_add_local_once(
            Duration::from_millis(200),
            glib::clone!(
                #[weak(rename_to = ui)]
                self,
                move || {
                    ui.st.borrow_mut().accurate_seek_source = None;
                    ui.player.seek(ui.seek.value(), true);
                }
            ),
        );
        self.st.borrow_mut().accurate_seek_source = Some(src);
    }

    fn seek_hover(&self, x: f64) {
        let Some(d) = self.st.borrow().duration else { return };
        let rect = self.seek.range_rect();
        let width = rect.width().max(1) as f64;
        let frac = ((x - rect.x() as f64) / width).clamp(0.0, 1.0);
        self.seek_label.set_label(&timefmt::format_secs(frac * d, d >= 3600.0));
        self.seek_popover.set_pointing_to(Some(&gdk::Rectangle::new(x as i32, 0, 1, 1)));
        if !self.seek_popover.is_visible() {
            self.seek_popover.popup();
        }
    }

    fn frame_step(&self, forward: bool) {
        if self.st.borrow().uri.is_none() || self.player.is_playing() {
            return;
        }
        self.st.borrow_mut().eos = false;
        if forward {
            self.player.step_forward();
        } else {
            let fd = self.st.borrow().frame_duration;
            self.player.step_back(fd);
        }
    }

    fn set_rate(&self, rate: f64) {
        self.player.set_rate(rate);
        self.set_rate_ui(self.player.rate());
        self.mpris.update(|s| s.rate = rate);
        self.mpris.changed(&[Prop::Rate]);
    }

    fn set_rate_ui(&self, rate: f64) {
        let key = SPEEDS
            .iter()
            .map(|(k, _)| *k)
            .find(|k| k.parse::<f64>().is_ok_and(|r| (r - rate).abs() < 1e-6))
            .map(str::to_string)
            .unwrap_or_else(|| format!("{rate}"));
        if let Some(a) = self.action("speed") {
            a.set_state(&key.to_variant());
        }
        self.speed_button.set_label(&format!("{key}×"));
    }

    fn set_volume(&self, v: f64) {
        self.player.set_volume(v);
        if v > 0.0 && self.player.muted() {
            self.player.set_muted(false);
        }
        self.sync_volume_ui();
        self.mpris.update(|s| s.volume = v.clamp(0.0, 1.0));
        self.mpris.changed(&[Prop::Volume]);
    }

    fn set_muted(&self, muted: bool) {
        self.player.set_muted(muted);
        self.sync_volume_ui();
    }

    fn sync_volume_ui(&self) {
        let v = self.player.volume();
        let muted = self.player.muted();
        self.updating.set(true);
        self.volume_scale.set_value(v.cbrt());
        self.mute_button.set_active(muted);
        self.updating.set(false);
        let icon = if muted || v <= 0.0 {
            "audio-volume-muted-symbolic"
        } else if v < 0.33 {
            "audio-volume-low-symbolic"
        } else if v < 0.66 {
            "audio-volume-medium-symbolic"
        } else {
            "audio-volume-high-symbolic"
        };
        self.volume_button.set_icon_name(icon);
    }

    fn set_inhibit(&self, on: bool) {
        let mut st = self.st.borrow_mut();
        match (on, st.inhibit) {
            (true, None) => {
                let cookie = self.app.inhibit(Some(&self.window), gtk::ApplicationInhibitFlags::IDLE, Some("Playing video"));
                if cookie != 0 {
                    st.inhibit = Some(cookie);
                }
            }
            (false, Some(cookie)) => {
                self.app.uninhibit(cookie);
                st.inhibit = None;
            }
            _ => {}
        }
    }

    fn rebuild_tracks_menu(&self) {
        let menu = gio::Menu::new();
        let audio = self.player.audio_tracks();
        if !audio.is_empty() {
            let section = gio::Menu::new();
            for t in &audio {
                let item = gio::MenuItem::new(Some(&t.label), None);
                item.set_action_and_target_value(Some("win.audio-track"), Some(&t.index.to_variant()));
                section.append_item(&item);
            }
            menu.append_section(Some("Audio"), &section);
        }
        let section = gio::Menu::new();
        let none = gio::MenuItem::new(Some("None"), None);
        none.set_action_and_target_value(Some("win.subtitle-track"), Some(&(-1i32).to_variant()));
        section.append_item(&none);
        for t in self.player.text_tracks() {
            let item = gio::MenuItem::new(Some(&t.label), None);
            item.set_action_and_target_value(Some("win.subtitle-track"), Some(&t.index.to_variant()));
            section.append_item(&item);
        }
        section.append(Some("Load Subtitle File…"), Some("win.load-subtitle"));
        menu.append_section(Some("Subtitles"), &section);
        self.tracks_button.set_menu_model(Some(&menu));
        if let Some(a) = self.action("audio-track") {
            a.set_state(&self.player.current_audio().max(0).to_variant());
        }
        if let Some(a) = self.action("subtitle-track") {
            let n = self.player.text_tracks().len() as i32;
            let cur = self.player.current_text();
            a.set_state(&(if cur >= 0 && cur < n { cur } else { -1 }).to_variant());
        }
    }

    // ---- controls visibility --------------------------------------------

    fn show_controls(&self) {
        self.header_revealer.set_reveal_child(true);
        self.controls_revealer.set_reveal_child(true);
        self.video_page.set_cursor(None);
        self.schedule_hide();
    }

    fn should_hide(&self) -> bool {
        let st = self.st.borrow();
        st.showing_video
            && !st.over_controls
            && self.player.is_playing()
            && !self.menu_buttons.iter().chain([&self.volume_button, &self.speed_button, &self.tracks_button]).any(|b| b.is_active())
    }

    fn schedule_hide(&self) {
        if let Some(src) = self.st.borrow_mut().hide_source.take() {
            src.remove();
        }
        let me = self.me.borrow().clone();
        let src = glib::timeout_add_local_once(HIDE_DELAY, move || {
            let Some(ui) = me.upgrade() else { return };
            ui.st.borrow_mut().hide_source = None;
            if ui.should_hide() {
                ui.header_revealer.set_reveal_child(false);
                ui.controls_revealer.set_reveal_child(false);
                ui.seek_popover.popdown();
                ui.video_page.set_cursor_from_name(Some("none"));
            }
        });
        self.st.borrow_mut().hide_source = Some(src);
    }

    // ---- screenshot -----------------------------------------------------

    fn screenshot(self: &Rc<Self>) {
        let Some(texture) = self.paintable.texture() else { return };
        let (name, pos) = (self.st.borrow().name.clone(), self.player.position().unwrap_or(0.0));
        let dir = screenshot::target_dir();
        let path: PathBuf = screenshot::unique_path(&dir, &screenshot::base_name(&name, pos), "png", |p| p.exists());
        let handle = gio::spawn_blocking(move || texture.save_to_png(&path).map(|_| path));
        glib::spawn_future_local(glib::clone!(
            #[weak(rename_to = ui)]
            self,
            async move {
                let msg = match handle.await {
                    Ok(Ok(path)) => format!(
                        "Screenshot saved as “{}”",
                        path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
                    ),
                    Ok(Err(err)) => format!("Could not save screenshot: {err}"),
                    Err(_) => "Could not save screenshot".to_string(),
                };
                ui.toasts.add_toast(adw::Toast::new(&msg));
            }
        ));
    }

    // ---- MPRIS ----------------------------------------------------------

    fn on_mpris(self: &Rc<Self>, cmd: Command) {
        match cmd {
            Command::Raise => self.window.present(),
            Command::Quit => {
                self.save_position();
                self.window.close();
            }
            Command::Play => self.play(),
            Command::Pause => self.pause(),
            Command::PlayPause => self.toggle_play(),
            Command::Stop => self.stop(),
            Command::Seek(us) => {
                if let Some(pos) = self.player.position() {
                    self.seek_to(pos + us as f64 / 1e6, true);
                }
            }
            Command::SetPosition(us) => {
                let pos = us as f64 / 1e6;
                if self.st.borrow().duration.is_none_or(|d| pos <= d) {
                    self.seek_to(pos, true);
                }
            }
            Command::OpenUri(uri) => self.open_uri(&uri),
            Command::SetVolume(v) => self.set_volume(v),
            Command::SetRate(r) => self.set_rate(r.clamp(mpris::MIN_RATE, mpris::MAX_RATE)),
            Command::SetFullscreen(f) => self.window.set_fullscreened(f),
        }
    }
}
