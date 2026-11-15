//! GTK user interface.

use adw::prelude::*;
use gtk::{gdk, gio, glib};
use helix_app_common::Labelled;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::{Rc, Weak};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use crate::meta::TrackInfo;
use crate::mpris::{self, Command, Snapshot, Status, Track};
use crate::peaks::{self, PEAKS_PER_SECOND};
use crate::player::{Event, Player};
use crate::seek::{self, SKIP_NS};
use crate::timefmt;
use crate::waveform;
use helix_app_common::i18n::{n_, tr};
use helix_app_common::trf;

pub const APP_ID: &str = "org.helix.AudioPlayer";
const APP_NAME: &str = n_("Audio Player");
const STATE_DIR: &str = "helix-audio-player";
const TICK: Duration = Duration::from_millis(50);
const COVER_SIZE: i32 = 160;

const ACCELS: &[(&str, &[&str])] = &[
    ("win.open", &["<Control>o"]),
    ("win.play-pause", &["AudioPlay"]),
    ("win.show-help-overlay", &["<Control>question"]),
];

/// Keys without modifiers, handled only when the focused widget does not
/// use them (Space presses a focused button, arrows move a focused slider).
const SINGLE_KEYS: &[(&str, &str)] = &[
    ("space", "win.play-pause"),
    ("Left", "win.seek-back"),
    ("Right", "win.seek-forward"),
    ("m", "win.mute"),
];

pub fn run(target: Option<String>) -> glib::ExitCode {
    let app = adw::Application::builder()
        .application_id(APP_ID)
        .flags(gio::ApplicationFlags::NON_UNIQUE)
        .build();
    helix_app_common::install_standard_actions(&app, tr(APP_NAME), tr("Play audio files"));
    app.connect_startup(|app| {
        load_css();
        for (action, accels) in ACCELS {
            app.set_accels_for_action(action, accels);
        }
    });
    let target = RefCell::new(target);
    // Owns the window controller for the lifetime of the application.
    let keep: Rc<RefCell<Option<Rc<Ui>>>> = Rc::default();
    let k = keep.clone();
    app.connect_shutdown(move |_| {
        if let Some(ui) = k.borrow_mut().take() {
            ui.shutdown();
        }
    });
    app.connect_activate(move |app| {
        if let Some(ui) = keep.borrow().as_ref() {
            ui.window.present();
            return;
        }
        let Some(ui) = Ui::new(app) else {
            app.quit();
            return;
        };
        if let Some(uri) = target.borrow_mut().take() {
            ui.open(&uri);
        }
        ui.window.present();
        *keep.borrow_mut() = Some(ui);
    });
    app.run_with_args(&[std::env::args().next().unwrap_or_default()])
}

fn load_css() {
    helix_app_common::style::add_css(
        ".ap-cover { border-radius: 12px; }
         .ap-play { min-width: 56px; min-height: 56px; }
         .ap-waveform { min-height: 96px; }
         .ap-waveform:focus-visible { outline: 2px solid alpha(@accent_color, 0.5);
                                      outline-offset: 2px; border-radius: 6px; }",
        ".ap-waveform:focus-visible { outline-color: @accent_color; }",
    );
}

#[derive(Default)]
struct State {
    uri: Option<String>,
    info: TrackInfo,
    track_id: u64,
    duration: Option<u64>,
    position: u64,
    /// Requested playing state.
    playing: bool,
    /// Rewound after end of stream / MPRIS Stop.
    stopped: bool,
    rate: f64,
    /// The rate could not be applied yet (pipeline not prerolled).
    rate_pending: bool,
    /// Perceptual volume 0–1 (slider value).
    volume: f64,
    muted: bool,
    repeat: bool,
    peaks: Rc<Vec<f32>>,
    peaks_complete: bool,
    /// Position under the pointer while dragging on the waveform.
    drag: Option<u64>,
    drag_start_x: f64,
}

enum WaveMsg {
    Partial(Vec<f32>),
    Done(Result<Vec<f32>, String>),
}

struct Ui {
    app: adw::Application,
    window: adw::ApplicationWindow,
    stack: gtk::Stack,
    error_page: adw::StatusPage,
    cover: gtk::Image,
    title: gtk::Label,
    subtitle: gtk::Label,
    waveform: gtk::DrawingArea,
    elapsed: gtk::Label,
    remaining: gtk::Label,
    play: gtk::Button,
    rate_button: gtk::MenuButton,
    volume_button: gtk::MenuButton,
    volume_scale: gtk::Scale,
    player: Player,
    state: RefCell<State>,
    mpris: Option<mpris::Handle>,
    last_snapshot: RefCell<Option<Snapshot>>,
    wave_cancel: RefCell<Option<Arc<AtomicBool>>>,
    wave_cache: RefCell<HashMap<String, Rc<Vec<f32>>>>,
    /// Bumped on every load so stale worker results are dropped.
    generation: Cell<u64>,
    _bus_watch: RefCell<Option<gst::bus::BusWatchGuard>>,
}

fn button(icon: &str, tooltip: &str, action: &str) -> gtk::Button {
    let b = gtk::Button::builder()
        .icon_name(icon)
        .tooltip_text(tooltip)
        .action_name(action)
        .valign(gtk::Align::Center)
        .build()
        .labelled(helix_app_common::a11y::name_from_tooltip(tooltip));
    b.add_css_class("flat");
    b
}

