//! Sound panel: outputs, inputs, apps and alert sounds, through the sound
//! server's PulseAudio interface.

use adw::prelude::*;
use gtk::glib;
use helix_app_common::Prefs;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::{Duration, Instant};

use super::widgets::{self, switch, watch};
use super::{follow, spawn_worker, Ctx, Panel};
use crate::sound::{self, Device, EventVolume, Server, Sound};
use helix_app_common::i18n::tr;
use helix_app_common::trf;

pub struct SoundPanel;

const SOUND: &str = "org.gnome.desktop.sound";
/// After the user moves a slider, readings leave it alone this long.
const HOLD: Duration = Duration::from_millis(800);

type Job = Box<dyn FnOnce() -> sound::Result<()> + Send>;

/// Runs one change at a time; while one runs, only the newest waiting
/// one is kept (a dragged slider sends many).
#[derive(Default)]
struct Throttle {
    running: Cell<bool>,
    next: RefCell<Option<Job>>,
}

impl Throttle {
    fn run(self: &Rc<Self>, job: impl FnOnce() -> sound::Result<()> + Send + 'static) {
        if self.running.get() {
            *self.next.borrow_mut() = Some(Box::new(job));
            return;
        }
        self.running.set(true);
        let this = self.clone();
        spawn_worker(job, move |_| {
            this.running.set(false);
            if let Some(next) = this.next.borrow_mut().take() {
                this.run(next);
            }
        });
    }
}

/// The controls of one direction (output or input).
struct Side {
    group: adw::PreferencesGroup,
    device: adw::ComboRow,
    volume: gtk::Scale,
    mute: gtk::ToggleButton,
    profile: adw::ComboRow,
    /// The device's ports (speakers or headphones…), when it has several.
    port: adw::ComboRow,
    /// Device and port names, as the port row lists them.
    ports: RefCell<(String, Vec<String>)>,
    /// Device names, as the device row lists them.
    names: RefCell<Vec<String>>,
    /// Card name and profile names, as the profile row lists them.
    profiles: RefCell<(String, Vec<String>)>,
}

struct State {
    ctx: Rc<Ctx>,
    server: Server,
    stack: gtk::Stack,
    out: Side,
    inp: Side,
    balance_row: adw::ActionRow,
    balance: gtk::Scale,
    fade_row: adw::ActionRow,
    fade: gtk::Scale,
    level: gtk::LevelBar,
    apps: adw::PreferencesGroup,
    app_rows: RefCell<Vec<(u64, gtk::Scale, gtk::ToggleButton, gtk::Widget)>>,
    /// The system sounds volume row; hidden when the sound server keeps
    /// no stream-restore entries.
    events_row: adw::ActionRow,
    events: gtk::Scale,
    event_volume: Cell<Option<EventVolume>>,
    sound: RefCell<Sound>,
    quiet: Cell<bool>,
    held: Cell<Option<Instant>>,
    throttle: Rc<Throttle>,
    meter: RefCell<Option<sound::Meter>>,
    metered: RefCell<String>,
    open: Cell<bool>,
    prefs: Prefs,
}

impl State {
    fn hold(&self) {
        self.held.set(Some(Instant::now()));
    }

    fn holding(&self) -> bool {
        self.held.get().is_some_and(|t| t.elapsed() < HOLD)
    }

    fn max_volume(&self) -> f64 {
        if self
            .prefs
            .get::<bool>("allow-volume-above-100-percent", false)
        {
            sound::MAX_AMPLIFIED
        } else {
            1.0
        }
    }
}

fn mute_button(name: &str) -> gtk::ToggleButton {
    let b = gtk::ToggleButton::builder()
        .icon_name("audio-volume-high-symbolic")
        .valign(gtk::Align::Center)
        .css_classes(["flat"])
        .tooltip_text(tr("Mute"))
        .build();
    helix_app_common::set_accessible_label(&b, name);
    b.connect_active_notify(|b| {
        b.set_icon_name(if b.is_active() {
            "audio-volume-muted-symbolic"
        } else {
            "audio-volume-high-symbolic"
        });
    });
    b
}

