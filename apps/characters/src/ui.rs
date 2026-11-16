//! GTK user interface.

use adw::prelude::*;
use gtk::{gio, glib};
use helix_app_common::Labelled;
use helix_app_common::Prefs;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use crate::category::{self, Category};
use crate::charinfo;
use crate::recent;
use crate::search;
use helix_app_common::i18n::tr;
use helix_app_common::trnf;

pub const APP_ID: &str = "org.helixdesktop.Characters";
const SCHEMA: &str = "org.gnome.Characters";

struct App {
    window: adw::ApplicationWindow,
    prefs: Prefs,
    model: gtk::StringList,
    grid: gtk::GridView,
    stack: gtk::Stack,
    title: adw::WindowTitle,
    toasts: adw::ToastOverlay,
    search_bar: gtk::SearchBar,
    search_entry: gtk::SearchEntry,
    split: adw::NavigationSplitView,
    sidebar: gtk::ListBox,
    sidebar_cats: Vec<Category>,
    current: Cell<Category>,
    /// Bumped on every category switch / search so stale async results are dropped.
    generation: Cell<u64>,
    cache: RefCell<HashMap<Category, Rc<Vec<String>>>>,
    /// Set while the sidebar selection is changed from code, so the
    /// selection handler leaves search alone.
    quiet_select: Cell<bool>,
}

/// How long a search may take before its progress indicator shows (so
/// fast searches do not flicker).
const SEARCH_SPINNER_DELAY: std::time::Duration = std::time::Duration::from_millis(150);

pub fn run(initial_search: Option<String>) -> glib::ExitCode {
    let app = adw::Application::builder()
        .application_id(APP_ID)
        .flags(gio::ApplicationFlags::NON_UNIQUE)
        .build();
    helix_app_common::install_standard_actions(
        &app,
        tr("Characters"),
        tr("Character map and emoji picker"),
    );
    let initial = RefCell::new(initial_search);
    app.connect_startup(|_| {
        // Build the search index in the background so the first search is fast.
        std::thread::spawn(|| {
            search::index();
        });
    });
    app.connect_activate(move |gapp| {
        let app = build(gapp);
        if let Some(q) = initial.borrow_mut().take() {
            app.search_bar.set_search_mode(true);
            app.search_entry.set_text(&q);
        }
        app.window.present();
    });

    app.run_with_args(&[std::env::args().next().unwrap_or_default()])
}

fn load_css() {
    let provider = gtk::CssProvider::new();
    provider.load_from_string(
        // Glyph previews are display text: em sizes follow the text size.
        ".char-cell { font-size: 2em; min-width: 56px; min-height: 56px; }
         .char-grid > child { padding: 2px; border-radius: 8px; }
         .char-detail-glyph { font-size: 6.5em; min-height: 120px; }
         .char-variant { font-size: 1.65em; min-width: 44px; min-height: 44px; }",
    );
    if let Some(display) = gtk::gdk::Display::default() {
        gtk::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    }
}

impl App {
    fn recent(&self) -> Vec<String> {
        recent::sanitize(
            self.prefs.get("recent-characters", Vec::<String>::new()),
            self.max_recent(),
        )
    }

    fn max_recent(&self) -> u32 {
        self.prefs.get("max-recent-characters", recent::DEFAULT_MAX)
    }

    fn searching(&self) -> bool {
        self.search_bar.is_search_mode() && !self.search_entry.text().trim().is_empty()
    }

    fn next_generation(&self) -> u64 {
        let g = self.generation.get() + 1;
        self.generation.set(g);
        g
    }

    fn set_items(&self, items: &[String], empty_page: &str) {
        let refs: Vec<&str> = items.iter().map(String::as_str).collect();
        self.model.splice(0, self.model.n_items(), &refs);
        if items.is_empty() {
            self.stack.set_visible_child_name(empty_page);
        } else {
            self.stack.set_visible_child_name("grid");
            self.grid.scroll_to(0, gtk::ListScrollFlags::NONE, None);
        }
    }

