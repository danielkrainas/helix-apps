//! GTK user interface: header bar, sidebar outline and WebKit view.

use crate::library::Library;
use crate::textsize;
use crate::uri::{self, DocRef, Location};
use adw::prelude::*;
use gtk::{gio, glib};
use helix_app_common::Prefs;
use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use webkit6::prelude::*;

const APP_ID: &str = "org.helix.Help";
const SCHEMA: &str = "org.gnome.yelp";
const SCHEMES: &[&str] = &["help", "ghelp", "help-list"];

type UiAction = Box<dyn Fn(&Rc<Ui>)>;

struct Ui {
    window: adw::ApplicationWindow,
    view: webkit6::WebView,
    split: adw::OverlaySplitView,
    sidebar_list: gtk::ListBox,
    search_bar: gtk::SearchBar,
    search_entry: gtk::SearchEntry,
    back: gtk::Button,
    forward: gtk::Button,
    library: Arc<Library>,
    prefs: Prefs,
    cwd: PathBuf,
    /// Document shown in the sidebar, and its rows' URIs.
    sidebar_doc: RefCell<Option<DocRef>>,
    sidebar_uris: RefCell<Vec<String>>,
}

pub fn run(initial: Option<String>) -> glib::ExitCode {
    let app = adw::Application::builder().application_id(APP_ID).flags(gio::ApplicationFlags::NON_UNIQUE).build();
    helix_app_common::install_standard_actions(&app, "Help", "View help documentation");
    let library = Arc::new(Library::from_env());
    let initial = RefCell::new(initial);
    let context_slot: Rc<RefCell<Option<webkit6::WebContext>>> = Rc::default();
    // Checked once, before any WebView exists: WebKit aborts the whole
    // process when its sandbox cannot start.
    let sandbox: RefCell<Option<Option<crate::sandbox::Cause>>> = RefCell::default();
    app.connect_activate(move |app| {
        let blocked = *sandbox.borrow_mut().get_or_insert_with(crate::sandbox::check);
        if let Some(cause) = blocked {
            match app.windows().first() {
                Some(w) => w.present(),
                None => {
                    eprintln!("helix-help: WebKit sandbox unavailable ({cause:?}); not showing help pages");
                    crate::sandbox::failure_window(app, "Help", "helix-help", cause).present();
                }
            }
            return;
        }
        let context = context_slot
            .borrow_mut()
            .get_or_insert_with(|| make_context(library.clone()))
            .clone();
        let ui = build(app, &context, library.clone());
        let target = initial.borrow_mut().take();
        ui.window.present();
        let start = match target {
            Some(t) => t,
            None if library.resolve(&DocRef::Id("gnome-help".into())).is_some() => "help:gnome-help".into(),
            None => "help:".into(),
        };
        navigate(&ui, &start);
    });
    app.run_with_args(&[std::env::args().next().unwrap_or_default()])
}

/// A WebContext with the help URI schemes registered. Requests are
/// answered on a worker thread.
fn make_context(library: Arc<Library>) -> webkit6::WebContext {
    let context = webkit6::WebContext::new();
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/"));
    for scheme in SCHEMES {
        if let Some(sm) = context.security_manager() {
            // Lets help pages link to file:// HTML help.
            sm.register_uri_scheme_as_local(scheme);
            sm.register_uri_scheme_as_secure(scheme);
        }
        let library = library.clone();
        let cwd = cwd.clone();
        context.register_uri_scheme(scheme, move |request| {
            let uri = request.uri().map(|u| u.to_string()).unwrap_or_default();
            let loc = uri::parse(&uri, &cwd);
            let library = library.clone();
            let request = request.clone();
            glib::spawn_future_local(async move {
                let response = gio::spawn_blocking(move || library.respond(&loc)).await;
                match response {
                    Ok(r) => {
                        let len = r.body.len() as i64;
                        let stream = gio::MemoryInputStream::from_bytes(&glib::Bytes::from_owned(r.body));
                        let mime = if r.mime.starts_with("text/") { format!("{}; charset=utf-8", r.mime) } else { r.mime };
                        let resp = webkit6::URISchemeResponse::new(&stream, len);
                        resp.set_content_type(&mime);
                        request.finish_with_response(&resp);
                    }
                    Err(_) => {
                        let mut err = glib::Error::new(gio::IOErrorEnum::Failed, "help page rendering failed");
                        request.finish_error(&mut err);
                    }
                }
            });
        });
    }
    context
}

