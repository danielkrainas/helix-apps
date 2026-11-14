//! GTK user interface: window, sidebar, search and the panel registry.

mod about;
mod access;
mod appearance;
mod datetime;
mod extensions;
mod input;
mod multitasking;
mod notifications;
mod power;
mod privacy;
mod widgets;

use adw::prelude::*;
use gtk::{gio, glib};
use helix_app_common::Prefs;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use crate::panels::{self, PanelId};

pub const APP_ID: &str = "org.helix.Settings";
const DEFAULT_PANEL: PanelId = PanelId::Appearance;

/// A settings panel implementation.
pub trait Panel {
    /// Builds the panel's content (usually an `AdwPreferencesPage`).
    fn build(&self, ctx: &Rc<Ctx>) -> gtk::Widget;
}

/// Implementation registry: `None` for panels that are not available yet.
fn panel_impl(id: PanelId) -> Option<&'static dyn Panel> {
    Some(match id {
        PanelId::Appearance => &appearance::AppearancePanel,
        PanelId::Mouse => &input::MousePanel,
        PanelId::Keyboard => &input::KeyboardPanel,
        PanelId::Accessibility => &access::AccessibilityPanel,
        PanelId::Privacy => &privacy::PrivacyPanel,
        PanelId::Notifications => &notifications::NotificationsPanel,
        PanelId::Extensions => &extensions::ExtensionsPanel,
        PanelId::Multitasking => &multitasking::MultitaskingPanel,
        PanelId::Power => &power::PowerPanel,
        PanelId::DateTime => &datetime::DateTimePanel,
        PanelId::About => &about::AboutPanel,
        PanelId::Wifi
        | PanelId::Network
        | PanelId::Bluetooth
        | PanelId::Display
        | PanelId::Sound
        | PanelId::Apps
        | PanelId::Users => return None,
    })
}

/// Shared state handed to panels.
pub struct Ctx {
    pub window: adw::ApplicationWindow,
    toasts: adw::ToastOverlay,
    prefs: RefCell<HashMap<&'static str, Prefs>>,
}

impl Ctx {
    /// Settings for `schema`, shared by every panel (so change handlers stay alive).
    pub fn prefs(&self, schema: &'static str) -> Prefs {
        self.prefs
            .borrow_mut()
            .entry(schema)
            .or_insert_with(|| Prefs::new(schema))
            .clone()
    }

    pub fn toast(&self, message: &str) {
        self.toasts
            .add_toast(adw::Toast::new(&glib::markup_escape_text(message)));
    }
}

/// Runs `work` on a worker thread and `done` with its result on the GTK thread.
pub fn spawn_worker<T: Send + 'static>(
    work: impl FnOnce() -> T + Send + 'static,
    done: impl FnOnce(T) + 'static,
) {
    let (tx, rx) = async_channel::bounded(1);
    std::thread::spawn(move || {
        let _ = tx.send_blocking(work());
    });
    glib::spawn_future_local(async move {
        if let Ok(v) = rx.recv().await {
            done(v);
        }
    });
}

struct Window {
    ctx: Rc<Ctx>,
    split: adw::NavigationSplitView,
    list: gtk::ListBox,
    rows: Vec<(PanelId, gtk::ListBoxRow)>,
    pages: RefCell<HashMap<PanelId, adw::NavigationPage>>,
    searching: RefCell<bool>,
}

