//! Login Screen panel: the Helix greeter's settings for everyone
//! (`/etc/helix/greeter.toml`, see [`crate::greeter`]). Shown only when
//! helix-greeter is the display manager's greeter (`ui::Window`).
//!
//! Edits are staged: the controls show what is wanted, and while that
//! differs from the file an action bar offers *Apply…* (one
//! `helix-login-config` run through `pkexec`, so one administrator prompt
//! for the batch) and *Discard*. Afterwards the file is read again.

use adw::prelude::*;
use gtk::gio;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

use super::appearance::{flow_append, install_css, wrapping_box};
use super::{displays, spawn_worker, widgets, Ctx, Panel};
use crate::greeter::{self, BannerError, Changes, Config, FileState, BANNER_MAX};
use crate::login_screen::{system_path, Helper, Outcome};
use crate::values::ACCENT_COLORS;
use helix_app_common::i18n::{n_, tr};
use helix_app_common::{trf, trnf};

pub struct LoginScreenPanel;

/// The clock format choices: automatic (no key), then the formats.
const CLOCKS: [(Option<&str>, &str); 3] = [
    (None, n_("Automatic")),
    (Some("24h"), n_("24-hour")),
    (Some("12h"), n_("12-hour")),
];

struct State {
    ctx: Rc<Ctx>,
    /// The file as last read, and how.
    current: RefCell<Config>,
    file: Cell<FileState>,
    helper: Option<Helper>,
    /// While the controls are set from the model (no staging then).
    syncing: Cell<bool>,
    /// While the helper runs.
    busy: Cell<bool>,
    notice: adw::Banner,
    user_list: adw::SwitchRow,
    banner_group: adw::PreferencesGroup,
    banner_on: adw::SwitchRow,
    banner_text: adw::EntryRow,
    accents: Vec<(gtk::ToggleButton, &'static str)>,
    clock: adw::ComboRow,
    bar: gtk::ActionBar,
    pending: gtk::Label,
    apply: gtk::Button,
    discard: gtk::Button,
    groups: Vec<adw::PreferencesGroup>,
}

impl Panel for LoginScreenPanel {
    fn build(&self, ctx: &Rc<Ctx>) -> gtk::Widget {
        install_css();
        let page = widgets::page();
        page.set_description(tr(
            "How the login screen looks for everyone on this computer. Changes need an administrator and apply the next time the login screen starts.",
        ));

        let users = widgets::group(tr("Users"));
        let user_list = adw::SwitchRow::builder()
            .use_markup(false)
            .title(tr("Show User List"))
            .subtitle(tr("When off, the login screen asks for a username"))
            .build();
        users.add(&user_list);
        page.add(&users);

        let banner_group = widgets::group(tr("Banner"));
        let banner_on = adw::SwitchRow::builder()
            .use_markup(false)
            .title(tr("Show a Banner"))
            .subtitle(tr(
                "A message under the clock, such as “Authorized use only.”",
            ))
            .build();
        let banner_text = adw::EntryRow::builder()
            .use_markup(false)
            .title(tr("Banner Message"))
            .build();
        banner_group.add(&banner_on);
        banner_group.add(&banner_text);
        page.add(&banner_group);

        let accent_group = widgets::group(tr("Accent Color"));
        let bx = wrapping_box(ACCENT_COLORS.len() as u32, 10);
        bx.set_margin_top(6);
        bx.set_margin_bottom(6);
        let mut accents: Vec<(gtk::ToggleButton, &'static str)> = Vec::new();
        for (nick, label, _) in ACCENT_COLORS {
            let b = gtk::ToggleButton::builder()
                .css_classes(["accent-swatch", &format!("accent-{nick}")])
                .tooltip_text(tr(label))
                .valign(gtk::Align::Center)
                .halign(gtk::Align::Center)
                .build();
            helix_app_common::set_accessible_label(&b, tr(label));
            if let Some((first, _)) = accents.first() {
                b.set_group(Some(first));
            }
            flow_append(&bx, &b);
            accents.push((b, nick));
        }
        accent_group.add(&bx);
        page.add(&accent_group);

        let clock_group = widgets::group(tr("Clock"));
        let labels: Vec<&str> = CLOCKS.iter().map(|(_, l)| tr(l)).collect();
        let clock = adw::ComboRow::builder()
            .use_markup(false)
            .title(tr("Clock Format"))
            .subtitle(tr("Automatic follows the system’s setting or the language"))
            .model(&gtk::StringList::new(&labels))
            .build();
        clock_group.add(&clock);
        page.add(&clock_group);

        // The monitor layout has its own helper and applies at once.
        if let Some(monitors) = Helper::system() {
            let group = widgets::group(tr("Displays"));
            group.add(&displays::login_layout_row(ctx, monitors));
            page.add(&group);
        }

        let notice = adw::Banner::builder().revealed(false).build();
        let pending = gtk::Label::builder()
            .label(tr("Changes not applied yet"))
            .wrap(true)
            .xalign(0.0)
            .build();
        let apply = gtk::Button::builder()
            .label(tr("_Apply…"))
            .use_underline(true)
            .css_classes(["suggested-action"])
            .build();
        helix_app_common::set_accessible_label(&apply, tr("Apply Login Screen Changes"));
        apply.set_tooltip_text(Some(tr("Needs an administrator")));
        let discard = gtk::Button::builder()
            .label(tr("_Discard"))
            .use_underline(true)
            .build();
        helix_app_common::set_accessible_label(&discard, tr("Discard Login Screen Changes"));
        let bar = gtk::ActionBar::builder().revealed(false).build();
        bar.pack_start(&pending);
        bar.pack_end(&apply);
        bar.pack_end(&discard);

        let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
        page.set_vexpand(true);
        content.append(&notice);
        content.append(&page);
        content.append(&bar);

        let state = Rc::new(State {
            ctx: ctx.clone(),
            current: RefCell::default(),
            file: Cell::new(FileState::Missing),
            helper: greeter::helper(),
            syncing: Cell::new(false),
            busy: Cell::new(false),
            notice,
            user_list,
            banner_group,
            banner_on,
            banner_text,
            accents,
            clock,
            bar,
            pending,
            apply,
            discard,
            groups: vec![users, accent_group, clock_group],
        });
        connect(&state);
        reload(&state, true);
        watch_file(&state, &content);
        // The state lives as long as the panel.
        content.connect_destroy(move |_| {
            let _ = &state;
        });
        content.upcast()
    }
}

/// What the controls say is wanted.
fn wanted(s: &State) -> Config {
    let text = s.banner_text.text().to_string();
    Config {
        user_list: Some(s.user_list.is_active()),
        banner: s.banner_on.is_active().then_some(text),
        accent: s
            .accents
            .iter()
            .find(|(b, _)| b.is_active())
            .map(|(_, n)| (*n).to_owned()),
        clock_format: CLOCKS
            .get(s.clock.selected() as usize)
            .and_then(|(f, _)| *f)
            .map(str::to_owned),
    }
}

/// The controls set to `c`.
fn show(s: &State, c: &Config) {
    s.syncing.set(true);
    s.user_list.set_active(c.user_list());
    let banner = c.banner();
    s.banner_on.set_active(banner.is_some());
    s.banner_text.set_text(banner.unwrap_or(""));
    if let Some((b, _)) = s.accents.iter().find(|(_, n)| *n == c.accent()) {
        b.set_active(true);
    }
    let i = CLOCKS
        .iter()
        .position(|(f, _)| *f == c.clock_format())
        .unwrap_or(0);
    s.clock.set_selected(i as u32);
    s.syncing.set(false);
    update(s);
}

/// Reads the file again; with `replace` (or nothing staged) the controls
/// show it.
fn reload(s: &Rc<State>, replace: bool) {
    let staged = !greeter::changes(&s.current.borrow(), &wanted(s)).is_empty();
    let (config, file) = Config::read_system();
    *s.current.borrow_mut() = config.clone();
    s.file.set(file);
    if replace || !staged {
        show(s, &config);
    } else {
        update(s);
    }
}

/// A file monitor: a change made elsewhere is shown (staged edits are
/// kept, compared with the new file).
fn watch_file(s: &Rc<State>, owner: &gtk::Box) {
    let file = gio::File::for_path(system_path(greeter::CONFIG));
    let Ok(monitor) = file.monitor_file(gio::FileMonitorFlags::WATCH_MOVES, gio::Cancellable::NONE)
    else {
        return;
    };
    let weak = Rc::downgrade(s);
    monitor.connect_changed(move |_, _, _, event| {
        if matches!(
            event,
            gio::FileMonitorEvent::ChangesDoneHint
                | gio::FileMonitorEvent::Deleted
                | gio::FileMonitorEvent::Created
                | gio::FileMonitorEvent::MovedIn
                | gio::FileMonitorEvent::Renamed
        ) {
            if let Some(s) = weak.upgrade() {
                if !s.busy.get() {
                    reload(&s, false);
                }
            }
        }
    });
    // Lives as long as the panel.
    owner.connect_destroy(move |_| {
        monitor.cancel();
    });
}

/// The banner's problem, in words.
fn banner_problem(e: BannerError) -> String {
    match e {
        BannerError::TooLong(n) => trnf!(
            "The banner is too long: {n} characters, at most {max}",
            "The banner is too long: {n} characters, at most {max}",
            n,
            max = BANNER_MAX
        ),
        BannerError::Control => {
            tr("The banner can’t contain line breaks, tabs or other control characters").to_string()
        }
        BannerError::Bidi => tr("The banner can’t contain text direction controls").to_string(),
    }
}

/// The action bar, the banner's problem and what can be changed, from the
/// controls and the file.
fn update(s: &State) {
    let changes = greeter::changes(&s.current.borrow(), &wanted(s));
    let usable = s.helper.is_some() && s.file.get() != FileState::Broken;
    match (s.helper.is_some(), s.file.get()) {
        (false, _) => {
            s.notice.set_title(tr(
                "These settings can’t be changed here: helix-login-config is not installed",
            ));
            s.notice.set_revealed(true);
        }
        (true, FileState::Broken) => {
            s.notice.set_title(&trf!(
                "The login screen’s settings file can’t be read. Fix {path} to change them here.",
                path = greeter::CONFIG
            ));
            s.notice.set_revealed(true);
        }
        _ => s.notice.set_revealed(false),
    }
    for g in &s.groups {
        g.set_sensitive(usable);
    }
    s.banner_group.set_sensitive(usable);
    s.banner_text.set_sensitive(s.banner_on.is_active());
    // Only a banner being sent is checked: one already in the file is
    // the administrator's, and other changes can still be applied.
    let problem = changes.check().err();
    match problem {
        Some(e) => {
            let text = banner_problem(e);
            s.banner_group.set_description(Some(&text));
            s.banner_text.add_css_class("error");
            if !s.syncing.get() {
                helix_app_common::announce_settled(&s.banner_text, &text);
            }
        }
        None => {
            s.banner_group.set_description(None);
            s.banner_text.remove_css_class("error");
        }
    }
    let busy = s.busy.get();
    s.bar.set_revealed(usable && (!changes.is_empty() || busy));
    s.apply
        .set_sensitive(!busy && problem.is_none() && !changes.is_empty());
    s.discard.set_sensitive(!busy);
    s.pending.set_label(if busy {
        tr("Waiting for an administrator…")
    } else if problem.is_some() {
        tr("Fix the banner to apply the changes")
    } else {
        tr("Changes not applied yet")
    });
}

fn connect(s: &Rc<State>) {
    let changed = {
        let weak = Rc::downgrade(s);
        move || {
            if let Some(s) = weak.upgrade() {
                if !s.syncing.get() {
                    update(&s);
                }
            }
        }
    };
    let c = changed.clone();
    s.user_list.connect_active_notify(move |_| c());
    let c = changed.clone();
    s.banner_on.connect_active_notify(move |_| c());
    let c = changed.clone();
    s.banner_text.connect_changed(move |_| c());
    for (b, _) in &s.accents {
        let c = changed.clone();
        b.connect_toggled(move |b| {
            if b.is_active() {
                c();
            }
        });
    }
    let c = changed;
    s.clock.connect_selected_notify(move |_| c());

    let weak = Rc::downgrade(s);
    s.discard.connect_clicked(move |_| {
        if let Some(s) = weak.upgrade() {
            let current = s.current.borrow().clone();
            show(&s, &current);
        }
    });
    let weak = Rc::downgrade(s);
    s.apply.connect_clicked(move |_| {
        if let Some(s) = weak.upgrade() {
            apply(&s);
        }
    });
}

/// Sends the staged changes in one helper run.
fn apply(s: &Rc<State>) {
    let changes: Changes = greeter::changes(&s.current.borrow(), &wanted(s));
    let Some(helper) = s.helper.clone() else {
        return;
    };
    if changes.is_empty() || changes.check().is_err() || s.busy.get() {
        return;
    }
    let input = changes.to_toml();
    if input.len() > greeter::INPUT_MAX {
        // Cannot happen with the banner's limit; the helper would refuse it.
        return;
    }
    s.busy.set(true);
    update(s);
    let weak = Rc::downgrade(s);
    spawn_worker(
        move || helper.run(&input),
        move |outcome| {
            let Some(s) = weak.upgrade() else { return };
            s.busy.set(false);
            match outcome {
                Outcome::Done => {
                    s.ctx.toast(tr(
                        "Saved. The login screen will use these settings the next time it starts.",
                    ));
                    reload(&s, true);
                }
                // Dismissed: nothing to say; the edits stay staged.
                Outcome::Cancelled => update(&s),
                Outcome::NotAuthorized => {
                    s.ctx
                        .toast(tr("Could not change the login screen: not authorized"));
                    update(&s);
                }
                Outcome::Failed(e) => {
                    s.ctx.toast(&trf!(
                        "Could not change the login screen settings: {e}",
                        e = e
                    ));
                    // The file may have changed meanwhile; edits stay.
                    reload(&s, false);
                }
            }
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clock_choices() {
        assert_eq!(CLOCKS.len(), 3);
        assert_eq!(CLOCKS[0].0, None, "Automatic first: no key");
        for f in greeter::CLOCK_FORMATS {
            assert!(CLOCKS.iter().any(|(c, _)| *c == Some(f)), "{f}");
        }
    }

    #[test]
    fn banner_problems_in_words() {
        assert_eq!(
            banner_problem(BannerError::TooLong(205)),
            "The banner is too long: 205 characters, at most 200"
        );
        assert!(banner_problem(BannerError::Control).contains("line breaks"));
        assert!(banner_problem(BannerError::Bidi).contains("direction"));
    }
}
