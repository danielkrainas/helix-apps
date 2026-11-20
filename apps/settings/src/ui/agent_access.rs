//! Privacy & Security › Agent Access (helixdesktop/apps#192): the
//! `org.helixdesktop.agent-access` settings, the agents the shell knows
//! and their reminders (`org.helixdesktop.AgentAccess1`, which can only
//! take access away and cancel reminders) and the activity log.
//! Contract: helixdesktop/spec `contracts/agent-access/`.

use adw::prelude::*;
use gtk::{gio, glib};
use helix_app_common::{Labelled, Prefs};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use super::picker::{self, Picker};
use super::widgets::{self, has_key, switch, watch, Num};
use super::{follow, spawn_worker, Ctx};
use crate::agents::{self, Agent, Entry, Reminder};
use crate::bus;
use crate::values::Choice;
use helix_app_common::i18n::{n_, tr, tr_dyn};
use helix_app_common::{nc_, trf, trnf};

/// The page's navigation tag and command-line name.
pub const TAG: &str = "agent-access";
/// The View Activity page's tag and command-line name.
pub const ACTIVITY_TAG: &str = "agent-activity";
/// The activity subpage lists at most this many entries.
const LOG_ROWS: usize = 300;

/// The three levels of the Access Level combo, with what each adds.
const LEVEL_CHOICES: [(&str, &str); 3] = [
    (
        n_("Read Only"),
        n_("See windows, apps and recent files, and ask you questions"),
    ),
    (
        n_("Control the Desktop"),
        n_("Also open apps and files, manage windows and change quick settings"),
    ),
    (
        n_("Control Apps"),
        n_("Also read and use the buttons and fields inside apps"),
    ),
];

/// Whether the shell's settings are installed (an older helix-desktop
/// has none: the page is then not offered).
pub fn available() -> bool {
    helix_app_common::lookup_schema(agents::SCHEMA).is_some()
}

struct State {
    ctx: Rc<Ctx>,
    group: adw::PreferencesGroup,
    rows: RefCell<Vec<gtk::Widget>>,
    revoke_all: gtk::Button,
    /// What the rows show, so they are rebuilt only on a change.
    shown: RefCell<Option<bus::Result<Vec<Agent>>>>,
}

/// The Agent Access page, for pushing onto `nav`.
pub fn page(ctx: &Rc<Ctx>, nav: &adw::NavigationView) -> adw::NavigationPage {
    let prefs = ctx.prefs(agents::SCHEMA);
    let page = widgets::page();

    let main = widgets::group("");
    main.set_description(Some(tr(
        "Agents are programs, such as AI assistants, that read and control your session through the desktop. Each agent must ask you before it gets access.",
    )));
    let enabled = switch(&prefs, "enabled", false, false, tr("Agent Access"), "");
    main.add(&enabled);
    page.add(&main);

    // Groups that mean nothing while access is off.
    let mut dependent: Vec<adw::PreferencesGroup> = Vec::new();

    let access = widgets::group(tr("Access"));
    let (level, screen) = level_rows(ctx, &prefs, &access);
    access.add(&level);
    access.add(&screen);
    page.add(&access);
    dependent.push(access);

    let asking = widgets::group(tr("Asking"));
    for row in remember_rows(&prefs) {
        asking.add(&row);
    }
    page.add(&asking);
    dependent.push(asking);

    let state = agents_group(ctx);
    page.add(&state.group);
    let reminders = reminders_group(ctx);
    page.add(&reminders.group);

    let data = widgets::group(tr("Information"));
    data.add(&switch(
        &prefs,
        "notifications",
        false,
        false,
        tr("Notifications Access"),
        tr("Agents can read the text of notifications, which can include messages and sign-in codes"),
    ));
    data.add(&switch(
        &prefs,
        "clipboard",
        false,
        false,
        tr("Clipboard Access"),
        tr("Agents can read and change the clipboard, which can hold passwords and other copied text"),
    ));
    // A newer key: hidden while the installed schema lacks it.
    if has_key(&prefs, "read-text-fields") {
        data.add(&switch(
            &prefs,
            "read-text-fields",
            false,
            false,
            tr("Read Text Fields"),
            tr("Agents controlling apps can see what is typed in fields; password fields are never shown"),
        ));
    }
    page.add(&data);
    dependent.push(data);

    let safety = widgets::group(tr("Safeguards"));
    safety.add(&switch(
        &prefs,
        "confirm-sensitive",
        true,
        false,
        tr("Confirm Sensitive Actions"),
        tr("Ask before an agent deletes files, sends messages or changes network settings"),
    ));
    safety.add(&switch(
        &prefs,
        "allow-remote",
        false,
        false,
        tr("Allow From Remote Sessions"),
        tr("Agents running in a remote login (SSH) can ask for access"),
    ));
    page.add(&safety);
    dependent.push(safety);

    let excluded = excluded_group(ctx, &prefs);
    page.add(&excluded);
    dependent.push(excluded);

    let more = widgets::group("");
    let activity = widgets::subpage_row(
        tr("View Activity"),
        tr("What agents did, and how long it is kept"),
        "document-open-recent-symbolic",
    );
    let (weak_nav, weak_ctx) = (nav.downgrade(), Rc::downgrade(ctx));
    activity.connect_activated(move |_| {
        if let (Some(nav), Some(ctx)) = (weak_nav.upgrade(), weak_ctx.upgrade()) {
            nav.push(&activity_page(&ctx));
        }
    });
    more.add(&activity);
    page.add(&more);

    let sync = {
        let (enabled, dependent) = (enabled.downgrade(), dependent);
        move || {
            let Some(enabled) = enabled.upgrade() else {
                return;
            };
            for g in &dependent {
                g.set_sensitive(enabled.is_active());
            }
        }
    };
    sync();
    enabled.connect_active_notify(move |_| sync());

    page.connect_destroy(move |_| {
        let _ = (&state, &reminders);
    });
    widgets::nav_page(tr("Agent Access"), &page, Some(TAG))
}

