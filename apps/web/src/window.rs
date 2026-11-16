//! Browser window: header bar, address entry, tabs, find bar, popovers.

use adw::prelude::*;
use gtk::{gdk, gio, glib};
use helix_app_common::Labelled;
use std::cell::{Cell, RefCell};
use std::path::Path;
use std::rc::{Rc, Weak};
use webkit6::prelude::*;

use crate::app::{self, App};
use crate::bookmarks::{self, Bookmark};
use crate::session::{TabState, WindowState};
use crate::settings;
use crate::store;
use crate::tab::{display_uri, Change, Security, Tab};
use crate::urlinput;
use helix_app_common::i18n::tr;
use helix_app_common::nc_;
use helix_app_common::trf;
use helix_app_common::trnf;

const MAX_CLOSED: usize = 10;
const MAX_SUGGESTIONS: usize = 8;

/// What an address-bar suggestion is; shown as an icon and spoken as a
/// word.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SuggestionKind {
    Search,
    Address,
    Bookmark,
    History,
}

impl SuggestionKind {
    fn icon(self) -> &'static str {
        match self {
            Self::Search => "system-search-symbolic",
            Self::Address => "web-browser-symbolic",
            Self::Bookmark => "starred-symbolic",
            Self::History => "document-open-recent-symbolic",
        }
    }

    fn word(self) -> &'static str {
        match self {
            Self::Search => tr("Search"),
            Self::Address => tr("Address"),
            Self::Bookmark => tr("Bookmark"),
            Self::History => tr("History"),
        }
    }
}

/// The accessible name of a suggestion row: its text plus its type, which
/// sighted users get from the icon.
fn suggestion_label(kind: SuggestionKind, title: &str, url: &str) -> String {
    match kind {
        SuggestionKind::Search | SuggestionKind::Address => title.to_string(),
        // Translators: accessible name of an address-bar suggestion: its
        // title and its kind ("Bookmark", "History", …).
        _ if url.is_empty() || url == title => {
            trf!("{title}, {kind}", title = title, kind = kind.word())
        }
        // Translators: accessible name of an address-bar suggestion: its
        // title, its kind ("Bookmark", "History", …) and its address.
        _ => trf!(
            "{title}, {kind}, {url}",
            title = title,
            kind = kind.word(),
            url = url
        ),
    }
}

/// The certificate problems in `flags`, in words, separated by commas.
fn tls_problems(flags: gio::TlsCertificateFlags) -> String {
    use gio::TlsCertificateFlags as F;
    let known = [
        (F::UNKNOWN_CA, tr("not signed by a trusted authority")),
        (F::BAD_IDENTITY, tr("issued for a different site")),
        (F::NOT_ACTIVATED, tr("not valid yet")),
        (F::EXPIRED, tr("expired")),
        (F::REVOKED, tr("revoked")),
        (F::INSECURE, tr("uses an insecure algorithm")),
        (F::GENERIC_ERROR, tr("could not be checked")),
    ];
    let words: Vec<&str> = known
        .iter()
        .filter(|(f, _)| flags.contains(*f))
        .map(|(_, w)| *w)
        .collect();
    if words.is_empty() {
        return tr("could not be checked").to_string();
    }
    // Translators: separator between certificate problems in a list.
    words.join(tr(nc_!("list separator", ", ")))
}

fn suggestion_count(n: usize) -> String {
    trnf!("{n} suggestion", "{n} suggestions", n)
}

const CSS: &str = "
.permission-bar, .tls-bar { border-bottom: 1px solid alpha(currentColor, .15); }
.permission-bar { background: alpha(@accent_bg_color, .18); }
.tls-bar { background: alpha(@error_bg_color, .22); }
.link-preview { background: @popover_bg_color; color: @popover_fg_color; padding: 3px 8px; margin: 4px;
  border-radius: 6px; box-shadow: 0 1px 3px alpha(black, .3); }
.private-indicator { font-weight: bold; padding: 2px 8px; border-radius: 99px;
  background: alpha(@accent_bg_color, .25); }
.download-attention { background: @accent_bg_color; border-radius: 99px;
  min-width: 8px; min-height: 8px; }
";

/// High contrast: the bars' separators reach 3:1 and their tints are
/// stronger.
const HIGH_CONTRAST_CSS: &str = "
.permission-bar, .tls-bar { border-bottom-color: alpha(currentColor, .6); }
.permission-bar { background: alpha(@accent_bg_color, .35); }
.tls-bar { background: alpha(@error_bg_color, .4); }
.link-preview { border: 1px solid alpha(currentColor, .6); }
.download-attention { box-shadow: 0 0 0 1px currentColor; }
";

pub fn install_css() {
    helix_app_common::style::add_css(CSS, HIGH_CONTRAST_CSS);
}

pub struct Window {
    pub this: Weak<Window>,
    app: Weak<App>,
    pub private: bool,
    pub session: webkit6::NetworkSession,
    pub win: adw::ApplicationWindow,
    pub tab_view: adw::TabView,
    overview: adw::TabOverview,
    toolbar: adw::ToolbarView,
    entry: gtk::Entry,
    /// Opens site information; its icon shows the connection security.
    site_button: gtk::Button,
    /// Adds or edits the bookmark for the current page.
    star_button: gtk::Button,
    back: gtk::Button,
    forward: gtk::Button,
    reload: gtk::Button,
    home: gtk::Button,
    reader: gtk::Button,
    zoom_reset: gtk::Button,
    /// Title shown instead of the address entry in web-app mode.
    app_title: Option<adw::WindowTitle>,
    suggest_pop: gtk::Popover,
    suggest_list: gtk::ListBox,
    suggestions: RefCell<Vec<String>>,
    /// Spoken text of each suggestion row, in order.
    suggestion_labels: RefCell<Vec<String>>,
    /// Set while the program (not the user) changes the entry text.
    setting_entry: Cell<bool>,
    /// The user has typed since the entry was last synced.
    editing: Cell<bool>,
    find_bar: gtk::SearchBar,
    find_entry: gtk::SearchEntry,
    find_label: gtk::Label,
    dl_button: gtk::MenuButton,
    /// Dot on the Downloads button: downloads started since it was opened.
    dl_attention: gtk::Box,
    dl_list: gtk::ListBox,
    bm_search: gtk::SearchEntry,
    bm_list: gtk::ListBox,
    /// Switches the bookmarks popover between its list and empty states.
    bm_stack: gtk::Stack,
    bm_status: adw::StatusPage,
    zoom_label: gtk::Label,
    closed: RefCell<Vec<TabState>>,
    info_pop: gtk::Popover,
    bm_pop: gtk::Popover,
    bm_title: adw::EntryRow,
    bm_tags: adw::EntryRow,
    bm_editing: RefCell<Option<String>>,
    toasts: adw::ToastOverlay,
    /// The tabs' context menu, refilled for each tab as it opens.
    tab_menu: gio::Menu,
    /// The page the tab context menu is open for.
    menu_page: RefCell<Option<adw::TabPage>>,
    /// The narrow-window breakpoint is applied.
    narrow: Cell<bool>,
}

fn icon_button(icon: &str, tooltip: &str, action: &str) -> gtk::Button {
    gtk::Button::builder()
        .icon_name(icon)
        .tooltip_text(tooltip)
        .action_name(action)
        .build()
        .labelled(tooltip)
}

fn web_app_menu() -> gio::Menu {
    let menu = gio::Menu::new();
    let zoom = gio::Menu::new();
    let item = gio::MenuItem::new(None, None);
    item.set_attribute_value("custom", Some(&"zoom".to_variant()));
    zoom.append_item(&item);
    menu.append_section(None, &zoom);
    let s1 = gio::Menu::new();
    s1.append(Some(tr("_Find…")), Some("win.find"));
    s1.append(Some(tr("_Print…")), Some("win.print"));
    s1.append(Some(tr("_Fullscreen")), Some("win.fullscreen"));
    s1.append(Some(tr("Open in _Browser")), Some("win.open-in-browser"));
    menu.append_section(None, &s1);
    let s2 = gio::Menu::new();
    s2.append(
        Some(tr("Web Application _Settings…")),
        Some("app.web-app-preferences"),
    );
    s2.append(
        Some(tr("_Uninstall Web Application…")),
        Some("app.uninstall-web-app"),
    );
    s2.append(
        Some(tr("_Keyboard Shortcuts")),
        Some("win.show-help-overlay"),
    );
    s2.append(Some(tr("_About Web")), Some("app.about"));
    menu.append_section(None, &s2);
    menu
}

fn main_menu() -> gio::Menu {
    let menu = gio::Menu::new();
    let s1 = gio::Menu::new();
    s1.append(Some(tr("_New Tab")), Some("win.new-tab"));
    s1.append(Some(tr("New _Window")), Some("app.new-window"));
    s1.append(
        Some(tr("New _Private Window")),
        Some("app.new-private-window"),
    );
    menu.append_section(None, &s1);
    let zoom = gio::Menu::new();
    let item = gio::MenuItem::new(None, None);
    item.set_attribute_value("custom", Some(&"zoom".to_variant()));
    zoom.append_item(&item);
    menu.append_section(None, &zoom);
    let s2 = gio::Menu::new();
    s2.append(Some(tr("_Reopen Closed Tab")), Some("win.reopen-tab"));
    s2.append(Some(tr("Site _Information")), Some("win.site-info"));
    s2.append(Some(tr("_Find…")), Some("win.find"));
    s2.append(Some(tr("_Print…")), Some("win.print"));
    s2.append(Some(tr("_Fullscreen")), Some("win.fullscreen"));
    s2.append(
        Some(tr("Install Site as _Web Application…")),
        Some("win.install-web-app"),
    );
    menu.append_section(None, &s2);
    let s3 = gio::Menu::new();
    s3.append(Some(tr("_History")), Some("app.history"));
    s3.append(Some(tr("_Passwords")), Some("app.passwords"));
    s3.append(Some(tr("_Import Bookmarks…")), Some("app.import-bookmarks"));
    s3.append(Some(tr("_Export Bookmarks…")), Some("app.export-bookmarks"));
    menu.append_section(None, &s3);
    let s4 = gio::Menu::new();
    s4.append(Some(tr("_Preferences")), Some("app.preferences"));
    s4.append(
        Some(tr("_Keyboard Shortcuts")),
        Some("win.show-help-overlay"),
    );
    s4.append(Some(tr("_About Web")), Some("app.about"));
    menu.append_section(None, &s4);
    menu
}

