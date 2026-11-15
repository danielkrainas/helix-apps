//! Users panel: the current user and, for administrators after unlocking,
//! the other users (AccountsService; changes authorized by polkit).

use adw::prelude::*;
use gtk::{gdk, gio, glib};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use super::widgets;
use super::{follow, spawn_worker, Ctx, Panel};
use crate::accounts::{self, User, Users};
use crate::bus;
use helix_app_common::i18n::tr;
use helix_app_common::trf;

pub struct UsersPanel;

/// Where account pictures to choose from are.
const FACES: &str = "/usr/share/pixmaps/faces";

struct State {
    ctx: Rc<Ctx>,
    stack: gtk::Stack,
    avatar: adw::Avatar,
    name: adw::EntryRow,
    auto_login: adw::SwitchRow,
    language: adw::ComboRow,
    locales: Vec<String>,
    others: adw::PreferencesGroup,
    other_rows: RefCell<Vec<gtk::Widget>>,
    unlock: gtk::Button,
    add: gtk::Button,
    users: RefCell<Option<Users>>,
    unlocked: Cell<bool>,
    quiet: Cell<bool>,
}

impl State {
    fn me(&self) -> Option<User> {
        self.users.borrow().as_ref().map(|u| u.me.clone())
    }
}

fn avatar_image(avatar: &adw::Avatar, file: &str) {
    let texture = (!file.is_empty())
        .then(|| gdk::Texture::from_filename(file).ok())
        .flatten();
    avatar.set_custom_image(texture.as_ref());
}