    fn show_category(self: &Rc<Self>, cat: Category) {
        self.current.set(cat);
        self.title.set_title(cat.label());
        self.title.set_subtitle("");
        let generation = self.next_generation();
        if cat == Category::Recent {
            self.set_items(&self.recent(), "empty-recent");
            return;
        }
        if let Some(items) = self.cache.borrow().get(&cat).cloned() {
            self.set_items(&items, "no-results");
            return;
        }
        self.stack.set_visible_child_name("loading");
        let app = self.clone();
        glib::spawn_future_local(async move {
            let items = gio::spawn_blocking(move || cat.characters())
                .await
                .unwrap_or_default();
            // Emoji newer than the installed emoji font would show as
            // boxes. (The other categories are too large to check here,
            // and their fonts cover far more.)
            let items = if category::EMOJI.contains(&cat) {
                drawable_only(&app.grid, items)
            } else {
                items
            };
            let items = Rc::new(items);
            app.cache.borrow_mut().insert(cat, items.clone());
            if app.generation.get() == generation {
                app.set_items(&items, "no-results");
            }
        });
    }

    fn run_search(self: &Rc<Self>) {
        let query = self.search_entry.text().trim().to_string();
        if query.is_empty() {
            // Back to the category the sidebar shows again.
            self.select_sidebar_quietly(self.current.get());
            self.show_category(self.current.get());
            return;
        }
        self.title.set_title(tr("Search"));
        self.title.set_subtitle(&query);
        // Search results are not a category: nothing in the sidebar is
        // current while they are shown.
        self.sidebar.unselect_all();
        let generation = self.next_generation();
        // Never show the previous category (or results) under the new
        // search title: hide the grid at once, and show progress if the
        // search takes a moment. (The model is replaced when the results
        // come.)
        self.stack.set_visible_child_name("pending");
        {
            let app = self.clone();
            glib::timeout_add_local_once(SEARCH_SPINNER_DELAY, move || {
                if app.generation.get() == generation
                    && app.stack.visible_child_name().as_deref() == Some("pending")
                {
                    app.stack.set_visible_child_name("loading");
                }
            });
        }
        let app = self.clone();
        glib::spawn_future_local(async move {
            let results = gio::spawn_blocking(move || search::index().search(&query))
                .await
                .unwrap_or_default();
            if app.generation.get() == generation {
                app.set_items(&results, "no-results");
                let text = match results.len() {
                    0 => tr("No results").to_string(),
                    n => trnf!("{n} character found", "{n} characters found", n),
                };
                helix_app_common::announce_settled(&app.stack, &text);
            }
        });
    }

    fn copy(self: &Rc<Self>, text: &str) {
        self.window.clipboard().set_text(text);
        let list = recent::push(&self.recent(), text, self.max_recent());
        self.prefs.set("recent-characters", list);
        if self.current.get() == Category::Recent && !self.searching() {
            self.show_category(Category::Recent);
        }
        self.toasts.add_toast(
            adw::Toast::builder()
                .title(tr("Character copied to clipboard"))
                .timeout(2)
                .build(),
        );
    }

    /// Selects `cat` in the sidebar without the selection handler running.
    fn select_sidebar_quietly(&self, cat: Category) {
        self.quiet_select.set(true);
        self.select_sidebar(cat);
        self.quiet_select.set(false);
    }

    fn select_sidebar(&self, cat: Category) {
        if let Some(i) = self.sidebar_cats.iter().position(|c| *c == cat) {
            if let Some(row) = self.sidebar.row_at_index(i as i32) {
                self.sidebar.select_row(Some(&row));
            }
        }
    }
}