/// The Access Level combo (first three levels) and the separate Screen
/// and Input switch, behind a warning, both over `access-level`.
/// What the chosen level allows is the group's description (a subtitle
/// would squeeze the combo's value).
fn level_rows(
    ctx: &Rc<Ctx>,
    prefs: &Prefs,
    group: &adw::PreferencesGroup,
) -> (adw::ComboRow, adw::SwitchRow) {
    let labels = gtk::StringList::new(&[]);
    for (label, _) in LEVEL_CHOICES {
        labels.append(&tr_dyn(label));
    }
    let combo = adw::ComboRow::builder()
        .use_markup(false)
        .title(tr("Access Level"))
        // The choice under the title: long level names fit.
        .use_subtitle(true)
        .model(&labels)
        .build();
    let screen = adw::SwitchRow::builder()
        .use_markup(false)
        .title(tr("Screen and Input"))
        .subtitle(tr(
            "Also take screenshots and use the pointer and keyboard as if they were you",
        ))
        .build();
    let updating = Rc::new(Cell::new(false));
    let load = {
        let (combo, screen, prefs, updating, group) = (
            combo.downgrade(),
            screen.downgrade(),
            prefs.clone(),
            updating.clone(),
            group.downgrade(),
        );
        move || {
            let (Some(combo), Some(screen), Some(group)) =
                (combo.upgrade(), screen.upgrade(), group.upgrade())
            else {
                return;
            };
            let (choice, on) =
                agents::level_choice(&prefs.get_enum_nick("access-level", "read-only"));
            updating.set(true);
            combo.set_selected(choice);
            group.set_description(Some(&tr_dyn(LEVEL_CHOICES[choice as usize].1)));
            // Screen and Input includes Control Apps.
            combo.set_sensitive(!on);
            screen.set_active(on);
            updating.set(false);
        }
    };
    load();
    {
        let (prefs, updating, screen) = (prefs.clone(), updating.clone(), screen.downgrade());
        combo.connect_selected_notify(move |c| {
            if updating.get() {
                return;
            }
            let on = screen.upgrade().is_some_and(|s| s.is_active());
            prefs.set_enum_nick("access-level", agents::level_value(c.selected(), on));
        });
    }
    {
        let (prefs, updating, load, ctx) = (
            prefs.clone(),
            updating.clone(),
            load.clone(),
            Rc::downgrade(ctx),
        );
        screen.connect_active_notify(move |s| {
            if updating.get() {
                return;
            }
            let stored = prefs.get_enum_nick("access-level", "read-only") == agents::SCREEN;
            if s.is_active() == stored {
                return;
            }
            if !s.is_active() {
                prefs.set_enum_nick("access-level", agents::level_value(2, false));
                return;
            }
            // Only the warning's Allow turns it on: the switch goes back
            // off (once GTK is done toggling it) until then.
            let load = load.clone();
            glib::idle_add_local_once(load);
            let Some(ctx) = ctx.upgrade() else { return };
            let dialog = adw::AlertDialog::new(
                Some(tr("Allow Screen and Input?")),
                Some(tr(
                    "Agents you allow will be able to see everything on the screen and type and click anywhere, as if they were you. Only turn this on for agents you trust.",
                )),
            );
            dialog.add_response("cancel", tr("_Cancel"));
            dialog.add_response("allow", tr("_Allow"));
            dialog.set_response_appearance("allow", adw::ResponseAppearance::Destructive);
            dialog.set_default_response(Some("cancel"));
            dialog.set_close_response("cancel");
            let prefs = prefs.clone();
            dialog.connect_response(Some("allow"), move |_, _| {
                prefs.set_enum_nick("access-level", agents::SCREEN);
            });
            dialog.present(Some(&ctx.window));
        });
    }
    watch(prefs, "access-level", load);
    (combo, screen)
}