impl Panel for UsersPanel {
    fn build(&self, ctx: &Rc<Ctx>) -> gtk::Widget {
        let stack = gtk::Stack::new();
        stack.add_named(&widgets::loading(tr("Loading Users…")), Some("loading"));
        stack.add_named(
            &widgets::status(
                "system-users-symbolic",
                tr("Users Are Not Available"),
                tr("The accounts service is not running."),
            ),
            Some("missing"),
        );
        let page = widgets::page();

        let top = widgets::group("");
        let avatar = adw::Avatar::builder().size(96).show_initials(true).build();
        let picture = gtk::Button::builder()
            .child(&avatar)
            .halign(gtk::Align::Center)
            .css_classes(["flat", "circular"])
            .tooltip_text(tr("Change Picture"))
            .build();
        helix_app_common::set_accessible_label(&picture, tr("Change Picture"));
        top.add(&picture);
        page.add(&top);

        let account = widgets::group(tr("Your Account"));
        let name = adw::EntryRow::builder()
            .use_markup(false)
            .title(tr("Name"))
            .show_apply_button(true)
            .build();
        account.add(&name);
        let (password_row, password) = widgets::action_row(tr("Password"), "", tr("Change…"), None);
        helix_app_common::set_accessible_label(&password, tr("Change Password"));
        account.add(&password_row);
        let auto_login = adw::SwitchRow::builder()
            .use_markup(false)
            .title(tr("Automatic Login"))
            .subtitle(tr("Log in without a password when the computer starts"))
            .build();
        account.add(&auto_login);
        let locales = accounts::locales();
        let mut labels = vec![tr("System Default").to_string()];
        labels.extend(locales.iter().cloned());
        let language = adw::ComboRow::builder()
            .use_markup(false)
            .title(tr("Language"))
            .model(&gtk::StringList::new(
                &labels.iter().map(String::as_str).collect::<Vec<_>>(),
            ))
            .enable_search(true)
            .build();
        account.add(&language);
        page.add(&account);

        let others = widgets::group(tr("Other Users"));
        let header = gtk::Box::builder().spacing(6).build();
        let unlock = gtk::Button::builder()
            .label(tr("_Unlock…"))
            .use_underline(true)
            .valign(gtk::Align::Center)
            .build();
        helix_app_common::set_accessible_label(&unlock, tr("Unlock to Change Other Users"));
        let add = gtk::Button::builder()
            .icon_name("list-add-symbolic")
            .valign(gtk::Align::Center)
            .css_classes(["flat"])
            .tooltip_text(tr("Add User…"))
            .visible(false)
            .build();
        helix_app_common::set_accessible_label(&add, tr("Add User…"));
        header.append(&unlock);
        header.append(&add);
        others.set_header_suffix(Some(&header));
        page.add(&others);
        stack.add_named(&page, Some("page"));
        stack.set_visible_child_name("loading");

        let state = Rc::new(State {
            ctx: ctx.clone(),
            stack: stack.clone(),
            avatar,
            name: name.clone(),
            auto_login: auto_login.clone(),
            language: language.clone(),
            locales,
            others,
            other_rows: RefCell::default(),
            unlock: unlock.clone(),
            add: add.clone(),
            users: RefCell::default(),
            unlocked: Cell::new(false),
            quiet: Cell::new(false),
        });

        let weak = Rc::downgrade(&state);
        picture.connect_clicked(move |_| {
            if let Some(state) = weak.upgrade() {
                if let Some(me) = state.me() {
                    picture_dialog(&state, &me);
                }
            }
        });
        let weak = Rc::downgrade(&state);
        name.connect_apply(move |e| {
            let Some(state) = weak.upgrade() else { return };
            let Some(me) = state.me() else { return };
            let text = e.text().trim().to_string();
            call(
                &state,
                move |c| accounts::set_real_name(c, &me.path, &text),
                tr("Could not change the name"),
            );
        });
        let weak = Rc::downgrade(&state);
        password.connect_clicked(move |_| {
            if let Some(state) = weak.upgrade() {
                if let Some(me) = state.me() {
                    password_dialog(&state, &me);
                }
            }
        });
        let weak = Rc::downgrade(&state);
        auto_login.connect_active_notify(move |r| {
            let Some(state) = weak.upgrade() else { return };
            if state.quiet.get() {
                return;
            }
            let Some(me) = state.me() else { return };
            let on = r.is_active();
            call(
                &state,
                move |c| accounts::set_automatic_login(c, &me.path, on),
                tr("Could not change automatic login"),
            );
        });
        let weak = Rc::downgrade(&state);
        language.connect_selected_notify(move |r| {
            let Some(state) = weak.upgrade() else { return };
            if state.quiet.get() {
                return;
            }
            let Some(me) = state.me() else { return };
            let l = (r.selected() as usize)
                .checked_sub(1)
                .and_then(|i| state.locales.get(i).cloned())
                .unwrap_or_default();
            call(
                &state,
                move |c| accounts::set_language(c, &me.path, &l),
                tr("Could not change the language"),
            );
        });
        let weak = Rc::downgrade(&state);
        unlock.connect_clicked(move |b| {
            let Some(state) = weak.upgrade() else { return };
            b.set_sensitive(false);
            let weak = Rc::downgrade(&state);
            spawn_worker(
                || bus::system().and_then(|c| accounts::unlock(&c)),
                move |r| {
                    let Some(state) = weak.upgrade() else { return };
                    state.unlock.set_sensitive(true);
                    match r {
                        Ok(true) => {
                            state.unlocked.set(true);
                            show_others(&state);
                        }
                        Ok(false) | Err(bus::Error::Denied) => {}
                        Err(e) => state.ctx.toast(&trf!("Could not unlock: {e}", e = e)),
                    }
                },
            );
        });
        let weak = Rc::downgrade(&state);
        add.connect_clicked(move |_| {
            if let Some(state) = weak.upgrade() {
                add_dialog(&state);
            }
        });

        refresh(&state);
        follow(
            &state,
            Duration::from_millis(300),
            |tell| {
                if let Ok(conn) = bus::system() {
                    accounts::watch(&conn, tell);
                }
            },
            refresh,
        );
        stack.connect_destroy(move |_| {
            let _ = &state;
        });
        stack.upcast()
    }
}

fn refresh(state: &Rc<State>) {
    let weak = Rc::downgrade(state);
    spawn_worker(
        || bus::system().and_then(|c| accounts::read(&c, accounts::my_uid())),
        move |r| {
            if let Some(state) = weak.upgrade() {
                show(&state, r);
            }
        },
    );
}

/// Runs a change on a worker; errors other than refusals become a toast
/// starting with `what`.
fn call(
    state: &Rc<State>,
    f: impl FnOnce(&zbus::blocking::Connection) -> bus::Result<()> + Send + 'static,
    what: &'static str,
) {
    let weak = Rc::downgrade(state);
    spawn_worker(
        move || bus::system().and_then(|c| f(&c)),
        move |r| {
            let Some(state) = weak.upgrade() else { return };
            if let Err(e) = r {
                if e != bus::Error::Denied {
                    state.ctx.toast(&format!("{what}: {e}"));
                }
            }
            refresh(&state);
        },
    );
}