fn two_line_row(title: &str, subtitle: &str, icon: &str) -> (gtk::Box, gtk::Box) {
    let row = gtk::Box::builder()
        .spacing(10)
        .margin_start(8)
        .margin_end(8)
        .margin_top(6)
        .margin_bottom(6)
        .build();
    row.append(&gtk::Image::from_icon_name(icon));
    let text = gtk::Box::new(gtk::Orientation::Vertical, 2);
    text.set_hexpand(true);
    let t = gtk::Label::builder()
        .label(title)
        .xalign(0.0)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .build();
    text.append(&t);
    if !subtitle.is_empty() {
        let s = gtk::Label::builder()
            .label(subtitle)
            .xalign(0.0)
            .ellipsize(gtk::pango::EllipsizeMode::Middle)
            .build();
        s.add_css_class("dim-label");
        s.add_css_class("suggestion-url");
        s.add_css_class("caption");
        text.append(&s);
    }
    row.append(&text);
    (row, text)
}

impl Window {
    /// Shows a short message at the bottom of the window (libadwaita
    /// also speaks it to screen readers).
    pub fn toast(&self, text: &str) {
        self.toasts.add_toast(adw::Toast::new(text));
    }

    pub fn new(app: &Rc<App>, private: bool, session: webkit6::NetworkSession) -> Rc<Window> {
        let win = adw::ApplicationWindow::builder()
            .application(&app.gtk)
            .default_width(1200)
            .default_height(800)
            .title(if private {
                tr("Private Browsing")
            } else {
                tr("Web")
            })
            .width_request(helix_app_common::MIN_WIDTH)
            .height_request(helix_app_common::MIN_HEIGHT)
            .build();
        helix_app_common::window_state::persist(
            &win,
            helix_app_common::window_state::Store::file("helix-web"),
        );
        win.set_help_overlay(Some(&crate::shortcuts::window()));

        let tab_view = adw::TabView::new();
        let tab_bar = adw::TabBar::builder()
            .view(&tab_view)
            .autohide(true)
            .build();

        // Header: navigation.
        let back = icon_button("go-previous-symbolic", tr("Back"), "win.back");
        let forward = icon_button("go-next-symbolic", tr("Forward"), "win.forward");
        let reload = icon_button("view-refresh-symbolic", tr("Reload"), "win.reload");
        let home = icon_button(
            "go-home-symbolic",
            tr(nc_!("home page", "Home")),
            "win.home",
        );
        // A button acting as a toggle: the pressed state follows the tab's
        // reader mode, which changes only once the page has reloaded.
        let reader = gtk::Button::builder()
            .icon_name("format-justify-left-symbolic")
            .tooltip_text(tr("Reader View"))
            .action_name("win.reader")
            .accessible_role(gtk::AccessibleRole::ToggleButton)
            .build()
            .labelled(tr("Reader View"));
        reader.update_state(&[gtk::accessible::State::Pressed(
            gtk::AccessibleTristate::False,
        )]);
        reader.set_visible(false);
        let web_app = app.web_app.is_some();
        let header = adw::HeaderBar::new();
        header.pack_start(&back);
        header.pack_start(&forward);
        header.pack_start(&reload);
        header.pack_start(&home);

        // Address entry.
        // A small minimum width so the header fits a 360 px window; the
        // entry still expands to fill the header.
        let entry = gtk::Entry::builder()
            .hexpand(true)
            .width_chars(8)
            .input_purpose(gtk::InputPurpose::Url)
            .placeholder_text(tr("Search or enter address"))
            .build()
            .labelled(tr("Address and Search"));
        // Site information and the bookmark star are real buttons joined
        // to the entry (entry icons cannot take keyboard focus).
        let site_button = gtk::Button::builder()
            .icon_name("web-browser-symbolic")
            .tooltip_text(tr("Site Information"))
            .action_name("win.site-info")
            .build()
            .labelled(tr("Site Information"));
        let star_button = gtk::Button::builder()
            .icon_name("non-starred-symbolic")
            .tooltip_text(tr("Bookmark This Page"))
            .action_name("win.bookmark")
            .build()
            .labelled(tr("Bookmark This Page"));
        let location = gtk::Box::builder()
            .css_classes(["linked"])
            .hexpand(true)
            .build();
        location.append(&site_button);
        location.append(&entry);
        location.append(&star_button);
        let clamp = adw::Clamp::builder()
            .maximum_size(760)
            .tightening_threshold(560)
            .child(&location)
            .build();
        clamp.set_hexpand(true);
        let app_title = web_app.then(|| adw::WindowTitle::new("", ""));
        match &app_title {
            Some(t) => header.set_title_widget(Some(t)),
            None => header.set_title_widget(Some(&clamp)),
        }

        // Header: end.
        let zoom_label = gtk::Label::new(Some("100%"));
        let zoom_box = gtk::Box::builder()
            .spacing(6)
            .halign(gtk::Align::Center)
            .hexpand(true)
            .build();
        let zout = icon_button("zoom-out-symbolic", tr("Zoom Out"), "win.zoom-out");
        let zin = icon_button("zoom-in-symbolic", tr("Zoom In"), "win.zoom-in");
        let zreset = gtk::Button::builder()
            .child(&zoom_label)
            .action_name("win.zoom-reset")
            .tooltip_text(tr("Reset Zoom"))
            .build()
            .labelled(tr("Reset Zoom"));
        for b in [&zout, &zreset, &zin] {
            b.add_css_class("flat");
        }
        zoom_box.append(&zout);
        zoom_box.append(&zreset);
        zoom_box.append(&zin);
        let menu_pop =
            gtk::PopoverMenu::from_model(Some(&if web_app { web_app_menu() } else { main_menu() }));
        menu_pop.add_child(&zoom_box, "zoom");
        let menu_button = gtk::MenuButton::builder()
            .icon_name("open-menu-symbolic")
            .tooltip_text(tr("Main Menu"))
            .popover(&menu_pop)
            .primary(true)
            .build()
            .labelled(tr("Main Menu"));
        header.pack_end(&menu_button);
        let tab_button = adw::TabButton::builder()
            .view(&tab_view)
            .action_name("win.overview")
            .visible(!web_app)
            .build();
        header.pack_end(&tab_button);

        let dl_list = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .build();
        dl_list.add_css_class("boxed-list");
        let dl_clear = gtk::Button::with_label(tr("Clear Finished"));
        let dl_box = gtk::Box::new(gtk::Orientation::Vertical, 8);
        let dl_scroll = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .propagate_natural_height(true)
            .max_content_height(420)
            .min_content_width(300)
            .child(&dl_list)
            .build();
        dl_box.append(&dl_scroll);
        dl_box.append(&dl_clear);
        let dl_pop = gtk::Popover::builder().child(&dl_box).build();
        // New downloads mark the button instead of opening the popover,
        // which would take focus away from the page.
        let dl_attention = gtk::Box::builder()
            .halign(gtk::Align::End)
            .valign(gtk::Align::Start)
            .visible(false)
            .can_target(false)
            .css_classes(["download-attention"])
            .build();
        let dl_icon = gtk::Overlay::builder()
            .child(&gtk::Image::from_icon_name("folder-download-symbolic"))
            .build();
        dl_icon.add_overlay(&dl_attention);
        let dl_button = gtk::MenuButton::builder()
            .child(&dl_icon)
            .tooltip_text(tr("Downloads"))
            .popover(&dl_pop)
            .visible(false)
            .build()
            .labelled(tr("Downloads"));
        {
            let (dot, button) = (dl_attention.clone(), dl_button.downgrade());
            dl_pop.connect_show(move |_| {
                dot.set_visible(false);
                if let Some(b) = button.upgrade() {
                    set_downloads_description(&b, None);
                }
            });
        }
        header.pack_end(&dl_button);

        let bm_search = gtk::SearchEntry::builder()
            .placeholder_text(tr("Search bookmarks"))
            .build()
            .labelled(tr("Search Bookmarks"));
        let bm_list = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .build();
        bm_list.add_css_class("navigation-sidebar");
        let bm_scroll = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .propagate_natural_height(true)
            .max_content_height(480)
            .min_content_width(300)
            .child(&bm_list)
            .build();
        let bm_status = adw::StatusPage::builder()
            .css_classes(["compact"])
            .width_request(300)
            .build();
        let bm_stack = gtk::Stack::builder().vhomogeneous(false).build();
        bm_stack.add_named(&bm_scroll, Some("list"));
        bm_stack.add_named(&bm_status, Some("empty"));
        let bm_box = gtk::Box::new(gtk::Orientation::Vertical, 6);
        bm_box.append(&bm_search);
        bm_box.append(&bm_stack);
        let bm_list_pop = gtk::Popover::builder().child(&bm_box).build();
        let bm_button = gtk::MenuButton::builder()
            .icon_name("user-bookmarks-symbolic")
            .tooltip_text(tr("Bookmarks"))
            .popover(&bm_list_pop)
            .visible(!web_app)
            .build()
            .labelled(tr("Bookmarks"));
        header.pack_end(&bm_button);
        header.pack_end(&reader);

        // The private-browsing pill collapses to an icon in narrow windows.
        let private_box = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        let pill = gtk::Label::new(Some(tr("Private")));
        pill.add_css_class("private-indicator");
        pill.set_tooltip_text(Some(tr(
            "Private browsing: history and site data are not saved",
        )));
        let private_icon = gtk::Image::builder()
            .icon_name("view-conceal-symbolic")
            .tooltip_text(tr("Private browsing: history and site data are not saved"))
            .visible(false)
            .build()
            .labelled(tr("Private Browsing"));
        private_box.append(&pill);
        private_box.append(&private_icon);
        private_box.set_visible(private);
        header.pack_start(&private_box);

        // Find bar.
        let find_entry = gtk::SearchEntry::builder()
            .placeholder_text(tr("Find in page"))
            .width_chars(8)
            .max_width_chars(28)
            .build()
            .labelled(tr("Find in Page"));
        let find_prev = icon_button("go-up-symbolic", tr("Previous Match"), "win.find-prev");
        let find_next = icon_button("go-down-symbolic", tr("Next Match"), "win.find-next");
        let find_label = gtk::Label::builder()
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .build();
        find_label.add_css_class("dim-label");
        let find_box = gtk::Box::builder().spacing(6).build();
        find_box.append(&find_entry);
        find_box.append(&find_prev);
        find_box.append(&find_next);
        find_box.append(&find_label);
        let find_bar = gtk::SearchBar::builder()
            .child(&find_box)
            .show_close_button(true)
            .build();
        find_bar.connect_entry(&find_entry);

        let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
        content.append(&find_bar);
        tab_view.set_vexpand(true);
        content.append(&tab_view);

        let toasts = adw::ToastOverlay::new();
        toasts.set_child(Some(&content));
        let toolbar = adw::ToolbarView::new();
        toolbar.add_top_bar(&header);
        toolbar.add_top_bar(&tab_bar);
        toolbar.set_content(Some(&toasts));

        // Narrow windows: navigation, reader view, bookmarks, downloads
        // and the tab button move from the header to a bar at the bottom,
        // so the address entry keeps its room. The widgets themselves move
        // (not copies), so their state, popovers and focus order go along.
        let bottom = gtk::ActionBar::builder().revealed(false).build();
        toolbar.add_bottom_bar(&bottom);
        let narrow = adw::Breakpoint::new(adw::BreakpointCondition::new_length(
            adw::BreakpointConditionLengthType::MaxWidth,
            600.0,
            adw::LengthUnit::Sp,
        ));
        narrow.add_setter(&bottom, "revealed", Some(&true.to_value()));
        narrow.add_setter(&pill, "visible", Some(&false.to_value()));
        narrow.add_setter(&private_icon, "visible", Some(&true.to_value()));
        let start: Vec<gtk::Widget> = vec![
            back.clone().upcast(),
            forward.clone().upcast(),
            reload.clone().upcast(),
            home.clone().upcast(),
        ];
        // In pack_end order: outermost first.
        let end: Vec<gtk::Widget> = vec![
            tab_button.clone().upcast(),
            dl_button.clone().upcast(),
            bm_button.clone().upcast(),
            reader.clone().upcast(),
        ];
        {
            let (header, bottom, start, end) =
                (header.clone(), bottom.clone(), start.clone(), end.clone());
            narrow.connect_apply(move |_| {
                for w in &start {
                    header.remove(w);
                    bottom.pack_start(w);
                }
                for w in &end {
                    header.remove(w);
                    bottom.pack_end(w);
                }
            });
        }
        {
            let (header, bottom, private_box) = (header.clone(), bottom.clone(), private_box);
            narrow.connect_unapply(move |_| {
                // Navigation goes back before the private indicator.
                header.remove(&private_box);
                for w in &start {
                    bottom.remove(w);
                    header.pack_start(w);
                }
                header.pack_start(&private_box);
                for w in &end {
                    bottom.remove(w);
                    header.pack_end(w);
                }
            });
        }

        let overview = adw::TabOverview::builder()
            .view(&tab_view)
            .child(&toolbar)
            .enable_new_tab(true)
            .build();
        win.set_content(Some(&overview));

        // Suggestions popover.
        let suggest_list = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::Single)
            .build();
        suggest_list.add_css_class("navigation-sidebar");
        suggest_list.set_can_focus(false);
        helix_app_common::set_accessible_label(&suggest_list, tr("Suggestions"));
        // Combobox pattern: focus stays in the entry, which says that it
        // has a list of completions, whether it is open, and (see
        // entry_key) which row is highlighted.
        entry.update_property(&[
            gtk::accessible::Property::Autocomplete(gtk::AccessibleAutocomplete::List),
            gtk::accessible::Property::HasPopup(true),
        ]);
        entry.update_relation(&[gtk::accessible::Relation::Controls(&[suggest_list
            .upcast_ref::<gtk::Accessible>(
        )])]);
        entry.update_state(&[gtk::accessible::State::Expanded(Some(false))]);
        let suggest_pop = gtk::Popover::builder()
            .child(&suggest_list)
            .autohide(false)
            .has_arrow(false)
            .position(gtk::PositionType::Bottom)
            .halign(gtk::Align::Start)
            .can_focus(false)
            .build();
        suggest_pop.set_parent(&entry);