/// "Ask Each Session" and "Remember" over `remember-mode`.
fn remember_rows(prefs: &Prefs) -> Vec<adw::ActionRow> {
    let choices = [
        (
            "ask",
            tr("Ask Each Session"),
            tr("Agents are asked for again in every session"),
        ),
        (
            "remember",
            tr("Remember"),
            tr("Agents you always allow are not asked again"),
        ),
    ];
    let mut rows = Vec::new();
    let mut checks: Vec<(&'static str, gtk::CheckButton)> = Vec::new();
    for (nick, title, subtitle) in choices {
        let check = gtk::CheckButton::builder()
            .valign(gtk::Align::Center)
            .build();
        if let Some((_, first)) = checks.first() {
            check.set_group(Some(first));
        }
        let row = adw::ActionRow::builder()
            .use_markup(false)
            .title(title)
            .subtitle(subtitle)
            .activatable_widget(&check)
            .build();
        row.add_prefix(&check);
        let p = prefs.clone();
        check.connect_toggled(move |c| {
            if c.is_active() && p.get_enum_nick("remember-mode", "ask") != nick {
                p.set_enum_nick("remember-mode", nick);
            }
        });
        checks.push((nick, check));
        rows.push(row);
    }
    let load = {
        let p = prefs.clone();
        move || {
            let mode = p.get_enum_nick("remember-mode", "ask");
            // An unknown value reads as "ask".
            let mode = if mode == "remember" {
                "remember"
            } else {
                "ask"
            };
            for (nick, check) in &checks {
                if *nick == mode && !check.is_active() {
                    check.set_active(true);
                }
            }
        }
    };
    load();
    watch(prefs, "remember-mode", load);
    rows
}

fn agents_group(ctx: &Rc<Ctx>) -> Rc<State> {
    let group = widgets::group(tr("Allowed Agents"));
    group.set_description(Some(tr(
        "While an agent is active, an indicator is always shown in the top bar, with Stop in its menu.",
    )));
    let revoke_all = gtk::Button::builder()
        .label(tr("Revoke All"))
        .css_classes(["flat"])
        .valign(gtk::Align::Center)
        .sensitive(false)
        .build();
    helix_app_common::set_accessible_label(&revoke_all, tr("Revoke Access of All Agents"));
    group.set_header_suffix(Some(&revoke_all));
    let state = Rc::new(State {
        ctx: ctx.clone(),
        group,
        rows: RefCell::default(),
        revoke_all: revoke_all.clone(),
        shown: RefCell::default(),
    });
    let weak = Rc::downgrade(&state);
    revoke_all.connect_clicked(move |_| {
        if let Some(state) = weak.upgrade() {
            act(&state, None, stop_all);
        }
    });
    load_agents(&state);
    follow(
        &state,
        Duration::from_millis(200),
        |tell| {
            if let Ok(conn) = bus::session() {
                agents::watch(&conn, tell);
            }
        },
        load_agents,
    );
    // "Last active" goes stale without a signal.
    let weak = Rc::downgrade(&state);
    glib::timeout_add_seconds_local(30, move || match weak.upgrade() {
        Some(state) => {
            if state.group.is_mapped() {
                load_agents(&state);
            }
            glib::ControlFlow::Continue
        }
        None => glib::ControlFlow::Break,
    });
    state
}

fn load_agents(state: &Rc<State>) {
    let weak = Rc::downgrade(state);
    spawn_worker(
        || bus::session().and_then(|c| agents::list(&c)),
        move |r| {
            if let Some(state) = weak.upgrade() {
                show_agents(&state, r, false);
            }
        },
    );
}

/// Revoke or Forget one agent (`Some(id)`), or Revoke All (`None`).
fn act(state: &Rc<State>, agent: Option<String>, call: Call) {
    let weak = Rc::downgrade(state);
    spawn_worker(
        move || bus::session().and_then(|c| call(&c, agent.as_deref())),
        move |r| {
            let Some(state) = weak.upgrade() else { return };
            if let Err(e) = r {
                state.ctx.toast(&match e {
                    bus::Error::Denied => {
                        tr("Only the installed Settings app can change agents' access").to_string()
                    }
                    e => trf!("Could not change the agent's access: {e}", e = e),
                });
            }
            load_agents(&state);
        },
    );
}

type Call = fn(&zbus::blocking::Connection, Option<&str>) -> bus::Result<()>;

fn stop_all(c: &zbus::blocking::Connection, _: Option<&str>) -> bus::Result<()> {
    agents::stop_all(c)
}

fn revoke_one(c: &zbus::blocking::Connection, agent: Option<&str>) -> bus::Result<()> {
    agents::revoke(c, agent.unwrap_or_default())
}

fn forget_one(c: &zbus::blocking::Connection, agent: Option<&str>) -> bus::Result<()> {
    agents::forget(c, agent.unwrap_or_default())
}

/// "Last active 5 minutes ago" for a row.
fn last_active(then: i64) -> String {
    let now = glib::DateTime::now_utc().map(|d| d.to_unix()).unwrap_or(0);
    match agents::ago(now, then) {
        None => tr("Not active this session").to_string(),
        Some((_, 's')) => tr("Active just now").to_string(),
        Some((n, 'm')) => trnf!("Active {n} minute ago", "Active {n} minutes ago", n),
        Some((n, 'h')) => trnf!("Active {n} hour ago", "Active {n} hours ago", n),
        Some((n, _)) => trnf!("Active {n} day ago", "Active {n} days ago", n),
    }
}

/// The second line of an agent's row: level, grant and activity.
fn agent_status(a: &Agent) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(level) = agents::level_label(&a.level) {
        parts.push(tr_dyn(level).to_string());
    }
    if a.denied {
        parts.push(tr("Denied this session").to_string());
    } else if a.always {
        parts.push(tr("Always allowed").to_string());
    } else if a.session {
        parts.push(tr("Allowed this session").to_string());
    }
    if !a.denied {
        parts.push(last_active(a.last_active));
    }
    parts.join(" · ")
}