fn show(state: &Rc<State>, r: bus::Result<Users>) {
    let users = match r {
        Ok(u) => u,
        Err(e) => {
            if e != bus::Error::Missing {
                eprintln!("helix-settings-app: accounts: {e}");
            }
            state.stack.set_visible_child_name("missing");
            return;
        }
    };
    state.stack.set_visible_child_name("page");
    state.quiet.set(true);
    let me = &users.me;
    state.avatar.set_text(Some(me.title()));
    avatar_image(&state.avatar, &me.icon);
    if !state.name.has_focus() && state.name.text() != me.real_name {
        state.name.set_text(&me.real_name);
    }
    state.auto_login.set_active(me.automatic_login);
    let index = state
        .locales
        .iter()
        .position(|l| {
            l.eq_ignore_ascii_case(&me.language) || l.replace(".UTF-8", "") == me.language
        })
        .map_or(0, |i| i + 1);
    state.language.set_selected(index as u32);
    state.quiet.set(false);
    // Only administrators can change other users.
    state.others.set_visible(me.admin());
    *state.users.borrow_mut() = Some(users);
    show_others(state);
}

fn show_others(state: &Rc<State>) {
    let unlocked = state.unlocked.get();
    state.unlock.set_visible(!unlocked);
    state.add.set_visible(unlocked);
    let others: Vec<User> = state
        .users
        .borrow()
        .as_ref()
        .map(|u| u.others.clone())
        .unwrap_or_default();
    state.others.set_description(if !unlocked {
        Some(tr("Unlock to add, remove or change other users."))
    } else if others.is_empty() {
        Some(tr("No other users."))
    } else {
        None
    });
    let rows: Vec<gtk::Widget> = others
        .iter()
        .map(|u| {
            let row = adw::ActionRow::builder()
                .use_markup(false)
                .title(u.title())
                .subtitle(if u.admin() {
                    tr("Administrator")
                } else {
                    tr("Standard")
                })
                .activatable(unlocked)
                .build();
            let avatar = adw::Avatar::builder()
                .size(32)
                .text(u.title())
                .show_initials(true)
                .build();
            avatar_image(&avatar, &u.icon);
            avatar.set_accessible_role(gtk::AccessibleRole::Presentation);
            row.add_prefix(&avatar);
            if unlocked {
                row.add_suffix(
                    &gtk::Image::builder()
                        .icon_name("go-next-symbolic")
                        .accessible_role(gtk::AccessibleRole::Presentation)
                        .build(),
                );
                let (weak, u) = (Rc::downgrade(state), u.clone());
                row.connect_activated(move |_| {
                    if let Some(state) = weak.upgrade() {
                        user_dialog(&state, &u);
                    }
                });
            }
            row.upcast()
        })
        .collect();
    widgets::replace_rows(&state.others, &state.other_rows, rows);
}