        // Site info and bookmark-edit popovers, anchored to their buttons.
        let info_pop = gtk::Popover::new();
        info_pop.set_parent(&site_button);
        // Visible field labels: libadwaita entry rows in a boxed list.
        let bm_title = adw::EntryRow::builder()
            .title(tr("_Name"))
            .use_underline(true)
            .build();
        let bm_tags = adw::EntryRow::builder()
            .title(tr("_Tags (comma separated)"))
            .use_underline(true)
            .build();
        let bm_fields = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .css_classes(["boxed-list"])
            .width_request(300)
            .build();
        bm_fields.append(&bm_title);
        bm_fields.append(&bm_tags);
        let bm_remove = gtk::Button::with_label(tr("Remove Bookmark"));
        bm_remove.add_css_class("destructive-action");
        let bm_edit_box = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(8)
            .margin_start(6)
            .margin_end(6)
            .margin_top(6)
            .margin_bottom(6)
            .build();
        let heading = gtk::Label::builder()
            .label(tr("Bookmark"))
            .xalign(0.0)
            .build();
        heading.add_css_class("heading");
        bm_edit_box.append(&heading);
        bm_edit_box.append(&bm_fields);
        bm_edit_box.append(&bm_remove);
        let bm_pop = gtk::Popover::builder().child(&bm_edit_box).build();
        bm_pop.set_parent(&star_button);

