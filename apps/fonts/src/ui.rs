//! GTK user interface.

use adw::prelude::*;
use gtk::{gio, glib, pango};
use helix_app_common::Labelled;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;

use crate::fclist::{self, Family};
use crate::fontinfo::{self, FaceFacts, FaceInfo};
use crate::install;
use crate::render::{self, FontData};

pub const APP_ID: &str = "org.helix.Fonts";
const PREVIEW_SIZES: [u32; 8] = [8, 10, 12, 18, 24, 36, 48, 72];

/// One face shown on a font page.
#[derive(Clone, Debug)]
struct FaceSource {
    file: PathBuf,
    index: u32,
    family: String,
    style: String,
    format: String,
}

impl FaceSource {
    fn from_entry(e: &fclist::FaceEntry) -> Self {
        FaceSource {
            file: e.file.clone(),
            index: e.index,
            family: e.family.clone(),
            style: e.style.clone(),
            format: e.format.clone(),
        }
    }
}

/// Per-window context.
struct Win {
    window: adw::ApplicationWindow,
    nav: adw::NavigationView,
    toasts: adw::ToastOverlay,
    /// Reloads the family list (list windows only).
    reload: RefCell<Option<Rc<dyn Fn()>>>,
}

impl Win {
    fn toast(&self, msg: &str) {
        self.toasts.add_toast(adw::Toast::new(msg));
    }
}

thread_local! {
    static WINDOWS: RefCell<Vec<Rc<Win>>> = const { RefCell::new(Vec::new()) };
}

pub fn run(files: Vec<PathBuf>) -> glib::ExitCode {
    let app = adw::Application::builder()
        .application_id(APP_ID)
        .flags(gio::ApplicationFlags::NON_UNIQUE)
        .build();
    helix_app_common::install_standard_actions(&app, "Fonts", "View and install fonts");
    app.connect_startup(|_| load_css());
    let files = RefCell::new(files);
    app.connect_activate(move |app| {
        let files = files.take();
        if files.is_empty() {
            new_window(app, None).window.present();
        } else {
            for f in files {
                new_window(app, Some(f)).window.present();
            }
        }
    });
    app.set_accels_for_action("win.open", &["<Control>o"]);
    app.set_accels_for_action("win.search", &["<Control>f"]);
    app.run_with_args(&[std::env::args().next().unwrap_or_default()])
}