fn picture_dialog(state: &Rc<State>, me: &User) {
    let page = widgets::page();
    let group = widgets::group("");
    let flow = gtk::FlowBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .max_children_per_line(5)
        .column_spacing(6)
        .row_spacing(6)
        .homogeneous(true)
        .build();
    let dialog = widgets::dialog(tr("Choose a Picture"), &page);
    let mut faces: Vec<std::path::PathBuf> = std::fs::read_dir(FACES)
        .map(|d| {
            d.flatten()
                .map(|e| e.path())
                .filter(|p| p.is_file())
                .collect()
        })
        .unwrap_or_default();
    faces.sort();
    let pick = {
        let (weak, path, d) = (Rc::downgrade(state), me.path.clone(), dialog.downgrade());
        move |file: String| {
            if let Some(d) = d.upgrade() {
                d.close();
            }
            if let Some(state) = weak.upgrade() {
                let path = path.clone();
                call(
                    &state,
                    move |c| accounts::set_icon(c, &path, &file),
                    tr("Could not change the picture"),
                );
            }
        }
    };
    for f in faces {
        let image = gtk::Picture::for_filename(&f);
        image.set_can_shrink(true);
        image.set_size_request(64, 64);
        let b = gtk::Button::builder()
            .child(&image)
            .css_classes(["flat"])
            .build();
        let name = f
            .file_stem()
            .map(|s| s.to_string_lossy().replace(['-', '_'], " "))
            .unwrap_or_default();
        helix_app_common::set_accessible_label(&b, &trf!("Use Picture “{name}”", name = name));
        let (pick, f) = (pick.clone(), f.to_string_lossy().into_owned());
        b.connect_clicked(move |_| pick(f.clone()));
        flow.append(&b);
    }
    group.add(&flow);
    page.add(&group);
    let files = widgets::group("");
    let (row, choose) = widgets::action_row(tr("Picture From a File"), "", tr("Choose…"), None);
    files.add(&row);
    page.add(&files);
    let (window, pick2) = (state.ctx.window.clone(), pick.clone());
    choose.connect_clicked(move |_| {
        let filter = gtk::FileFilter::new();
        filter.set_name(Some(tr("Images")));
        filter.add_mime_type("image/*");
        let filters = gio::ListStore::new::<gtk::FileFilter>();
        filters.append(&filter);
        let chooser = gtk::FileDialog::builder()
            .title(tr("Choose a Picture"))
            .modal(true)
            .filters(&filters)
            .build();
        let pick = pick2.clone();
        chooser.open(Some(&window), gio::Cancellable::NONE, move |r| {
            if let Some(p) = r.ok().and_then(|f| f.path()) {
                pick(p.to_string_lossy().into_owned());
            }
        });
    });
    dialog.set_content_height(480);
    dialog.present(Some(&state.ctx.window));
}

/// A new password, twice, with a hint below; `button` is sensitive
/// while they are good.
fn password_entries(
    group: &adw::PreferencesGroup,
    button: &gtk::Button,
) -> (adw::PasswordEntryRow, adw::PasswordEntryRow, gtk::Label) {
    let first = adw::PasswordEntryRow::builder()
        .title(tr("New Password"))
        .build();
    let second = adw::PasswordEntryRow::builder()
        .title(tr("Confirm Password"))
        .build();
    let hint = gtk::Label::builder()
        .wrap(true)
        .xalign(0.0)
        .css_classes(["dim-label", "caption"])
        .margin_top(6)
        .build();
    group.add(&first);
    group.add(&second);
    group.add(&hint);
    let sync = {
        let (first, second, hint, button) =
            (first.clone(), second.clone(), hint.clone(), button.clone());
        move || {
            let (a, b) = (first.text(), second.text());
            let message = if a.chars().count() < 8 {
                tr("Use at least 8 characters.")
            } else if a != b {
                tr("The passwords do not match.")
            } else {
                ""
            };
            hint.set_label(message);
            button.set_sensitive(message.is_empty());
        }
    };
    sync();
    let s = sync.clone();
    first.connect_changed(move |_| s());
    second.connect_changed(move |_| sync());
    (first, second, hint)
}

fn password_dialog(state: &Rc<State>, user: &User) {
    let page = widgets::page();
    let group = widgets::group("");
    group.set_description(Some(tr(
        "You may be asked for your current password or an administrator's.",
    )));
    let view = adw::ToolbarView::new();
    let header = adw::HeaderBar::new();
    let change = gtk::Button::builder()
        .label(tr("C_hange"))
        .use_underline(true)
        .css_classes(["suggested-action"])
        .build();
    header.pack_end(&change);
    let (first, _, _) = password_entries(&group, &change);
    page.add(&group);
    view.add_top_bar(&header);
    view.set_content(Some(&page));
    let dialog = adw::Dialog::builder()
        .title(trf!("Password for {name}", name = user.title()))
        .content_width(460)
        .child(&view)
        .build();
    let (weak, path, d) = (Rc::downgrade(state), user.path.clone(), dialog.downgrade());
    change.connect_clicked(move |_| {
        let (Some(state), Some(d)) = (weak.upgrade(), d.upgrade()) else {
            return;
        };
        let (path, password) = (path.clone(), first.text().to_string());
        d.close();
        call(
            &state,
            move |c| accounts::set_password(c, &path, &password),
            tr("Could not change the password"),
        );
    });
    dialog.present(Some(&state.ctx.window));
}