        let w = Rc::new_cyclic(|this| Window {
            this: this.clone(),
            app: Rc::downgrade(app),
            private,
            session,
            win,
            tab_view,
            overview,
            toolbar,
            entry,
            site_button,
            star_button,
            back,
            forward,
            reload,
            home,
            reader,
            app_title,
            suggest_pop,
            suggest_list,
            suggestions: RefCell::default(),
            suggestion_labels: RefCell::default(),
            setting_entry: Cell::new(false),
            editing: Cell::new(false),
            find_bar,
            find_entry,
            find_label,
            dl_button,
            dl_attention,
            dl_list,
            bm_search,
            bm_list,
            bm_stack,
            bm_status,
            zoom_label,
            zoom_reset: zreset.clone(),
            closed: RefCell::default(),
            info_pop,
            bm_pop,
            bm_title,
            bm_tags,
            bm_editing: RefCell::default(),
            toasts,
            tab_menu: {
                let m = gio::Menu::new();
                fill_tab_menu(&m, false, false);
                m
            },
            menu_page: RefCell::default(),
            narrow: Cell::new(false),
        });
        let cb = w.weak_fn(|w, ()| w.set_narrow(true));
        narrow.connect_apply(move |_| cb(()));
        let cb = w.weak_fn(|w, ()| w.set_narrow(false));
        narrow.connect_unapply(move |_| cb(()));
        w.win.add_breakpoint(narrow);
        w.install_actions();
        w.connect_signals(&dl_clear, &bm_list_pop, &bm_remove);
        // Popovers parented to a widget must be unparented with it.
        for (parent, pop) in [
            (
                w.entry.clone().upcast::<gtk::Widget>(),
                w.suggest_pop.clone(),
            ),
            (w.site_button.clone().upcast(), w.info_pop.clone()),
            (w.star_button.clone().upcast(), w.bm_pop.clone()),
        ] {
            parent.connect_destroy(move |_| pop.unparent());
        }
        w.sync();
        w
    }

    fn app(&self) -> Option<Rc<App>> {
        self.app.upgrade()
    }

    /// Runs `f` with this window if it is still alive.
    fn weak_fn<A>(&self, f: impl Fn(&Rc<Window>, A) + 'static) -> impl Fn(A) + 'static {
        let weak = self.this.clone();
        move |a| {
            if let Some(w) = weak.upgrade() {
                f(&w, a)
            }
        }
    }

    fn install_actions(&self) {
        let add = |name: &str, f: fn(&Rc<Window>)| {
            let a = gio::SimpleAction::new(name, None);
            let cb = self.weak_fn(move |w, ()| f(w));
            a.connect_activate(move |_, _| cb(()));
            self.win.add_action(&a);
        };
        add("new-tab", |w| {
            w.open_home(true);
        });
        add("close-tab", |w| {
            if let Some(p) = w.tab_view.selected_page() {
                w.tab_view.close_page(&p);
            }
        });
        add("reopen-tab", |w| w.reopen_closed());
        add("location", |w| w.focus_location());
        add("next-pane", |w| w.cycle_focus(true));
        add("previous-pane", |w| w.cycle_focus(false));
        add("back", |w| w.with_view(|v| v.go_back()));
        add("forward", |w| w.with_view(|v| v.go_forward()));
        add("reload", |w| {
            if let Some(t) = w.current() {
                reload_or_stop(&t);
            }
        });
        add("reload-bypass", |w| {
            w.with_view(|v| v.reload_bypass_cache())
        });
        add("home", |w| {
            if let (Some(app), Some(t)) = (w.app(), w.current()) {
                t.load(&app.home_uri());
            }
        });
        add("bookmark", |w| w.bookmark_current());
        add("site-info", |w| w.show_site_info());
        add("overview", |w| w.overview.set_open(!w.overview.is_open()));
        add("find", |w| {
            w.find_bar.set_search_mode(true);
            w.find_entry.grab_focus();
        });
        add("find-next", |w| w.find_step(true));
        add("find-prev", |w| w.find_step(false));
        add("print", |w| {
            w.with_view(|v| {
                let op = webkit6::PrintOperation::new(v);
                op.run_dialog(Some(&w.win));
            })
        });
        add("zoom-in", |w| w.zoom(1));
        add("zoom-out", |w| w.zoom(-1));
        add("zoom-reset", |w| w.zoom(0));
        add("reader", |w| {
            if let Some(t) = w.current() {
                t.toggle_reader();
            }
        });
        add("install-web-app", crate::dialogs::install_web_app);
        add("open-in-browser", |w| {
            if let Some(t) = w.current() {
                crate::tab::open_externally(&w.win, &t.uri());
            }
        });
        add("fullscreen", |w| {
            w.win.set_fullscreened(!w.win.is_fullscreen())
        });

        // Ctrl+M, and the tab context menu (which acts on the tab it was
        // opened for, see setup_tab_menu).
        add("mute", |w| {
            if let Some(p) = w.tab_view.selected_page() {
                w.toggle_mute(&p);
            }
        });
        add("tab-reload", |w| {
            if let Some(t) = w.menu_target().and_then(|p| w.tab_for_page(&p)) {
                reload_or_stop(&t);
            }
        });
        add("tab-duplicate", |w| {
            if let Some(t) = w.menu_target().and_then(|p| w.tab_for_page(&p)) {
                let copy = w.new_tab_after(&t);
                copy.load(&t.uri());
                w.select(&copy);
            }
        });
        add("tab-pin", |w| {
            if let Some(p) = w.menu_target() {
                w.tab_view.set_page_pinned(&p, !p.is_pinned());
            }
        });
        add("tab-mute", |w| {
            if let Some(p) = w.menu_target() {
                w.toggle_mute(&p);
            }
        });
        add("tab-move-to-new-window", |w| {
            let (Some(p), Some(app)) = (w.menu_target(), w.app()) else {
                return;
            };
            if w.tab_view.n_pages() < 2 {
                return;
            }
            let nw = app.new_window(w.private);
            w.tab_view.transfer_page(&p, &nw.tab_view, 0);
            nw.win.present();
        });
        add("tab-close-others", |w| {
            if let Some(p) = w.menu_target() {
                w.tab_view.close_other_pages(&p);
            }
        });
        add("tab-close-right", |w| {
            if let Some(p) = w.menu_target() {
                w.tab_view.close_pages_after(&p);
            }
        });
        add("tab-close", |w| {
            if let Some(p) = w.menu_target() {
                w.tab_view.close_page(&p);
            }
        });
    }

    fn set_action_enabled(&self, name: &str, enabled: bool) {
        if let Some(a) = self
            .win
            .lookup_action(name)
            .and_downcast::<gio::SimpleAction>()
        {
            a.set_enabled(enabled);
        }
    }

    /// The page a tab context menu was opened for, else the selected one.
    fn menu_target(&self) -> Option<adw::TabPage> {
        let page = self.menu_page.borrow().clone();
        page.filter(|p| {
            p.child()
                .ancestor(adw::TabView::static_type())
                .is_some_and(|a| a == *self.tab_view.upcast_ref::<gtk::Widget>())
        })
        .or_else(|| self.tab_view.selected_page())
    }

    /// Fills the tab context menu for `page` as it opens (`None` once it
    /// has closed).
    fn setup_tab_menu(&self, page: Option<adw::TabPage>) {
        let Some(page) = page else {
            // Menu items activate before the menu reports it closed; drop
            // the page afterwards so later shortcuts act on the selection.
            let weak = self.this.clone();
            glib::idle_add_local_once(move || {
                if let Some(w) = weak.upgrade() {
                    w.menu_page.replace(None);
                }
            });
            return;
        };
        let muted = self.tab_for_page(&page).is_some_and(|t| t.view.is_muted());
        fill_tab_menu(&self.tab_menu, page.is_pinned(), muted);
        let n = self.tab_view.n_pages();
        let pos = self.tab_view.page_position(&page);
        self.set_action_enabled("tab-close-others", n > 1);
        self.set_action_enabled("tab-close-right", pos + 1 < n);
        self.set_action_enabled("tab-move-to-new-window", n > 1);
        self.menu_page.replace(Some(page));
    }

    fn toggle_mute(&self, page: &adw::TabPage) {
        let Some(t) = self.tab_for_page(page) else {
            return;
        };
        let muted = !t.view.is_muted();
        t.view.set_is_muted(muted);
        helix_app_common::announce(
            &self.tab_view,
            if muted {
                tr("Tab muted")
            } else {
                tr("Tab unmuted")
            },
            helix_app_common::Urgency::Polite,
        );
    }

    fn connect_signals(
        &self,
        dl_clear: &gtk::Button,
        bm_list_pop: &gtk::Popover,
        bm_remove: &gtk::Button,
    ) {
        // Window lifecycle.
        let cb = self.weak_fn(|w, ()| {
            if let Some(app) = w.app() {
                app.window_closed(w);
            }
        });
        self.win.connect_close_request(move |_| {
            cb(());
            glib::Propagation::Proceed
        });
        let cb = self.weak_fn(|w, fs: bool| {
            w.toolbar.set_reveal_top_bars(!fs);
            w.toolbar.set_reveal_bottom_bars(!fs);
            if fs {
                w.find_bar.set_search_mode(false);
            }
        });
        self.win
            .connect_fullscreened_notify(move |win| cb(win.is_fullscreen()));

        // Tab view.
        let cb = self.weak_fn(|w, ()| w.selection_changed());
        self.tab_view.connect_selected_page_notify(move |_| cb(()));
        let cb = self.weak_fn(|w, page: adw::TabPage| w.page_closing(&page));
        self.tab_view.connect_close_page(move |_, page| {
            cb(page.clone());
            glib::Propagation::Proceed
        });
        let cb = self.weak_fn(|w, ()| {
            if w.tab_view.n_pages() == 0 {
                w.win.close();
            } else if let Some(app) = w.app() {
                app.schedule_session_save();
            }
        });
        self.tab_view.connect_page_detached(move |_, _, _| cb(()));
        let cb = self.weak_fn(|w, page: adw::TabPage| {
            if let Some(t) = w.tab_for_page(&page) {
                w.refresh_page(&t);
                // Tabs moved in from another window take this one's layout.
                t.set_narrow(w.narrow.get());
            }
            if let Some(app) = w.app() {
                app.schedule_session_save();
            }
        });
        self.tab_view
            .connect_page_attached(move |_, page, _| cb(page.clone()));
        let cb = self.weak_fn(|w, ()| {
            if let Some(app) = w.app() {
                app.schedule_session_save();
            }
        });
        self.tab_view.connect_page_reordered(move |_, _, _| cb(()));
        let weak = self.this.clone();
        self.tab_view.connect_create_window(move |_| {
            let w = weak.upgrade()?;
            let app = w.app()?;
            let nw = app.new_window(w.private);
            nw.win.present();
            Some(nw.tab_view.clone())
        });
        let cb = self.weak_fn(|w, page: adw::TabPage| w.toggle_mute(&page));
        self.tab_view
            .connect_indicator_activated(move |_, page| cb(page.clone()));
        self.tab_view.set_menu_model(Some(&self.tab_menu));
        let cb = self.weak_fn(|w, page: Option<adw::TabPage>| w.setup_tab_menu(page));
        self.tab_view
            .connect_setup_menu(move |_, page| cb(page.cloned()));
        let weak = self.this.clone();
        self.overview.connect_create_tab(move |o| {
            match weak.upgrade() {
                Some(w) => {
                    let t = w.open_home(true);
                    w.tab_view.page(&t.root)
                }
                // Unreachable in practice: the overview lives in the window.
                None => o
                    .view()
                    .expect("tab view")
                    .append(&gtk::Box::new(gtk::Orientation::Vertical, 0)),
            }
        });

        // Address entry.
        let cb = self.weak_fn(|w, ()| w.entry_activated(false));
        self.entry.connect_activate(move |_| cb(()));
        let cb = self.weak_fn(|w, ()| {
            if w.setting_entry.get() || !w.entry_focused() {
                return;
            }
            w.editing.set(true);
            w.update_suggestions();
        });
        self.entry.connect_changed(move |_| cb(()));
        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        let weak = self.this.clone();
        keys.connect_key_pressed(move |_, key, _, mods| {
            let Some(w) = weak.upgrade() else {
                return glib::Propagation::Proceed;
            };
            w.entry_key(key, mods)
        });
        self.entry.add_controller(keys);

        let focus = gtk::EventControllerFocus::new();
        let cb = self.weak_fn(|w, ()| {
            let weak = w.this.clone();
            glib::timeout_add_local_once(std::time::Duration::from_millis(200), move || {
                if let Some(w) = weak.upgrade() {
                    if !w.entry_focused() {
                        w.suggest_pop.popdown();
                        w.editing.set(false);
                        w.sync_entry();
                    }
                }
            });
        });
        focus.connect_leave(move |_| cb(()));
        self.entry.add_controller(focus);

        let cb = self.weak_fn(|w, index: i32| w.activate_suggestion(index as usize, false));
        self.suggest_list
            .connect_row_activated(move |_, row| cb(row.index()));
        let cb = self.weak_fn(|w, ()| {
            let open = w.suggest_pop.is_visible();
            w.entry
                .update_state(&[gtk::accessible::State::Expanded(Some(open))]);
            if !open {
                w.entry
                    .reset_relation(gtk::AccessibleRelation::ActiveDescendant);
            }
        });
        self.suggest_pop.connect_visible_notify(move |_| cb(()));

        // Find bar.
        let cb = self.weak_fn(|w, ()| w.find_changed());
        self.find_entry.connect_search_changed(move |_| cb(()));
        let cb = self.weak_fn(|w, ()| w.find_step(true));
        self.find_entry.connect_activate(move |_| cb(()));
        let cb = self.weak_fn(|w, ()| w.find_step(false));
        self.find_entry.connect_previous_match(move |_| cb(()));
        let cb = self.weak_fn(|w, ()| w.find_step(true));
        self.find_entry.connect_next_match(move |_| cb(()));
        let cb = self.weak_fn(|w, on: bool| {
            if !on {
                w.with_view(|v| {
                    if let Some(fc) = v.find_controller() {
                        fc.search_finish();
                    }
                });
                w.find_label.set_text("");
                w.set_find_error(false);
                w.with_view(|v| {
                    v.grab_focus();
                });
            }
        });
        self.find_bar
            .connect_search_mode_enabled_notify(move |b| cb(b.is_search_mode()));

        // Downloads.
        let cb = self.weak_fn(|w, ()| {
            let mut child = w.dl_list.first_child();
            while let Some(row) = child {
                child = row.next_sibling();
                if row.widget_name() == "finished" {
                    w.dl_list.remove(&row);
                }
            }
            if w.dl_list.first_child().is_none() {
                w.dl_button.popdown();
                w.dl_button.set_visible(false);
            }
        });
        dl_clear.connect_clicked(move |_| cb(()));

        // Bookmarks list.
        let cb = self.weak_fn(|w, ()| w.fill_bookmarks_list());
        bm_list_pop.connect_show(move |_| cb(()));
        let cb = self.weak_fn(|w, ()| w.fill_bookmarks_list());
        self.bm_search.connect_search_changed(move |_| cb(()));
        let pop = bm_list_pop.clone();
        let cb = self.weak_fn(move |w, row: gtk::ListBoxRow| {
            let url = row.widget_name().to_string();
            pop.popdown();
            if let Some(t) = w.current() {
                t.load(&url);
            }
        });
        self.bm_list
            .connect_row_activated(move |_, row| cb(row.clone()));

        // Bookmark edit popover.
        let cb = self.weak_fn(|w, ()| w.bookmark_popover_closed());
        self.bm_pop.connect_closed(move |_| cb(()));
        let cb = self.weak_fn(|w, ()| {
            if let (Some(app), Some(url)) = (w.app(), w.bm_editing.take()) {
                app.bookmarks.borrow_mut().remove(&url);
                app.bookmarks_changed();
            }
            w.bm_pop.popdown();
        });
        bm_remove.connect_clicked(move |_| cb(()));
        let pop = self.bm_pop.clone();
        self.bm_title
            .connect_entry_activated(move |_| pop.popdown());
        let pop = self.bm_pop.clone();
        self.bm_tags.connect_entry_activated(move |_| pop.popdown());
    }

    // ---- tabs -------------------------------------------------------------

    pub fn contains(&self, tab: &Tab) -> bool {
        tab.root
            .ancestor(adw::TabView::static_type())
            .is_some_and(|a| a == *self.tab_view.upcast_ref::<gtk::Widget>())
    }

    /// The entry's focus lives on its internal text child.
    fn entry_focused(&self) -> bool {
        self.entry
            .state_flags()
            .contains(gtk::StateFlags::FOCUS_WITHIN)
    }

    fn focus_location(&self) {
        self.entry.grab_focus();
        self.entry.select_region(0, -1);
    }

    /// F6: moves focus between the address bar, the current tab's prompt
    /// bars and the page, so the bars' buttons can be reached from the
    /// keyboard (the page keeps Tab to itself).
    fn cycle_focus(&self, forward: bool) {
        let mut stops: Vec<gtk::Widget> = Vec::new();
        if self.app_title.is_none() {
            stops.push(self.entry.clone().upcast());
        }
        if let Some(t) = self.current() {
            stops.extend(t.prompt_bars());
            stops.push(t.view.clone().upcast());
        }
        let focused = gtk::prelude::GtkWindowExt::focus(&self.win);
        let at = focused.and_then(|f| stops.iter().position(|s| f == *s || f.is_ancestor(s)));
        let Some(next) = next_stop(at, stops.len(), forward) else {
            return;
        };
        let target = &stops[next];
        if target == self.entry.upcast_ref::<gtk::Widget>() {
            self.focus_location();
        } else if target.is::<webkit6::WebView>() {
            target.grab_focus();
        } else {
            target.child_focus(gtk::DirectionType::TabForward);
        }
    }

    pub fn current(&self) -> Option<Rc<Tab>> {
        let page = self.tab_view.selected_page()?;
        self.tab_for_page(&page)
    }

    fn tab_for_page(&self, page: &adw::TabPage) -> Option<Rc<Tab>> {
        let child = page.child();
        let app = self.app()?;
        let tabs = app.tabs.borrow();
        tabs.iter()
            .find(|t| t.root.upcast_ref::<gtk::Widget>() == &child)
            .cloned()
    }

    fn page_of(&self, tab: &Tab) -> Option<adw::TabPage> {
        self.contains(tab).then(|| self.tab_view.page(&tab.root))
    }

    fn with_view(&self, f: impl FnOnce(&webkit6::WebView)) {
        if let Some(t) = self.current() {
            f(&t.view);
        }
    }

    fn new_tab_widget(&self) -> Option<Rc<Tab>> {
        let app = self.app()?;
        Some(app.create_tab(self.private, &self.session))
    }

    fn setup_page(&self, tab: &Rc<Tab>) {
        self.refresh_page(tab);
        tab.set_narrow(self.narrow.get());
    }

    fn set_narrow(&self, narrow: bool) {
        self.narrow.set(narrow);
        for i in 0..self.tab_view.n_pages() {
            if let Some(t) = self.tab_for_page(&self.tab_view.nth_page(i)) {
                t.set_narrow(narrow);
            }
        }
    }

    /// Appends a tab that opens the homepage.
    pub fn open_home(&self, select: bool) -> Rc<Tab> {
        let home = self
            .app()
            .map(|a| a.home_uri())
            .unwrap_or_else(|| settings::OVERVIEW.into());
        let tab = self.open_uri(&home, select);
        if select && home == settings::OVERVIEW {
            self.entry.grab_focus();
        }
        tab
    }

    pub fn open_uri(&self, uri: &str, select: bool) -> Rc<Tab> {
        let tab = self.new_tab_widget().expect("app alive");
        let page = self.tab_view.append(&tab.root);
        self.setup_page(&tab);
        tab.load(uri);
        if select {
            self.tab_view.set_selected_page(&page);
        }
        tab
    }

    /// Creates an empty tab right after `from`.
    pub fn new_tab_after(&self, from: &Tab) -> Rc<Tab> {
        let tab = self.new_tab_widget().expect("app alive");
        self.insert_tab_after(from, &tab);
        tab
    }

    pub fn insert_tab_after(&self, from: &Tab, tab: &Rc<Tab>) {
        let parent = self.page_of(from);
        self.tab_view.add_page(&tab.root, parent.as_ref());
        self.setup_page(tab);
    }

    pub fn select(&self, tab: &Tab) {
        if let Some(p) = self.page_of(tab) {
            self.tab_view.set_selected_page(&p);
        }
    }

    pub fn close_tab(&self, tab: &Tab) {
        if let Some(p) = self.page_of(tab) {
            self.tab_view.close_page(&p);
        }
    }

    fn page_closing(&self, page: &adw::TabPage) {
        let Some(t) = self.tab_for_page(page) else {
            return;
        };
        t.deny_all_permissions();
        if let Some(state) = app::tab_state(&t) {
            let mut closed = self.closed.borrow_mut();
            closed.push(state);
            if closed.len() > MAX_CLOSED {
                closed.remove(0);
            }
        }
        if let Some(app) = self.app() {
            app.forget_tab(&t);
        }
    }

    fn reopen_closed(&self) {
        let last = self.closed.borrow_mut().pop();
        if let Some(state) = last {
            let uri = if state.uri == settings::OVERVIEW {
                crate::tab::START_URI.to_string()
            } else {
                state.uri
            };
            self.open_uri(&uri, true);
        }
    }

    pub fn restore(&self, ws: &WindowState) {
        let mut selected = None;
        for (i, ts) in ws.tabs.iter().enumerate() {
            let Some(tab) = self.new_tab_widget() else {
                return;
            };
            let page = self.tab_view.append(&tab.root);
            if i == ws.selected {
                tab.load(&ts.uri);
                selected = Some(page);
            } else {
                tab.set_lazy(&ts.uri, &ts.title);
            }
            self.setup_page(&tab);
        }
        if let Some(p) = selected {
            self.tab_view.set_selected_page(&p);
        }
    }

    pub fn session_state(&self) -> WindowState {
        let mut tabs = Vec::new();
        let mut selected = 0;
        let current = self.tab_view.selected_page();
        for i in 0..self.tab_view.n_pages() {
            let page = self.tab_view.nth_page(i);
            let Some(t) = self.tab_for_page(&page) else {
                continue;
            };
            if let Some(state) = app::tab_state(&t) {
                if Some(&page) == current.as_ref() {
                    selected = tabs.len();
                }
                tabs.push(state);
            }
        }
        WindowState { tabs, selected }
    }

    fn selection_changed(&self) {
        if let Some(t) = self.current() {
            t.realize_lazy();
            if self.find_bar.is_search_mode() {
                self.find_changed();
            }
        }
        self.editing.set(false);
        self.sync();
        if let Some(app) = self.app() {
            app.schedule_session_save();
        }
    }

    pub fn set_page_fullscreen(&self, on: bool) {
        if on {
            self.find_bar.set_search_mode(false);
        }
    }

    // ---- state sync -------------------------------------------------------

    fn refresh_page(&self, tab: &Tab) {
        let Some(page) = self.page_of(tab) else {
            return;
        };
        page.set_title(&tab.title());
        page.set_tooltip(&glib::markup_escape_text(&display_uri(&tab.uri())));
        page.set_loading(tab.view.is_loading());
        match tab.view.favicon() {
            Some(tex) => page.set_icon(Some(&tex)),
            None => page.set_icon(None::<&gio::Icon>),
        }
        if tab.view.is_playing_audio() || tab.view.is_muted() {
            let icon = if tab.view.is_muted() {
                "audio-volume-muted-symbolic"
            } else {
                "audio-volume-high-symbolic"
            };
            page.set_indicator_icon(Some(&gio::ThemedIcon::new(icon)));
            page.set_indicator_tooltip(if tab.view.is_muted() {
                tr("Unmute Tab")
            } else {
                tr("Mute Tab")
            });
            page.set_indicator_activatable(true);
        } else {
            page.set_indicator_icon(None::<&gio::Icon>);
        }
    }

    pub fn tab_changed(&self, tab: &Rc<Tab>, what: Change) {
        if matches!(what, Change::Title | Change::Uri | Change::Loading) {
            self.refresh_page(tab);
        }
        let is_current = self.current().is_some_and(|c| Rc::ptr_eq(&c, tab));
        if !is_current {
            return;
        }
        match what {
            Change::Progress => self.sync_progress(),
            Change::Zoom => self.sync_zoom(),
            Change::Title => self.sync_title(),
            Change::Reader => self.sync_reader(),
            _ => self.sync(),
        }
    }

    pub fn sync(&self) {
        let tab = self.current();
        self.back
            .set_sensitive(tab.as_ref().is_some_and(|t| t.view.can_go_back()));
        self.forward
            .set_sensitive(tab.as_ref().is_some_and(|t| t.view.can_go_forward()));
        let loading = tab.as_ref().is_some_and(|t| t.view.is_loading());
        self.reload.set_icon_name(if loading {
            "process-stop-symbolic"
        } else {
            "view-refresh-symbolic"
        });
        self.reload
            .set_tooltip_text(Some(if loading { tr("Stop") } else { tr("Reload") }));
        helix_app_common::set_accessible_label(
            &self.reload,
            &self.reload.tooltip_text().unwrap_or_default(),
        );
        let has_home = self
            .app()
            .is_some_and(|a| a.settings.borrow().home_url().is_some() && a.web_app.is_none());
        self.home.set_visible(has_home);
        self.sync_reader();
        let web_page = tab.as_ref().is_some_and(|t| {
            let u = t.uri();
            u.starts_with("https://") || u.starts_with("http://")
        });
        if let Some(a) = self
            .win
            .lookup_action("install-web-app")
            .and_downcast::<gio::SimpleAction>()
        {
            a.set_enabled(web_page && !self.private);
        }
        if let Some(a) = self
            .win
            .lookup_action("site-info")
            .and_downcast::<gio::SimpleAction>()
        {
            a.set_enabled(self.app_title.is_none());
        }
        self.sync_entry();
        self.sync_progress();
        self.sync_zoom();
        self.sync_title();
        self.sync_bookmark_state();
    }

    fn sync_reader(&self) {
        let (available, active) = self
            .current()
            .map(|t| (t.reader_available(), t.reader_active.get()))
            .unwrap_or((false, false));
        self.reader.set_visible(available);
        self.reader.set_tooltip_text(Some(if active {
            tr("Leave Reader View")
        } else {
            tr("Reader View")
        }));
        // The name stays "Reader View"; the state says whether it is on.
        self.reader
            .update_state(&[gtk::accessible::State::Pressed(if active {
                gtk::AccessibleTristate::True
            } else {
                gtk::AccessibleTristate::False
            })]);
        if active {
            self.reader.add_css_class("suggested-action");
        } else {
            self.reader.remove_css_class("suggested-action");
        }
    }

    fn sync_title(&self) {
        let title = self.current().map(|t| t.title()).unwrap_or_default();
        if let (Some(t), Some(app)) = (&self.app_title, self.app()) {
            t.set_title(&title);
            t.set_subtitle(
                &app.web_app
                    .as_ref()
                    .and_then(|w| urlinput::host_of(&w.url))
                    .unwrap_or_default(),
            );
            self.win.set_title(Some(&title));
            return;
        }
        let suffix = if self.private {
            tr("Private Browsing")
        } else {
            tr("Web")
        };
        if title.is_empty() {
            self.win.set_title(Some(suffix));
        } else {
            // Translators: window title: page title, then "Web" or "Private Browsing".
            self.win.set_title(Some(&trf!(
                "{title} — {suffix}",
                title = title,
                suffix = suffix
            )));
        }
    }

    fn set_entry_text(&self, text: &str) {
        self.setting_entry.set(true);
        self.entry.set_text(text);
        self.setting_entry.set(false);
    }

    fn sync_entry(&self) {
        let Some(t) = self.current() else {
            self.set_entry_text("");
            return;
        };
        if !self.editing.get() {
            self.set_entry_text(&display_uri(&t.uri()));
        }
        let (icon, tip) = security_icon_and_text(t.security.get(), t.is_start_page());
        self.site_button.set_icon_name(icon);
        self.site_button.set_tooltip_text(Some(tip));
        // Screen readers hear the connection security with the address
        // and with the Site Information button.
        self.site_button
            .update_property(&[gtk::accessible::Property::Description(tip)]);
        self.entry
            .update_property(&[gtk::accessible::Property::Description(tip)]);
    }

    fn sync_progress(&self) {
        let p = self
            .current()
            .filter(|t| t.view.is_loading())
            .map(|t| t.view.estimated_load_progress())
            .unwrap_or(0.0);
        self.entry
            .set_progress_fraction(if p >= 1.0 { 0.0 } else { p });
    }

    fn sync_zoom(&self) {
        let z = self.current().map(|t| t.view.zoom_level()).unwrap_or(1.0);
        let level = format!("{:.0}%", z * 100.0);
        self.zoom_label.set_text(&level);
        self.zoom_reset
            .update_property(&[gtk::accessible::Property::Description(&trf!(
                "Current zoom {level}",
                level = level
            ))]);
    }

    pub fn sync_bookmark_state(&self) {
        let Some(app) = self.app() else { return };
        let uri = self.current().map(|t| t.uri()).unwrap_or_default();
        let bookmarkable = bookmarkable(&uri);
        let marked = bookmarkable && app.bookmarks.borrow().contains(&uri);
        self.star_button.set_visible(bookmarkable);
        if marked {
            helix_app_common::set_icon_and_label(
                &self.star_button,
                "starred-symbolic",
                tr("Edit Bookmark"),
            );
        } else {
            helix_app_common::set_icon_and_label(
                &self.star_button,
                "non-starred-symbolic",
                tr("Bookmark This Page"),
            );
        }
    }

    fn zoom(&self, dir: i32) {
        let default = self
            .app()
            .map(|a| a.settings.borrow().default_zoom)
            .unwrap_or(1.0);
        self.with_view(|v| {
            let z = if dir == 0 {
                default
            } else {
                settings::zoom_step(v.zoom_level(), dir)
            };
            v.set_zoom_level(z);
        });
    }

    // ---- address entry ----------------------------------------------------

    fn entry_key(&self, key: gdk::Key, mods: gdk::ModifierType) -> glib::Propagation {
        let visible = self.suggest_pop.is_visible();
        match key {
            gdk::Key::Down | gdk::Key::Up if visible => {
                let n = self.suggestions.borrow().len() as i32;
                if n == 0 {
                    return glib::Propagation::Stop;
                }
                let cur = self
                    .suggest_list
                    .selected_row()
                    .map(|r| r.index())
                    .unwrap_or(-1);
                let next = if key == gdk::Key::Down {
                    (cur + 1).min(n - 1)
                } else {
                    (cur - 1).max(-1)
                };
                match self.suggest_list.row_at_index(next) {
                    Some(row) if next >= 0 => {
                        self.suggest_list.select_row(Some(&row));
                        if let Some(url) = self.suggestions.borrow().get(next as usize) {
                            self.set_entry_text(url);
                            self.entry.set_position(-1);
                        }
                        // Focus stays in the entry: point at the row and
                        // read it (with its type) out.
                        self.entry
                            .update_relation(&[gtk::accessible::Relation::ActiveDescendant(
                                row.upcast_ref::<gtk::Accessible>(),
                            )]);
                        if let Some(label) = self.suggestion_labels.borrow().get(next as usize) {
                            helix_app_common::announce(
                                &self.entry,
                                label,
                                helix_app_common::Urgency::Polite,
                            );
                        }
                    }
                    _ => {
                        self.suggest_list.unselect_all();
                        self.entry
                            .reset_relation(gtk::AccessibleRelation::ActiveDescendant);
                    }
                }
                glib::Propagation::Stop
            }
            gdk::Key::Escape => {
                self.suggest_pop.popdown();
                self.editing.set(false);
                self.sync_entry();
                self.with_view(|v| {
                    v.grab_focus();
                });
                glib::Propagation::Stop
            }
            gdk::Key::Return | gdk::Key::KP_Enter | gdk::Key::ISO_Enter
                if mods
                    .intersects(gdk::ModifierType::ALT_MASK | gdk::ModifierType::CONTROL_MASK) =>
            {
                self.entry_activated(true);
                glib::Propagation::Stop
            }
            _ => glib::Propagation::Proceed,
        }
    }

    fn entry_activated(&self, new_tab: bool) {
        let selected = self
            .suggest_list
            .selected_row()
            .filter(|_| self.suggest_pop.is_visible());
        if let Some(row) = selected {
            self.activate_suggestion(row.index() as usize, new_tab);
            return;
        }
        let Some(app) = self.app() else { return };
        let text = self.entry.text();
        let Some(uri) = app.resolve_input(&text) else {
            return;
        };
        self.navigate(&uri, new_tab);
    }

    fn activate_suggestion(&self, index: usize, new_tab: bool) {
        let url = self.suggestions.borrow().get(index).cloned();
        if let Some(url) = url {
            self.navigate(&url, new_tab);
        }
    }

    fn navigate(&self, uri: &str, new_tab: bool) {
        self.suggest_pop.popdown();
        self.editing.set(false);
        match self.current().filter(|_| !new_tab) {
            Some(t) => t.load(uri),
            None => {
                self.open_uri(uri, true);
            }
        }
        self.sync_entry();
        self.with_view(|v| {
            v.grab_focus();
        });
    }

    fn update_suggestions(&self) {
        let Some(app) = self.app() else { return };
        let text = self.entry.text().to_string();
        while let Some(row) = self.suggest_list.row_at_index(0) {
            self.suggest_list.remove(&row);
        }
        let mut urls: Vec<String> = Vec::new();
        let query = text.trim();
        if query.is_empty() {
            self.suggestions.replace(urls);
            self.suggest_pop.popdown();
            return;
        }
        let home = glib::home_dir().to_string_lossy().into_owned();
        let engine = app.settings.borrow().engine();
        let mut rows: Vec<(String, String, SuggestionKind, String)> = Vec::new();
        match urlinput::classify(query, &home) {
            Some(urlinput::Input::Search(q)) => rows.push((
                trf!("Search {} for “{}”", engine.name, q),
                String::new(),
                SuggestionKind::Search,
                app.resolve_input(query)
                    .unwrap_or_else(|| engine.search_url(&q)),
            )),
            Some(urlinput::Input::Url(u)) => {
                rows.push((u.clone(), String::new(), SuggestionKind::Address, u))
            }
            None => {}
        }
        for b in app.bookmarks.borrow().search(query).into_iter().take(3) {
            let title = if b.title.is_empty() {
                b.url.clone()
            } else {
                b.title.clone()
            };
            rows.push((title, b.url.clone(), SuggestionKind::Bookmark, b.url));
        }
        if !self.private || app.history.borrow().len() > 0 {
            for e in app
                .history
                .borrow()
                .suggest(query, MAX_SUGGESTIONS, store::now())
            {
                let title = if e.title.is_empty() {
                    e.url.clone()
                } else {
                    e.title.clone()
                };
                rows.push((title, e.url.clone(), SuggestionKind::History, e.url));
            }
        }
        let mut seen = std::collections::HashSet::new();
        let mut labels = Vec::new();
        for (title, subtitle, kind, url) in rows {
            if urls.len() > MAX_SUGGESTIONS || !seen.insert(url.clone()) {
                continue;
            }
            let (content, _) = two_line_row(&title, &subtitle, kind.icon());
            let label = suggestion_label(kind, &title, &subtitle);
            let row = gtk::ListBoxRow::builder()
                .child(&content)
                .focusable(false)
                .build();
            helix_app_common::set_accessible_label(&row, &label);
            self.suggest_list.append(&row);
            urls.push(url);
            labels.push(label);
        }
        self.suggestion_labels.replace(labels);
        helix_app_common::announce_settled(&self.suggest_list, &suggestion_count(urls.len()));
        self.entry
            .reset_relation(gtk::AccessibleRelation::ActiveDescendant);
        self.suggestions.replace(urls);
        let width = self.entry.width().max(320);
        self.suggest_list.set_size_request(width - 24, -1);
        self.suggest_pop.popup();
        // Keep typing in the entry.
        self.entry.grab_focus_without_selecting();
    }

    // ---- site info & bookmarks -------------------------------------------

    fn show_site_info(&self) {
        let (Some(app), Some(t)) = (self.app(), self.current()) else {
            return;
        };
        // Web applications have no address bar to anchor to.
        if self.app_title.is_some() {
            return;
        }
        let uri = t.uri();
        let vbox = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(8)
            .margin_start(6)
            .margin_end(6)
            .margin_top(6)
            .margin_bottom(6)
            .width_request(300)
            .build();
        let host = urlinput::host_of(&uri).unwrap_or_else(|| display_uri(&uri));
        let host = if host.is_empty() {
            tr("Start Page").to_string()
        } else {
            host
        };
        let heading = gtk::Label::builder()
            .label(&host)
            .xalign(0.0)
            .ellipsize(gtk::pango::EllipsizeMode::Middle)
            .build();
        heading.add_css_class("heading");
        vbox.append(&heading);
        let status = match t.security.get() {
            _ if t.is_start_page() => tr("This is a page built into Web."),
            Security::Secure => tr("Your connection to this site is secure."),
            Security::Insecure => tr("Your connection to this site is not encrypted. Do not enter passwords or other sensitive information."),
            Security::Broken => tr("This site's security certificate has problems. The connection may not be private."),
            Security::Local => tr("This page is stored on your computer."),
        };
        vbox.append(
            &gtk::Label::builder()
                .label(status)
                .xalign(0.0)
                .wrap(true)
                .max_width_chars(40)
                .build(),
        );
        if let Some((cert, flags)) = t.view.tls_info() {
            let mut lines = Vec::new();
            if let Some(s) = cert.subject_name() {
                lines.push(trf!("Issued to: {s}", s = s));
            }
            if let Some(s) = cert.issuer_name() {
                lines.push(trf!("Issued by: {s}", s = s));
            }
            if let Some(d) = cert.not_valid_after().and_then(|d| d.format("%x").ok()) {
                lines.push(trf!("Expires: {d}", d = d));
            }
            if !flags.is_empty() {
                lines.push(trf!("Problems: {list}", list = tls_problems(flags)));
            }
            let l = gtk::Label::builder()
                .label(lines.join("\n"))
                .xalign(0.0)
                .wrap(true)
                .selectable(true)
                .build();
            l.add_css_class("dim-label");
            l.add_css_class("caption");
            vbox.append(&l);
        }
        if let Some(origin) = urlinput::origin_of(&uri) {
            let perms = app.permissions.borrow().for_origin(&origin);
            if !perms.is_empty() {
                let h = gtk::Label::builder()
                    .label(tr("Permissions"))
                    .xalign(0.0)
                    .build();
                h.add_css_class("heading");
                vbox.append(&h);
                let resets: Rc<RefCell<Vec<gtk::Button>>> = Rc::default();
                for (kind, allowed) in perms {
                    let row = gtk::Box::builder().spacing(6).build();
                    let l = gtk::Label::builder()
                        .label(format!(
                            "{}: {}",
                            kind.label(),
                            if allowed { tr("Allowed") } else { tr("Denied") }
                        ))
                        .xalign(0.0)
                        .hexpand(true)
                        .build();
                    let reset = gtk::Button::with_label(tr("Reset"))
                        .labelled(&trf!("Reset {} Permission", kind.label()));
                    reset.add_css_class("flat");
                    resets.borrow_mut().push(reset.clone());
                    let weak_app = Rc::downgrade(&app);
                    let origin = origin.clone();
                    let row_c = row.clone();
                    let resets = resets.clone();
                    let pop = self.info_pop.clone();
                    reset.connect_clicked(move |b| {
                        if let Some(app) = weak_app.upgrade() {
                            app.permissions.borrow_mut().reset(&origin, kind);
                            app.save_permissions();
                        }
                        // Keep keyboard focus on a remaining Reset button
                        // (the next one, else the previous one).
                        let next = {
                            let list = resets.borrow();
                            let at = list.iter().position(|r| r == b).unwrap_or(0);
                            list[at + 1..]
                                .iter()
                                .chain(list[..at].iter().rev())
                                .find(|r| r.parent().is_some_and(|p| p.is_visible()))
                                .cloned()
                        };
                        row_c.set_visible(false);
                        helix_app_common::announce(
                            &pop,
                            &trf!("{} permission reset", kind.label()),
                            helix_app_common::Urgency::Polite,
                        );
                        match next {
                            Some(r) => {
                                r.grab_focus();
                            }
                            None => pop.popdown(),
                        }
                    });
                    row.append(&l);
                    row.append(&reset);
                    vbox.append(&row);
                }
            }
        }
        self.info_pop.set_child(Some(&vbox));
        self.info_pop.popup();
        // The summary is not focusable; say it when the popover opens.
        helix_app_common::announce(
            &self.info_pop,
            // Translators: announced when the site information opens: the
            // site's host name, then its security status sentence.
            &trf!("{host}. {status}", host = host, status = status),
            helix_app_common::Urgency::Polite,
        );
    }

    fn bookmark_current(&self) {
        let (Some(app), Some(t)) = (self.app(), self.current()) else {
            return;
        };
        let uri = t.uri();
        if !bookmarkable(&uri) || t.is_start_page() || self.app_title.is_some() {
            return;
        }
        let existing = app.bookmarks.borrow().get(&uri).cloned();
        let bm = match existing {
            Some(b) => b,
            None => {
                let b = Bookmark {
                    url: uri.clone(),
                    title: t.title(),
                    tags: Vec::new(),
                    added: store::now(),
                };
                app.bookmarks.borrow_mut().add(b.clone());
                app.bookmarks_changed();
                b
            }
        };
        self.bm_title.set_text(&bm.title);
        self.bm_tags.set_text(&bm.tags.join(", "));
        self.bm_editing.replace(Some(uri));
        self.bm_pop.popup();
        self.bm_title.grab_focus();
    }

    fn bookmark_popover_closed(&self) {
        let Some(url) = self.bm_editing.take() else {
            return;
        };
        let Some(app) = self.app() else { return };
        let title = self.bm_title.text().to_string();
        let tags = bookmarks::parse_tags(&self.bm_tags.text());
        if app.bookmarks.borrow_mut().update(&url, &title, &tags) {
            app.bookmarks_changed();
        }
    }

    fn fill_bookmarks_list(&self) {
        let Some(app) = self.app() else { return };
        while let Some(row) = self.bm_list.row_at_index(0) {
            self.bm_list.remove(&row);
        }
        let query = self.bm_search.text();
        let items = app.bookmarks.borrow().search(&query);
        if items.is_empty() {
            let (icon, title, description) = bookmarks_empty_state(&query);
            self.bm_status.set_icon_name(Some(icon));
            self.bm_status.set_title(title);
            self.bm_status.set_description(Some(description));
            self.bm_stack.set_visible_child_name("empty");
            if !query.is_empty() {
                helix_app_common::announce_settled(&self.bm_status, title);
            }
            return;
        }
        self.bm_stack.set_visible_child_name("list");
        if !query.is_empty() {
            helix_app_common::announce_settled(&self.bm_list, &result_count(items.len()));
        }
        for b in items {
            let title = if b.title.is_empty() {
                b.url.clone()
            } else {
                b.title.clone()
            };
            let sub = if b.tags.is_empty() {
                b.url.clone()
            } else {
                format!("{} · {}", b.tags.join(", "), b.url)
            };
            let (content, _) = two_line_row(&title, &sub, "starred-symbolic");
            let remove = gtk::Button::builder()
                .icon_name("user-trash-symbolic")
                .tooltip_text(tr("Remove Bookmark"))
                .valign(gtk::Align::Center)
                .build()
                .labelled(&trf!("Remove Bookmark {title}", title = title));
            remove.add_css_class("flat");
            content.append(&remove);
            let row = gtk::ListBoxRow::builder()
                .child(&content)
                .name(b.url.as_str())
                .build();
            let url = b.url.clone();
            let cb = self.weak_fn(move |w, ()| {
                if let Some(app) = w.app() {
                    app.bookmarks.borrow_mut().remove(&url);
                    app.bookmarks_changed();
                }
                w.fill_bookmarks_list();
            });
            remove.connect_clicked(move |_| cb(()));
            self.bm_list.append(&row);
        }
    }

    // ---- find -------------------------------------------------------------

    fn find_options() -> u32 {
        (webkit6::FindOptions::CASE_INSENSITIVE | webkit6::FindOptions::WRAP_AROUND).bits()
    }

    fn find_changed(&self) {
        let text = self.find_entry.text().to_string();
        self.with_view(|v| {
            let Some(fc) = v.find_controller() else {
                return;
            };
            if text.is_empty() {
                fc.search_finish();
            } else {
                fc.count_matches(&text, Self::find_options(), u32::MAX);
                fc.search(&text, Self::find_options(), u32::MAX);
            }
        });
        if text.is_empty() {
            self.find_label.set_text("");
            self.set_find_error(false);
        }
    }

    fn find_step(&self, forward: bool) {
        if !self.find_bar.is_search_mode() {
            self.find_bar.set_search_mode(true);
            self.find_entry.grab_focus();
            return;
        }
        self.with_view(|v| {
            if let Some(fc) = v.find_controller() {
                if forward {
                    fc.search_next();
                } else {
                    fc.search_previous();
                }
            }
        });
    }

    pub fn find_result(&self, tab: &Rc<Tab>, matches: Option<u32>) {
        if !self.current().is_some_and(|c| Rc::ptr_eq(&c, tab)) || self.find_entry.text().is_empty()
        {
            return;
        }
        let text = match matches {
            Some(0) => tr("Not found").to_string(),
            Some(n) => trnf!("{n} match", "{n} matches", n),
            None => String::new(),
        };
        if !text.is_empty() && text != self.find_label.text() {
            helix_app_common::announce_settled(&self.find_label, &text);
        }
        self.find_label.set_text(&text);
        self.set_find_error(matches == Some(0));
    }

    /// "Not found" is shown in the error style and marks the search
    /// entry invalid (not by colour alone: the label says it).
    fn set_find_error(&self, error: bool) {
        if error {
            self.find_label.add_css_class("error");
            self.find_label.remove_css_class("dim-label");
            self.find_entry.add_css_class("error");
            self.find_entry
                .update_state(&[gtk::accessible::State::Invalid(
                    gtk::AccessibleInvalidState::True,
                )]);
        } else {
            self.find_label.remove_css_class("error");
            self.find_label.add_css_class("dim-label");
            self.find_entry.remove_css_class("error");
            self.find_entry.reset_state(gtk::AccessibleState::Invalid);
        }
    }

    // ---- downloads --------------------------------------------------------

    pub fn add_download(&self, d: &webkit6::Download, path: &Path) {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let row_box = gtk::Box::builder()
            .spacing(10)
            .margin_start(10)
            .margin_end(6)
            .margin_top(8)
            .margin_bottom(8)
            .build();
        row_box.append(&gtk::Image::from_icon_name("folder-download-symbolic"));
        let text = gtk::Box::new(gtk::Orientation::Vertical, 4);
        text.set_hexpand(true);
        let name_label = gtk::Label::builder()
            .label(&name)
            .xalign(0.0)
            .ellipsize(gtk::pango::EllipsizeMode::Middle)
            .build();
        let progress = gtk::ProgressBar::new();
        let status = gtk::Label::builder()
            .label(tr("Starting…"))
            .xalign(0.0)
            .build();
        status.add_css_class("dim-label");
        status.add_css_class("caption");
        text.append(&name_label);
        text.append(&progress);
        text.append(&status);
        row_box.append(&text);
        helix_app_common::label_widget(&progress, &name_label);
        let cancel = gtk::Button::builder()
            .icon_name("process-stop-symbolic")
            .tooltip_text(tr("Cancel"))
            .valign(gtk::Align::Center)
            .build()
            .labelled(&trf!("Cancel {name}", name = name));
        let open = gtk::Button::builder()
            .icon_name("document-open-symbolic")
            .tooltip_text(tr("Open"))
            .valign(gtk::Align::Center)
            .visible(false)
            .build()
            .labelled(&trf!("Open {name}", name = name));
        let folder = gtk::Button::builder()
            .icon_name("folder-open-symbolic")
            .tooltip_text(tr("Open Folder"))
            .valign(gtk::Align::Center)
            .visible(false)
            .build()
            .labelled(&trf!("Open Folder of {name}", name = name));
        for b in [&cancel, &open, &folder] {
            b.add_css_class("flat");
            row_box.append(b);
        }
        let row = gtk::ListBoxRow::builder()
            .child(&row_box)
            .activatable(false)
            .build();
        self.dl_list.prepend(&row);
        self.dl_button.set_visible(true);
        if !self.dl_button.is_active() {
            self.dl_attention.set_visible(true);
            set_downloads_description(&self.dl_button, Some(tr("New downloads")));
        }
        helix_app_common::announce(
            &self.dl_button,
            &trf!("Downloading {name}", name = name),
            helix_app_common::Urgency::Polite,
        );

        let dc = d.clone();
        cancel.connect_clicked(move |_| dc.cancel());
        let file = gio::File::for_path(path);
        let win = self.win.clone();
        let f = file.clone();
        let me = self.this.clone();
        open.connect_clicked(move |_| {
            let me = me.clone();
            gtk::FileLauncher::new(Some(&f)).launch(
                Some(&win),
                None::<&gio::Cancellable>,
                move |r| {
                    if let (Err(e), Some(w)) = (r, me.upgrade()) {
                        if !is_dismissed(&e) {
                            w.toast(&trf!("Could not open the download: {}", e.message()));
                        }
                    }
                },
            );
        });
        let win = self.win.clone();
        let me = self.this.clone();
        folder.connect_clicked(move |_| {
            let me = me.clone();
            gtk::FileLauncher::new(Some(&file)).open_containing_folder(
                Some(&win),
                None::<&gio::Cancellable>,
                move |r| {
                    if let (Err(e), Some(w)) = (r, me.upgrade()) {
                        if !is_dismissed(&e) {
                            w.toast(&trf!("Could not open the folder: {}", e.message()));
                        }
                    }
                },
            );
        });

        let (p, s) = (progress.clone(), status.clone());
        d.connect_received_data(move |d, _| {
            p.set_fraction(d.estimated_progress());
            let got = d.received_data_length();
            let total = d.response().map(|r| r.content_length()).unwrap_or(0);
            let text = if total > 0 {
                format!("{} of {}", glib::format_size(got), glib::format_size(total))
            } else {
                glib::format_size(got).to_string()
            };
            s.set_text(&text);
        });
        let failed = Rc::new(Cell::new(false));
        let (p, s, c, o, fo, r, fl) = (
            progress.clone(),
            status.clone(),
            cancel.clone(),
            open.clone(),
            folder.clone(),
            row.clone(),
            failed.clone(),
        );
        d.connect_finished(move |_| {
            r.set_widget_name("finished");
            c.set_visible(false);
            if fl.get() {
                return;
            }
            p.set_fraction(1.0);
            s.set_text(tr("Finished"));
            helix_app_common::announce(
                &s,
                &trf!("Download finished: {name}", name = name),
                helix_app_common::Urgency::Polite,
            );
            o.set_visible(true);
            fo.set_visible(true);
        });
        let (s, c, r) = (status, cancel, row);
        d.connect_failed(move |_, err| {
            failed.set(true);
            r.set_widget_name("finished");
            c.set_visible(false);
            if err.matches(webkit6::DownloadError::CancelledByUser) {
                s.set_text(tr("Cancelled"));
            } else {
                s.set_text(&trf!("Failed: {}", err.message()));
                helix_app_common::announce(
                    &s,
                    &trf!("Download failed: {}", err.message()),
                    helix_app_common::Urgency::Assertive,
                );
            }
        });
    }
}