fn is_help_uri(u: &str) -> bool {
    let scheme = u.split_once(':').map(|(s, _)| s.to_ascii_lowercase()).unwrap_or_default();
    SCHEMES.contains(&scheme.as_str())
}

/// Turns any user/page-supplied target into what the WebView should load,
/// or `Err(uri)` for things handed to the system (http, mailto…).
fn resolve_target(ui: &Ui, target: &str) -> Result<String, String> {
    match uri::parse(target, &ui.cwd) {
        Location::External(u) if u.starts_with("file:") => Ok(u),
        Location::External(u) => Err(u),
        Location::Home { search: None } => Ok("help:".into()),
        Location::Home { search: Some(q) } => Ok(uri::home_search_uri(&q)),
        Location::Doc { doc, page, search, fragment } => {
            if let Some(file) = ui.library.html_entry(&doc) {
                return Ok(file);
            }
            Ok(match search {
                Some(q) => doc.search_uri(&q),
                None => doc.page_uri(page.as_deref(), fragment.as_deref()),
            })
        }
        Location::Resource { doc, path } => Ok(doc.resource_uri(&path)),
    }
}

fn navigate(ui: &Ui, target: &str) {
    match resolve_target(ui, target) {
        Ok(u) => ui.view.load_uri(&u),
        Err(u) => launch_external(&ui.window, &u),
    }
}

fn launch_external(window: &adw::ApplicationWindow, uri: &str) {
    gtk::UriLauncher::new(uri).launch(Some(window), gio::Cancellable::NONE, |r| {
        if let Err(e) = r {
            eprintln!("helix-help: could not open link: {e}");
        }
    });
}

/// The document a WebView URI belongs to.
fn current_doc(ui: &Ui) -> Option<DocRef> {
    let u = ui.view.uri()?;
    match uri::parse(&u, &ui.cwd) {
        Location::Doc { doc, .. } | Location::Resource { doc, .. } => Some(doc),
        _ => None,
    }
}

/// Applies `font-adjustment` (a point offset) to the WebKit settings.
fn apply_text_size(settings: &webkit6::Settings, adjust: i32) {
    let (body, mono) = textsize::font_px(adjust);
    settings.set_default_font_size(body);
    settings.set_default_monospace_font_size(mono);
}

