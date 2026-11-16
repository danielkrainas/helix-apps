//! Apps panel: default apps, removable media and each installed app's
//! permissions (the portals' permission store), file types, links and
//! search.

use adw::prelude::*;
use gtk::{gio, glib};
use helix_app_common::Prefs;
use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use super::notifications::app_path;
use super::widgets::{self, switch, watch, InfoRow};
use super::{follow, spawn_worker, Ctx, Panel};
use crate::bus;
use crate::defaults::{self, Action, Lists};
use crate::permissions::{self, AppPermissions};
use helix_app_common::i18n::{tr, tr_dyn};
use helix_app_common::trf;

pub struct AppsPanel;

const MEDIA: &str = "org.gnome.desktop.media-handling";
const SEARCH: &str = "org.gnome.desktop.search-providers";
const NOTIFICATIONS: &str = "org.gnome.desktop.notifications";
const NOTIFICATION_APP: &str = "org.gnome.desktop.notifications.application";

/// An open app dialog: the desktop id, the permissions group and its rows.
type OpenApp = (String, adw::PreferencesGroup, RefCell<Vec<gtk::Widget>>);

struct State {
    ctx: Rc<Ctx>,
    installed: adw::PreferencesGroup,
    rows: RefCell<Vec<gtk::Widget>>,
    shown: RefCell<Vec<String>>,
    /// The open app dialog: its desktop id and permission rows.
    open: RefCell<Option<OpenApp>>,
    _monitor: gio::AppInfoMonitor,
}

fn apps() -> Vec<gio::AppInfo> {
    let mut all: Vec<gio::AppInfo> = gio::AppInfo::all()
        .into_iter()
        .filter(|a| a.should_show())
        .collect();
    all.sort_by_key(|a| a.display_name().to_lowercase());
    all
}

fn app_icon(info: &gio::AppInfo, size: i32) -> gtk::Image {
    let image = gtk::Image::builder()
        .pixel_size(size)
        .accessible_role(gtk::AccessibleRole::Presentation)
        .build();
    match info.icon() {
        Some(icon) => image.set_from_gicon(&icon),
        None => image.set_icon_name(Some("application-x-executable")),
    }
    image
}

impl Panel for AppsPanel {
    fn build(&self, ctx: &Rc<Ctx>) -> gtk::Widget {
        let page = widgets::page();
        page.add(&defaults_group(ctx));
        page.add(&media_group(ctx, &ctx.prefs(MEDIA)));
        let installed = widgets::group(tr("Installed Apps"));
        page.add(&installed);
        let monitor = gio::AppInfoMonitor::get();
        let state = Rc::new(State {
            ctx: ctx.clone(),
            installed,
            rows: RefCell::default(),
            shown: RefCell::default(),
            open: RefCell::default(),
            _monitor: monitor.clone(),
        });
        list_apps(&state);
        // Apps installed or removed meanwhile.
        let weak = Rc::downgrade(&state);
        monitor.connect_changed(move |_| {
            if let Some(state) = weak.upgrade() {
                list_apps(&state);
            }
        });
        // Permissions changed elsewhere (a portal asking, another tool).
        follow(
            &state,
            Duration::from_millis(300),
            |tell| {
                if let Ok(conn) = bus::session() {
                    permissions::watch(&conn, tell);
                }
            },
            |state| {
                let id = state.open.borrow().as_ref().map(|o| o.0.clone());
                if let Some(id) = id {
                    load_permissions(state, &id);
                }
            },
        );
        page.connect_destroy(move |_| {
            let _ = &state;
        });
        page.upcast()
    }
}

fn list_apps(state: &Rc<State>) {
    let all = apps();
    let ids: Vec<String> = all
        .iter()
        .filter_map(|a| a.id().map(|s| s.to_string()))
        .collect();
    if *state.shown.borrow() == ids {
        return;
    }
    let rows: Vec<gtk::Widget> = all
        .iter()
        .map(|info| {
            let row = adw::ActionRow::builder()
                .use_markup(false)
                .title(info.display_name())
                .activatable(true)
                .build();
            row.add_prefix(&app_icon(info, 32));
            row.add_suffix(
                &gtk::Image::builder()
                    .icon_name("go-next-symbolic")
                    .accessible_role(gtk::AccessibleRole::Presentation)
                    .build(),
            );
            let (weak, info) = (Rc::downgrade(state), info.clone());
            row.connect_activated(move |_| {
                if let Some(state) = weak.upgrade() {
                    app_dialog(&state, &info);
                }
            });
            row.upcast()
        })
        .collect();
    widgets::replace_rows(&state.installed, &state.rows, rows);
    *state.shown.borrow_mut() = ids;
}