fn shortcuts_window() -> gtk::ShortcutsWindow {
    helix_app_common::shortcuts::window(SHORTCUT_GROUPS)
}

const SHORTCUT_GROUPS: &[helix_app_common::shortcuts::Group] = {
    use helix_app_common::shortcuts::Item::Keys;
    &[
        (
            n_("Playback"),
            &[
                Keys(n_("Play or Pause"), "space AudioPlay"),
                Keys(n_("Back 10 Seconds"), "Left"),
                Keys(n_("Forward 10 Seconds"), "Right"),
                Keys(n_("Mute or Unmute"), "m"),
            ],
        ),
        (
            n_("General"),
            &[
                Keys(n_("Open File"), "<Control>o"),
                Keys(n_("Keyboard Shortcuts"), "<Control>question"),
                Keys(n_("Close Window"), "<Control>w"),
                Keys(n_("Quit"), "<Control>q"),
            ],
        ),
    ]
};

impl Ui {
    fn new(app: &adw::Application) -> Option<Rc<Self>> {
        let player = match Player::new() {
            Ok(p) => p,
            Err(e) => {
                eprintln!("helix-audio-player: cannot create player: {e}");
                return None;
            }
        };
        let window = adw::ApplicationWindow::builder()
            .application(app)
            .title(tr(APP_NAME))
            .default_width(600)
            .default_height(420)
            .width_request(360)
            .height_request(294)
            .build();
        helix_app_common::window_state::persist(
            &window,
            helix_app_common::window_state::Store::file(STATE_DIR),
        );
        window.set_help_overlay(Some(&shortcuts_window()));
        helix_app_common::add_bubble_shortcuts(&window, SINGLE_KEYS);

        // Header bar.
        let header = adw::HeaderBar::builder().show_title(false).build();
        let open_button = gtk::Button::builder()
            .label(tr("Open"))
            .action_name("win.open")
            .tooltip_text(tr("Open a File"))
            .build();
        header.pack_start(&open_button);
        let menu = gio::Menu::new();
        menu.append(Some(tr("_Open…")), Some("win.open"));
        let section = gio::Menu::new();
        section.append(
            Some(helix_app_common::i18n::tr(
                helix_app_common::shortcuts::MENU_LABEL,
            )),
            Some("win.show-help-overlay"),
        );
        section.append(Some(tr("_About Audio Player")), Some("app.about"));
        menu.append_section(None, &section);
        let menu_button = gtk::MenuButton::builder()
            .icon_name("open-menu-symbolic")
            .menu_model(&menu)
            .primary(true)
            .tooltip_text(tr("Main Menu"))
            .build()
            .labelled(tr("Main Menu"));
        header.pack_end(&menu_button);

        // Empty page.
        let empty_open = gtk::Button::builder()
            .label(tr("_Open…"))
            .use_underline(true)
            .action_name("win.open")
            .halign(gtk::Align::Center)
            .build();
        empty_open.add_css_class("pill");
        empty_open.add_css_class("suggested-action");
        let empty = adw::StatusPage::builder()
            .icon_name("audio-x-generic-symbolic")
            .title(tr("Open an Audio File"))
            .description(tr("Drag and drop a file here"))
            .child(&empty_open)
            .build();

        // Error page.
        let error_open = gtk::Button::builder()
            .label(tr("Open _Another File…"))
            .use_underline(true)
            .action_name("win.open")
            .halign(gtk::Align::Center)
            .build();
        error_open.add_css_class("pill");
        let error_page = adw::StatusPage::builder()
            .icon_name("dialog-error-symbolic")
            .title(tr("Unable to Play File"))
            .child(&error_open)
            .build();

        // Player page.
        // Decorative: the title below names the track.
        let cover = gtk::Image::builder()
            .accessible_role(gtk::AccessibleRole::Presentation)
            .pixel_size(COVER_SIZE)
            .halign(gtk::Align::Center)
            .build();
        cover.add_css_class("ap-cover");
        let title = gtk::Label::builder()
            .wrap(true)
            .justify(gtk::Justification::Center)
            .max_width_chars(40)
            .build();
        title.add_css_class("title-2");
        let subtitle = gtk::Label::builder()
            .wrap(true)
            .justify(gtk::Justification::Center)
            .build();
        subtitle.add_css_class("dim-label");

        // Focusable and operated with the arrow, Page Up/Down, Home and End
        // keys (see `connect_signals`). The `accent` class makes its own
        // colour the accent colour, for the played bars.
        let waveform = gtk::DrawingArea::builder()
            .accessible_role(gtk::AccessibleRole::Slider)
            .tooltip_text(tr("Position"))
            .hexpand(true)
            .content_height(96)
            .focusable(true)
            .css_classes(["ap-waveform", "accent"])
            .build();
        waveform.set_cursor_from_name(Some("pointer"));
        helix_app_common::set_accessible_label(&waveform, tr("Position"));

        let elapsed = gtk::Label::builder()
            .label("0:00")
            .halign(gtk::Align::Start)
            .hexpand(true)
            .build();
        let remaining = gtk::Label::builder()
            .label("\u{2212}0:00")
            .halign(gtk::Align::End)
            .build();
        for l in [&elapsed, &remaining] {
            l.add_css_class("numeric");
            l.add_css_class("caption");
            l.add_css_class("dim-label");
        }
        let times = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        times.append(&elapsed);
        times.append(&remaining);

        // Rate menu.
        let rate_menu = gio::Menu::new();
        for r in seek::RATES {
            rate_menu.append(
                Some(&seek::rate_label(r)),
                Some(&format!("win.rate::{r:.2}")),
            );
        }
        let rate_button = gtk::MenuButton::builder()
            .label(seek::rate_label(1.0))
            .menu_model(&rate_menu)
            .tooltip_text(if player.pitch_corrected {
                tr("Playback Speed")
            } else {
                tr("Playback Speed (pitch not preserved)")
            })
            .valign(gtk::Align::Center)
            .build();
        rate_button.add_css_class("flat");
        helix_app_common::set_accessible_label(&rate_button, &seek::rate_name(1.0));

        let play = gtk::Button::builder()
            .icon_name("media-playback-start-symbolic")
            .action_name("win.play-pause")
            .tooltip_text(tr("Play"))
            .valign(gtk::Align::Center)
            .build()
            .labelled(tr("Play"));
        play.add_css_class("circular");
        play.add_css_class("suggested-action");
        play.add_css_class("ap-play");
        let back = button(
            "media-seek-backward-symbolic",
            tr("Back 10 Seconds"),
            "win.seek-back",
        );
        let forward = button(
            "media-seek-forward-symbolic",
            tr("Forward 10 Seconds"),
            "win.seek-forward",
        );
        let repeat = gtk::ToggleButton::builder()
            .icon_name("media-playlist-repeat-symbolic")
            .action_name("win.repeat")
            .tooltip_text(tr("Repeat"))
            .valign(gtk::Align::Center)
            .build()
            .labelled(tr("Repeat"));
        repeat.add_css_class("flat");

        // Volume popover.
        let mute = gtk::ToggleButton::builder()
            .icon_name("audio-volume-muted-symbolic")
            .action_name("win.mute")
            .tooltip_text(tr("Mute"))
            .build()
            .labelled(tr("Mute"));
        mute.add_css_class("flat");
        let volume_scale = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, 1.0, 0.05);
        volume_scale.set_width_request(180);
        volume_scale.set_hexpand(true);
        volume_scale.set_value(1.0);
        helix_app_common::set_accessible_label(&volume_scale, tr("Volume"));
        let vbox = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        vbox.append(&mute);
        vbox.append(&volume_scale);
        let popover = gtk::Popover::builder().child(&vbox).build();
        let volume_button = gtk::MenuButton::builder()
            .icon_name("audio-volume-high-symbolic")
            .popover(&popover)
            .tooltip_text(tr("Volume"))
            .valign(gtk::Align::Center)
            .build()
            .labelled(tr("Volume"));
        volume_button.add_css_class("flat");