/// Keeps the sequences every code point of which some font available to
/// `widget` has a glyph for (joiners, variation selectors and tag
/// characters need none of their own).
fn drawable_only(widget: &impl IsA<gtk::Widget>, items: Vec<String>) -> Vec<String> {
    use gtk::pango::prelude::*;
    let ctx = widget.pango_context();
    let desc = ctx.font_description().unwrap_or_default();
    let Some(fontset) = ctx.load_fontset(&desc, &gtk::pango::Language::default()) else {
        return items;
    };
    let mut fonts = Vec::new();
    fontset.foreach(|_, font| {
        fonts.push(font.clone());
        false
    });
    if fonts.is_empty() {
        return items;
    }
    let mut drawable: std::collections::HashMap<char, bool> = Default::default();
    items
        .into_iter()
        .filter(|s| {
            s.chars().filter(|&c| needs_glyph(c)).all(|c| {
                *drawable
                    .entry(c)
                    .or_insert_with(|| fonts.iter().any(|f| f.has_char(c)))
            })
        })
        .collect()
}

/// Whether `c` is drawn with a glyph of its own inside an emoji sequence.
fn needs_glyph(c: char) -> bool {
    !matches!(
        c,
        '\u{200D}' | '\u{FE0E}' | '\u{FE0F}' | '\u{E0020}'..='\u{E007F}'
    )
}

fn sidebar_row(cat: Category) -> gtk::ListBoxRow {
    let hbox = gtk::Box::builder()
        .spacing(12)
        .margin_start(6)
        .margin_end(6)
        .build();
    let icon = gtk::Image::from_icon_name(cat.icon_name());
    hbox.append(&icon);
    hbox.append(&gtk::Label::builder().label(cat.label()).xalign(0.0).build());
    gtk::ListBoxRow::builder().child(&hbox).build()
}