impl Window {
    fn new(app: &adw::Application) -> Rc<Self> {
        let window = adw::ApplicationWindow::builder()
            .application(app)
            .title("Settings")
            .default_width(980)
            .default_height(680)
            .width_request(360)
            .height_request(294)
            .build();
        let toasts = adw::ToastOverlay::new();
        let split = adw::NavigationSplitView::builder()
            .min_sidebar_width(220.0)
            .max_sidebar_width(280.0)
            .build();

        // Sidebar.
        let search_entry = gtk::SearchEntry::builder()
            .placeholder_text("Search settings")
            .hexpand(true)
            .build();
        let search_bar = gtk::SearchBar::builder()
            .child(&search_entry)
            .show_close_button(false)
            .build();
        search_bar.connect_entry(&search_entry);
        let search_button = gtk::ToggleButton::builder()
            .icon_name("system-search-symbolic")
            .tooltip_text("Search")
            .build();
        search_button
            .bind_property("active", &search_bar, "search-mode-enabled")
            .bidirectional()
            .build();

        let menu = gio::Menu::new();
        menu.append(Some("_About Settings"), Some("app.about"));
        let menu_button = gtk::MenuButton::builder()
            .icon_name("open-menu-symbolic")
            .menu_model(&menu)
            .primary(true)
            .tooltip_text("Main Menu")
            .build();
        let header = adw::HeaderBar::new();
        header.pack_start(&search_button);
        header.pack_end(&menu_button);

        let list = gtk::ListBox::builder()
            .css_classes(["navigation-sidebar"])
            .build();
        let mut rows = Vec::new();
        for p in panels::PANELS {
            let row = Self::make_row(p);
            list.append(&row);
            rows.push((p.id, row));
        }
        let scrolled = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .child(&list)
            .build();
        let sidebar_view = adw::ToolbarView::new();
        sidebar_view.add_top_bar(&header);
        sidebar_view.add_top_bar(&search_bar);
        sidebar_view.set_content(Some(&scrolled));
        split.set_sidebar(Some(&adw::NavigationPage::new(&sidebar_view, "Settings")));

        toasts.set_child(Some(&split));
        window.set_content(Some(&toasts));

        let bp = adw::Breakpoint::new(adw::BreakpointCondition::new_length(
            adw::BreakpointConditionLengthType::MaxWidth,
            700.0,
            adw::LengthUnit::Sp,
        ));
        bp.add_setter(&split, "collapsed", Some(&true.to_value()));
        window.add_breakpoint(bp);

        let ctx = Rc::new(Ctx {
            window: window.clone(),
            toasts,
            prefs: RefCell::default(),
        });
        let this = Rc::new(Window {
            ctx,
            split,
            list: list.clone(),
            rows,
            pages: RefCell::default(),
            searching: RefCell::new(false),
        });

        list.set_header_func(glib::clone!(
            #[weak(rename_to = this)]
            this,
            move |row, before| this.update_header(row, before)
        ));
        list.connect_row_activated(glib::clone!(
            #[weak(rename_to = this)]
            this,
            move |_, row| {
                if let Some(id) = this.id_of(row) {
                    this.show_panel(id);
                }
            }
        ));
        search_entry.connect_search_changed(glib::clone!(
            #[weak(rename_to = this)]
            this,
            move |e| this.filter(&e.text())
        ));
        search_entry.connect_activate(glib::clone!(
            #[weak(rename_to = this)]
            this,
            move |_| {
                if let Some(row) = this.list.row_at_index(0) {
                    row.activate();
                }
            }
        ));
        search_entry.connect_stop_search(glib::clone!(
            #[weak]
            search_bar,
            move |e| {
                e.set_text("");
                search_bar.set_search_mode(false);
            }
        ));
        search_bar.connect_search_mode_enabled_notify(glib::clone!(
            #[weak]
            search_entry,
            move |bar| {
                if bar.is_search_mode() {
                    search_entry.grab_focus();
                } else {
                    search_entry.set_text("");
                }
            }
        ));
        search_bar.set_key_capture_widget(Some(&window));

        let search_action = gio::ActionEntry::builder("search")
            .activate(move |_: &adw::ApplicationWindow, _, _| search_bar.set_search_mode(true))
            .build();
        window.add_action_entries([search_action]);
        app.set_accels_for_action("win.search", &["<Control>f"]);
        this
    }