        let controls = gtk::CenterBox::new();
        let left = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        left.append(&rate_button);
        let center = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        center.append(&back);
        center.append(&play);
        center.append(&forward);
        let right = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        right.append(&repeat);
        right.append(&volume_button);
        controls.set_start_widget(Some(&left));
        controls.set_center_widget(Some(&center));
        controls.set_end_widget(Some(&right));

        let content = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(12)
            .valign(gtk::Align::Center)
            .margin_top(12)
            .margin_bottom(24)
            .margin_start(12)
            .margin_end(12)
            .build();
        content.append(&cover);
        let text_box = gtk::Box::new(gtk::Orientation::Vertical, 4);
        text_box.append(&title);
        text_box.append(&subtitle);
        content.append(&text_box);
        let wave_box = gtk::Box::new(gtk::Orientation::Vertical, 2);
        wave_box.append(&waveform);
        wave_box.append(&times);
        content.append(&wave_box);
        content.append(&controls);
        let clamp = adw::Clamp::builder()
            .maximum_size(640)
            .child(&content)
            .build();
        let scroller = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .child(&clamp)
            .build();

        let stack = gtk::Stack::builder()
            .transition_type(gtk::StackTransitionType::Crossfade)
            .build();
        stack.add_named(&empty, Some("empty"));
        stack.add_named(&scroller, Some("player"));
        stack.add_named(&error_page, Some("error"));
        stack.set_visible_child_name("empty");

        let toolbar = adw::ToolbarView::new();
        toolbar.add_top_bar(&header);
        toolbar.set_content(Some(&stack));
        window.set_content(Some(&toolbar));

        let mpris = if std::env::var_os("HELIX_AUDIO_NO_MPRIS").is_some() {
            None
        } else {
            let (tx, rx) = async_channel::unbounded();
            Some((mpris::spawn(tx), rx))
        };
        let (mpris, mpris_rx) = match mpris {
            Some((h, rx)) => (Some(h), Some(rx)),
            None => (None, None),
        };

        let ui = Rc::new(Ui {
            app: app.clone(),
            window,
            stack,
            error_page,
            cover,
            title,
            subtitle,
            waveform,
            elapsed,
            remaining,
            play,
            rate_button,
            volume_button,
            volume_scale,
            player,
            state: RefCell::new(State {
                rate: 1.0,
                volume: 1.0,
                ..Default::default()
            }),
            mpris,
            last_snapshot: RefCell::new(None),
            wave_cancel: RefCell::new(None),
            wave_cache: RefCell::new(HashMap::new()),
            generation: Cell::new(0),
            _bus_watch: RefCell::new(None),
        });