fn volume_scale(name: &str) -> gtk::Scale {
    let s = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, 1.0, 0.01);
    s.set_hexpand(true);
    s.set_valign(gtk::Align::Center);
    s.set_draw_value(false);
    s.set_width_request(150);
    s.set_increments(0.05, 0.1);
    helix_app_common::set_accessible_label(&s, name);
    s
}

fn side(output: bool) -> Side {
    let group = widgets::group(if output { tr("Output") } else { tr("Input") });
    let device = adw::ComboRow::builder()
        .use_markup(false)
        .title(if output {
            tr("Output Device")
        } else {
            tr("Input Device")
        })
        .build();
    group.add(&device);
    let port = adw::ComboRow::builder()
        .use_markup(false)
        .title(if output {
            tr("Output Port")
        } else {
            tr("Input Port")
        })
        .visible(false)
        .build();
    group.add(&port);
    let row = adw::ActionRow::builder()
        .use_markup(false)
        .title(tr("Volume"))
        .build();
    let mute = mute_button(if output {
        tr("Mute Output")
    } else {
        tr("Mute Input")
    });
    let volume = volume_scale(if output {
        tr("Output Volume")
    } else {
        tr("Input Volume")
    });
    row.add_prefix(&mute);
    row.add_suffix(&volume);
    group.add(&row);
    let profile = adw::ComboRow::builder()
        .use_markup(false)
        .title(tr("Configuration"))
        .visible(false)
        .build();
    helix_app_common::set_accessible_label(
        &profile,
        if output {
            tr("Output Configuration")
        } else {
            tr("Input Configuration")
        },
    );
    Side {
        group,
        device,
        volume,
        mute,
        profile,
        port,
        ports: RefCell::default(),
        names: RefCell::default(),
        profiles: RefCell::default(),
    }
}