fn show_agents(state: &Rc<State>, r: bus::Result<Vec<Agent>>, force: bool) {
    if !force && state.shown.borrow().as_ref() == Some(&r) {
        // Only "last active" may have changed.
        for (row, a) in state.rows.borrow().iter().zip(r.iter().flatten()) {
            if let Some(row) = row.downcast_ref::<adw::ActionRow>() {
                row.set_subtitle(&subtitle(a));
            }
        }
        return;
    }
    let new: Vec<gtk::Widget> = match &r {
        Err(e) => vec![placeholder(match e {
            bus::Error::Missing => {
                tr("The desktop shell is not running, so agents cannot be listed")
            }
            bus::Error::Denied => tr("Only the installed Settings app can list agents"),
            bus::Error::Other(_) => tr("Agents could not be listed"),
        })],
        Ok(list) if list.is_empty() => vec![placeholder(tr(
            "Agents that ask for access are listed here",
        ))],
        Ok(list) => list.iter().map(|a| agent_row(state, a).upcast()).collect(),
    };
    state
        .revoke_all
        .set_sensitive(r.as_ref().is_ok_and(|l| l.iter().any(Agent::can_revoke)));
    let group = state.group.clone();
    helix_app_common::focus::rebuild_keeping_focus(
        &group,
        Some(state.revoke_all.upcast_ref()),
        || widgets::replace_rows(&state.group, &state.rows, new),
    );
    *state.shown.borrow_mut() = Some(r);
}

fn placeholder(text: &str) -> gtk::Widget {
    adw::ActionRow::builder()
        .use_markup(false)
        .title(text)
        .css_classes(["dim-label"])
        .build()
        .upcast()
}

/// Program on the first line when a display name is the title (a name
/// proves nothing), then the status.
fn subtitle(a: &Agent) -> String {
    let status = agent_status(a);
    if a.name.is_empty() {
        status
    } else {
        format!("{}\n{status}", a.program())
    }
}

fn agent_row(state: &Rc<State>, a: &Agent) -> adw::ActionRow {
    let row = adw::ActionRow::builder()
        .use_markup(false)
        .title(a.title())
        .subtitle(subtitle(a))
        .subtitle_lines(0)
        .title_lines(1)
        .build();
    row.add_prefix(
        &gtk::Image::builder()
            .icon_name("system-run-symbolic")
            .accessible_role(gtk::AccessibleRole::Presentation)
            .build(),
    );
    helix_app_common::focus::set_key(&row, &a.id);
    let buttons: [(&str, String, bool, Call); 2] = [
        (
            tr("Revoke"),
            trf!("Revoke Access of {agent}", agent = a.title()),
            a.can_revoke(),
            revoke_one,
        ),
        (
            tr("Forget"),
            trf!("Forget {agent}", agent = a.title()),
            a.can_forget(),
            forget_one,
        ),
    ];
    for (label, name, show, call) in buttons {
        if !show {
            continue;
        }
        let b = gtk::Button::builder()
            .label(label)
            .valign(gtk::Align::Center)
            .build()
            .labelled(&name);
        helix_app_common::focus::set_part(&b, label);
        let (weak, id) = (Rc::downgrade(state), a.id.clone());
        b.connect_clicked(move |_| {
            if let Some(state) = weak.upgrade() {
                act(&state, Some(id.clone()), call);
            }
        });
        row.add_suffix(&b);
    }
    row
}