        ui.install_actions();
        ui.connect_signals();
        ui.install_bus_watch();
        if let Some(rx) = mpris_rx {
            let weak = Rc::downgrade(&ui);
            glib::spawn_future_local(async move {
                while let Ok(cmd) = rx.recv().await {
                    let Some(ui) = weak.upgrade() else { break };
                    ui.handle_command(cmd);
                }
            });
        }
        let weak = Rc::downgrade(&ui);
        glib::timeout_add_local(TICK, move || match weak.upgrade() {
            Some(ui) => {
                ui.tick();
                glib::ControlFlow::Continue
            }
            None => glib::ControlFlow::Break,
        });
        ui.sync_controls();
        ui.publish();
        Some(ui)
    }

    fn install_actions(self: &Rc<Self>) {
        let w = Rc::downgrade(self);
        let with = move |f: fn(&Rc<Ui>)| {
            let w = w.clone();
            move |_: &adw::ApplicationWindow, _: &gio::SimpleAction, _: Option<&glib::Variant>| {
                if let Some(ui) = w.upgrade() {
                    f(&ui);
                }
            }
        };
        let open = gio::ActionEntry::builder("open")
            .activate(with(|ui| ui.choose_file()))
            .build();
        let play_pause = gio::ActionEntry::builder("play-pause")
            .activate(with(|ui| ui.toggle()))
            .build();
        let back = gio::ActionEntry::builder("seek-back")
            .activate(with(|ui| ui.skip(-SKIP_NS)))
            .build();
        let fwd = gio::ActionEntry::builder("seek-forward")
            .activate(with(|ui| ui.skip(SKIP_NS)))
            .build();
        let w = Rc::downgrade(self);
        let mute = gio::ActionEntry::builder("mute")
            .state(false.to_variant())
            .change_state(
                move |_: &adw::ApplicationWindow, action: &gio::SimpleAction, value| {
                    let Some(v) = value.and_then(|v| v.get::<bool>()) else {
                        return;
                    };
                    action.set_state(&v.to_variant());
                    if let Some(ui) = w.upgrade() {
                        ui.state.borrow_mut().muted = v;
                        ui.apply_volume();
                    }
                },
            )
            .build();
        let w = Rc::downgrade(self);
        let repeat = gio::ActionEntry::builder("repeat")
            .state(false.to_variant())
            .change_state(
                move |_: &adw::ApplicationWindow, action: &gio::SimpleAction, value| {
                    let Some(v) = value.and_then(|v| v.get::<bool>()) else {
                        return;
                    };
                    action.set_state(&v.to_variant());
                    if let Some(ui) = w.upgrade() {
                        ui.state.borrow_mut().repeat = v;
                        ui.publish();
                    }
                },
            )
            .build();
        let w = Rc::downgrade(self);
        let rate = gio::ActionEntry::builder("rate")
            .parameter_type(Some(glib::VariantTy::STRING))
            .state("1.00".to_variant())
            .change_state(
                move |_: &adw::ApplicationWindow, action: &gio::SimpleAction, value| {
                    let Some(r) = value
                        .and_then(|v| v.str())
                        .and_then(|s| s.parse::<f64>().ok())
                    else {
                        return;
                    };
                    let r = seek::clamp_rate(r);
                    action.set_state(&format!("{r:.2}").to_variant());
                    if let Some(ui) = w.upgrade() {
                        ui.set_rate(r);
                    }
                },
            )
            .build();
        self.window
            .add_action_entries([open, play_pause, back, fwd, mute, repeat, rate]);
    }

    fn action(&self, name: &str) -> Option<gio::SimpleAction> {
        self.window
            .lookup_action(name)
            .and_then(|a| a.downcast::<gio::SimpleAction>().ok())
    }

    fn connect_signals(self: &Rc<Self>) {
        // Waveform drawing and seeking.
        let w = Rc::downgrade(self);
        self.waveform.set_draw_func(move |area, cr, width, height| {
            if let Some(ui) = w.upgrade() {
                ui.draw_waveform(area, cr, width as f64, height as f64);
            }
        });
        let drag = gtk::GestureDrag::new();
        let w = Rc::downgrade(self);
        drag.connect_drag_begin(move |_, x, _| {
            if let Some(ui) = w.upgrade() {
                ui.state.borrow_mut().drag_start_x = x;
                ui.scrub(x);
            }
        });
        let w = Rc::downgrade(self);
        drag.connect_drag_update(move |_, dx, _| {
            if let Some(ui) = w.upgrade() {
                let x = ui.state.borrow().drag_start_x + dx;
                ui.scrub(x);
            }
        });
        let w = Rc::downgrade(self);
        drag.connect_drag_end(move |_, dx, _| {
            if let Some(ui) = w.upgrade() {
                let x = ui.state.borrow().drag_start_x + dx;
                ui.scrub(x);
                let target = ui.state.borrow_mut().drag.take();
                if let Some(t) = target {
                    ui.seek_to(t);
                }
            }
        });
        self.waveform.add_controller(drag);
        helix_app_common::style::redraw_on_style_change(&self.waveform);

        // Keyboard seeking while the waveform has focus.
        let keys = gtk::EventControllerKey::new();
        let w = Rc::downgrade(self);
        keys.connect_key_pressed(move |_, key, _, mods| {
            use seek::KeySeek;
            let Some(ui) = w.upgrade() else {
                return glib::Propagation::Proceed;
            };
            if mods.intersects(
                gdk::ModifierType::CONTROL_MASK
                    | gdk::ModifierType::ALT_MASK
                    | gdk::ModifierType::SHIFT_MASK
                    | gdk::ModifierType::SUPER_MASK,
            ) {
                return glib::Propagation::Proceed;
            }
            // Left is "back" in left-to-right text, "forward" otherwise.
            let rtl = ui.waveform.direction() == gtk::TextDirection::Rtl;
            let (back, fwd) = if rtl {
                (seek::KEY_STEP_NS, -seek::KEY_STEP_NS)
            } else {
                (-seek::KEY_STEP_NS, seek::KEY_STEP_NS)
            };
            let request = match key {
                gdk::Key::Left | gdk::Key::KP_Left => KeySeek::By(back),
                gdk::Key::Right | gdk::Key::KP_Right => KeySeek::By(fwd),
                gdk::Key::Down | gdk::Key::KP_Down => KeySeek::By(-seek::KEY_STEP_NS),
                gdk::Key::Up | gdk::Key::KP_Up => KeySeek::By(seek::KEY_STEP_NS),
                gdk::Key::Page_Down | gdk::Key::KP_Page_Down => KeySeek::By(-seek::KEY_PAGE_NS),
                gdk::Key::Page_Up | gdk::Key::KP_Page_Up => KeySeek::By(seek::KEY_PAGE_NS),
                gdk::Key::Home | gdk::Key::KP_Home => KeySeek::Start,
                gdk::Key::End | gdk::Key::KP_End => KeySeek::End,
                _ => return glib::Propagation::Proceed,
            };
            if ui.has_file() {
                let (pos, dur) = {
                    let st = ui.state.borrow();
                    (st.position, st.duration)
                };
                ui.seek_to(seek::key_target(request, pos, dur));
                ui.waveform.queue_draw();
            }
            glib::Propagation::Stop
        });
        self.waveform.add_controller(keys);

        // Volume slider.
        let w = Rc::downgrade(self);
        self.volume_scale.connect_value_changed(move |s| {
            if let Some(ui) = w.upgrade() {
                ui.state.borrow_mut().volume = s.value();
                ui.apply_volume();
            }
        });

        // Drag and drop of files.
        let drop = gtk::DropTarget::new(gdk::FileList::static_type(), gdk::DragAction::COPY);
        let w = Rc::downgrade(self);
        drop.connect_drop(move |_, value, _, _| {
            let Some(ui) = w.upgrade() else { return false };
            let Ok(list) = value.get::<gdk::FileList>() else {
                return false;
            };
            match list.files().first() {
                Some(f) => {
                    ui.open(&f.uri());
                    true
                }
                None => false,
            }
        });
        self.window.add_controller(drop);

        let w = Rc::downgrade(self);
        self.window.connect_close_request(move |_| {
            if let Some(ui) = w.upgrade() {
                ui.shutdown();
            }
            glib::Propagation::Proceed
        });
    }

    /// Stops playback.
    fn shutdown(&self) {
        self.cancel_waveform();
        self.player.unload();
    }

    fn install_bus_watch(self: &Rc<Self>) {
        let w: Weak<Ui> = Rc::downgrade(self);
        let guard = self.player.bus().add_watch_local(move |_, msg| {
            let Some(ui) = w.upgrade() else {
                return glib::ControlFlow::Break;
            };
            if let Some(ev) = ui.player.translate(msg) {
                ui.handle_event(ev);
            }
            glib::ControlFlow::Continue
        });
        match guard {
            Ok(g) => *self._bus_watch.borrow_mut() = Some(g),
            Err(e) => eprintln!("helix-audio-player: bus watch failed: {e}"),
        }
    }

    // ---- Opening -----------------------------------------------------------

    fn choose_file(self: &Rc<Self>) {
        let filters = gio::ListStore::new::<gtk::FileFilter>();
        let audio = gtk::FileFilter::new();
        audio.set_name(Some(tr("Audio Files")));
        audio.add_mime_type("audio/*");
        filters.append(&audio);
        let all = gtk::FileFilter::new();
        all.set_name(Some(tr("All Files")));
        all.add_pattern("*");
        filters.append(&all);
        let dialog = gtk::FileDialog::builder()
            .title(tr("Open Audio File"))
            .modal(true)
            .filters(&filters)
            .default_filter(&audio)
            .build();
        let w = Rc::downgrade(self);
        dialog.open(Some(&self.window), None::<&gio::Cancellable>, move |res| {
            if let (Some(ui), Ok(file)) = (w.upgrade(), res) {
                ui.open(&file.uri());
            }
        });
    }

    fn open(self: &Rc<Self>, uri: &str) {
        self.cancel_waveform();
        let generation = self.generation.get() + 1;
        self.generation.set(generation);
        {
            let mut st = self.state.borrow_mut();
            st.uri = Some(uri.to_string());
            st.info = TrackInfo::default();
            st.track_id += 1;
            st.duration = None;
            st.position = 0;
            st.playing = true;
            st.stopped = false;
            st.rate = 1.0;
            st.rate_pending = false;
            st.peaks = Rc::default();
            st.peaks_complete = false;
            st.drag = None;
        }
        if let Some(a) = self.action("rate") {
            a.set_state(&"1.00".to_variant());
        }
        self.rate_button.set_label(&seek::rate_label(1.0));
        helix_app_common::set_accessible_label(&self.rate_button, &seek::rate_name(1.0));
        self.stack.set_visible_child_name("player");
        self.update_info();
        self.focus_into_stack(&self.play);
        let title = self.title.label();
        helix_app_common::announce(
            &self.title,
            &trf!("Playing {title}", title = title),
            helix_app_common::Urgency::Polite,
        );
        self.player.load(uri);
        self.apply_volume();
        self.player.play();
        self.start_waveform(uri, generation);
        self.sync_controls();
        self.publish();
    }

    fn show_error(&self, message: &str) {
        self.cancel_waveform();
        self.generation.set(self.generation.get() + 1);
        self.player.unload();
        {
            let mut st = self.state.borrow_mut();
            st.uri = None;
            st.playing = false;
            st.duration = None;
            st.position = 0;
        }
        self.error_page
            .set_description(Some(&glib::markup_escape_text(message)));
        self.stack.set_visible_child_name("error");
        if let Some(button) = self.error_page.child() {
            self.focus_into_stack(&button);
        }
        helix_app_common::announce(
            &self.error_page,
            &trf!("Unable to play file. {message}", message = message),
            helix_app_common::Urgency::Assertive,
        );
        self.window.set_title(Some(tr(APP_NAME)));
        self.sync_controls();
        self.publish();
    }

    /// Moves keyboard focus to `target` when focus was on a page of the
    /// stack (which may just have been hidden) or nowhere.
    fn focus_into_stack(&self, target: &impl IsA<gtk::Widget>) {
        let focus = gtk::prelude::GtkWindowExt::focus(&self.window);
        if focus.is_none_or(|f| f.is_ancestor(&self.stack)) {
            target.grab_focus();
        }
    }

    fn update_info(&self) {
        let st = self.state.borrow();
        let Some(uri) = &st.uri else { return };
        let title = st.info.display_title(uri);
        self.title.set_label(&title);
        self.window.set_title(Some(&title));
        let sub = st.info.subtitle();
        self.subtitle.set_label(sub.as_deref().unwrap_or(""));
        self.subtitle.set_visible(sub.is_some());
        let texture = st
            .info
            .cover
            .as_ref()
            .and_then(|b| gdk::Texture::from_bytes(&glib::Bytes::from(b)).ok());
        match texture {
            Some(t) => {
                self.cover.set_paintable(Some(&t));
                self.cover.remove_css_class("dim-label");
            }
            None => {
                self.cover.set_icon_name(Some("audio-x-generic-symbolic"));
                self.cover.add_css_class("dim-label");
            }
        }
    }

    // ---- Waveform ----------------------------------------------------------

    fn cache_key(uri: &str) -> String {
        let meta = glib::filename_from_uri(uri)
            .ok()
            .and_then(|(p, _)| std::fs::metadata(p).ok());
        let mtime = meta
            .as_ref()
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        format!("{uri}\n{}\n{mtime}", meta.map(|m| m.len()).unwrap_or(0))
    }

    fn cancel_waveform(&self) {
        if let Some(c) = self.wave_cancel.borrow_mut().take() {
            c.store(true, Ordering::Relaxed);
        }
    }

    fn start_waveform(self: &Rc<Self>, uri: &str, generation: u64) {
        let key = Self::cache_key(uri);
        if let Some(peaks) = self.wave_cache.borrow().get(&key).cloned() {
            let mut st = self.state.borrow_mut();
            st.peaks = peaks;
            st.peaks_complete = true;
            drop(st);
            self.waveform.queue_draw();
            return;
        }
        let cancel = Arc::new(AtomicBool::new(false));
        *self.wave_cancel.borrow_mut() = Some(cancel.clone());
        let (tx, rx) = async_channel::unbounded::<WaveMsg>();
        let uri_owned = uri.to_string();
        std::thread::Builder::new()
            .name("waveform".into())
            .spawn(move || {
                let res = waveform::decode(&uri_owned, &cancel, |p| {
                    let _ = tx.send_blocking(WaveMsg::Partial(p.to_vec()));
                });
                match res {
                    Err(waveform::DecodeError::Cancelled) => {}
                    other => {
                        let _ = tx.send_blocking(WaveMsg::Done(other.map_err(|e| e.to_string())));
                    }
                }
            })
            .expect("spawn waveform thread");
        let w = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            while let Ok(msg) = rx.recv().await {
                let Some(ui) = w.upgrade() else { break };
                if ui.generation.get() != generation {
                    break;
                }
                let mut st = ui.state.borrow_mut();
                match msg {
                    WaveMsg::Partial(p) => st.peaks = Rc::new(p),
                    WaveMsg::Done(Ok(p)) => {
                        let p = Rc::new(p);
                        ui.wave_cache.borrow_mut().insert(key.clone(), p.clone());
                        st.peaks = p;
                        st.peaks_complete = true;
                    }
                    WaveMsg::Done(Err(e)) => {
                        eprintln!("helix-audio-player: waveform unavailable: {e}");
                        st.peaks_complete = true;
                    }
                }
                drop(st);
                ui.waveform.queue_draw();
            }
        });
    }

    fn draw_waveform(
        &self,
        area: &gtk::DrawingArea,
        cr: &gtk::cairo::Context,
        width: f64,
        height: f64,
    ) {
        let st = self.state.borrow();
        // The area carries the `accent` class, so its own colour is the
        // accent colour; bars that are not played yet use its parent's
        // (normal) foreground colour.
        let accent = area.color();
        let fg = area.parent().map_or(accent, |p| p.color());
        let unplayed = seek::unplayed_alpha(helix_app_common::style::is_high_contrast());
        let pos = st.drag.unwrap_or(st.position);
        let frac = seek::fraction(pos, st.duration);
        let mid = height / 2.0;
        let step = 3.0;
        let n = (width / step).floor().max(0.0) as usize;
        let total = if st.peaks_complete {
            st.peaks.len()
        } else {
            st.duration
                .map(|d| (d as f64 / 1e9 * PEAKS_PER_SECOND as f64).ceil() as usize)
                .unwrap_or(st.peaks.len())
        };
        let bins = peaks::bins(&st.peaks, total, n);
        let norm = peaks::max_peak(&st.peaks) as f64;
        let set = |c: &gdk::RGBA, alpha: f32| {
            cr.set_source_rgba(
                c.red() as f64,
                c.green() as f64,
                c.blue() as f64,
                (c.alpha() * alpha) as f64,
            );
        };
        // Baseline for silent / not yet decoded parts.
        set(&fg, 0.15);
        cr.rectangle(0.0, mid - 0.5, width, 1.0);
        let _ = cr.fill();
        let play_x = frac * width;
        for (i, bin) in bins.iter().enumerate() {
            let Some(v) = bin else { continue };
            let x = i as f64 * step;
            let half = ((*v as f64 / norm) * (mid - 1.0)).max(0.5);
            if x + 1.0 <= play_x {
                set(&accent, 1.0);
            } else {
                set(&fg, unplayed);
            }
            cr.rectangle(x, mid - half, 2.0, half * 2.0);
            let _ = cr.fill();
        }
        if st.duration.is_some() {
            set(&accent, 1.0);
            cr.rectangle(
                (play_x - 1.0).clamp(0.0, (width - 2.0).max(0.0)),
                0.0,
                2.0,
                height,
            );
            let _ = cr.fill();
        }
    }

    // ---- Playback ----------------------------------------------------------

    fn has_file(&self) -> bool {
        self.state.borrow().uri.is_some()
    }

    fn play(&self) {
        if !self.has_file() {
            return;
        }
        {
            let mut st = self.state.borrow_mut();
            st.playing = true;
            st.stopped = false;
        }
        self.player.play();
        self.sync_controls();
        self.publish();
    }

    fn pause(&self) {
        if !self.has_file() {
            return;
        }
        self.state.borrow_mut().playing = false;
        self.player.pause();
        self.sync_controls();
        self.publish();
    }

    fn toggle(&self) {
        if self.state.borrow().playing {
            self.pause();
        } else {
            self.play();
        }
    }

    fn stop(&self) {
        if !self.has_file() {
            return;
        }
        self.pause();
        self.seek_to(0);
        self.state.borrow_mut().stopped = true;
        self.publish();
    }

    fn seek_to(&self, position: u64) {
        if !self.has_file() {
            return;
        }
        let (rate, duration) = {
            let st = self.state.borrow();
            (st.rate, st.duration)
        };
        let position = duration.map_or(position, |d| position.min(d));
        self.player.seek(position, rate);
        // A seek racing an asynchronous state change (e.g. right after
        // end-of-stream) can leave the pipeline paused; re-assert it.
        if self.state.borrow().playing {
            self.player.play();
        }
        self.state.borrow_mut().position = position;
        self.refresh_position();
        self.publish();
        if let Some(m) = &self.mpris {
            m.seeked((position / 1000) as i64);
        }
    }

    fn skip(&self, delta: i64) {
        let (pos, dur) = {
            let st = self.state.borrow();
            (st.position, st.duration)
        };
        self.seek_to(seek::offset(pos, delta, dur));
    }

    fn scrub(&self, x: f64) {
        let Some(d) = self.state.borrow().duration else {
            return;
        };
        let target = seek::position_at(x, self.waveform.width() as f64, d);
        self.state.borrow_mut().drag = Some(target);
        let rate = self.state.borrow().rate;
        self.player.seek(target, rate);
        self.refresh_position();
    }

    fn set_rate(&self, rate: f64) {
        self.state.borrow_mut().rate = rate;
        self.rate_button.set_label(&seek::rate_label(rate));
        helix_app_common::set_accessible_label(&self.rate_button, &seek::rate_name(rate));
        if self.has_file() {
            let pos = self
                .player
                .position()
                .unwrap_or(self.state.borrow().position);
            let ok = self.player.seek(pos, rate);
            self.state.borrow_mut().rate_pending = !ok;
        }
        self.publish();
    }

    fn apply_volume(&self) {
        let (v, muted) = {
            let st = self.state.borrow();
            (st.volume, st.muted)
        };
        self.player.set_volume(seek::cubic_to_linear(v));
        self.player.set_mute(muted);
        self.volume_button
            .set_icon_name(seek::volume_icon(v, muted));
        self.volume_scale
            .update_property(&[gtk::accessible::Property::ValueText(&seek::volume_text(
                v, muted,
            ))]);
        self.publish();
    }

    fn handle_event(self: &Rc<Self>, ev: Event) {
        match ev {
            Event::Eos => {
                let repeat = self.state.borrow().repeat;
                if repeat {
                    self.seek_to(0);
                } else {
                    self.pause();
                    self.seek_to(0);
                    self.state.borrow_mut().stopped = true;
                    self.publish();
                }
            }
            Event::Error(msg) => {
                if self.has_file() {
                    self.show_error(&msg);
                }
            }
            Event::Tags(info) => {
                let changed = self.state.borrow_mut().info.merge(info);
                if changed {
                    self.update_info();
                    self.publish();
                }
            }
            Event::DurationChanged => self.update_duration(),
            Event::Prerolled(n_audio) => {
                if !self.has_file() {
                    return;
                }
                let info = self.player.stream_info();
                if self.state.borrow_mut().info.merge(info) {
                    self.update_info();
                    self.publish();
                }
                if n_audio == 0 {
                    self.show_error(tr("The file contains no audio stream"));
                    return;
                }
                self.update_duration();
                // Re-apply a rate chosen before the pipeline was ready.
                if self.state.borrow().rate_pending {
                    let (pos, rate) = {
                        let mut st = self.state.borrow_mut();
                        st.rate_pending = false;
                        (st.position, st.rate)
                    };
                    self.player.seek(pos, rate);
                }
            }
        }
    }

    fn update_duration(&self) {
        if let Some(d) = self.player.duration() {
            let changed = self.state.borrow().duration != Some(d);
            if changed {
                self.state.borrow_mut().duration = Some(d);
                self.refresh_position();
                self.publish();
            }
        }
    }

    fn handle_command(self: &Rc<Self>, cmd: Command) {
        match cmd {
            Command::Raise => self.window.present(),
            Command::Quit => self.app.quit(),
            Command::Play => self.play(),
            Command::Pause => self.pause(),
            Command::PlayPause => self.toggle(),
            Command::Stop => self.stop(),
            Command::Seek(us) => self.skip(us.saturating_mul(1000)),
            Command::SetPosition(us) => self.seek_to(us.max(0) as u64 * 1000),
            Command::OpenUri(uri) => self.open(&uri),
            Command::SetRate(r) => {
                if let Some(a) = self.action("rate") {
                    a.change_state(&format!("{r:.2}").to_variant());
                }
            }
            Command::SetVolume(linear) => {
                self.volume_scale.set_value(seek::linear_to_cubic(linear))
            }
            Command::SetLoop(on) => {
                if let Some(a) = self.action("repeat") {
                    a.change_state(&on.to_variant());
                }
            }
        }
    }

    // ---- Periodic refresh -------------------------------------------------

    fn tick(&self) {
        if !self.has_file() {
            return;
        }
        if self.state.borrow().duration.is_none() {
            self.update_duration();
        }
        if self.state.borrow().drag.is_none() {
            if let Some(p) = self.player.position() {
                let changed = self.state.borrow().position != p;
                if changed {
                    self.state.borrow_mut().position = p;
                    self.refresh_position();
                    self.publish();
                }
            }
        }
    }

    fn refresh_position(&self) {
        let st = self.state.borrow();
        let pos = st.drag.unwrap_or(st.position);
        let (e, r) = timefmt::elapsed_remaining(pos, st.duration);
        self.elapsed.set_label(&e);
        self.remaining.set_label(&r);
        let max = st.duration.unwrap_or(0) as f64 / 1e9;
        self.waveform.update_property(&[
            gtk::accessible::Property::ValueMin(0.0),
            gtk::accessible::Property::ValueMax(max),
            gtk::accessible::Property::ValueNow((pos as f64 / 1e9).min(max)),
            gtk::accessible::Property::ValueText(&trf!(
                "{position} of {duration}",
                position = e,
                duration = timefmt::format(st.duration.unwrap_or(0), timefmt::is_long(st.duration))
            )),
        ]);
        drop(st);
        self.waveform.queue_draw();
    }

    fn sync_controls(&self) {
        let (has, playing) = {
            let st = self.state.borrow();
            (st.uri.is_some(), st.playing)
        };
        for name in ["play-pause", "seek-back", "seek-forward"] {
            if let Some(a) = self.action(name) {
                a.set_enabled(has);
            }
        }
        let (icon, tip) = if playing {
            ("media-playback-pause-symbolic", tr("Pause"))
        } else {
            ("media-playback-start-symbolic", tr("Play"))
        };
        helix_app_common::set_icon_and_label(&self.play, icon, tip);
        if !has {
            self.refresh_position();
        }
    }

    fn snapshot(&self) -> Snapshot {
        let st = self.state.borrow();
        let status = match (&st.uri, st.playing, st.stopped) {
            (None, _, _) => Status::Stopped,
            (Some(_), true, _) => Status::Playing,
            (Some(_), false, true) => Status::Stopped,
            (Some(_), false, false) => Status::Paused,
        };
        Snapshot {
            status,
            loop_track: st.repeat,
            rate: st.rate,
            volume: seek::cubic_to_linear(st.volume),
            position_us: (st.position / 1000) as i64,
            track: st.uri.as_ref().map(|uri| Track {
                id: st.track_id,
                title: st.info.display_title(uri),
                artist: st.info.artist.clone(),
                album: st.info.album.clone(),
                url: uri.clone(),
                length_us: st.duration.map(|d| (d / 1000) as i64),
            }),
        }
    }

    /// Pushes the current state to the MPRIS thread when it changed.
    fn publish(&self) {
        let Some(m) = &self.mpris else { return };
        let snap = self.snapshot();
        let mut last = self.last_snapshot.borrow_mut();
        if last.as_ref() != Some(&snap) {
            m.update(snap.clone());
            *last = Some(snap);
        }
    }
}

#[cfg(test)]
mod shortcut_tests {
    use super::*;

    #[test]
    fn every_shortcut_is_listed() {
        helix_app_common::shortcuts::assert_all_listed(SHORTCUT_GROUPS, ACCELS);
        let listed = helix_app_common::shortcuts::listed_keys(SHORTCUT_GROUPS);
        for (key, action) in SINGLE_KEYS {
            assert!(listed.contains(key), "{action} {key} is not listed");
        }
    }
}