fn build(gapp: &adw::Application) -> Rc<App> {
    let prefs = Prefs::new(SCHEMA);
    load_css();

    let window = adw::ApplicationWindow::builder()
        .application(gapp)
        .title(tr("Characters"))
        .default_width(860)
        .default_height(620)
        .width_request(360)
        .height_request(294)
        .build();
    // `org.gnome.Characters` has no window keys: the size is app-private.
    helix_app_common::window_state::persist(
        &window,
        helix_app_common::window_state::Store::file("helix-characters"),
    );

    // Sidebar.
    let sidebar_cats: Vec<Category> = std::iter::once(Category::Recent)
        .chain(category::EMOJI)
        .chain(category::LETTERS_AND_SYMBOLS)
        .collect();
    let sidebar = gtk::ListBox::new();
    sidebar.add_css_class("navigation-sidebar");
    for cat in &sidebar_cats {
        sidebar.append(&sidebar_row(*cat));
    }
    let emoji_start = 1;
    let letters_start = 1 + category::EMOJI.len() as i32;
    sidebar.set_header_func(move |row, _| {
        let heading = match row.index() {
            i if i == emoji_start => Some(tr("Emoji")),
            i if i == letters_start => Some(tr("Letters & Symbols")),
            _ => None,
        };
        row.set_header(
            heading
                .map(|h| {
                    let label = gtk::Label::builder()
                        .label(h)
                        .xalign(0.0)
                        .margin_start(12)
                        .margin_top(12)
                        .margin_bottom(3)
                        .build();
                    label.add_css_class("heading");
                    label.add_css_class("dim-label");
                    label
                })
                .as_ref(),
        );
    });
    let sidebar_toolbar = adw::ToolbarView::new();
    let sidebar_header = adw::HeaderBar::new();
    sidebar_toolbar.add_top_bar(&sidebar_header);
    sidebar_toolbar.set_content(Some(
        &gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .child(&sidebar)
            .build(),
    ));
    let sidebar_page = adw::NavigationPage::builder()
        .title(tr("Characters"))
        .child(&sidebar_toolbar)
        .build();

    // Content.
    let title = adw::WindowTitle::new(tr("Characters"), "");
    let header = adw::HeaderBar::new();
    header.set_title_widget(Some(&title));
    let search_button = gtk::ToggleButton::builder()
        .icon_name("system-search-symbolic")
        .tooltip_text(tr("Search"))
        .build()
        .labelled(tr("Search"));
    header.pack_start(&search_button);
    let menu = gio::Menu::new();
    menu.append(
        Some(helix_app_common::i18n::tr(
            helix_app_common::shortcuts::MENU_LABEL,
        )),
        Some(helix_app_common::shortcuts::ACTION),
    );
    menu.append(Some(tr("_About Characters")), Some("app.about"));
    header.pack_end(
        &gtk::MenuButton::builder()
            .icon_name("open-menu-symbolic")
            .menu_model(&menu)
            .tooltip_text(tr("Main Menu"))
            .build()
            .labelled(tr("Main Menu")),
    );

    let search_entry = gtk::SearchEntry::builder()
        .placeholder_text(tr("Search characters"))
        .hexpand(true)
        .build();
    let clamp = adw::Clamp::builder()
        .maximum_size(500)
        .child(&search_entry)
        .build();
    let search_bar = gtk::SearchBar::builder().child(&clamp).build();
    search_bar.connect_entry(&search_entry);
    search_bar.set_key_capture_widget(Some(&window));
    search_button
        .bind_property("active", &search_bar, "search-mode-enabled")
        .bidirectional()
        .sync_create()
        .build();

    let model = gtk::StringList::new(&[]);
    let factory = gtk::SignalListItemFactory::new();
    factory.connect_setup(|_, item| {
        let item = item.downcast_ref::<gtk::ListItem>().unwrap();
        let label = gtk::Label::new(None);
        label.add_css_class("char-cell");
        item.set_child(Some(&label));
    });
    factory.connect_bind(|_, item| {
        let item = item.downcast_ref::<gtk::ListItem>().unwrap();
        let (Some(label), Some(obj)) = (
            item.child().and_downcast::<gtk::Label>(),
            item.item().and_downcast::<gtk::StringObject>(),
        ) else {
            return;
        };
        let text = obj.string();
        let name = charinfo::display_name(&text);
        label.set_label(&charinfo::grid_label(&text));
        label.set_tooltip_text(Some(&name));
        // The focused grid cell is the list item: read the character's
        // name ("Grinning face", "Combining acute accent"), not the glyph,
        // with its code points as the description.
        item.set_accessible_label(&charinfo::spoken_name(&text));
        item.set_accessible_description(&charinfo::codepoints(&text));
    });
    let grid = gtk::GridView::builder()
        .model(&gtk::NoSelection::new(Some(model.clone())))
        .factory(&factory)
        .single_click_activate(true)
        .max_columns(64)
        .min_columns(3)
        .build();
    grid.add_css_class("char-grid");
    helix_app_common::set_accessible_label(&grid, tr("Characters"));

    let stack = gtk::Stack::new();
    stack.add_named(
        &gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .child(&grid)
            .build(),
        Some("grid"),
    );
    stack.add_named(
        &adw::StatusPage::builder()
            .icon_name("document-open-recent-symbolic")
            .title(tr("No Recent Characters"))
            .description(tr("Characters you copy will appear here"))
            .build(),
        Some("empty-recent"),
    );
    stack.add_named(
        &adw::StatusPage::builder()
            .icon_name("system-search-symbolic")
            .title(tr("No Results"))
            .description(tr("Try a different search"))
            .build(),
        Some("no-results"),
    );
    let spinner = gtk::Spinner::builder()
        .spinning(true)
        .width_request(32)
        .height_request(32)
        .halign(gtk::Align::Center)
        .valign(gtk::Align::Center)
        .build()
        .labelled(tr("Loading Characters"));
    stack.add_named(&spinner, Some("loading"));
    // A search just started: empty until results come or the spinner shows.
    stack.add_named(
        &gtk::Box::new(gtk::Orientation::Vertical, 0),
        Some("pending"),
    );

    let toasts = adw::ToastOverlay::new();
    toasts.set_child(Some(&stack));
    let content_toolbar = adw::ToolbarView::new();
    content_toolbar.add_top_bar(&header);
    content_toolbar.add_top_bar(&search_bar);
    content_toolbar.set_content(Some(&toasts));
    let content_page = adw::NavigationPage::builder()
        .title(tr("Characters"))
        .child(&content_toolbar)
        .build();

    let split = adw::NavigationSplitView::builder()
        .sidebar(&sidebar_page)
        .content(&content_page)
        .show_content(true)
        .build();
    window.set_content(Some(&split));
    let breakpoint =
        adw::Breakpoint::new(adw::BreakpointCondition::parse("max-width: 560sp").unwrap());
    breakpoint.add_setter(&split, "collapsed", Some(&true.to_value()));
    window.add_breakpoint(breakpoint);

    let app = Rc::new(App {
        window: window.clone(),
        prefs,
        model,
        grid: grid.clone(),
        stack,
        title,
        toasts,
        search_bar: search_bar.clone(),
        search_entry: search_entry.clone(),
        split,
        sidebar: sidebar.clone(),
        sidebar_cats,
        current: Cell::new(Category::Recent),
        generation: Cell::new(0),
        cache: RefCell::default(),
        quiet_select: Cell::new(false),
    });

    // Signals.
    let a = app.clone();
    sidebar.connect_row_selected(move |_, row| {
        let Some(row) = row else { return };
        if a.quiet_select.get() {
            return;
        }
        let Some(cat) = a.sidebar_cats.get(row.index() as usize).copied() else {
            return;
        };
        if a.search_bar.is_search_mode() {
            // Leaving search: clearing the entry would re-show the old category,
            // so switch first and then close the bar.
            a.current.set(cat);
            a.search_entry.set_text("");
            a.search_bar.set_search_mode(false);
        }
        a.show_category(cat);
    });
    let a = app.clone();
    sidebar.connect_row_activated(move |_, _| a.split.set_show_content(true));

    let a = app.clone();
    search_entry.connect_search_changed(move |_| a.run_search());
    let a = app.clone();
    search_bar.connect_search_mode_enabled_notify(move |bar| {
        if !bar.is_search_mode() && !a.search_entry.text().is_empty() {
            a.search_entry.set_text("");
        }
    });

    let a = app.clone();
    grid.connect_activate(move |_, pos| {
        if let Some(obj) = a.model.item(pos).and_downcast::<gtk::StringObject>() {
            show_details(&a, obj.string().to_string());
        }
    });

    let search_action = gio::ActionEntry::builder("search")
        .activate(move |_: &adw::ApplicationWindow, _, _| {
            search_button.set_active(!search_button.is_active())
        })
        .build();
    window.add_action_entries([search_action]);
    helix_app_common::shortcuts::install(gapp, crate::shortcuts::ACCELS, crate::shortcuts::GROUPS);

    let initial = if app.recent().is_empty() {
        Category::Smileys
    } else {
        Category::Recent
    };
    app.select_sidebar(initial);
    app
}