struct Reminders {
    ctx: Rc<Ctx>,
    group: adw::PreferencesGroup,
    rows: RefCell<Vec<gtk::Widget>>,
    /// What the rows show, so they are rebuilt only on a change.
    shown: RefCell<Option<bus::Result<Vec<Reminder>>>>,
    twelve: bool,
}

/// Every agent's reminders not yet shown (`ListReminders`), each with a
/// Cancel button (`CancelReminder`). Read when the page is shown, on
/// `RemindersChanged` and the shell coming or going, and after a cancel.
/// Hidden when the shell does not answer (not running, or too old to
/// have reminders).
fn reminders_group(ctx: &Rc<Ctx>) -> Rc<Reminders> {
    let group = widgets::group(tr("Reminders"));
    group.set_description(Some(tr(
        "Reminders agents have set for you. They are shown as notifications when they are due.",
    )));
    group.set_visible(false);
    let twelve = ctx
        .prefs("org.gnome.desktop.interface")
        .get_enum_nick("clock-format", "24h")
        == "12h";
    let state = Rc::new(Reminders {
        ctx: ctx.clone(),
        group,
        rows: RefCell::default(),
        shown: RefCell::default(),
        twelve,
    });
    load_reminders(&state);
    // Back from another page, or another panel.
    let weak = Rc::downgrade(&state);
    state.group.connect_map(move |_| {
        if let Some(state) = weak.upgrade() {
            load_reminders(&state);
        }
    });
    follow(
        &state,
        Duration::from_millis(200),
        |tell| {
            if let Ok(conn) = bus::session() {
                agents::watch_reminders(&conn, tell);
            }
        },
        load_reminders,
    );
    state
}

fn load_reminders(state: &Rc<Reminders>) {
    let weak = Rc::downgrade(state);
    spawn_worker(
        || bus::session().and_then(|c| agents::reminders(&c)),
        move |r| {
            if let Some(state) = weak.upgrade() {
                show_reminders(&state, r);
            }
        },
    );
}

fn show_reminders(state: &Rc<Reminders>, r: bus::Result<Vec<Reminder>>) {
    if state.shown.borrow().as_ref() == Some(&r) {
        return;
    }
    // Hidden without a shell that answers; the shell coming back is
    // followed.
    state.group.set_visible(r.is_ok());
    let new: Vec<gtk::Widget> = match &r {
        Ok(list) if list.is_empty() => vec![placeholder(tr("No reminders"))],
        Ok(list) => list
            .iter()
            .map(|x| reminder_row(state, x).upcast())
            .collect(),
        Err(_) => Vec::new(),
    };
    let group = state.group.clone();
    helix_app_common::focus::rebuild_keeping_focus(&group, None, || {
        widgets::replace_rows(&state.group, &state.rows, new)
    });
    *state.shown.borrow_mut() = Some(r);
}