/// Sets (or clears) the accessible description of the Downloads button
/// and of the inner button that takes focus.
fn set_downloads_description(button: &gtk::MenuButton, text: Option<&str>) {
    let inner = button.first_child();
    for w in std::iter::once(button.clone().upcast::<gtk::Widget>()).chain(inner) {
        match text {
            Some(t) => w.update_property(&[gtk::accessible::Property::Description(t)]),
            None => w.reset_property(gtk::AccessibleProperty::Description),
        }
    }
}

/// Icon, title and description of the bookmarks popover with nothing to
/// list: no bookmarks at all, or none matching `query`.
fn bookmarks_empty_state(query: &str) -> (&'static str, &'static str, &'static str) {
    if query.trim().is_empty() {
        (
            "starred-symbolic",
            tr("No Bookmarks"),
            tr("Bookmark pages with the star in the address bar or Ctrl+D"),
        )
    } else {
        (
            "edit-find-symbolic",
            tr("No Results Found"),
            tr("Try a different search"),
        )
    }
}

/// "1 result", "3 results".
pub fn result_count(n: usize) -> String {
    trnf!("{n} result", "{n} results", n)
}

/// Stops a loading tab, loads a lazily restored one, else reloads.
fn reload_or_stop(t: &Tab) {
    if t.view.is_loading() {
        t.view.stop_loading();
    } else if t.pending_uri.borrow().is_some() {
        t.realize_lazy();
    } else {
        t.view.reload();
    }
}