impl Panel for SoundPanel {
    fn build(&self, ctx: &Rc<Ctx>) -> gtk::Widget {
        let prefs = ctx.prefs(SOUND);
        let stack = gtk::Stack::new();
        stack.add_named(&widgets::loading(tr("Loading Sound…")), Some("loading"));
        stack.add_named(
            &widgets::status(
                "audio-volume-muted-symbolic",
                tr("Sound Is Not Available"),
                tr("No sound server is running."),
            ),
            Some("missing"),
        );
        let page = widgets::page();

        let out = side(true);
        let balance_row = adw::ActionRow::builder()
            .use_markup(false)
            .title(tr("Balance"))
            .build();
        let balance = gtk::Scale::with_range(gtk::Orientation::Horizontal, -1.0, 1.0, 0.05);
        balance.set_hexpand(true);
        balance.set_valign(gtk::Align::Center);
        balance.set_draw_value(false);
        balance.set_width_request(150);
        balance.add_mark(-1.0, gtk::PositionType::Bottom, Some(tr("Left")));
        balance.add_mark(0.0, gtk::PositionType::Bottom, None);
        balance.add_mark(1.0, gtk::PositionType::Bottom, Some(tr("Right")));
        helix_app_common::set_accessible_label(&balance, tr("Balance"));
        balance_row.add_suffix(&balance);
        out.group.add(&balance_row);
        let fade_row = adw::ActionRow::builder()
            .use_markup(false)
            .title(tr("Fade"))
            .visible(false)
            .build();
        let fade = gtk::Scale::with_range(gtk::Orientation::Horizontal, -1.0, 1.0, 0.05);
        fade.set_hexpand(true);
        fade.set_valign(gtk::Align::Center);
        fade.set_draw_value(false);
        fade.set_width_request(150);
        fade.add_mark(-1.0, gtk::PositionType::Bottom, Some(tr("Rear")));
        fade.add_mark(0.0, gtk::PositionType::Bottom, None);
        fade.add_mark(1.0, gtk::PositionType::Bottom, Some(tr("Front")));
        helix_app_common::set_accessible_label(&fade, tr("Fade"));
        fade_row.add_suffix(&fade);
        out.group.add(&fade_row);
        out.group.add(&out.profile);
        let (test_row, test) = widgets::action_row(
            tr("Test Speakers"),
            tr("Play a sound on each speaker"),
            tr("Test…"),
            None,
        );
        out.group.add(&test_row);
        page.add(&out.group);

        let inp = side(false);
        let level_row = adw::ActionRow::builder()
            .use_markup(false)
            .title(tr("Input Level"))
            .build();
        let level = gtk::LevelBar::builder()
            .min_value(0.0)
            .max_value(1.0)
            .hexpand(true)
            .valign(gtk::Align::Center)
            .width_request(150)
            .build();
        // The meter moves all the time: no offset colours, nothing spoken.
        level.remove_offset_value(Some(gtk::LEVEL_BAR_OFFSET_LOW));
        level.remove_offset_value(Some(gtk::LEVEL_BAR_OFFSET_HIGH));
        level.remove_offset_value(Some(gtk::LEVEL_BAR_OFFSET_FULL));
        helix_app_common::set_accessible_label(&level, tr("Input Level"));
        level_row.add_suffix(&level);
        inp.group.add(&level_row);
        inp.group.add(&inp.profile);
        page.add(&inp.group);

        let apps = widgets::group(tr("Apps"));
        page.add(&apps);

        let sounds = widgets::group(tr("Sounds"));
        sounds.add(&switch(
            &prefs,
            "allow-volume-above-100-percent",
            false,
            false,
            tr("Over-Amplification"),
            tr("Lets the volume go above 100 %, which can distort the sound"),
        ));
        sounds.add(&switch(
            &prefs,
            "event-sounds",
            true,
            false,
            tr("System Sounds"),
            tr("Sounds for events such as a message arriving"),
        ));
        let events_row = adw::ActionRow::builder()
            .use_markup(false)
            .title(tr("System Sounds Volume"))
            .visible(false)
            .build();
        let events = volume_scale(tr("System Sounds Volume"));
        events_row.add_suffix(&events);
        sounds.add(&events_row);
        sounds.add(&alert_row(ctx, &prefs));
        page.add(&sounds);
        stack.add_named(&page, Some("page"));
        stack.set_visible_child_name("loading");

        let state = Rc::new(State {
            ctx: ctx.clone(),
            server: Server::default(),
            stack: stack.clone(),
            out,
            inp,
            balance_row,
            balance,
            fade_row,
            fade,
            level,
            apps,
            app_rows: RefCell::default(),
            events_row,
            events,
            event_volume: Cell::default(),
            sound: RefCell::default(),
            quiet: Cell::new(false),
            held: Cell::new(None),
            throttle: Rc::default(),
            meter: RefCell::default(),
            metered: RefCell::default(),
            open: Cell::new(false),
            prefs: prefs.clone(),
        });
        connect_side(&state, true);
        connect_side(&state, false);
        let weak = Rc::downgrade(&state);
        state.balance.connect_value_changed(move |_| {
            if let Some(state) = weak.upgrade() {
                if !state.quiet.get() {
                    send_volume(&state, true);
                }
            }
        });
        let weak = Rc::downgrade(&state);
        state.fade.connect_value_changed(move |_| {
            if let Some(state) = weak.upgrade() {
                if !state.quiet.get() {
                    send_volume(&state, true);
                }
            }
        });
        let weak = Rc::downgrade(&state);
        state.events.connect_value_changed(move |sc| {
            if let Some(state) = weak.upgrade() {
                if !state.quiet.get() {
                    send_event_volume(&state, sc.value());
                }
            }
        });
        // System sounds are off: their volume does nothing.
        let row = state.events_row.clone();
        let events_on = move |p: &Prefs| row.set_sensitive(p.get::<bool>("event-sounds", true));
        events_on(&prefs);
        let p = prefs.clone();
        watch(&prefs, "event-sounds", move || events_on(&p));
        let weak = Rc::downgrade(&state);
        test.connect_clicked(move |_| {
            if let Some(state) = weak.upgrade() {
                speaker_test(&state);
            }
        });
        let weak = Rc::downgrade(&state);
        watch(&prefs, "allow-volume-above-100-percent", move || {
            if let Some(state) = weak.upgrade() {
                let s = state.sound.borrow().clone();
                show(&state, Ok(s));
            }
        });

        // The input level only while the page is on screen.
        let weak = Rc::downgrade(&state);
        stack.connect_map(move |_| {
            if let Some(state) = weak.upgrade() {
                state.open.set(true);
                run_meter(&state);
            }
        });
        let weak = Rc::downgrade(&state);
        stack.connect_unmap(move |_| {
            if let Some(state) = weak.upgrade() {
                state.open.set(false);
                run_meter(&state);
            }
        });

        refresh(&state);
        let server = state.server.clone();
        follow(
            &state,
            Duration::from_millis(150),
            move |tell| sound::watch(&server, tell),
            refresh,
        );
        refresh_events(&state);
        let server = state.server.clone();
        follow(
            &state,
            Duration::from_millis(150),
            move |tell| sound::watch_event_volume(&server, tell),
            refresh_events,
        );
        stack.connect_destroy(move |_| {
            let _ = &state;
        });
        stack.upcast()
    }
}