fn reminder_row(state: &Rc<Reminders>, r: &Reminder) -> adw::ActionRow {
    let who = r.who();
    let text = if r.text.is_empty() {
        tr(nc_!("agent reminder without text", "Reminder")).to_string()
    } else {
        r.text.clone()
    };
    let mut parts: Vec<String> = Vec::new();
    if let Some(due) = glib::DateTime::from_unix_local(r.time)
        .ok()
        .filter(|_| r.time > 0)
    {
        let when = super::datetime::format_time(&due, state.twelve);
        parts.push(trf!("Due {time}", time = when.replace(' ', "\u{a0}")));
    }
    if !who.title().is_empty() {
        parts.push(who.title().to_string());
    }
    let row = adw::ActionRow::builder()
        .use_markup(false)
        .title(&text)
        .subtitle(parts.join(" · "))
        .subtitle_lines(0)
        .build();
    // A display name proves nothing: the program on hover.
    if !who.name.is_empty() && !who.program().is_empty() {
        row.set_tooltip_text(Some(who.program()));
    }
    row.add_prefix(
        &gtk::Image::builder()
            .icon_name("alarm-symbolic")
            .accessible_role(gtk::AccessibleRole::Presentation)
            .build(),
    );
    helix_app_common::focus::set_key(&row, &r.id);
    let cancel = gtk::Button::builder()
        .label(tr(nc_!("cancel an agent's reminder", "Cancel")))
        .valign(gtk::Align::Center)
        .build()
        .labelled(&trf!("Cancel Reminder “{text}”", text = text));
    helix_app_common::focus::set_part(&cancel, "cancel");
    let (weak, id) = (Rc::downgrade(state), r.id.clone());
    cancel.connect_clicked(move |b| {
        let Some(state) = weak.upgrade() else { return };
        b.set_sensitive(false);
        let (weak, id) = (Rc::downgrade(&state), id.clone());
        spawn_worker(
            move || bus::session().and_then(|c| agents::cancel_reminder(&c, &id)),
            move |r| {
                let Some(state) = weak.upgrade() else { return };
                if let Err(e) = r {
                    state.ctx.toast(&match e {
                        bus::Error::Denied => {
                            tr("Only the installed Settings app can cancel reminders").to_string()
                        }
                        e => trf!("Could not cancel the reminder: {e}", e = e),
                    });
                    // The row comes back usable.
                    *state.shown.borrow_mut() = None;
                }
                load_reminders(&state);
            },
        );
    });
    row.add_suffix(&cancel);
    row
}

/// An app's name and icon, from its desktop file when installed.
fn app_info(id: &str) -> Option<gio::DesktopAppInfo> {
    gio::DesktopAppInfo::new(&format!("{id}.desktop"))
}

fn excluded_group(ctx: &Rc<Ctx>, prefs: &Prefs) -> adw::PreferencesGroup {
    let group = widgets::group(tr("Excluded Apps"));
    group.set_description(Some(tr(
        "Agents can never read or control these apps. Password fields, the lock screen, system password prompts and Settings are always excluded.",
    )));
    let add = gtk::Button::builder()
        .label(tr("Add App…"))
        .css_classes(["flat"])
        .valign(gtk::Align::Center)
        .build();
    group.set_header_suffix(Some(&add));
    let rows: Rc<RefCell<Vec<gtk::Widget>>> = Rc::default();
    let rebuild: Rc<dyn Fn()> = {
        let (group_weak, prefs, rows, add) = (
            group.downgrade(),
            prefs.clone(),
            rows.clone(),
            add.downgrade(),
        );
        Rc::new(move || {
            let (Some(group), Some(add)) = (group_weak.upgrade(), add.upgrade()) else {
                return;
            };
            let list = prefs.get::<Vec<String>>("excluded-apps", Vec::new());
            let new: Vec<gtk::Widget> = if list.is_empty() {
                vec![placeholder(tr("No apps excluded"))]
            } else {
                list.iter()
                    .map(|id| excluded_row(&prefs, id).upcast())
                    .collect()
            };
            helix_app_common::focus::rebuild_keeping_focus(&group, Some(add.upcast_ref()), || {
                widgets::replace_rows(&group, &rows, new)
            });
        })
    };
    rebuild();
    let r = rebuild.clone();
    watch(prefs, "excluded-apps", move || r());
    let (ctx, prefs) = (Rc::downgrade(ctx), prefs.clone());
    add.connect_clicked(move |_| {
        if let Some(ctx) = ctx.upgrade() {
            add_app_dialog(&ctx, &prefs, rebuild.clone());
        }
    });
    group
}

fn excluded_row(prefs: &Prefs, id: &str) -> adw::ActionRow {
    let info = app_info(id);
    let name = info
        .as_ref()
        .map(|i| i.display_name().to_string())
        .unwrap_or_else(|| id.to_string());
    let row = adw::ActionRow::builder()
        .use_markup(false)
        .title(&name)
        .build();
    if info.is_some() {
        row.set_subtitle(id);
    }
    let icon = gtk::Image::builder()
        .pixel_size(32)
        .accessible_role(gtk::AccessibleRole::Presentation)
        .build();
    match info.and_then(|i| i.icon()) {
        Some(gicon) => icon.set_from_gicon(&gicon),
        None => icon.set_icon_name(Some("application-x-executable")),
    }
    row.add_prefix(&icon);
    helix_app_common::focus::set_key(&row, id);
    let rm = gtk::Button::builder()
        .icon_name("user-trash-symbolic")
        .tooltip_text(tr("Remove"))
        .valign(gtk::Align::Center)
        .css_classes(["flat"])
        .build()
        .labelled(&trf!("Remove {app}", app = name));
    helix_app_common::focus::set_part(&rm, "remove");
    let (p, id) = (prefs.clone(), id.to_string());
    rm.connect_clicked(move |_| {
        let list = p.get::<Vec<String>>("excluded-apps", Vec::new());
        p.set("excluded-apps", agents::without_excluded(&list, &id));
    });
    row.add_suffix(&rm);
    row
}