    fn make_row(p: &panels::PanelInfo) -> gtk::ListBoxRow {
        let bx = gtk::Box::builder()
            .spacing(12)
            .margin_top(6)
            .margin_bottom(6)
            .margin_start(6)
            .margin_end(6)
            .build();
        bx.append(&gtk::Image::from_icon_name(p.icon));
        let labels = gtk::Box::new(gtk::Orientation::Vertical, 0);
        labels.set_valign(gtk::Align::Center);
        labels.append(&gtk::Label::builder().label(p.title).xalign(0.0).build());
        if !p.available {
            labels.append(
                &gtk::Label::builder()
                    .label("Not yet available")
                    .xalign(0.0)
                    .css_classes(["caption"])
                    .build(),
            );
            bx.add_css_class("dim-label");
        }
        bx.append(&labels);
        let row = gtk::ListBoxRow::builder().child(&bx).build();
        row.set_widget_name(p.name);
        row.update_property(&[gtk::accessible::Property::Label(p.title)]);
        row
    }

    fn id_of(&self, row: &gtk::ListBoxRow) -> Option<PanelId> {
        self.rows.iter().find(|(_, r)| r == row).map(|(id, _)| *id)
    }

    fn update_header(&self, row: &gtk::ListBoxRow, before: Option<&gtk::ListBoxRow>) {
        let group = |r: &gtk::ListBoxRow| self.id_of(r).map(|id| panels::info(id).group);
        let needs = !*self.searching.borrow() && before.is_some_and(|b| group(b) != group(row));
        if needs {
            if row.header().is_none() {
                row.set_header(Some(&gtk::Separator::new(gtk::Orientation::Horizontal)));
            }
        } else {
            row.set_header(None::<&gtk::Widget>);
        }
    }

    fn filter(&self, query: &str) {
        *self.searching.borrow_mut() = !query.trim().is_empty();
        let order = panels::search(query);
        for (_, row) in &self.rows {
            if row.parent().is_some() {
                self.list.remove(row);
            }
        }
        for id in order {
            if let Some((_, row)) = self.rows.iter().find(|(i, _)| *i == id) {
                self.list.append(row);
            }
        }
        self.list.invalidate_headers();
    }

    fn show_panel(&self, id: PanelId) {
        let page = self.pages.borrow().get(&id).cloned();
        let page = page.unwrap_or_else(|| {
            let page = self.build_page(id);
            self.pages.borrow_mut().insert(id, page.clone());
            page
        });
        self.split.set_content(Some(&page));
        self.split.set_show_content(true);
        if let Some((_, row)) = self.rows.iter().find(|(i, _)| *i == id) {
            if row.parent().is_some() {
                self.list.select_row(Some(row));
            }
        }
    }

    fn build_page(&self, id: PanelId) -> adw::NavigationPage {
        let info = panels::info(id);
        let view = adw::ToolbarView::new();
        view.add_top_bar(&adw::HeaderBar::new());
        let content: gtk::Widget = match panel_impl(id) {
            Some(panel) => panel.build(&self.ctx),
            None => adw::StatusPage::builder()
                .icon_name(info.icon)
                .title(format!("{} Is Not Yet Available", info.title))
                .description("This panel will be added in a later version of Helix Settings.")
                .build()
                .upcast(),
        };
        view.set_content(Some(&content));
        adw::NavigationPage::builder()
            .title(info.title)
            .tag(info.name)
            .child(&view)
            .build()
    }
}

pub fn run(args: Vec<String>) -> glib::ExitCode {
    let app = adw::Application::builder()
        .application_id(APP_ID)
        .flags(gio::ApplicationFlags::HANDLES_COMMAND_LINE)
        .build();
    helix_app_common::install_standard_actions(&app, "Settings", "Configure the Helix desktop");
    app.set_accels_for_action("app.about", &["F1"]);
    let window: Rc<RefCell<Option<Rc<Window>>>> = Rc::default();
    app.connect_command_line(move |app, cmd| {
        let panel = cmd
            .arguments()
            .get(1)
            .and_then(|a| a.to_str().map(str::to_string))
            .and_then(|a| panels::lookup(&a));
        let existing = window.borrow().clone();
        let win = match existing {
            Some(w) => {
                if let Some(id) = panel {
                    w.show_panel(id);
                }
                w
            }
            None => {
                let w = Window::new(app);
                w.show_panel(panel.unwrap_or(DEFAULT_PANEL));
                *window.borrow_mut() = Some(w.clone());
                w
            }
        };
        win.ctx.window.present();
        0
    });
    app.run_with_args(&args)
}