/// The tab context menu's sections of (label, action) for a tab.
fn tab_menu_sections(pinned: bool, muted: bool) -> [Vec<(&'static str, &'static str)>; 3] {
    [
        vec![
            (tr("_Reload"), "win.tab-reload"),
            (tr("_Duplicate"), "win.tab-duplicate"),
        ],
        vec![
            (
                if pinned {
                    tr("Un_pin Tab")
                } else {
                    tr("_Pin Tab")
                },
                "win.tab-pin",
            ),
            (
                if muted {
                    tr("Un_mute Tab")
                } else {
                    tr("_Mute Tab")
                },
                "win.tab-mute",
            ),
            (tr("Move to New _Window"), "win.tab-move-to-new-window"),
        ],
        vec![
            (tr("Close _Other Tabs"), "win.tab-close-others"),
            (tr("Close Tabs to the _Right"), "win.tab-close-right"),
            (tr("_Close Tab"), "win.tab-close"),
        ],
    ]
}

/// Refills `menu` in place, so an open popover follows the change.
fn fill_tab_menu(menu: &gio::Menu, pinned: bool, muted: bool) {
    menu.remove_all();
    for section in tab_menu_sections(pinned, muted) {
        let s = gio::Menu::new();
        for (label, action) in section {
            s.append(Some(label), Some(action));
        }
        menu.append_section(None, &s);
    }
}