fn build(app: &adw::Application, context: &webkit6::WebContext, library: Arc<Library>) -> Rc<Ui> {
    let prefs = Prefs::new(SCHEMA);
    let settings = webkit6::Settings::new();
    settings.set_enable_developer_extras(false);
    settings.set_enable_caret_browsing(prefs.get("show-cursor", false));
    settings.set_javascript_can_open_windows_automatically(false);
    apply_text_size(&settings, prefs.get("font-adjustment", 0i32));
    let view = webkit6::WebView::builder().web_context(context).settings(&settings).build();
    view.set_vexpand(true);
    view.set_hexpand(true);

    // Shown over the view if its web process dies.
    let crash_reload = gtk::Button::builder().label("_Reload").use_underline(true).halign(gtk::Align::Center).build();
    crash_reload.add_css_class("pill");
    crash_reload.add_css_class("suggested-action");
    let crash_page = crate::sandbox::crashed_page(webkit6::WebProcessTerminationReason::Crashed);
    crash_page.set_child(Some(&crash_reload));
    crash_page.add_css_class("view");
    crash_page.set_visible(false);
    let view_overlay = gtk::Overlay::builder().child(&view).build();
    view_overlay.add_overlay(&crash_page);
    view.connect_web_process_terminated(glib::clone!(
        #[weak]
        crash_page,
        move |_, reason| {
            if reason != webkit6::WebProcessTerminationReason::TerminatedByApi {
                let page = crate::sandbox::crashed_page(reason);
                crash_page.set_description(page.description().as_deref());
                crash_page.set_visible(true);
            }
        }
    ));
    crash_reload.connect_clicked(glib::clone!(
        #[weak]
        view,
        #[weak]
        crash_page,
        move |_| {
            crash_page.set_visible(false);
            view.reload();
        }
    ));

    let back = gtk::Button::builder().icon_name("go-previous-symbolic").tooltip_text("Back").action_name("win.back").build();
    let forward =
        gtk::Button::builder().icon_name("go-next-symbolic").tooltip_text("Forward").action_name("win.forward").build();
    let home = gtk::Button::builder().icon_name("go-home-symbolic").tooltip_text("All Help").action_name("win.home").build();
    let sidebar_btn = gtk::ToggleButton::builder()
        .icon_name("sidebar-show-symbolic")
        .tooltip_text("Show Sidebar")
        .action_name("win.sidebar")
        .build();
    let search_btn = gtk::ToggleButton::builder().icon_name("system-search-symbolic").tooltip_text("Search").build();

    let menu = gio::Menu::new();
    let sec1 = gio::Menu::new();
    sec1.append(Some("_Print…"), Some("win.print"));
    menu.append_section(None, &sec1);
    let zoom = gio::Menu::new();
    zoom.append(Some("_Larger Text"), Some("win.zoom-in"));
    zoom.append(Some("_Smaller Text"), Some("win.zoom-out"));
    zoom.append(Some("_Normal Size"), Some("win.zoom-reset"));
    menu.append_section(None, &zoom);
    let sec3 = gio::Menu::new();
    sec3.append(Some("_Caret Browsing"), Some("win.caret"));
    sec3.append(Some("_Keyboard Shortcuts"), Some("win.show-help-overlay"));
    sec3.append(Some("_About Help"), Some("app.about"));
    menu.append_section(None, &sec3);
    let menu_btn = gtk::MenuButton::builder().icon_name("open-menu-symbolic").menu_model(&menu).tooltip_text("Main Menu").build();

    let header = adw::HeaderBar::new();
    header.pack_start(&back);
    header.pack_start(&forward);
    header.pack_start(&home);
    header.pack_start(&sidebar_btn);
    header.pack_end(&menu_btn);
    header.pack_end(&search_btn);
    let title = adw::WindowTitle::new("Help", "");
    header.set_title_widget(Some(&title));

    let search_entry = gtk::SearchEntry::builder().placeholder_text("Search help").hexpand(true).build();
    let clamp = adw::Clamp::builder().maximum_size(560).child(&search_entry).build();
    let search_bar = gtk::SearchBar::builder().child(&clamp).build();
    search_bar.connect_entry(&search_entry);
    search_btn.bind_property("active", &search_bar, "search-mode-enabled").bidirectional().sync_create().build();

    let sidebar_list = gtk::ListBox::new();
    sidebar_list.add_css_class("navigation-sidebar");
    let sidebar_scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .child(&sidebar_list)
        .vexpand(true)
        .build();
    let sidebar_header = adw::HeaderBar::builder().show_end_title_buttons(false).build();
    sidebar_header.set_title_widget(Some(&adw::WindowTitle::new("Contents", "")));
    let sidebar_view = adw::ToolbarView::new();
    sidebar_view.add_top_bar(&sidebar_header);
    sidebar_view.set_content(Some(&sidebar_scroll));

    let content = adw::ToolbarView::new();
    content.add_top_bar(&header);
    content.add_top_bar(&search_bar);
    content.set_content(Some(&view_overlay));

    let split = adw::OverlaySplitView::builder()
        .sidebar(&sidebar_view)
        .content(&content)
        .show_sidebar(false)
        .min_sidebar_width(220.0)
        .build();

    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("Help")
        .default_width(900)
        .default_height(700)
        .content(&split)
        .build();
    let bp = adw::Breakpoint::new(adw::BreakpointCondition::new_length(
        adw::BreakpointConditionLengthType::MaxWidth,
        600.0,
        adw::LengthUnit::Sp,
    ));
    bp.add_setter(&split, "collapsed", Some(&true.to_value()));
    window.add_breakpoint(bp);

    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/"));
    let ui = Rc::new(Ui {
        window: window.clone(),
        view: view.clone(),
        split,
        sidebar_list,
        search_bar,
        search_entry: search_entry.clone(),
        back,
        forward,
        library,
        prefs,
        cwd,
        sidebar_doc: RefCell::default(),
        sidebar_uris: RefCell::default(),
    });

    install_actions(&ui);
    install_shortcuts_window(&ui);

    // Title follows the page.
    view.connect_title_notify(glib::clone!(
        #[weak]
        title,
        #[weak]
        window,
        move |v| {
            let t = v.title().map(|t| t.to_string()).filter(|t| !t.is_empty()).unwrap_or_else(|| "Help".into());
            title.set_title(&t);
            window.set_title(Some(&t));
        }
    ));
    {
        let ui2 = Rc::downgrade(&ui);
        view.connect_uri_notify(move |_| {
            if let Some(ui) = ui2.upgrade() {
                update_nav(&ui);
                update_sidebar(&ui);
            }
        });
    }
    {
        let ui2 = Rc::downgrade(&ui);
        view.connect_load_changed(move |_, _| {
            if let Some(ui) = ui2.upgrade() {
                update_nav(&ui);
            }
        });
    }
    {
        let ui2 = Rc::downgrade(&ui);
        view.connect_decide_policy(move |_, decision, ty| {
            let Some(ui) = ui2.upgrade() else { return false };
            decide_policy(&ui, decision, ty)
        });
    }
    {
        let ui2 = Rc::downgrade(&ui);
        search_entry.connect_activate(move |e| {
            let Some(ui) = ui2.upgrade() else { return };
            let q = e.text().trim().to_string();
            if q.is_empty() {
                return;
            }
            let target = match current_doc(&ui) {
                Some(doc) => doc.search_uri(&q),
                None => uri::home_search_uri(&q),
            };
            ui.view.load_uri(&target);
        });
    }
    {
        let ui2 = Rc::downgrade(&ui);
        ui.sidebar_list.connect_row_activated(move |_, row| {
            let Some(ui) = ui2.upgrade() else { return };
            let idx = row.index();
            let uri = ui.sidebar_uris.borrow().get(idx as usize).cloned();
            if let Some(u) = uri {
                ui.view.load_uri(&u);
                if ui.split.is_collapsed() {
                    ui.split.set_show_sidebar(false);
                }
            }
        });
    }
    // Typing anywhere starts a search.
    ui.search_bar.set_key_capture_widget(Some(&ui.window));
    ui
}