/// The default app for each category.
fn defaults_group(ctx: &Rc<Ctx>) -> adw::PreferencesGroup {
    let group = widgets::group(tr("Default Apps"));
    for cat in &defaults::CATEGORIES {
        let primary = cat.types[0];
        let choices: Vec<gio::AppInfo> = gio::AppInfo::all_for_type(primary);
        let row = adw::ComboRow::builder()
            .use_markup(false)
            .title(tr(cat.label))
            .build();
        if choices.is_empty() {
            row.set_subtitle(tr("No apps installed for this"));
            row.set_sensitive(false);
            group.add(&row);
            continue;
        }
        let names = gtk::StringList::new(&[]);
        for c in &choices {
            names.append(&c.display_name());
        }
        row.set_model(Some(&names));
        let current = gio::AppInfo::default_for_type(primary, false).and_then(|d| d.id());
        if let Some(i) = choices.iter().position(|c| c.id() == current) {
            row.set_selected(i as u32);
        }
        let ctx = Rc::downgrade(ctx);
        row.connect_selected_notify(move |r| {
            let Some(app) = choices.get(r.selected() as usize) else {
                return;
            };
            let mut failed = None;
            for t in cat.types {
                // The first type must work; the others where the app takes them.
                let supports = t == &primary || app.supports_uris() || {
                    app.downcast_ref::<gio::DesktopAppInfo>()
                        .is_some_and(|d| d.supported_types().iter().any(|s| s == t))
                };
                if supports {
                    if let Err(e) = app.set_as_default_for_type(t) {
                        if *t == primary {
                            failed = Some(e.to_string());
                        }
                    }
                }
            }
            if let (Some(e), Some(ctx)) = (failed, ctx.upgrade()) {
                ctx.toast(&trf!("Could not change the default app: {e}", e = e));
            }
        });
        group.add(&row);
    }
    group
}

/// What happens when removable media are inserted.
fn media_group(ctx: &Rc<Ctx>, prefs: &Prefs) -> adw::PreferencesGroup {
    let group = widgets::group(tr("Removable Media"));
    let never = switch(
        prefs,
        "autorun-never",
        false,
        false,
        tr("Never Ask or Start Apps"),
        tr("Inserting media does nothing"),
    );
    group.add(&never);
    let lists = move |p: &Prefs| Lists {
        start_app: p.get::<Vec<String>>("autorun-x-content-start-app", Vec::new()),
        ignore: p.get::<Vec<String>>("autorun-x-content-ignore", Vec::new()),
        open_folder: p.get::<Vec<String>>("autorun-x-content-open-folder", Vec::new()),
    };
    let mut rows = Vec::new();
    for (kind, label) in defaults::MEDIA {
        let apps: Vec<gio::AppInfo> = gio::AppInfo::all_for_type(kind);
        let mut choices = vec![Action::Ask, Action::Nothing, Action::OpenFolder];
        let model =
            gtk::StringList::new(&[tr("Ask What to Do"), tr("Do Nothing"), tr("Open Folder")]);
        for a in &apps {
            if let Some(id) = a.id() {
                choices.push(Action::App(id.to_string()));
                model.append(&trf!("Open {app}", app = a.display_name()));
            }
        }
        let row = adw::ComboRow::builder()
            .use_markup(false)
            .title(tr_dyn(label))
            .model(&model)
            .build();
        let show = {
            let (row, choices, p) = (row.downgrade(), choices.clone(), prefs.clone());
            move || {
                let Some(row) = row.upgrade() else { return };
                let default = gio::AppInfo::default_for_type(kind, false).and_then(|a| a.id());
                let action = lists(&p).action(kind, default.as_deref());
                if let Some(i) = choices.iter().position(|c| *c == action) {
                    row.set_selected(i as u32);
                }
            }
        };
        show();
        let (p, ctx) = (prefs.clone(), Rc::downgrade(ctx));
        let all = choices.clone();
        row.connect_selected_notify(move |r| {
            let Some(action) = all.get(r.selected() as usize) else {
                return;
            };
            let mut l = lists(&p);
            if l.action(
                kind,
                gio::AppInfo::default_for_type(kind, false)
                    .and_then(|a| a.id())
                    .as_deref(),
            ) == *action
            {
                return;
            }
            if let Action::App(id) = action {
                let ok = gio::DesktopAppInfo::new(id).map(|a| a.set_as_default_for_type(kind));
                if let (Some(Err(e)), Some(ctx)) = (ok, ctx.upgrade()) {
                    ctx.toast(&trf!("Could not change the default app: {e}", e = e));
                }
            }
            l.set(kind, action);
            p.set("autorun-x-content-start-app", l.start_app);
            p.set("autorun-x-content-ignore", l.ignore);
            p.set("autorun-x-content-open-folder", l.open_folder);
        });
        for key in [
            "autorun-x-content-start-app",
            "autorun-x-content-ignore",
            "autorun-x-content-open-folder",
        ] {
            watch(prefs, key, show.clone());
        }
        group.add(&row);
        rows.push(row);
    }
    let sync = {
        let p = prefs.clone();
        move || {
            let on = !p.get::<bool>("autorun-never", false);
            for r in &rows {
                r.set_sensitive(on);
            }
        }
    };
    sync();
    let s = sync.clone();
    never.connect_active_notify(move |_| s());
    watch(prefs, "autorun-never", sync);
    group
}