fn load_css() {
    let css = gtk::CssProvider::new();
    css.load_from_string(
        ".font-tile { padding: 8px; }
         .font-tile-sample { font-size: 34pt; min-height: 64px; }
         .font-size-label { font-size: 9pt; min-width: 28px; }
         .glyph-cell { border-radius: 6px; }
         .glyph-cell:hover { background: alpha(currentColor, 0.08); }",
    );
    if let Some(display) = gtk::gdk::Display::default() {
        gtk::style_context_add_provider_for_display(
            &display,
            &css,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    }
}

fn main_menu() -> gtk::MenuButton {
    let menu = gio::Menu::new();
    menu.append(Some("_Open…"), Some("win.open"));
    let section = gio::Menu::new();
    section.append(Some("_About Fonts"), Some("app.about"));
    section.append(Some("_Quit"), Some("app.quit"));
    menu.append_section(None, &section);
    gtk::MenuButton::builder()
        .icon_name("open-menu-symbolic")
        .menu_model(&menu)
        .tooltip_text("Main Menu")
        .primary(true)
        .build()
        .labelled("Main Menu")
}

fn new_window(app: &adw::Application, file: Option<PathBuf>) -> Rc<Win> {
    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("Fonts")
        .default_width(900)
        .default_height(680)
        .build();
    let nav = adw::NavigationView::new();
    let toasts = adw::ToastOverlay::new();
    toasts.set_child(Some(&nav));
    window.set_content(Some(&toasts));
    let win = Rc::new(Win {
        window,
        nav,
        toasts,
        reload: RefCell::new(None),
    });
    // Keep the window context alive while the window is open; widgets and
    // actions only hold weak references to it.
    WINDOWS.with(|w| w.borrow_mut().push(win.clone()));
    win.window.connect_close_request(|window| {
        WINDOWS.with(|w| w.borrow_mut().retain(|x| &x.window != window));
        glib::Propagation::Proceed
    });

    let open = gio::SimpleAction::new("open", None);
    let w = Rc::downgrade(&win);
    open.connect_activate(move |_, _| {
        if let Some(win) = w.upgrade() {
            open_dialog(&win);
        }
    });
    win.window.add_action(&open);

    match file {
        None => {
            let page = list_page(&win);
            win.nav.add(&page);
        }
        Some(path) => {
            let page = adw::NavigationPage::builder()
                .title("Fonts")
                .tag("loading")
                .build();
            let tv = adw::ToolbarView::new();
            tv.add_top_bar(&adw::HeaderBar::new());
            let spinner = gtk::Spinner::builder()
                .spinning(true)
                .width_request(32)
                .height_request(32)
                .halign(gtk::Align::Center)
                .valign(gtk::Align::Center)
                .build();
            tv.set_content(Some(&spinner));
            page.set_child(Some(&tv));
            win.nav.add(&page);
            let w = win.clone();
            glib::spawn_future_local(async move {
                match read_faces(path.clone()).await {
                    Ok(faces) => {
                        let fp = font_page(&w, faces, 0, Some(path), true);
                        w.nav.replace(&[fp]);
                    }
                    Err(e) => {
                        let status = adw::StatusPage::builder()
                            .icon_name("dialog-error-symbolic")
                            .title("Could Not Open Font")
                            .description(
                                glib::markup_escape_text(&format!("{}: {e}", path.display()))
                                    .as_str(),
                            )
                            .build();
                        tv.set_content(Some(&status));
                    }
                }
            });
        }
    }
    win
}

/// Reads all faces in a file on a worker thread.
async fn read_faces(path: PathBuf) -> Result<Vec<FaceSource>, String> {
    let p = path.clone();
    let res = gio::spawn_blocking(move || fontinfo::read_file(&p).map_err(|e| e.to_string()))
        .await
        .map_err(|_| "worker failed".to_string())??;
    Ok(res
        .into_iter()
        .map(|f| FaceSource {
            file: path.clone(),
            index: f.index,
            family: f.family,
            style: f.style,
            format: f.format,
        })
        .collect())
}

fn open_dialog(win: &Rc<Win>) {
    let filter = gtk::FileFilter::new();
    filter.set_name(Some("Font Files"));
    for m in [
        "font/ttf",
        "font/otf",
        "font/collection",
        "application/x-font-ttf",
        "application/x-font-otf",
        "font/sfnt",
    ] {
        filter.add_mime_type(m);
    }
    for p in [
        "*.ttf", "*.otf", "*.ttc", "*.otc", "*.TTF", "*.OTF", "*.TTC", "*.OTC",
    ] {
        filter.add_pattern(p);
    }
    let filters = gio::ListStore::new::<gtk::FileFilter>();
    filters.append(&filter);
    let dialog = gtk::FileDialog::builder()
        .title("Open Font")
        .modal(true)
        .filters(&filters)
        .default_filter(&filter)
        .build();
    let win = win.clone();
    glib::spawn_future_local(async move {
        let Ok(file) = dialog.open_future(Some(&win.window)).await else {
            return;
        };
        let Some(path) = file.path() else {
            win.toast("Only local files can be opened");
            return;
        };
        match read_faces(path.clone()).await {
            Ok(faces) => {
                let page = font_page(&win, faces, 0, Some(path), false);
                win.nav.push(&page);
            }
            Err(e) => win.toast(&format!(
                "Could not open “{}”: {e}",
                path.file_name().unwrap_or_default().to_string_lossy()
            )),
        }
    });
}

// ---------------------------------------------------------------------------
// Font list

fn list_page(win: &Rc<Win>) -> adw::NavigationPage {
    let header = adw::HeaderBar::new();
    let search_btn = gtk::ToggleButton::builder()
        .icon_name("system-search-symbolic")
        .tooltip_text("Search")
        .build()
        .labelled("Search");
    let open_btn = gtk::Button::builder()
        .icon_name("document-open-symbolic")
        .tooltip_text("Open Font File")
        .action_name("win.open")
        .build()
        .labelled("Open Font File");
    header.pack_start(&open_btn);
    header.pack_end(&main_menu());
    header.pack_end(&search_btn);

    let entry = gtk::SearchEntry::builder()
        .placeholder_text("Search fonts")
        .hexpand(true)
        .build();
    let clamp = adw::Clamp::builder()
        .maximum_size(500)
        .child(&entry)
        .build();
    let bar = gtk::SearchBar::builder().child(&clamp).build();
    bar.connect_entry(&entry);
    bar.set_key_capture_widget(Some(&win.window));
    search_btn
        .bind_property("active", &bar, "search-mode-enabled")
        .bidirectional()
        .sync_create()
        .build();

    let search = gio::SimpleAction::new("search", None);
    let sb = search_btn.downgrade();
    let w = Rc::downgrade(win);
    search.connect_activate(move |_, _| {
        let (Some(b), Some(win)) = (sb.upgrade(), w.upgrade()) else {
            return;
        };
        // Only meaningful while the list is visible.
        if win.nav.visible_page().and_then(|p| p.tag()).as_deref() == Some("list") {
            b.set_active(!b.is_active());
        }
    });
    win.window.add_action(&search);

    let store = gio::ListStore::new::<glib::BoxedAnyObject>();
    let query = Rc::new(RefCell::new(String::new()));
    let q = query.clone();
    let filter = gtk::CustomFilter::new(move |obj| {
        let q = q.borrow();
        q.is_empty()
            || obj
                .downcast_ref::<glib::BoxedAnyObject>()
                .is_some_and(|b| b.borrow::<Family>().matches(&q))
    });
    let filtered = gtk::FilterListModel::new(Some(store.clone()), Some(filter.clone()));

    let factory = gtk::SignalListItemFactory::new();
    factory.connect_setup(|_, item| {
        let item = item.downcast_ref::<gtk::ListItem>().unwrap();
        let sample = gtk::Label::builder()
            .label("Aa")
            .css_classes(["font-tile-sample"])
            .build();
        let name = gtk::Label::builder()
            .ellipsize(pango::EllipsizeMode::End)
            .max_width_chars(16)
            .build();
        let styles = gtk::Label::builder()
            .css_classes(["dim-label", "caption"])
            .build();
        let b = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(4)
            .width_request(150)
            .css_classes(["font-tile"])
            .build();
        b.append(&sample);
        b.append(&name);
        b.append(&styles);
        item.set_child(Some(&b));
    });
    factory.connect_bind(|_, item| {
        let item = item.downcast_ref::<gtk::ListItem>().unwrap();
        let (Some(obj), Some(child)) = (
            item.item().and_downcast::<glib::BoxedAnyObject>(),
            item.child(),
        ) else {
            return;
        };
        let fam = obj.borrow::<Family>();
        let sample = child.first_child().and_downcast::<gtk::Label>().unwrap();
        let name = sample.next_sibling().and_downcast::<gtk::Label>().unwrap();
        let styles = name.next_sibling().and_downcast::<gtk::Label>().unwrap();
        let attrs = pango::AttrList::new();
        let mut desc = pango::FontDescription::new();
        desc.set_family(&fam.name);
        attrs.insert(pango::AttrFontDesc::new(&desc));
        sample.set_attributes(Some(&attrs));
        name.set_label(&fam.name);
        child.set_tooltip_text(Some(&fam.name));
        let n = fam.faces.len();
        styles.set_label(&if n == 1 {
            "1 style".to_string()
        } else {
            format!("{n} styles")
        });
    });
    let grid = gtk::GridView::builder()
        .model(&gtk::NoSelection::new(Some(filtered.clone())))
        .factory(&factory)
        .max_columns(24)
        .min_columns(1)
        .single_click_activate(true)
        .build();
    let w = Rc::downgrade(win);
    grid.connect_activate(move |grid, pos| {
        let (Some(win), Some(model)) = (w.upgrade(), grid.model()) else {
            return;
        };
        let Some(obj) = model.item(pos).and_downcast::<glib::BoxedAnyObject>() else {
            return;
        };
        let fam = obj.borrow::<Family>();
        let faces: Vec<FaceSource> = fam.faces.iter().map(FaceSource::from_entry).collect();
        let page = font_page(&win, faces, fclist::default_face(&fam), None, false);
        win.nav.push(&page);
    });
    let scrolled = gtk::ScrolledWindow::builder()
        .child(&grid)
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vexpand(true)
        .build();

    let stack = gtk::Stack::new();
    let spinner = gtk::Spinner::builder()
        .spinning(true)
        .width_request(32)
        .height_request(32)
        .halign(gtk::Align::Center)
        .valign(gtk::Align::Center)
        .build();
    stack.add_named(&spinner, Some("loading"));
    stack.add_named(&scrolled, Some("grid"));
    let empty = adw::StatusPage::builder()
        .icon_name("system-search-symbolic")
        .title("No Fonts Found")
        .description("Try a different search")
        .build();
    stack.add_named(&empty, Some("empty"));

    let update_visible = {
        let stack = stack.clone();
        let filtered = filtered.clone();
        move || {
            if stack.visible_child_name().as_deref() != Some("loading") {
                stack.set_visible_child_name(if filtered.n_items() == 0 {
                    "empty"
                } else {
                    "grid"
                });
            }
        }
    };
    let uv = update_visible.clone();
    filtered.connect_items_changed(move |_, _, _, _| uv());

    let f = filter.clone();
    entry.connect_search_changed(move |e| {
        *query.borrow_mut() = e.text().to_string();
        f.changed(gtk::FilterChange::Different);
    });

    let reload: Rc<dyn Fn()> = {
        let stack = stack.clone();
        let store = store.clone();
        Rc::new(move || {
            stack.set_visible_child_name("loading");
            let stack = stack.clone();
            let store = store.clone();
            let uv = update_visible.clone();
            glib::spawn_future_local(async move {
                let fams = gio::spawn_blocking(fclist::list_families)
                    .await
                    .unwrap_or_default();
                let objs: Vec<glib::BoxedAnyObject> =
                    fams.into_iter().map(glib::BoxedAnyObject::new).collect();
                store.splice(0, store.n_items(), &objs);
                stack.set_visible_child_name("grid");
                uv();
            });
        })
    };
    reload();
    *win.reload.borrow_mut() = Some(reload);

    let tv = adw::ToolbarView::new();
    tv.add_top_bar(&header);
    tv.add_top_bar(&bar);
    tv.set_content(Some(&stack));
    adw::NavigationPage::builder()
        .title("Fonts")
        .tag("list")
        .child(&tv)
        .build()
}

// ---------------------------------------------------------------------------
// Font page

/// Result of loading one face from disk.
struct Loaded {
    data: Option<Arc<Vec<u8>>>,
    info: Option<FaceInfo>,
    coverage: Vec<char>,
    size: Option<u64>,
}

fn load_face(path: &Path, index: u32, cached: Option<Arc<Vec<u8>>>) -> Loaded {
    let size = std::fs::metadata(path).ok().map(|m| m.len());
    let data = cached.or_else(|| std::fs::read(path).ok().map(Arc::new));
    let parsed = data.as_ref().and_then(|d| {
        let info = fontinfo::face_info(d, index)?;
        let face = ttf_parser::Face::parse(d, index).ok()?;
        Some((info, fontinfo::coverage(&face)))
    });
    match parsed {
        Some((info, coverage)) => Loaded {
            data,
            info: Some(info),
            coverage,
            size,
        },
        None => Loaded {
            data: None,
            info: None,
            coverage: vec![],
            size,
        },
    }
}

/// Shared state of a font page.
struct PageState {
    font: RefCell<Option<FontData>>,
    text: RefCell<String>,
    /// Whether the user typed their own preview text.
    edited: Cell<bool>,
    cache: RefCell<HashMap<PathBuf, Arc<Vec<u8>>>>,
    generation: Cell<u64>,
}

struct SizeRow {
    pt: u32,
    area: gtk::DrawingArea,
    label: gtk::Label,
}

fn pt_to_px(pt: u32) -> f64 {
    pt as f64 * 96.0 / 72.0
}

fn font_page(
    win: &Rc<Win>,
    faces: Vec<FaceSource>,
    initial: usize,
    install_path: Option<PathBuf>,
    root: bool,
) -> adw::NavigationPage {
    let faces = Rc::new(faces);
    let first = faces[initial.min(faces.len() - 1)].clone();
    let state = Rc::new(PageState {
        font: RefCell::new(None),
        text: RefCell::new(fontinfo::PANGRAM.to_string()),
        edited: Cell::new(false),
        cache: RefCell::new(HashMap::new()),
        generation: Cell::new(0),
    });

    // Header.
    let title = adw::WindowTitle::new(&first.family, &first.style);
    let header = adw::HeaderBar::builder().title_widget(&title).build();
    if root {
        let open_btn = gtk::Button::builder()
            .icon_name("document-open-symbolic")
            .tooltip_text("Open Font File")
            .action_name("win.open")
            .build()
            .labelled("Open Font File");
        header.pack_start(&open_btn);
    }
    header.pack_end(&main_menu());
    let dropdown = if faces.len() > 1 {
        let labels: Vec<String> = faces
            .iter()
            .map(|f| {
                if faces.iter().all(|g| g.family == faces[0].family) {
                    f.style.clone()
                } else {
                    format!("{} {}", f.family, f.style)
                }
            })
            .collect();
        let refs: Vec<&str> = labels.iter().map(String::as_str).collect();
        let dd = gtk::DropDown::from_strings(&refs);
        dd.set_selected(initial as u32);
        dd.set_tooltip_text(Some("Style"));
        header.pack_end(&dd);
        Some(dd)
    } else {
        None
    };
    if let Some(path) = install_path.clone() {
        let btn = gtk::Button::builder()
            .label("Install")
            .css_classes(["suggested-action"])
            .build();
        header.pack_end(&btn);
        let p = path.clone();
        let b = btn.clone();
        glib::spawn_future_local(async move {
            let installed = gio::spawn_blocking(move || {
                install::installed_location(
                    &p,
                    &fclist::fallback_dirs()[1..],
                    &fclist::user_fonts_dir(),
                )
                .is_some()
            })
            .await
            .unwrap_or(false);
            if installed {
                mark_installed(&b);
            }
        });
        let w = Rc::downgrade(win);
        btn.connect_clicked(move |b| {
            b.set_sensitive(false);
            b.set_label("Installing…");
            let p = path.clone();
            let b = b.clone();
            let w = w.clone();
            glib::spawn_future_local(async move {
                let res = gio::spawn_blocking(move || {
                    let dir = fclist::user_fonts_dir();
                    let r = install::install_into(&p, &dir)?;
                    let cache = install::refresh_cache(&dir);
                    Ok::<_, std::io::Error>((r, cache.err().map(|e| e.to_string())))
                })
                .await;
                let Some(win) = w.upgrade() else { return };
                match res {
                    Ok(Ok((r, cache_err))) => {
                        mark_installed(&b);
                        let name = r
                            .path()
                            .file_name()
                            .unwrap_or_default()
                            .to_string_lossy()
                            .to_string();
                        match cache_err {
                            None => win.toast(&format!("Installed “{name}”")),
                            Some(e) => win.toast(&format!(
                                "Installed “{name}”, but updating the font cache failed: {e}"
                            )),
                        }
                        let reload = win.reload.borrow().clone();
                        if let Some(reload) = reload {
                            reload();
                        }
                    }
                    Ok(Err(e)) => {
                        b.set_label("Install");
                        b.set_sensitive(true);
                        win.toast(&format!("Could not install font: {e}"));
                    }
                    Err(_) => {
                        b.set_label("Install");
                        b.set_sensitive(true);
                        win.toast("Could not install font");
                    }
                }
            });
        });
    }

    let stack = adw::ViewStack::new();
    let switcher = adw::ViewSwitcher::builder()
        .stack(&stack)
        .policy(adw::ViewSwitcherPolicy::Wide)
        .halign(gtk::Align::Center)
        .build();
    let switcher_bar = gtk::Box::builder()
        .halign(gtk::Align::Center)
        .margin_bottom(6)
        .build();
    switcher_bar.append(&switcher);

    // Preview view.
    let entry = gtk::Entry::builder()
        .text(fontinfo::PANGRAM)
        .placeholder_text("Preview text")
        .hexpand(true)
        .build();
    entry.set_tooltip_text(Some("Preview text"));
    let preview_box = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(6)
        .margin_top(12)
        .margin_bottom(24)
        .margin_start(18)
        .margin_end(18)
        .build();
    preview_box.append(&entry);
    let alpha_area = gtk::DrawingArea::builder()
        .margin_top(12)
        .margin_bottom(12)
        .build();
    let alpha_label = gtk::Label::builder()
        .label(fontinfo::ALPHABET)
        .xalign(0.0)
        .wrap(true)
        .margin_top(12)
        .margin_bottom(12)
        .build();
    preview_box.append(&alpha_area);
    preview_box.append(&alpha_label);
    {
        let st = state.clone();
        alpha_area.set_draw_func(move |a, cr, _, h| {
            let font = st.font.borrow();
            let Some(face) = font.as_ref().and_then(|f| f.face()) else {
                return;
            };
            let px = pt_to_px(18);
            let (asc, _, upem) = render::metrics(&face);
            let c = a.color();
            cr.set_source_rgba(
                c.red() as f64,
                c.green() as f64,
                c.blue() as f64,
                c.alpha() as f64,
            );
            let line_h = h as f64 / 2.0;
            let (l1, l2) = fontinfo::ALPHABET.split_at(fontinfo::ALPHABET.find(" 0").unwrap_or(0));
            render::draw_text(cr, &face, l1, 0.0, asc * px / upem, px);
            render::draw_text(
                cr,
                &face,
                l2.trim_start(),
                0.0,
                line_h + asc * px / upem,
                px,
            );
        });
    }
    let rows: Rc<Vec<SizeRow>> = Rc::new(
        PREVIEW_SIZES
            .iter()
            .map(|&pt| {
                let size_label = gtk::Label::builder()
                    .label(pt.to_string())
                    .css_classes(["dim-label", "font-size-label"])
                    .xalign(1.0)
                    .valign(gtk::Align::Center)
                    .build();
                let area = gtk::DrawingArea::builder().hexpand(true).build();
                let label = gtk::Label::builder()
                    .xalign(0.0)
                    .hexpand(true)
                    .single_line_mode(true)
                    .build();
                let row = gtk::Box::builder().spacing(12).build();
                row.append(&size_label);
                row.append(&area);
                row.append(&label);
                preview_box.append(&row);
                let st = state.clone();
                area.set_draw_func(move |a, cr, _, h| {
                    let font = st.font.borrow();
                    let Some(face) = font.as_ref().and_then(|f| f.face()) else {
                        return;
                    };
                    let px = pt_to_px(pt);
                    let (asc, desc, upem) = render::metrics(&face);
                    let line_h = (asc - desc) * px / upem;
                    let baseline = (h as f64 - line_h) / 2.0 + asc * px / upem;
                    let c = a.color();
                    cr.set_source_rgba(
                        c.red() as f64,
                        c.green() as f64,
                        c.blue() as f64,
                        c.alpha() as f64,
                    );
                    render::draw_text(cr, &face, &st.text.borrow(), 0.0, baseline, px);
                });
                SizeRow { pt, area, label }
            })
            .collect(),
    );
    let preview_scroll = gtk::ScrolledWindow::builder()
        .child(&preview_box)
        .vexpand(true)
        .build();
    stack.add_titled_with_icon(
        &preview_scroll,
        Some("preview"),
        "Preview",
        "font-x-generic-symbolic",
    );

    // Characters view.
    let chars_model = gtk::StringList::new(&[]);
    let chars_count = gtk::Label::builder()
        .xalign(0.0)
        .css_classes(["dim-label"])
        .margin_start(18)
        .margin_top(12)
        .margin_bottom(6)
        .build();
    let cfactory = gtk::SignalListItemFactory::new();
    {
        let st = state.clone();
        cfactory.connect_setup(move |_, item| {
            let item = item.downcast_ref::<gtk::ListItem>().unwrap();
            let area = gtk::DrawingArea::builder()
                .content_width(56)
                .content_height(56)
                .css_classes(["glyph-cell"])
                .build();
            let weak_item = item.downgrade();
            let st = st.clone();
            area.set_draw_func(move |a, cr, w, h| {
                let Some(item) = weak_item.upgrade() else {
                    return;
                };
                let Some(s) = item.item().and_downcast::<gtk::StringObject>() else {
                    return;
                };
                let Some(ch) = s.string().chars().next() else {
                    return;
                };
                let font = st.font.borrow();
                let Some(face) = font.as_ref().and_then(|f| f.raw_face()) else {
                    return;
                };
                let c = a.color();
                cr.set_source_rgba(
                    c.red() as f64,
                    c.green() as f64,
                    c.blue() as f64,
                    c.alpha() as f64,
                );
                render::draw_char_centered(cr, &face, ch, w as f64, h as f64, 30.0);
            });
            item.set_child(Some(&area));
        });
    }
    cfactory.connect_bind(|_, item| {
        let item = item.downcast_ref::<gtk::ListItem>().unwrap();
        let (Some(s), Some(child)) = (
            item.item().and_downcast::<gtk::StringObject>(),
            item.child(),
        ) else {
            return;
        };
        if let Some(ch) = s.string().chars().next() {
            child.set_tooltip_text(Some(&format!("U+{:04X}", ch as u32)));
        }
        child.queue_draw();
    });
    let cgrid = gtk::GridView::builder()
        .model(&gtk::NoSelection::new(Some(chars_model.clone())))
        .factory(&cfactory)
        .max_columns(64)
        .build();
    let cscroll = gtk::ScrolledWindow::builder()
        .child(&cgrid)
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vexpand(true)
        .build();
    let cbox = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .build();
    cbox.append(&chars_count);
    cbox.append(&cscroll);
    let cstack = gtk::Stack::new();
    cstack.add_named(&cbox, Some("grid"));
    let cunavailable = adw::StatusPage::builder()
        .icon_name("font-x-generic-symbolic")
        .title("Characters Unavailable")
        .description("The character map of this font format cannot be shown")
        .build();
    cstack.add_named(&cunavailable, Some("none"));
    stack.add_titled_with_icon(
        &cstack,
        Some("characters"),
        "Characters",
        "accessories-character-map-symbolic",
    );

    // Info view.
    let info_bin = adw::Bin::new();
    stack.add_titled_with_icon(&info_bin, Some("info"), "Info", "help-about-symbolic");

    // Preview text editing.
    {
        let st = state.clone();
        let rows = rows.clone();
        entry.connect_changed(move |e| {
            *st.text.borrow_mut() = e.text().to_string();
            st.edited.set(true);
            refresh_preview(&st, &rows, None);
        });
    }

    // Face loading.
    let select_face: Rc<dyn Fn(usize)> = {
        let st = state.clone();
        let faces = faces.clone();
        let rows = rows.clone();
        let entry = entry.clone();
        let title = title.clone();
        let alpha_area = alpha_area.clone();
        let alpha_label = alpha_label.clone();
        let chars_model = chars_model.clone();
        let chars_count = chars_count.clone();
        let cstack = cstack.clone();
        let info_bin = info_bin.clone();
        Rc::new(move |i: usize| {
            let Some(src) = faces.get(i).cloned() else {
                return;
            };
            title.set_title(&src.family);
            title.set_subtitle(&src.style);
            let generation = st.generation.get() + 1;
            st.generation.set(generation);
            let cached = st.cache.borrow().get(&src.file).cloned();
            let st = st.clone();
            let rows = rows.clone();
            let entry = entry.clone();
            let alpha_area = alpha_area.clone();
            let alpha_label = alpha_label.clone();
            let chars_model = chars_model.clone();
            let chars_count = chars_count.clone();
            let cstack = cstack.clone();
            let info_bin = info_bin.clone();
            glib::spawn_future_local(async move {
                let (file, index) = (src.file.clone(), src.index);
                let Ok(loaded) = gio::spawn_blocking(move || load_face(&file, index, cached)).await
                else {
                    return;
                };
                if st.generation.get() != generation {
                    return; // A newer selection superseded this one.
                }
                if let Some(d) = &loaded.data {
                    st.cache.borrow_mut().insert(src.file.clone(), d.clone());
                }
                *st.font.borrow_mut() = loaded.data.clone().map(|data| FontData {
                    data,
                    index: src.index,
                });

                if !st.edited.get() {
                    let sample = fontinfo::sample_text(&loaded.coverage);
                    entry.set_text(&sample);
                    st.edited.set(false);
                }
                let has_font = st.font.borrow().is_some();
                let show_alpha = loaded.coverage.is_empty()
                    || fontinfo::covers(&loaded.coverage, fontinfo::ALPHABET);
                alpha_area.set_visible(has_font && show_alpha);
                alpha_label.set_visible(!has_font && show_alpha);
                if has_font {
                    if let Some(face) = st.font.borrow().as_ref().and_then(|f| f.face()) {
                        let (w, h) =
                            render::measure(&face, "abcdefghijklmnopqrstuvwxyz", pt_to_px(18));
                        alpha_area.set_content_width(w.ceil() as i32 + 4);
                        alpha_area.set_content_height((h * 2.0).ceil() as i32);
                    }
                    alpha_area.queue_draw();
                } else {
                    let attrs = pango::AttrList::new();
                    attrs.insert(pango::AttrFontDesc::new(&pango_desc(&src, 18)));
                    alpha_label.set_attributes(Some(&attrs));
                }
                refresh_preview(&st, &rows, Some(&src));

                let strs: Vec<String> = loaded.coverage.iter().map(|c| c.to_string()).collect();
                let refs: Vec<&str> = strs.iter().map(String::as_str).collect();
                chars_model.splice(0, chars_model.n_items(), &refs);
                chars_count.set_label(&match loaded.coverage.len() {
                    1 => "1 character".to_string(),
                    n => format!("{n} characters"),
                });
                cstack.set_visible_child_name(if has_font { "grid" } else { "none" });

                let path_str = src.file.display().to_string();
                let facts = FaceFacts {
                    family: &src.family,
                    style: &src.style,
                    format: &src.format,
                    path: &path_str,
                    size: loaded.size,
                };
                info_bin.set_child(Some(&info_page(&fontinfo::info_rows(
                    loaded.info.as_ref(),
                    &facts,
                    loaded.coverage.len(),
                ))));
            });
        })
    };
    if let Some(dd) = &dropdown {
        let sf = select_face.clone();
        dd.connect_selected_notify(move |dd| sf(dd.selected() as usize));
    }
    select_face(initial.min(faces.len() - 1));

    let tv = adw::ToolbarView::new();
    tv.add_top_bar(&header);
    tv.add_top_bar(&switcher_bar);
    tv.set_content(Some(&stack));
    adw::NavigationPage::builder()
        .title(&first.family)
        .tag("font")
        .child(&tv)
        .build()
}

fn mark_installed(b: &gtk::Button) {
    b.set_label("Installed");
    b.set_sensitive(false);
    b.remove_css_class("suggested-action");
}

fn pango_desc(src: &FaceSource, pt: u32) -> pango::FontDescription {
    pango::FontDescription::from_string(&format!("{}, {} {pt}", src.family, src.style))
}

/// Updates size rows after the text or font changed. `src` is used for the
/// Pango fallback when the file can't be rendered directly.
fn refresh_preview(st: &PageState, rows: &[SizeRow], src: Option<&FaceSource>) {
    let font = st.font.borrow();
    let face = font.as_ref().and_then(|f| f.face());
    let text = st.text.borrow();
    for r in rows {
        match &face {
            Some(face) => {
                let (w, h) = render::measure(face, &text, pt_to_px(r.pt));
                r.area.set_content_width(w.ceil() as i32 + 2);
                r.area.set_content_height((h * 1.15).ceil().max(8.0) as i32);
                r.area.set_visible(true);
                r.label.set_visible(false);
                r.area.queue_draw();
            }
            None => {
                r.area.set_visible(false);
                r.label.set_visible(true);
                r.label.set_label(&text);
                if let Some(src) = src {
                    let attrs = pango::AttrList::new();
                    attrs.insert(pango::AttrFontDesc::new(&pango_desc(src, r.pt)));
                    r.label.set_attributes(Some(&attrs));
                }
            }
        }
    }
}

fn info_page(rows: &[(&'static str, String)]) -> adw::PreferencesPage {
    let page = adw::PreferencesPage::new();
    let group = adw::PreferencesGroup::new();
    for (k, v) in rows {
        let row = adw::ActionRow::builder()
            .title(*k)
            .subtitle(glib::markup_escape_text(v).as_str())
            .subtitle_selectable(true)
            .build();
        row.add_css_class("property");
        group.add(&row);
    }
    page.add(&group);
    page
}