fn decide_policy(ui: &Ui, decision: &webkit6::PolicyDecision, ty: webkit6::PolicyDecisionType) -> bool {
    use webkit6::PolicyDecisionType as T;
    if !matches!(ty, T::NavigationAction | T::NewWindowAction) {
        return false;
    }
    let Some(nav) = decision.downcast_ref::<webkit6::NavigationPolicyDecision>() else { return false };
    let Some(mut action) = nav.navigation_action() else { return false };
    let Some(target) = action.request().and_then(|r| r.uri()).map(|u| u.to_string()) else { return false };
    let scheme = target.split_once(':').map(|(s, _)| s.to_ascii_lowercase()).unwrap_or_default();
    match scheme.as_str() {
        "about" | "data" | "blob" => false,
        _ if is_help_uri(&target) => {
            // Plain HTML help is shown straight from disk.
            if let Ok(resolved) = resolve_target(ui, &target) {
                if resolved.starts_with("file:") {
                    decision.ignore();
                    ui.view.load_uri(&resolved);
                    return true;
                }
            }
            if ty == T::NewWindowAction {
                decision.ignore();
                ui.view.load_uri(&target);
                return true;
            }
            false
        }
        "file" => {
            // Mallard/DocBook files linked from HTML help are transformed.
            match resolve_target(ui, &target) {
                Ok(r) if !r.starts_with("file:") => {
                    decision.ignore();
                    ui.view.load_uri(&r);
                    true
                }
                _ if ty == T::NewWindowAction => {
                    decision.ignore();
                    ui.view.load_uri(&target);
                    true
                }
                _ => false,
            }
        }
        _ => {
            decision.ignore();
            launch_external(&ui.window, &target);
            true
        }
    }
}