fn load_permissions(state: &Rc<State>, desktop_id: &str) {
    let (weak, app) = (
        Rc::downgrade(state),
        permissions::app_id(desktop_id).to_string(),
    );
    let id = desktop_id.to_string();
    spawn_worker(
        move || bus::session().and_then(|c| permissions::read(&c, &app)),
        move |r| {
            if let Some(state) = weak.upgrade() {
                show_permissions(&state, &id, r);
            }
        },
    );
}

fn show_permissions(state: &Rc<State>, desktop_id: &str, r: bus::Result<AppPermissions>) {
    let open = state.open.borrow();
    let Some((id, group, rows)) = open.as_ref() else {
        return;
    };
    if id != desktop_id {
        return;
    }
    let app = permissions::app_id(desktop_id).to_string();
    let new: Vec<gtk::Widget> = match r {
        Err(e) => {
            group.set_description(Some(if e == bus::Error::Missing {
                tr("The permission store is not running, so permissions cannot be shown.")
            } else {
                tr("The permissions could not be read.")
            }));
            Vec::new()
        }
        Ok(perms) if perms.is_empty() => {
            group.set_description(Some(tr("This app has not asked for any permissions yet.")));
            Vec::new()
        }
        Ok(perms) => {
            group.set_description(None);
            perms
                .into_iter()
                .map(|(kind, allowed)| {
                    let row = adw::SwitchRow::builder()
                        .use_markup(false)
                        .title(kind.title())
                        .active(allowed)
                        .build();
                    let (app, ctx) = (app.clone(), Rc::downgrade(&state.ctx));
                    row.connect_active_notify(move |r| {
                        let (app, ctx, on) = (app.clone(), ctx.clone(), r.is_active());
                        spawn_worker(
                            move || {
                                bus::session().and_then(|c| permissions::set(&c, &app, &kind, on))
                            },
                            move |r| {
                                if let (Err(e), Some(ctx)) = (r, ctx.upgrade()) {
                                    ctx.toast(&trf!("Could not change the permission: {e}", e = e));
                                }
                            },
                        );
                    });
                    row.upcast()
                })
                .collect()
        }
    };
    // Rows are rebuilt only when what they show changed.
    let same = rows.borrow().len() == new.len()
        && rows.borrow().iter().zip(&new).all(|(a, b)| {
            let (a, b) = (a.downcast_ref::<adw::SwitchRow>(), b.downcast_ref::<adw::SwitchRow>());
            matches!((a, b), (Some(a), Some(b)) if a.title() == b.title() && a.is_active() == b.is_active())
        });
    if !same {
        widgets::replace_rows(group, rows, new);
    }
}