fn side_of(state: &State, output: bool) -> &Side {
    if output {
        &state.out
    } else {
        &state.inp
    }
}

fn connect_side(state: &Rc<State>, output: bool) {
    let s = side_of(state, output);
    let weak = Rc::downgrade(state);
    s.device.connect_selected_notify(move |row| {
        let Some(state) = weak.upgrade() else { return };
        if state.quiet.get() {
            return;
        }
        let name = side_of(&state, output)
            .names
            .borrow()
            .get(row.selected() as usize)
            .cloned();
        if let Some(name) = name {
            let (server, weak) = (state.server.clone(), Rc::downgrade(&state));
            spawn_worker(
                move || sound::set_default(&server, output, &name),
                move |r| done(&weak, r),
            );
        }
    });
    let weak = Rc::downgrade(state);
    s.volume.connect_value_changed(move |_| {
        if let Some(state) = weak.upgrade() {
            if !state.quiet.get() {
                send_volume(&state, output);
            }
        }
    });
    let weak = Rc::downgrade(state);
    s.mute.connect_active_notify(move |b| {
        let Some(state) = weak.upgrade() else { return };
        if state.quiet.get() {
            return;
        }
        let device = current(&state, output);
        let Some(d) = device else { return };
        let (server, weak, muted) = (state.server.clone(), Rc::downgrade(&state), b.is_active());
        spawn_worker(
            move || sound::set_mute(&server, output, &d.name, muted),
            move |r| done(&weak, r),
        );
    });
    let weak = Rc::downgrade(state);
    s.port.connect_selected_notify(move |row| {
        let Some(state) = weak.upgrade() else { return };
        if state.quiet.get() {
            return;
        }
        let (device, names) = side_of(&state, output).ports.borrow().clone();
        if let Some(p) = names.get(row.selected() as usize).cloned() {
            let (server, weak) = (state.server.clone(), Rc::downgrade(&state));
            spawn_worker(
                move || sound::set_port(&server, output, &device, &p),
                move |r| done(&weak, r),
            );
        }
    });
    let weak = Rc::downgrade(state);
    s.profile.connect_selected_notify(move |row| {
        let Some(state) = weak.upgrade() else { return };
        if state.quiet.get() {
            return;
        }
        let (card, names) = side_of(&state, output).profiles.borrow().clone();
        if let Some(p) = names.get(row.selected() as usize).cloned() {
            let (server, weak) = (state.server.clone(), Rc::downgrade(&state));
            spawn_worker(
                move || sound::set_profile(&server, &card, &p),
                move |r| done(&weak, r),
            );
        }
    });
}