fn update_nav(ui: &Ui) {
    ui.back.set_sensitive(ui.view.can_go_back());
    ui.forward.set_sensitive(ui.view.can_go_forward());
    if let Some(a) = ui.window.lookup_action("back").and_downcast::<gio::SimpleAction>() {
        a.set_enabled(ui.view.can_go_back());
    }
    if let Some(a) = ui.window.lookup_action("forward").and_downcast::<gio::SimpleAction>() {
        a.set_enabled(ui.view.can_go_forward());
    }
}

/// Refreshes the sidebar outline when the current document changes and
/// selects the row for the current page.
fn update_sidebar(ui: &Rc<Ui>) {
    let doc = current_doc(ui);
    if *ui.sidebar_doc.borrow() != doc {
        *ui.sidebar_doc.borrow_mut() = doc.clone();
        while let Some(c) = ui.sidebar_list.first_child() {
            ui.sidebar_list.remove(&c);
        }
        ui.sidebar_uris.borrow_mut().clear();
        let Some(doc) = doc else { return };
        let library = ui.library.clone();
        let weak = Rc::downgrade(ui);
        glib::spawn_future_local(async move {
            let d2 = doc.clone();
            let toc = gio::spawn_blocking(move || library.load(&d2).map(|d| d.toc()).unwrap_or_default())
                .await
                .unwrap_or_default();
            let Some(ui) = weak.upgrade() else { return };
            if ui.sidebar_doc.borrow().as_ref() != Some(&doc) {
                return;
            }
            let mut uris = Vec::new();
            for (depth, page, title) in toc {
                let label = gtk::Label::builder()
                    .label(if title.is_empty() { page.clone().unwrap_or_default() } else { title })
                    .xalign(0.0)
                    .ellipsize(gtk::pango::EllipsizeMode::End)
                    .margin_start(6 + 14 * depth.min(6) as i32)
                    .build();
                if depth == 0 {
                    label.add_css_class("heading");
                }
                ui.sidebar_list.append(&label);
                uris.push(doc.page_uri(page.as_deref(), None));
            }
            *ui.sidebar_uris.borrow_mut() = uris;
            select_sidebar_row(&ui);
        });
    } else {
        select_sidebar_row(ui);
    }
}

fn select_sidebar_row(ui: &Ui) {
    let Some(cur) = ui.view.uri() else { return };
    let cur = cur.split('#').next().unwrap_or("").to_string();
    let idx = ui.sidebar_uris.borrow().iter().position(|u| *u == cur);
    match idx.and_then(|i| ui.sidebar_list.row_at_index(i as i32)) {
        Some(row) => ui.sidebar_list.select_row(Some(&row)),
        None => ui.sidebar_list.unselect_all(),
    }
}

fn set_text_size(ui: &Ui, adjust: i32) {
    let adjust = textsize::clamp(adjust);
    ui.prefs.set("font-adjustment", adjust);
    if let Some(s) = WebViewExt::settings(&ui.view) {
        apply_text_size(&s, adjust);
    }
}