fn user_dialog(state: &Rc<State>, user: &User) {
    let page = widgets::page();
    let group = widgets::group("");
    let kind = adw::ComboRow::builder()
        .use_markup(false)
        .title(tr("Account Type"))
        .model(&gtk::StringList::new(&[
            tr("Standard"),
            tr("Administrator"),
        ]))
        .selected(u32::from(user.admin()))
        .build();
    group.add(&kind);
    let auto = adw::SwitchRow::builder()
        .use_markup(false)
        .title(tr("Automatic Login"))
        .active(user.automatic_login)
        .build();
    group.add(&auto);
    let (pw_row, pw) = widgets::action_row(tr("Password"), "", tr("Change…"), None);
    helix_app_common::set_accessible_label(
        &pw,
        &trf!("Change Password of {name}", name = user.title()),
    );
    group.add(&pw_row);
    page.add(&group);
    let more = widgets::group("");
    if glib::find_program_in_path("malcontent-control").is_some() {
        let (row, parental) = widgets::action_row(
            tr("Parental Controls"),
            tr("Limit what this user can use"),
            tr("Open"),
            None,
        );
        let (name, ctx) = (user.name.clone(), Rc::downgrade(&state.ctx));
        parental.connect_clicked(move |_| {
            let r = std::process::Command::new("malcontent-control")
                .arg(format!("--user={name}"))
                .spawn()
                .map(|mut c| std::thread::spawn(move || c.wait()));
            if let (Err(e), Some(ctx)) = (r, ctx.upgrade()) {
                ctx.toast(&trf!("Could not open parental controls: {e}", e = e));
            }
        });
        more.add(&row);
    }
    let (remove_row, remove) = widgets::action_row(
        tr("Remove User"),
        "",
        tr("Remove…"),
        Some("destructive-action"),
    );
    more.add(&remove_row);
    page.add(&more);
    let dialog = widgets::dialog(user.title(), &page);
    let (weak, path) = (Rc::downgrade(state), user.path.clone());
    kind.connect_selected_notify(move |r| {
        let Some(state) = weak.upgrade() else { return };
        let (path, k) = (path.clone(), r.selected() as i32);
        call(
            &state,
            move |c| accounts::set_account_type(c, &path, k),
            tr("Could not change the account type"),
        );
    });
    let (weak, path) = (Rc::downgrade(state), user.path.clone());
    auto.connect_active_notify(move |r| {
        let Some(state) = weak.upgrade() else { return };
        let (path, on) = (path.clone(), r.is_active());
        call(
            &state,
            move |c| accounts::set_automatic_login(c, &path, on),
            tr("Could not change automatic login"),
        );
    });
    let (weak, u) = (Rc::downgrade(state), user.clone());
    pw.connect_clicked(move |_| {
        if let Some(state) = weak.upgrade() {
            password_dialog(&state, &u);
        }
    });
    let (weak, u, d) = (Rc::downgrade(state), user.clone(), dialog.downgrade());
    remove.connect_clicked(move |_| {
        let (Some(state), Some(d)) = (weak.upgrade(), d.upgrade()) else {
            return;
        };
        confirm_remove(&state, &d, &u);
    });
    dialog.present(Some(&state.ctx.window));
}

fn confirm_remove(state: &Rc<State>, parent: &adw::Dialog, user: &User) {
    let alert = adw::AlertDialog::new(
        Some(&trf!("Remove {name}?", name = user.title())),
        Some(tr(
            "Their files can be kept (in their home folder) or deleted.",
        )),
    );
    alert.add_response("cancel", tr("_Cancel"));
    alert.add_response("keep", tr("_Keep Files"));
    alert.add_response("delete", tr("_Delete Files"));
    alert.set_response_appearance("delete", adw::ResponseAppearance::Destructive);
    alert.set_default_response(Some("cancel"));
    alert.set_close_response("cancel");
    let (weak, uid, p) = (Rc::downgrade(state), user.uid, parent.downgrade());
    alert.connect_response(None, move |_, response| {
        let remove_files = match response {
            "keep" => false,
            "delete" => true,
            _ => return,
        };
        if let Some(p) = p.upgrade() {
            p.close();
        }
        if let Some(state) = weak.upgrade() {
            call(
                &state,
                move |c| accounts::delete(c, uid, remove_files),
                tr("Could not remove the user"),
            );
        }
    });
    alert.present(Some(parent));
}