fn current(state: &State, output: bool) -> Option<Device> {
    let s = state.sound.borrow();
    if output {
        s.output().cloned()
    } else {
        s.input().cloned()
    }
}

fn send_volume(state: &Rc<State>, output: bool) {
    let Some(d) = current(state, output) else {
        return;
    };
    state.hold();
    let level = side_of(state, output).volume.value();
    let volumes = if output {
        let fade = if d.fade().is_some() {
            state.fade.value()
        } else {
            0.0
        };
        d.volumes(level, state.balance.value(), fade)
    } else {
        d.volumes(level, 0.0, 0.0)
    };
    let server = state.server.clone();
    state
        .throttle
        .run(move || sound::set_volume(&server, output, &d.name, &volumes));
}

fn refresh(state: &Rc<State>) {
    let (weak, server) = (Rc::downgrade(state), state.server.clone());
    spawn_worker(
        move || sound::read(&server),
        move |r| {
            if let Some(state) = weak.upgrade() {
                show(&state, r);
            }
        },
    );
}

/// Sets the system sounds volume, and that of the event sounds playing
/// now (the stored one only reaches sounds to come).
fn send_event_volume(state: &Rc<State>, level: f64) {
    let Some(current) = state.event_volume.get() else {
        return;
    };
    state.hold();
    let volume = EventVolume { level, ..current };
    state.event_volume.set(Some(volume));
    let playing: Vec<u64> = state
        .sound
        .borrow()
        .apps
        .iter()
        .filter(|a| a.role == "event")
        .map(|a| a.index)
        .collect();
    let server = state.server.clone();
    state.throttle.run(move || {
        sound::set_event_volume(&server, &volume)?;
        for i in playing {
            // It may have ended meanwhile.
            let _ = sound::set_app_volume(&server, i, volume.level);
        }
        Ok(())
    });
}

fn refresh_events(state: &Rc<State>) {
    let (weak, server) = (Rc::downgrade(state), state.server.clone());
    spawn_worker(
        move || sound::read_event_volume(&server),
        move |r| {
            if let Some(state) = weak.upgrade() {
                show_events(&state, r.ok().flatten());
            }
        },
    );
}

fn show_events(state: &Rc<State>, volume: Option<EventVolume>) {
    state.events_row.set_visible(volume.is_some());
    if let Some(v) = &volume {
        state.quiet.set(true);
        state.events.set_range(0.0, v.level.max(1.0));
        if !state.holding() {
            state.events.set_value(v.level);
        }
        state.quiet.set(false);
        state
            .events
            .update_property(&[gtk::accessible::Property::ValueText(&format!(
                "{:.0} %",
                v.level * 100.0
            ))]);
    }
    if !state.holding() || volume.is_none() {
        state.event_volume.set(volume);
    }
}

fn done(weak: &std::rc::Weak<State>, r: sound::Result<()>) {
    let Some(state) = weak.upgrade() else { return };
    if let Err(e) = r {
        state
            .ctx
            .toast(&trf!("Could not change the sound settings: {e}", e = e));
    }
    refresh(&state);
}

fn string_list(items: &[String]) -> gtk::StringList {
    gtk::StringList::new(&items.iter().map(String::as_str).collect::<Vec<_>>())
}