fn add_app_dialog(ctx: &Rc<Ctx>, prefs: &Prefs, rebuild: Rc<dyn Fn()>) {
    let picker = Picker::new(
        tr("Exclude an App"),
        tr("Search apps"),
        tr("Search Apps"),
        tr("Loading Apps…"),
    );
    let excluded = prefs.get::<Vec<String>>("excluded-apps", Vec::new());
    // (id without .desktop, name), sorted by name; excluded apps left out.
    let mut apps: Vec<(String, String)> = gio::AppInfo::all()
        .into_iter()
        .filter(|a| a.should_show())
        .filter_map(|a| {
            let id = a.id()?.to_string();
            let id = id.strip_suffix(".desktop").unwrap_or(&id).to_string();
            // Settings is always excluded.
            if id == super::APP_ID {
                return None;
            }
            agents::with_excluded(&excluded, &id).map(|_| (id, a.display_name().to_string()))
        })
        .collect();
    apps.sort_by_key(|(_, name)| name.to_lowercase());
    let fill = {
        let picker = Rc::downgrade(&picker);
        move |q: &str| {
            let Some(picker) = picker.upgrade() else {
                return;
            };
            let q = q.trim().to_lowercase();
            let matches: Vec<picker::Choice> = apps
                .iter()
                .filter(|(id, name)| {
                    q.is_empty()
                        || name.to_lowercase().contains(&q)
                        || id.to_lowercase().contains(&q)
                })
                .map(|(id, name)| (id.clone(), name.clone(), id.clone()))
                .collect();
            let total = matches.len();
            let shown: Vec<picker::Choice> = matches.into_iter().take(picker::MAX_ROWS).collect();
            picker.set_choices(&shown, total);
        }
    };
    fill("");
    picker
        .search
        .connect_search_changed(move |e| fill(&e.text()));
    let p = prefs.clone();
    picker.connect_pick(move |id| {
        let list = p.get::<Vec<String>>("excluded-apps", Vec::new());
        if let Some(list) = agents::with_excluded(&list, id) {
            p.set("excluded-apps", list);
            // Memory-only keys (no schema watch) still show it.
            rebuild();
        }
    });
    picker.present(&ctx.window);
}

/// View Activity: retention, Clear, and what agents did.
pub fn activity_page(ctx: &Rc<Ctx>) -> adw::NavigationPage {
    let prefs = ctx.prefs(agents::SCHEMA);
    let page = widgets::page();

    let keep = widgets::group("");
    keep.add(&widgets::preset_combo(
        &prefs,
        "log-retention-days",
        Num::U32,
        30,
        retention_choices,
        widgets::days_label,
        tr("Keep Activity For"),
    ));
    let (clear_row, clear) = widgets::action_row(
        tr("Clear Activity"),
        tr("Remove everything agents have done from the log"),
        tr("Clear…"),
        Some("destructive-action"),
    );
    keep.add(&clear_row);
    page.add(&keep);

    let log = widgets::group(tr("Recent Activity"));
    page.add(&log);
    let rows: Rc<RefCell<Vec<gtk::Widget>>> = Rc::default();
    let twelve = ctx
        .prefs("org.gnome.desktop.interface")
        .get_enum_nick("clock-format", "24h")
        == "12h";
    let load: Rc<dyn Fn()> = {
        let (log, rows) = (log.downgrade(), rows.clone());
        Rc::new(move || {
            let (log, rows) = (log.clone(), rows.clone());
            spawn_worker(
                || agents::read_log(&log_dir(), LOG_ROWS),
                move |(entries, more)| {
                    let Some(log) = log.upgrade() else { return };
                    show_log(&log, &rows, &entries, more, twelve);
                },
            );
        })
    };
    load();

    let (ctx_weak, load2) = (Rc::downgrade(ctx), load.clone());
    clear.connect_clicked(move |_| {
        let Some(ctx) = ctx_weak.upgrade() else {
            return;
        };
        let dialog = adw::AlertDialog::new(
            Some(tr("Clear Agent Activity?")),
            Some(tr(
                "The record of what agents did is removed. This cannot be undone.",
            )),
        );
        dialog.add_response("cancel", tr("_Cancel"));
        dialog.add_response("clear", tr("_Clear"));
        dialog.set_response_appearance("clear", adw::ResponseAppearance::Destructive);
        dialog.set_default_response(Some("cancel"));
        dialog.set_close_response("cancel");
        let (c, load) = (Rc::downgrade(&ctx), load2.clone());
        dialog.connect_response(Some("clear"), move |_, _| {
            let (c, load) = (c.clone(), load.clone());
            spawn_worker(
                || bus::session().and_then(|c| agents::clear_log(&c)),
                move |r| {
                    let Some(ctx) = c.upgrade() else { return };
                    match r {
                        Ok(()) => ctx.toast(tr("Agent activity cleared")),
                        Err(bus::Error::Missing) => ctx.toast(tr(
                            "The desktop shell is not running, so the activity cannot be cleared",
                        )),
                        Err(bus::Error::Denied) => {
                            ctx.toast(tr("Only the installed Settings app can clear the activity"))
                        }
                        Err(e) => ctx.toast(&trf!("Could not clear the activity: {e}", e = e)),
                    }
                    load();
                },
            );
        });
        dialog.present(Some(&ctx.window));
    });

    widgets::nav_page(tr("Agent Activity"), &page, Some(ACTIVITY_TAG))
}