fn add_dialog(state: &Rc<State>) {
    let page = widgets::page();
    let group = widgets::group("");
    let real = adw::EntryRow::builder()
        .use_markup(false)
        .title(tr("Full Name"))
        .build();
    let name = adw::EntryRow::builder()
        .use_markup(false)
        .title(tr("Username"))
        .build();
    let kind = adw::ComboRow::builder()
        .use_markup(false)
        .title(tr("Account Type"))
        .model(&gtk::StringList::new(&[
            tr("Standard"),
            tr("Administrator"),
        ]))
        .build();
    let set_now = adw::SwitchRow::builder()
        .use_markup(false)
        .title(tr("Set a Password Now"))
        .subtitle(tr("Otherwise they choose one when they first log in"))
        .build();
    group.add(&real);
    group.add(&name);
    group.add(&kind);
    group.add(&set_now);
    page.add(&group);
    let pw_group = widgets::group("");
    let view = adw::ToolbarView::new();
    let header = adw::HeaderBar::new();
    let add = gtk::Button::builder()
        .label(tr("_Add"))
        .use_underline(true)
        .css_classes(["suggested-action"])
        .sensitive(false)
        .build();
    // The rows' own verdict is not used here: the name counts too.
    let (first, second, _) = password_entries(&pw_group, &gtk::Button::new());
    pw_group.set_visible(false);
    page.add(&pw_group);
    header.pack_end(&add);
    view.add_top_bar(&header);
    view.set_content(Some(&page));
    let dialog = adw::Dialog::builder()
        .title(tr("Add User"))
        .content_width(460)
        .child(&view)
        .build();
    // The user name follows the full name until it is edited.
    let edited = Rc::new(Cell::new(false));
    let valid = {
        let (real, name, set_now, first, second) = (
            real.clone(),
            name.clone(),
            set_now.clone(),
            first.clone(),
            second.clone(),
        );
        move || {
            let names_ok = !real.text().trim().is_empty() && accounts::user_name_ok(&name.text());
            let pw = !set_now.is_active()
                || (first.text().chars().count() >= 8 && first.text() == second.text());
            names_ok && pw
        }
    };
    let sync = {
        let (add, name, valid) = (add.clone(), name.clone(), valid.clone());
        move || {
            let bad = !name.text().is_empty() && !accounts::user_name_ok(&name.text());
            helix_app_common::set_input_error(
                &name,
                None,
                bad.then_some(tr(
                    "Use lower case letters, digits, - and _, starting with a letter",
                )),
            );
            add.set_sensitive(valid());
        }
    };
    {
        let (name, edited, sync) = (name.clone(), edited.clone(), sync.clone());
        real.connect_changed(move |r| {
            if !edited.get() {
                name.set_text(&accounts::user_name_for(&r.text()));
                edited.set(false);
            }
            sync();
        });
    }
    {
        let (edited, sync, real) = (edited.clone(), sync.clone(), real.clone());
        name.connect_changed(move |n| {
            edited.set(n.text() != accounts::user_name_for(&real.text()));
            sync();
        });
    }
    {
        let (pw_group, sync) = (pw_group.clone(), sync.clone());
        set_now.connect_active_notify(move |s| {
            pw_group.set_visible(s.is_active());
            sync();
        });
    }
    for e in [&first, &second] {
        let sync = sync.clone();
        e.connect_changed(move |_| sync());
    }
    let (weak, d) = (Rc::downgrade(state), dialog.downgrade());
    add.connect_clicked(move |_| {
        let (Some(state), Some(d)) = (weak.upgrade(), d.upgrade()) else {
            return;
        };
        let (r, n, k) = (
            real.text().trim().to_string(),
            name.text().to_string(),
            kind.selected() as i32,
        );
        let password = set_now.is_active().then(|| first.text().to_string());
        d.close();
        call(
            &state,
            move |c| {
                let path = accounts::create(c, &n, &r, k)?;
                match password {
                    Some(p) => accounts::set_password(c, &path, &p),
                    None => accounts::set_password_at_login(c, &path),
                }
            },
            tr("Could not add the user"),
        );
    });
    dialog.present(Some(&state.ctx.window));
}