fn show_side(state: &State, s: &Sound, output: bool) {
    let side = side_of(state, output);
    let devices = if output { &s.outputs } else { &s.inputs };
    let current = if output { s.output() } else { s.input() };
    side.group.set_visible(true);
    let names: Vec<String> = devices.iter().map(|d| d.name.clone()).collect();
    if *side.names.borrow() != names {
        let labels: Vec<String> = devices.iter().map(|d| d.description.clone()).collect();
        side.device.set_model(Some(&string_list(&labels)));
        *side.names.borrow_mut() = names.clone();
    }
    side.device.set_sensitive(devices.len() > 1);
    let Some(d) = current else {
        side.device.set_subtitle(if output {
            tr("No output devices")
        } else {
            tr("No input devices")
        });
        side.volume.set_sensitive(false);
        side.mute.set_sensitive(false);
        side.profile.set_visible(false);
        side.port.set_visible(false);
        return;
    };
    side.device.set_subtitle("");
    side.volume.set_sensitive(true);
    side.mute.set_sensitive(true);
    if let Some(i) = names.iter().position(|n| *n == d.name) {
        side.device.set_selected(i as u32);
    }
    let max = if output { state.max_volume() } else { 1.0 };
    side.volume.set_range(0.0, max.max(d.level()));
    side.volume.clear_marks();
    if max > 1.0 {
        side.volume
            .add_mark(1.0, gtk::PositionType::Bottom, Some("100 %"));
    }
    if !state.holding() {
        side.volume.set_value(d.level());
    }
    side.volume
        .update_property(&[gtk::accessible::Property::ValueText(&format!(
            "{:.0} %",
            d.level() * 100.0
        ))]);
    side.mute.set_active(d.muted);

    // Its ports (speakers or headphones…): the plugged-in ones, and the
    // one in use.
    let ports: Vec<&sound::Port> = d
        .ports
        .iter()
        .filter(|p| p.available || p.name == d.active_port)
        .collect();
    if ports.len() > 1 {
        let names: Vec<String> = ports.iter().map(|p| p.name.clone()).collect();
        let key = (d.name.clone(), names.clone());
        if *side.ports.borrow() != key {
            let labels: Vec<String> = ports.iter().map(|p| p.description.clone()).collect();
            side.port.set_model(Some(&string_list(&labels)));
            *side.ports.borrow_mut() = key;
        }
        if let Some(i) = names.iter().position(|n| *n == d.active_port) {
            side.port.set_selected(i as u32);
        }
        side.port.set_visible(true);
    } else {
        side.port.set_visible(false);
    }

    // The card's profiles (HDMI or analogue, A2DP or HFP…).
    match s
        .card(d.card)
        .filter(|c| c.profiles.iter().filter(|p| p.available).count() > 1)
    {
        Some(card) => {
            let available: Vec<&sound::Profile> =
                card.profiles.iter().filter(|p| p.available).collect();
            let names: Vec<String> = available.iter().map(|p| p.name.clone()).collect();
            let key = (card.name.clone(), names.clone());
            if *side.profiles.borrow() != key {
                let labels: Vec<String> = available.iter().map(|p| p.description.clone()).collect();
                side.profile.set_model(Some(&string_list(&labels)));
                *side.profiles.borrow_mut() = key;
            }
            if let Some(i) = names.iter().position(|n| *n == card.active) {
                side.profile.set_selected(i as u32);
            }
            side.profile.set_visible(true);
        }
        None => side.profile.set_visible(false),
    }
}

fn show(state: &Rc<State>, r: sound::Result<Sound>) {
    let s = match r {
        Ok(s) => s,
        Err(e) => {
            if e != sound::Error::Missing {
                eprintln!("helix-settings-app: sound: {e}");
            }
            state.stack.set_visible_child_name("missing");
            return;
        }
    };
    state.stack.set_visible_child_name("page");
    state.quiet.set(true);
    show_side(state, &s, true);
    show_side(state, &s, false);
    let balance = s.output().and_then(Device::balance);
    state.balance_row.set_visible(balance.is_some());
    if let (Some(b), false) = (balance, state.holding()) {
        state.balance.set_value(b);
    }
    let fade = s.output().and_then(Device::fade);
    state.fade_row.set_visible(fade.is_some());
    if let (Some(f), false) = (fade, state.holding()) {
        state.fade.set_value(f);
    }
    show_apps(state, &s);
    state.quiet.set(false);
    *state.sound.borrow_mut() = s;
    run_meter(state);
}