/// `$XDG_STATE_HOME/helix/agent-activity`, where the shell keeps the log.
fn log_dir() -> std::path::PathBuf {
    let state = std::env::var_os("XDG_STATE_HOME")
        .filter(|v| !v.is_empty())
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| glib::home_dir().join(".local/state"));
    state.join("helix/agent-activity")
}

/// Retention presets in days (the schema allows 1–365).
fn retention_choices() -> Vec<Choice<i64>> {
    [1, 7, 30, 90, 365]
        .into_iter()
        .map(|d| Choice {
            value: d,
            label: widgets::days_label(d),
        })
        .collect()
}

fn show_log(
    group: &adw::PreferencesGroup,
    rows: &RefCell<Vec<gtk::Widget>>,
    entries: &[Entry],
    more: bool,
    twelve: bool,
) {
    group.set_description(
        more.then(|| {
            trnf!(
                "Showing the latest {n} entry.",
                "Showing the latest {n} entries.",
                entries.len()
            )
        })
        .as_deref(),
    );
    let new: Vec<gtk::Widget> = if entries.is_empty() {
        vec![placeholder(tr("What agents do will be listed here"))]
    } else {
        entries
            .iter()
            .map(|e| log_row(e, twelve).upcast())
            .collect()
    };
    widgets::replace_rows(group, rows, new);
}

fn log_row(e: &Entry, twelve: bool) -> adw::ActionRow {
    let d = agents::describe(e);
    let mut parts: Vec<String> = Vec::new();
    if !d.who.is_empty() {
        parts.push(d.who);
    }
    parts.extend(d.status);
    // The time wraps as a whole, not in the middle of the date.
    parts.push(log_time(e, twelve).replace(' ', "\u{a0}"));
    let what: Vec<String> = std::iter::once(d.target)
        .filter(|t| !t.is_empty())
        .chain(d.details)
        .collect();
    adw::ActionRow::builder()
        .use_markup(true)
        .title(glib::markup_escape_text(&d.title))
        .title_lines(2)
        .subtitle(log_subtitle(&what.join(" · "), &parts.join(" · ")))
        .subtitle_lines(0)
        .build()
}

/// A log row's subtitle as markup: what was acted on (when anything),
/// then who · how it went · when, a little apart so the two do not read
/// as one wrapped line.
fn log_subtitle(what: &str, rest: &str) -> String {
    let rest = glib::markup_escape_text(rest);
    if what.is_empty() {
        return rest.to_string();
    }
    format!(
        "{}\n<span size=\"35%\"> </span>\n{rest}",
        glib::markup_escape_text(what)
    )
}

/// When a line's call came: "19 November 2026, 14:02", and for coalesced
/// refusals until when ("…, 14:02–14:03", the date again on another day).
fn log_time(e: &Entry, twelve: bool) -> String {
    let at = |t: i64| glib::DateTime::from_unix_local(t).ok();
    let Some(start) = at(e.time) else {
        return String::new();
    };
    let first = super::datetime::format_time(&start, twelve);
    let Some(end) = e
        .coalesced()
        .map(|(_, until)| until)
        .filter(|&u| u / 60 != e.time / 60)
        .and_then(at)
    else {
        return first;
    };
    let last = if end.ymd() == start.ymd() {
        end.format(if twelve { "%l:%M %p" } else { "%H:%M" })
            .map(|s| s.trim().to_string())
            .unwrap_or_default()
    } else {
        super::datetime::format_time(&end, twelve)
    };
    format!("{first}–{last}")
}