fn app_dialog(state: &Rc<State>, info: &gio::AppInfo) {
    let Some(desktop_id) = info.id().map(|s| s.to_string()) else {
        return;
    };
    let app = permissions::app_id(&desktop_id).to_string();
    let desktop = info.downcast_ref::<gio::DesktopAppInfo>().cloned();
    let types: Vec<String> = desktop
        .as_ref()
        .map(|d| d.supported_types().iter().map(|s| s.to_string()).collect())
        .unwrap_or_default();
    let page = widgets::page();

    let header = widgets::group("");
    let top = adw::ActionRow::builder()
        .use_markup(false)
        .title(info.display_name())
        .subtitle(
            info.description()
                .map(|d| d.to_string())
                .unwrap_or_default(),
        )
        .subtitle_lines(0)
        .build();
    top.add_prefix(&app_icon(info, 48));
    header.add(&top);
    page.add(&header);

    let perms = widgets::group(tr("Permissions"));
    page.add(&perms);

    // Notifications: the same switch as in the Notifications panel.
    let children: Vec<String> = state
        .ctx
        .prefs(NOTIFICATIONS)
        .get::<Vec<String>>("application-children", Vec::new());
    let dashed = app.to_lowercase().replace('.', "-");
    let child = children
        .iter()
        .find(|c| **c == app || **c == dashed)
        .cloned();
    if let (Some(child), Some(schema)) = (child, helix_app_common::lookup_schema(NOTIFICATION_APP))
    {
        let group = widgets::group(tr("Notifications"));
        let settings = gio::Settings::new_full(
            &schema,
            None::<&gio::SettingsBackend>,
            Some(&app_path(&child)),
        );
        let row = adw::SwitchRow::builder()
            .use_markup(false)
            .title(tr("Show Notifications"))
            .build();
        settings.bind("enable", &row, "active").build();
        group.add(&row);
        page.add(&group);
    }

    // Search in the overview, for apps with a search provider.
    let search_prefs = state.ctx.prefs(SEARCH);
    let mut dirs = vec![glib::user_data_dir()];
    dirs.extend(glib::system_data_dirs());
    if defaults::search_provider_ids(&dirs).contains(&desktop_id) {
        let group = widgets::group(tr("Search"));
        let row = adw::SwitchRow::builder()
            .use_markup(false)
            .title(tr("Search in the Overview"))
            .active(
                !search_prefs
                    .get::<Vec<String>>("disabled", Vec::new())
                    .contains(&desktop_id),
            )
            .build();
        let (p, id) = (search_prefs.clone(), desktop_id.clone());
        row.connect_active_notify(move |r| {
            let mut disabled = p.get::<Vec<String>>("disabled", Vec::new());
            disabled.retain(|d| *d != id);
            if !r.is_active() {
                disabled.push(id.clone());
            }
            p.set("disabled", disabled);
        });
        group.add(&row);
        page.add(&group);
    }

    // What it opens.
    let opens = widgets::group(tr("Opens"));
    let mine: Vec<&str> = defaults::file_types(&types)
        .into_iter()
        .filter(|t| {
            gio::AppInfo::default_for_type(t, false)
                .and_then(|d| d.id())
                .as_deref()
                == Some(desktop_id.as_str())
        })
        .collect();
    let described: Vec<String> = mine
        .iter()
        .map(|t| gio::content_type_get_description(t).to_string())
        .collect();
    let files = InfoRow::new(
        tr("Default for"),
        &if described.is_empty() {
            tr("No file types").to_string()
        } else {
            described.join(", ")
        },
    );
    opens.add(&files.row);
    if !mine.is_empty() {
        let (reset_row, reset) = widgets::action_row(
            tr("Reset File Types"),
            tr("Forget your choices of default app for these file types"),
            tr("Reset"),
            None,
        );
        let (types, row) = (
            mine.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
            reset_row.clone(),
        );
        reset.connect_clicked(move |_| {
            for t in &types {
                gio::AppInfo::reset_type_associations(t);
            }
            row.set_sensitive(false);
        });
        opens.add(&reset_row);
    }
    let links = defaults::schemes(&types);
    if !links.is_empty() {
        opens.add(&InfoRow::new(tr("Links"), &links.join(", ")).row);
    }
    page.add(&opens);

    // Flatpak apps: their size and data, and uninstalling; Software for
    // the rest.
    let flatpak = desktop
        .as_ref()
        .and_then(|d| d.filename())
        .and_then(|f| defaults::FlatpakApp::from_desktop_file(&f, &glib::home_dir()));
    let more = widgets::group("");
    if let Some(fp) = &flatpak {
        let installed = InfoRow::new(tr("Installed Size"), tr("Calculating…"));
        more.add(&installed.row);
        let (s, fp2) = (installed.clone(), fp.clone());
        spawn_worker(
            move || fp2.deploy_dir().map(|d| defaults::dir_size(&d)),
            move |n| match n {
                Some(n) => s.set(&defaults::size_label(n)),
                None => s.set(tr("Unknown")),
            },
        );
        let storage = InfoRow::new(tr("App Data"), tr("Calculating…"));
        more.add(&storage.row);
        let dir = glib::home_dir().join(".var/app").join(&app);
        let s = storage.clone();
        spawn_worker(
            move || defaults::dir_size(&dir),
            move |n| s.set(&defaults::size_label(n)),
        );
        let (row, button) = widgets::action_row(
            tr("Uninstall"),
            tr("Removes the app; its data stays"),
            tr("Uninstall…"),
            None,
        );
        button.add_css_class("destructive-action");
        let (ctx, fp, name) = (Rc::downgrade(&state.ctx), fp.clone(), info.display_name());
        button.connect_clicked(move |b| {
            let Some(ctx) = ctx.upgrade() else { return };
            let dialog = adw::AlertDialog::new(
                Some(&trf!("Uninstall {name}?", name = name)),
                Some(tr(
                    "The app is removed from this computer. Its data and settings are kept.",
                )),
            );
            dialog.add_response("cancel", tr("_Cancel"));
            dialog.add_response("uninstall", tr("_Uninstall"));
            dialog.set_response_appearance("uninstall", adw::ResponseAppearance::Destructive);
            dialog.set_default_response(Some("cancel"));
            dialog.set_close_response("cancel");
            let (c, fp, b) = (Rc::downgrade(&ctx), fp.clone(), b.clone());
            dialog.connect_response(Some("uninstall"), move |_, _| {
                b.set_sensitive(false);
                let (c, cmd, b) = (c.clone(), fp.uninstall_command(), b.clone());
                spawn_worker(
                    move || {
                        std::process::Command::new(&cmd[0])
                            .args(&cmd[1..])
                            .stdin(std::process::Stdio::null())
                            .output()
                    },
                    move |r| {
                        let Some(ctx) = c.upgrade() else { return };
                        match r {
                            Ok(o) if o.status.success() => ctx.toast(tr("App uninstalled")),
                            Ok(o) => {
                                b.set_sensitive(true);
                                ctx.toast(&trf!(
                                    "Could not uninstall: {e}",
                                    e = helix_app_common::errors::detail(
                                        String::from_utf8_lossy(&o.stderr).trim()
                                    )
                                ));
                            }
                            Err(e) => {
                                b.set_sensitive(true);
                                ctx.toast(&trf!(
                                    "Could not uninstall: {e}",
                                    e = helix_app_common::errors::io_detail(&e)
                                ));
                            }
                        }
                    },
                );
            });
            dialog.present(Some(&ctx.window));
        });
        more.add(&row);
    }
    let (software_row, software) = widgets::action_row(
        tr("Open in Software"),
        tr("Details, updates and uninstalling"),
        tr("Open"),
        None,
    );
    let (ctx, id) = (Rc::downgrade(&state.ctx), app.clone());
    software.connect_clicked(move |_| {
        let Some(ctx) = ctx.upgrade() else { return };
        let launcher = gtk::UriLauncher::new(&format!("appstream://{id}"));
        let c = Rc::downgrade(&ctx);
        launcher.launch(Some(&ctx.window), gio::Cancellable::NONE, move |r| {
            if r.is_err() {
                if let Some(ctx) = c.upgrade() {
                    ctx.toast(tr("No software center is installed"));
                }
            }
        });
    });
    more.add(&software_row);
    let (reset_row, reset) = widgets::action_row(
        tr("Reset Permissions"),
        tr("The app asks again next time"),
        tr("Reset"),
        None,
    );
    let (weak, id2) = (Rc::downgrade(state), desktop_id.clone());
    reset.connect_clicked(move |_| {
        let (weak, app, id) = (
            weak.clone(),
            permissions::app_id(&id2).to_string(),
            id2.clone(),
        );
        spawn_worker(
            move || bus::session().and_then(|c| permissions::reset(&c, &app)),
            move |r| {
                let Some(state) = weak.upgrade() else { return };
                if let Err(e) = r {
                    state
                        .ctx
                        .toast(&trf!("Could not reset the permissions: {e}", e = e));
                }
                load_permissions(&state, &id);
            },
        );
    });
    more.add(&reset_row);
    page.add(&more);

    let dialog = widgets::dialog(&info.display_name(), &page);
    dialog.set_content_height(600);
    let weak = Rc::downgrade(state);
    dialog.connect_closed(move |_| {
        if let Some(state) = weak.upgrade() {
            state.open.borrow_mut().take();
        }
    });
    *state.open.borrow_mut() = Some((desktop_id.clone(), perms, RefCell::default()));
    load_permissions(state, &desktop_id);
    dialog.present(Some(&state.ctx.window));
}