fn show_apps(state: &Rc<State>, s: &Sound) {
    // Event sounds, speaker tests and meters are not apps.
    let apps: Vec<&sound::App> = s
        .apps
        .iter()
        .filter(|a| !matches!(a.role.as_str(), "event" | "test" | "filter"))
        .collect();
    let indexes: Vec<u64> = apps.iter().map(|a| a.index).collect();
    let same = state
        .app_rows
        .borrow()
        .iter()
        .map(|r| r.0)
        .collect::<Vec<_>>()
        == indexes;
    if !same {
        for (_, _, _, w) in state.app_rows.borrow_mut().drain(..) {
            state.apps.remove(&w);
        }
        for a in &apps {
            let name = if a.name.is_empty() {
                tr("Unknown App").to_string()
            } else {
                a.name.clone()
            };
            let row = adw::ActionRow::builder()
                .use_markup(false)
                .title(&name)
                .subtitle(&a.media)
                .build();
            let mute = mute_button(&trf!("Mute {app}", app = name));
            let scale = volume_scale(&trf!("Volume of {app}", app = name));
            row.add_prefix(&mute);
            row.add_suffix(&scale);
            let (weak, index) = (Rc::downgrade(state), a.index);
            scale.connect_value_changed(move |sc| {
                let Some(state) = weak.upgrade() else { return };
                if state.quiet.get() {
                    return;
                }
                state.hold();
                let (server, v) = (state.server.clone(), sc.value());
                state
                    .throttle
                    .run(move || sound::set_app_volume(&server, index, v));
            });
            let weak = Rc::downgrade(state);
            mute.connect_active_notify(move |b| {
                let Some(state) = weak.upgrade() else { return };
                if state.quiet.get() {
                    return;
                }
                let (server, weak, m) =
                    (state.server.clone(), Rc::downgrade(&state), b.is_active());
                spawn_worker(
                    move || sound::set_app_mute(&server, index, m),
                    move |r| done(&weak, r),
                );
            });
            state.apps.add(&row);
            state
                .app_rows
                .borrow_mut()
                .push((a.index, scale, mute, row.upcast()));
        }
    }
    let max = state.max_volume();
    for ((_, scale, mute, _), a) in state.app_rows.borrow().iter().zip(&apps) {
        scale.set_range(0.0, max.max(a.volume));
        if !state.holding() {
            scale.set_value(a.volume);
        }
        mute.set_active(a.muted);
    }
    state.apps.set_description(if apps.is_empty() {
        Some(tr("No apps are playing sound."))
    } else {
        None
    });
}

/// Runs the input level meter for the current input while the page is
/// on screen.
fn run_meter(state: &Rc<State>) {
    let source = if state.open.get() {
        state.sound.borrow().input().map(|d| d.name.clone())
    } else {
        None
    };
    let source = source.unwrap_or_default();
    if *state.metered.borrow() == source {
        return;
    }
    state.meter.borrow_mut().take();
    state.level.set_value(0.0);
    *state.metered.borrow_mut() = source.clone();
    if source.is_empty() {
        return;
    }
    let (tx, rx) = async_channel::bounded::<f32>(4);
    *state.meter.borrow_mut() = sound::meter(&state.server, &source, move |l| {
        // A full channel only means the page is busy; skip a reading.
        !matches!(tx.try_send(l), Err(async_channel::TrySendError::Closed(_)))
    });
    let level = state.level.downgrade();
    glib::spawn_future_local(async move {
        while let Ok(l) = rx.recv().await {
            let Some(level) = level.upgrade() else { break };
            // Fall back slowly, rise at once.
            level.set_value((level.value() * 0.7).max(l as f64));
        }
    });
}