fn install_actions(ui: &Rc<Ui>) {
    let win = &ui.window;
    let add = |name: &str, accels: &[&str], f: UiAction| {
        let a = gio::SimpleAction::new(name, None);
        let weak = Rc::downgrade(ui);
        a.connect_activate(move |_, _| {
            if let Some(ui) = weak.upgrade() {
                f(&ui);
            }
        });
        win.add_action(&a);
        if let Some(app) = win.application() {
            app.set_accels_for_action(&format!("win.{name}"), accels);
        }
    };
    add("back", &["<Alt>Left", "Back"], Box::new(|ui| ui.view.go_back()));
    add("forward", &["<Alt>Right", "Forward"], Box::new(|ui| ui.view.go_forward()));
    add("home", &["<Alt>Home"], Box::new(|ui| ui.view.load_uri("help:")));
    add(
        "search",
        &["<Control>f"],
        Box::new(|ui| {
            ui.search_bar.set_search_mode(true);
            ui.search_entry.grab_focus();
        }),
    );
    add(
        "print",
        &["<Control>p"],
        Box::new(|ui| {
            let op = webkit6::PrintOperation::new(&ui.view);
            op.run_dialog(Some(&ui.window));
        }),
    );
    add(
        "zoom-in",
        &["<Control>plus", "<Control>equal", "<Control>KP_Add"],
        Box::new(|ui| set_text_size(ui, ui.prefs.get("font-adjustment", 0i32) + 1)),
    );
    add(
        "zoom-out",
        &["<Control>minus", "<Control>KP_Subtract"],
        Box::new(|ui| set_text_size(ui, ui.prefs.get("font-adjustment", 0i32) - 1)),
    );
    add("zoom-reset", &["<Control>0", "<Control>KP_0"], Box::new(|ui| set_text_size(ui, 0)));
    add("close", &["<Control>w"], Box::new(|ui| ui.window.close()));

    let sidebar = gio::SimpleAction::new_stateful("sidebar", None, &false.to_variant());
    {
        let weak = Rc::downgrade(ui);
        sidebar.connect_activate(move |a, _| {
            let Some(ui) = weak.upgrade() else { return };
            let on = !a.state().and_then(|s| s.get::<bool>()).unwrap_or(false);
            a.set_state(&on.to_variant());
            ui.split.set_show_sidebar(on);
        });
    }
    {
        let s = sidebar.clone();
        ui.split.connect_show_sidebar_notify(move |sp| s.set_state(&sp.shows_sidebar().to_variant()));
    }
    win.add_action(&sidebar);

    let caret = gio::SimpleAction::new_stateful("caret", None, &ui.prefs.get("show-cursor", false).to_variant());
    {
        let weak = Rc::downgrade(ui);
        caret.connect_activate(move |a, _| {
            let Some(ui) = weak.upgrade() else { return };
            let on = !a.state().and_then(|s| s.get::<bool>()).unwrap_or(false);
            a.set_state(&on.to_variant());
            ui.prefs.set("show-cursor", on);
            if let Some(s) = WebViewExt::settings(&ui.view) {
                s.set_enable_caret_browsing(on);
            }
        });
    }
    win.add_action(&caret);
    if let Some(app) = win.application() {
        app.set_accels_for_action("win.sidebar", &["F9"]);
        app.set_accels_for_action("win.caret", &["F7"]);
    }
    update_nav(ui);
}

fn install_shortcuts_window(ui: &Ui) {
    let rows: &[(&str, &str)] = &[
        ("<Alt>Left", "Back"),
        ("<Alt>Right", "Forward"),
        ("<Alt>Home", "All help documents"),
        ("<Control>f", "Search"),
        ("F9", "Toggle sidebar"),
        ("<Control>p", "Print"),
        ("<Control>plus", "Larger text"),
        ("<Control>minus", "Smaller text"),
        ("<Control>0", "Normal text size"),
        ("F7", "Caret browsing"),
        ("<Control>w", "Close window"),
        ("<Control>q", "Quit"),
    ];
    let mut xml = String::from(
        "<interface><object class=\"GtkShortcutsWindow\" id=\"w\"><property name=\"modal\">1</property>\
         <child><object class=\"GtkShortcutsSection\"><child><object class=\"GtkShortcutsGroup\">\
         <property name=\"title\">General</property>",
    );
    for (accel, title) in rows {
        xml.push_str(&format!(
            "<child><object class=\"GtkShortcutsShortcut\"><property name=\"accelerator\">{}</property>\
             <property name=\"title\">{}</property></object></child>",
            crate::xmlutil::escape(accel),
            crate::xmlutil::escape(title)
        ));
    }
    xml.push_str("</object></child></object></child></object></interface>");
    let builder = gtk::Builder::from_string(&xml);
    if let Some(w) = builder.object::<gtk::ShortcutsWindow>("w") {
        ui.window.set_help_overlay(Some(&w));
    }
    if let Some(app) = ui.window.application() {
        app.set_accels_for_action("win.show-help-overlay", &["<Control>question"]);
    }
}