/// The index of the next focus stop after `at` (or before it, going
/// backwards) among `n` stops; from outside every stop, forward starts at
/// the first (the address bar) and backward at the last (the page).
fn next_stop(at: Option<usize>, n: usize, forward: bool) -> Option<usize> {
    if n == 0 {
        return None;
    }
    Some(match (at, forward) {
        (Some(i), true) => (i + 1) % n,
        (Some(i), false) => (i + n - 1) % n,
        (None, true) => 0,
        (None, false) => n - 1,
    })
}

/// Whether a page can be bookmarked (built-in pages cannot).
fn bookmarkable(uri: &str) -> bool {
    !uri.is_empty() && !uri.starts_with("helix-about:") && uri != "about:blank"
}

/// The Site Information button's icon and the spoken connection state.
fn security_icon_and_text(security: Security, start_page: bool) -> (&'static str, &'static str) {
    match security {
        _ if start_page => ("web-browser-symbolic", tr("Start page")),
        Security::Secure => ("channel-secure-symbolic", tr("Secure connection")),
        Security::Insecure => ("channel-insecure-symbolic", tr("Connection is not secure")),
        Security::Broken => ("channel-insecure-symbolic", tr("Certificate problems")),
        Security::Local => ("text-x-generic-symbolic", tr("Local page")),
    }
}