fn speaker_test(state: &Rc<State>) {
    let Some(d) = current(state, true) else {
        return;
    };
    let page = widgets::page();
    let group = widgets::group("");
    group.set_description(Some(tr("Select a speaker to play a sound on it.")));
    let flow = gtk::FlowBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .max_children_per_line(3)
        .column_spacing(12)
        .row_spacing(12)
        .homogeneous(true)
        .build();
    for ch in &d.channels {
        let b = gtk::Button::builder()
            .label(sound::channel_label(ch))
            .build();
        let (server, sink, ch, ctx) = (
            state.server.clone(),
            d.name.clone(),
            ch.clone(),
            Rc::downgrade(&state.ctx),
        );
        b.connect_clicked(move |b| {
            b.set_sensitive(false);
            let (server, sink, ch, ctx, b) = (
                server.clone(),
                sink.clone(),
                ch.clone(),
                ctx.clone(),
                b.downgrade(),
            );
            spawn_worker(
                move || sound::test_speaker(&server, &sink, &ch),
                move |r| {
                    if let Some(b) = b.upgrade() {
                        b.set_sensitive(true);
                    }
                    if let (Err(e), Some(ctx)) = (r, ctx.upgrade()) {
                        ctx.toast(&trf!("Could not play the test sound: {e}", e = e));
                    }
                },
            );
        });
        flow.append(&b);
    }
    group.add(&flow);
    page.add(&group);
    let dialog = widgets::dialog(&trf!("Test {device}", device = d.description), &page);
    dialog.present(Some(&state.ctx.window));
}

/// The alert sound: the theme's own bell or one of the alert sounds.
fn alert_row(ctx: &Rc<Ctx>, prefs: &Prefs) -> adw::ComboRow {
    let data_dirs: Vec<std::path::PathBuf> = glib::system_data_dirs();
    let choices = sound::alert_choices(&data_dirs);
    let mut labels = vec![tr("Default").to_string()];
    labels.extend(choices.iter().map(|(n, _)| n.clone()));
    let row = adw::ComboRow::builder()
        .use_markup(false)
        .title(tr("Alert Sound"))
        .model(&string_list(&labels))
        .build();
    let data_home = glib::user_data_dir();
    let theme = prefs.get::<String>("theme-name", "freedesktop".into());
    let current = sound::current_alert(&data_home, &theme);
    let index = current
        .and_then(|c| choices.iter().position(|(_, p)| *p == c))
        .map_or(0, |i| i + 1);
    row.set_selected(index as u32);
    let (p, ctx) = (prefs.clone(), Rc::downgrade(ctx));
    row.connect_selected_notify(move |r| {
        let i = r.selected() as usize;
        let file = i
            .checked_sub(1)
            .and_then(|i| choices.get(i))
            .map(|(_, f)| f.clone());
        let theme = p.get::<String>("theme-name", "freedesktop".into());
        let previous = if theme == sound::CUSTOM_THEME {
            sound::inherited_theme(&data_home)
        } else {
            theme
        };
        match sound::set_alert(&data_home, file.as_deref(), &previous) {
            Ok(name) => {
                p.set("theme-name", name);
                if let Some(f) = file {
                    // Let the user hear it.
                    let _ = std::process::Command::new("paplay")
                        .arg(&f)
                        .stdout(std::process::Stdio::null())
                        .stderr(std::process::Stdio::null())
                        .spawn()
                        .map(|mut c| std::thread::spawn(move || c.wait()));
                }
            }
            Err(e) => {
                if let Some(ctx) = ctx.upgrade() {
                    ctx.toast(&trf!("Could not change the alert sound: {e}", e = e));
                }
            }
        }
    });
    row
}