/// Shows the details dialog for `text`.
fn show_details(app: &Rc<App>, text: String) {
    let dialog = adw::Dialog::builder().content_width(380).build();
    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&adw::HeaderBar::new());

    let glyph = gtk::Label::builder().selectable(true).build();
    glyph.add_css_class("char-detail-glyph");
    let name = gtk::Label::builder()
        .wrap(true)
        .justify(gtk::Justification::Center)
        .selectable(true)
        .build();
    name.add_css_class("title-3");
    let group = adw::PreferencesGroup::new();
    let cp_row = adw::ActionRow::builder()
        .title(tr("Code point"))
        .subtitle_selectable(true)
        .build();
    cp_row.add_css_class("property");
    let utf8_row = adw::ActionRow::builder()
        .title("UTF-8")
        .subtitle_selectable(true)
        .build();
    utf8_row.add_css_class("property");
    group.add(&cp_row);
    group.add(&utf8_row);

    let see_also_label = gtk::Label::builder()
        .label(tr("See Also"))
        .xalign(0.0)
        .build();
    see_also_label.add_css_class("heading");
    let see_also = gtk::FlowBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .max_children_per_line(8)
        .column_spacing(6)
        .row_spacing(6)
        .homogeneous(true)
        .build();

    let copy_button = gtk::Button::builder()
        .label(tr("_Copy Character"))
        .use_underline(true)
        .halign(gtk::Align::Center)
        .build();
    copy_button.add_css_class("suggested-action");
    copy_button.add_css_class("pill");

    let body = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(12)
        .margin_start(18)
        .margin_end(18)
        .margin_bottom(18)
        .build();
    body.append(&glyph);
    body.append(&name);
    body.append(&group);
    body.append(&see_also_label);
    body.append(&see_also);
    body.append(&copy_button);
    toolbar.set_content(Some(
        &gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .propagate_natural_height(true)
            .child(&body)
            .build(),
    ));
    dialog.set_child(Some(&toolbar));

    let current = Rc::new(RefCell::new(String::new()));

    // `fill` needs to reference itself for the "see also" buttons.
    type Fill = Rc<RefCell<Option<Box<dyn Fn(&str)>>>>;
    let fill: Fill = Rc::new(RefCell::new(None));
    let weak_fill = Rc::downgrade(&fill);
    let cur = current.clone();
    let dlg = dialog.clone();
    *fill.borrow_mut() = Some(Box::new(move |s: &str| {
        cur.replace(s.to_string());
        let display = charinfo::display_name(s);
        dlg.set_title(&display);
        glyph.set_label(&charinfo::grid_label(s));
        // Read by name; the code points are its description.
        glyph.update_property(&[
            gtk::accessible::Property::Label(&charinfo::spoken_name(s)),
            gtk::accessible::Property::Description(&charinfo::codepoints(s)),
        ]);
        name.set_label(&display);
        cp_row.set_subtitle(&charinfo::codepoints(s));
        utf8_row.set_subtitle(&charinfo::utf8_hex(s));
        see_also.remove_all();
        let variants = charinfo::see_also(s);
        see_also_label.set_visible(!variants.is_empty());
        see_also.set_visible(!variants.is_empty());
        for v in variants {
            let b = gtk::Button::builder()
                .label(charinfo::grid_label(&v))
                .tooltip_text(charinfo::display_name(&v))
                .build()
                .labelled(&charinfo::spoken_name(&v));
            b.add_css_class("flat");
            b.add_css_class("char-variant");
            let wf = weak_fill.clone();
            b.connect_clicked(move |_| {
                if let Some(f) = wf.upgrade() {
                    if let Some(f) = f.borrow().as_ref() {
                        f(&v);
                    }
                }
            });
            see_also.append(&b);
        }
    }));
    if let Some(f) = fill.borrow().as_ref() {
        f(&text);
    }

    let do_copy: Rc<dyn Fn()> = {
        let app = app.clone();
        let dialog = dialog.clone();
        let current = current.clone();
        let fill = fill.clone();
        Rc::new(move || {
            let _keep_alive = &fill;
            let text = current.borrow().clone();
            dialog.close();
            app.copy(&text);
        })
    };
    let c = do_copy.clone();
    copy_button.connect_clicked(move |_| c());
    let shortcuts = gtk::ShortcutController::new();
    let c = do_copy.clone();
    shortcuts.add_shortcut(gtk::Shortcut::new(
        gtk::ShortcutTrigger::parse_string("<Control>c"),
        Some(gtk::CallbackAction::new(move |_, _| {
            c();
            glib::Propagation::Stop
        })),
    ));
    dialog.add_controller(shortcuts);
    dialog.present(Some(&app.window));
    copy_button.grab_focus();
}