/// Whether a launcher error only means the user closed the app chooser.
fn is_dismissed(e: &glib::Error) -> bool {
    e.matches(gtk::DialogError::Dismissed) || e.matches(gtk::DialogError::Cancelled)
}

#[cfg(test)]
mod tests {
    use super::{
        bookmarkable, bookmarks_empty_state, next_stop, result_count, security_icon_and_text,
        suggestion_count, suggestion_label, tab_menu_sections, SuggestionKind,
    };

    #[test]
    fn empty_bookmarks_and_no_results_differ() {
        assert_eq!(bookmarks_empty_state("").1, "No Bookmarks");
        assert_eq!(bookmarks_empty_state("  ").1, "No Bookmarks");
        assert_eq!(bookmarks_empty_state("rust").1, "No Results Found");
        assert_eq!(result_count(1), "1 result");
        assert_eq!(result_count(0), "0 results");
    }
    use crate::tab::{prompt_announcement, Security};

    #[test]
    fn tab_menu_follows_the_tab() {
        let labels = |pinned, muted| -> Vec<&str> {
            tab_menu_sections(pinned, muted)
                .iter()
                .flatten()
                .map(|(l, _)| *l)
                .collect()
        };
        assert_eq!(
            labels(false, false),
            [
                "_Reload",
                "_Duplicate",
                "_Pin Tab",
                "_Mute Tab",
                "Move to New _Window",
                "Close _Other Tabs",
                "Close Tabs to the _Right",
                "_Close Tab"
            ]
        );
        let l = labels(true, true);
        assert!(l.contains(&"Un_pin Tab") && l.contains(&"Un_mute Tab"));
        // Every item is a window action this file installs.
        let src = include_str!("window.rs");
        for (_, action) in tab_menu_sections(false, false).iter().flatten() {
            let name = action.strip_prefix("win.").unwrap();
            assert!(src.contains(&format!("add(\"{name}\"")), "{action}");
        }
    }

    #[test]
    fn f6_cycles_through_address_bar_prompts_and_page() {
        // Address bar, one prompt bar, page.
        assert_eq!(next_stop(Some(0), 3, true), Some(1));
        assert_eq!(next_stop(Some(2), 3, true), Some(0));
        assert_eq!(next_stop(Some(0), 3, false), Some(2));
        assert_eq!(next_stop(None, 3, true), Some(0));
        assert_eq!(next_stop(None, 3, false), Some(2));
        assert_eq!(next_stop(None, 0, true), None);
        assert_eq!(
            prompt_announcement("Allow example.org to use your camera?"),
            "Allow example.org to use your camera? Press F6 to answer."
        );
        assert_eq!(
            prompt_announcement("Save password for a@b"),
            "Save password for a@b. Press F6 to answer."
        );
    }

    #[test]
    fn security_is_spoken_and_built_in_pages_are_not_bookmarkable() {
        assert_eq!(
            security_icon_and_text(Security::Broken, false).1,
            "Certificate problems"
        );
        assert_eq!(
            security_icon_and_text(Security::Secure, true).1,
            "Start page"
        );
        assert!(bookmarkable("https://gnome.org/"));
        assert!(!bookmarkable("helix-about:overview"));
        assert!(!bookmarkable("about:blank"));
        assert!(!bookmarkable(""));
    }

    #[test]
    fn suggestion_rows_say_their_type() {
        assert_eq!(
            suggestion_label(SuggestionKind::Bookmark, "GNOME", "https://gnome.org/"),
            "GNOME, Bookmark, https://gnome.org/"
        );
        assert_eq!(
            suggestion_label(
                SuggestionKind::History,
                "https://a.example/",
                "https://a.example/"
            ),
            "https://a.example/, History"
        );
        assert_eq!(
            suggestion_label(SuggestionKind::Search, "Search DuckDuckGo for “x”", ""),
            "Search DuckDuckGo for “x”"
        );
        assert_eq!(suggestion_count(1), "1 suggestion");
        assert_eq!(suggestion_count(4), "4 suggestions");
    }
}
