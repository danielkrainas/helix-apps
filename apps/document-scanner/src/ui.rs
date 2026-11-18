//! GTK user interface.

use crate::crop::{self, Handle, PAPERS};
use crate::document::{Document, Reorder};
use crate::export::{self, Format};
use crate::page::{Page, Rect};
use crate::postproc::PostProc;
use crate::sane::Device;
use crate::scanner::{Capabilities, Cmd, Event, PageSide, ScanKind, ScanRequest, Scanner};
use crate::settings::{ColorChoice, Private};
use adw::prelude::*;
use gtk::{cairo, gdk, gio, glib};
use helix_app_common::i18n::{n_, tr};
use helix_app_common::trf;
use helix_app_common::trnf;
use helix_app_common::Labelled;
use helix_app_common::Prefs;
use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::{Rc, Weak};

pub const APP_ID: &str = "org.helixdesktop.DocumentScanner";
const APP_NAME: &str = n_("Document Scanner");
const SCHEMA: &str = "org.gnome.SimpleScan";
const DEFAULT_DPI: [i32; 7] = [75, 150, 200, 300, 600, 1200, 2400];

pub(crate) const ACCELS: &[(&str, &[&str])] = &[
    ("win.scan-single", &["<Control>1"]),
    ("win.scan-feeder", &["<Control>f"]),
    ("win.scan-batch", &["<Control>m"]),
    ("win.stop", &["Escape"]),
    ("win.new", &["<Control>n"]),
    ("win.save", &["<Control>s"]),
    ("win.copy", &["<Control>c"]),
    ("win.print", &["<Control>p"]),
    ("win.email", &["<Control>e"]),
    ("win.rotate-left", &["bracketleft"]),
    ("win.rotate-right", &["bracketright"]),
    ("win.move-left", &["<Control>Page_Up"]),
    ("win.move-right", &["<Control>Page_Down"]),
    ("win.delete", &["Delete"]),
    ("win.preferences", &["<Control>comma"]),
    ("win.shortcuts", &["<Control>question"]),
    ("win.refresh", &["F5"]),
];

pub fn run(device: Option<String>) -> glib::ExitCode {
    let app = adw::Application::builder()
        .application_id(APP_ID)
        .flags(gio::ApplicationFlags::NON_UNIQUE)
        .build();
    helix_app_common::install_standard_actions(
        &app,
        tr(APP_NAME),
        tr("Make a digital copy of your photos and documents"),
    );
    app.connect_startup(|app| {
        for (action, accels) in ACCELS {
            app.set_accels_for_action(action, accels);
        }
        let css = gtk::CssProvider::new();
        css.load_from_string(
            ".page-thumb { padding: 6px; } .page-view { background: alpha(@view_fg_color, 0.06); }",
        );
        if let Some(display) = gdk::Display::default() {
            gtk::style_context_add_provider_for_display(
                &display,
                &css,
                gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
            );
        }
    });
    let device = RefCell::new(device);
    app.connect_activate(move |app| {
        let win = Win::new(app, device.borrow_mut().take());
        win.window.present();
    });
    app.run_with_args::<&str>(&[])
}

/// Creates a GDK texture showing `page`.
fn texture(page: &Page) -> gdk::Texture {
    let format = if page.is_color() {
        gdk::MemoryFormat::R8g8b8
    } else {
        gdk::MemoryFormat::G8
    };
    let bytes = glib::Bytes::from(&page.pixels[..]);
    gdk::MemoryTexture::new(
        page.width as i32,
        page.height as i32,
        format,
        &bytes,
        page.stride(),
    )
    .upcast()
}

/// Creates a cairo surface (for printing).
fn surface(page: &Page) -> Option<cairo::ImageSurface> {
    let fmt = cairo::Format::Rgb24;
    let stride = fmt.stride_for_width(page.width).ok()?;
    let mut data = vec![0u8; stride as usize * page.height as usize];
    let rgb = page.to_rgb();
    for y in 0..page.height as usize {
        for x in 0..page.width as usize {
            let s = (y * page.width as usize + x) * 3;
            let v = 0xFF00_0000u32
                | (rgb[s] as u32) << 16
                | (rgb[s + 1] as u32) << 8
                | rgb[s + 2] as u32;
            let d = y * stride as usize + x * 4;
            data[d..d + 4].copy_from_slice(&v.to_ne_bytes());
        }
    }
    cairo::ImageSurface::create_for_data(data, fmt, page.width as i32, page.height as i32, stride)
        .ok()
}

#[derive(Default)]
struct State {
    doc: Document,
    selected: Option<usize>,
    textures: Vec<gdk::Texture>,
    devices: Vec<Device>,
    device: Option<String>,
    caps: Capabilities,
    searching: bool,
    unavailable: Option<String>,
    scanning: bool,
    insert_at: usize,
    drag: Option<(Rect, Handle)>,
}

struct Win {
    window: adw::ApplicationWindow,
    toasts: adw::ToastOverlay,
    title: adw::WindowTitle,
    stack: gtk::Stack,
    none_page: adw::StatusPage,
    scan_button: adw::SplitButton,
    stop_button: gtk::Button,
    progress: gtk::ProgressBar,
    device_list: gtk::StringList,
    device_drop: gtk::DropDown,
    page_list: gtk::ListBox,
    picture: gtk::Picture,
    crop_area: gtk::DrawingArea,
    action_bar: gtk::ActionBar,
    state: RefCell<State>,
    prefs: Prefs,
    private: RefCell<Private>,
    scanner: Scanner,
    /// Suppresses feedback while widgets are updated programmatically.
    updating: Cell<bool>,
    wanted_device: RefCell<Option<String>>,
}

fn scan_menu() -> gio::Menu {
    let menu = gio::Menu::new();
    let kinds = gio::Menu::new();
    kinds.append(Some(tr("_Single Page")), Some("win.scan-single"));
    kinds.append(Some(tr("All Pages From _Feeder")), Some("win.scan-feeder"));
    kinds.append(
        Some(tr("_Multiple Pages From Flatbed")),
        Some("win.scan-batch"),
    );
    menu.append_section(None, &kinds);
    let types = gio::Menu::new();
    types.append(Some(tr("_Text")), Some("win.document-type::text"));
    types.append(Some(tr("_Image")), Some("win.document-type::photo"));
    menu.append_section(None, &types);
    let prefs = gio::Menu::new();
    prefs.append(Some(tr("_Preferences")), Some("win.preferences"));
    menu.append_section(None, &prefs);
    menu
}

fn main_menu() -> gio::Menu {
    let menu = gio::Menu::new();
    let doc = gio::Menu::new();
    doc.append(Some(tr("_New Document")), Some("win.new"));
    doc.append(Some(tr("_Reorder Pages…")), Some("win.reorder"));
    doc.append(Some(tr("_Copy Page")), Some("win.copy"));
    doc.append(Some(tr("_Print…")), Some("win.print"));
    doc.append(Some(tr("_Email…")), Some("win.email"));
    menu.append_section(None, &doc);
    let other = gio::Menu::new();
    other.append(Some(tr("_Refresh Scanners")), Some("win.refresh"));
    other.append(Some(tr("_Preferences")), Some("win.preferences"));
    other.append(
        Some(helix_app_common::i18n::tr(
            helix_app_common::shortcuts::MENU_LABEL,
        )),
        Some("win.shortcuts"),
    );
    other.append(Some(tr("_About Document Scanner")), Some("app.about"));
    menu.append_section(None, &other);
    menu
}

fn crop_menu() -> gio::Menu {
    let menu = gio::Menu::new();
    let presets = gio::Menu::new();
    presets.append(Some(tr("_None")), Some("win.crop::none"));
    presets.append(Some(tr("_Custom")), Some("win.crop::custom"));
    for p in PAPERS {
        presets.append(Some(tr(p.label)), Some(&format!("win.crop::{}", p.id)));
    }
    menu.append_section(None, &presets);
    let rot = gio::Menu::new();
    rot.append(Some(tr("_Rotate Crop")), Some("win.rotate-crop"));
    menu.append_section(None, &rot);
    menu
}

fn page_menu() -> gio::Menu {
    let menu = gio::Menu::new();
    let a = gio::Menu::new();
    a.append(Some(tr("Rotate _Left")), Some("win.rotate-left"));
    a.append(Some(tr("Rotate _Right")), Some("win.rotate-right"));
    a.append(Some(tr("Move Left")), Some("win.move-left"));
    a.append(Some(tr("Move Right")), Some("win.move-right"));
    menu.append_section(None, &a);
    menu.append_submenu(Some(tr("_Crop")), &crop_menu());
    let b = gio::Menu::new();
    b.append(Some(tr("_Copy")), Some("win.copy"));
    b.append(Some(tr("_Delete")), Some("win.delete"));
    menu.append_section(None, &b);
    menu
}

/// A page-list row: thumbnail and "Page N" caption, named "Page N".
fn page_row(i: usize, tex: &gdk::Texture) -> gtk::ListBoxRow {
    let pic = gtk::Picture::builder()
        .paintable(tex)
        .content_fit(gtk::ContentFit::Contain)
        .can_shrink(true)
        .width_request(110)
        .height_request(140)
        .build();
    let label = gtk::Label::new(None);
    label.add_css_class("caption");
    let b = gtk::Box::new(gtk::Orientation::Vertical, 4);
    b.add_css_class("page-thumb");
    b.append(&pic);
    b.append(&label);
    let row = gtk::ListBoxRow::builder().child(&b).build();
    name_page_row(&row, i);
    row
}

/// Numbers row `i` (0-based) as "Page i+1": caption, thumbnail text
/// alternative and the row's accessible name.
fn name_page_row(row: &gtk::ListBoxRow, i: usize) {
    let text = trf!("Page {}", i + 1);
    let Some(b) = row.child() else { return };
    if let Some(pic) = b.first_child().and_downcast::<gtk::Picture>() {
        pic.set_alternative_text(Some(&text));
    }
    if let Some(label) = b.last_child().and_downcast::<gtk::Label>() {
        label.set_label(&text);
    }
    helix_app_common::set_accessible_label(row, &text);
}

fn icon_button(icon: &str, tooltip: &str, action: &str) -> gtk::Button {
    gtk::Button::builder()
        .icon_name(icon)
        .tooltip_text(tooltip)
        .action_name(action)
        .build()
        .labelled(tooltip)
}

impl Win {
    fn new(app: &adw::Application, device: Option<String>) -> Rc<Win> {
        let window = adw::ApplicationWindow::builder()
            .application(app)
            .title(tr(APP_NAME))
            .default_width(900)
            .default_height(680)
            .width_request(helix_app_common::MIN_WIDTH)
            .height_request(helix_app_common::MIN_HEIGHT)
            .build();
        helix_app_common::window_state::persist(
            &window,
            helix_app_common::window_state::Store::file("helix-document-scanner"),
        );

        // Header bar.
        let header = adw::HeaderBar::new();
        let title = adw::WindowTitle::new(tr(APP_NAME), "");
        header.set_title_widget(Some(&title));
        let scan_button = adw::SplitButton::builder()
            .label(tr("_Scan"))
            .use_underline(true)
            .menu_model(&scan_menu())
            .action_name("win.scan-single")
            .tooltip_text(tr("Scan a single page"))
            .dropdown_tooltip(tr("Scan options"))
            .build();
        scan_button.add_css_class("suggested-action");
        let stop_button = gtk::Button::builder()
            .label(tr("_Stop"))
            .use_underline(true)
            .action_name("win.stop")
            .visible(false)
            .build();
        stop_button.add_css_class("destructive-action");
        header.pack_start(&scan_button);
        header.pack_start(&stop_button);
        let menu_button = gtk::MenuButton::builder()
            .icon_name("open-menu-symbolic")
            .menu_model(&main_menu())
            .primary(true)
            .tooltip_text(tr("Main Menu"))
            .build()
            .labelled(tr("Main Menu"));
        header.pack_end(&menu_button);
        header.pack_end(&icon_button(
            "document-save-symbolic",
            tr("Save"),
            "win.save",
        ));

        // States.
        let stack = gtk::Stack::new();
        stack.set_transition_type(gtk::StackTransitionType::Crossfade);
        let spinner = gtk::Spinner::builder()
            .spinning(true)
            .width_request(32)
            .height_request(32)
            .build();
        stack.add_named(
            &adw::StatusPage::builder()
                .title(tr("Searching for Scanners…"))
                .child(&spinner)
                .build(),
            Some("searching"),
        );
        let refresh = gtk::Button::builder()
            .label(tr("_Refresh"))
            .use_underline(true)
            .halign(gtk::Align::Center)
            .action_name("win.refresh")
            .build();
        refresh.add_css_class("pill");
        let none_page = adw::StatusPage::builder()
            .icon_name("scanner-symbolic")
            .title(tr("No Scanners Detected"))
            .description(tr("Please check your scanner is connected and powered on."))
            .child(&refresh)
            .build();
        stack.add_named(&none_page, Some("none"));

        let device_list = gtk::StringList::new(&[]);
        let device_drop = gtk::DropDown::builder()
            .model(&device_list)
            .halign(gtk::Align::Center)
            .build();
        helix_app_common::set_accessible_label(&device_drop, tr("Scanner"));
        let big_scan = gtk::Button::builder()
            .label(tr("_Scan"))
            .use_underline(true)
            .halign(gtk::Align::Center)
            .action_name("win.scan-single")
            .build();
        big_scan.add_css_class("pill");
        big_scan.add_css_class("suggested-action");
        let ready_box = gtk::Box::new(gtk::Orientation::Vertical, 18);
        ready_box.append(&device_drop);
        ready_box.append(&big_scan);
        stack.add_named(
            &adw::StatusPage::builder()
                .icon_name("scanner-symbolic")
                .title(tr("Ready to Scan"))
                .child(&ready_box)
                .build(),
            Some("ready"),
        );

        // Document view.
        let page_list = gtk::ListBox::new();
        page_list.add_css_class("navigation-sidebar");
        page_list.set_selection_mode(gtk::SelectionMode::Browse);
        let list_scroll = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .child(&page_list)
            .width_request(150)
            .build();
        let picture = gtk::Picture::builder()
            .content_fit(gtk::ContentFit::Contain)
            .can_shrink(true)
            .build();
        let crop_area = gtk::DrawingArea::builder()
            .hexpand(true)
            .vexpand(true)
            .accessible_role(gtk::AccessibleRole::Img)
            .focusable(false)
            .build();
        crop_area.add_css_class("crop-area");
        crop_area.update_property(&[gtk::accessible::Property::Description(tr(
            "Arrow keys move the crop area, Shift+arrow keys resize it, \
             Control makes the steps larger",
        ))]);
        helix_app_common::style::add_css(
            ".crop-area:focus-visible { outline: 2px solid @accent_color; outline-offset: -2px; }",
            ".crop-area:focus-visible { outline: 3px solid @accent_color; outline-offset: -3px; }",
        );
        let overlay = gtk::Overlay::builder()
            .child(&picture)
            .margin_top(12)
            .margin_bottom(12)
            .margin_start(12)
            .margin_end(12)
            .build();
        overlay.add_overlay(&crop_area);
        let view = gtk::Box::new(gtk::Orientation::Vertical, 0);
        view.add_css_class("page-view");
        view.append(&overlay);
        overlay.set_vexpand(true);
        let paned = gtk::Paned::builder()
            .start_child(&list_scroll)
            .end_child(&view)
            .shrink_start_child(false)
            .resize_start_child(false)
            .position(170)
            .build();
        stack.add_named(&paned, Some("document"));

        let progress = gtk::ProgressBar::builder()
            .valign(gtk::Align::Start)
            .visible(false)
            .build();
        helix_app_common::set_accessible_label(&progress, tr("Scanning"));
        progress.add_css_class("osd");
        let content_overlay = gtk::Overlay::builder().child(&stack).build();
        content_overlay.add_overlay(&progress);
        let toasts = adw::ToastOverlay::new();
        toasts.set_child(Some(&content_overlay));

        // Page action bar.
        let action_bar = gtk::ActionBar::new();
        action_bar.pack_start(&icon_button(
            "object-rotate-left-symbolic",
            tr("Rotate Left"),
            "win.rotate-left",
        ));
        action_bar.pack_start(&icon_button(
            "object-rotate-right-symbolic",
            tr("Rotate Right"),
            "win.rotate-right",
        ));
        let crop_button = gtk::MenuButton::builder()
            .icon_name("crop-symbolic")
            .tooltip_text(tr("Crop"))
            .menu_model(&crop_menu())
            .direction(gtk::ArrowType::Up)
            .build()
            .labelled(tr("Crop"));
        action_bar.pack_start(&crop_button);
        action_bar.pack_start(&icon_button(
            "go-previous-symbolic",
            tr("Move Left"),
            "win.move-left",
        ));
        action_bar.pack_start(&icon_button(
            "go-next-symbolic",
            tr("Move Right"),
            "win.move-right",
        ));
        action_bar.pack_end(&icon_button(
            "user-trash-symbolic",
            tr("Delete Page"),
            "win.delete",
        ));
        action_bar.set_revealed(false);

        let toolbar = adw::ToolbarView::new();
        toolbar.add_top_bar(&header);
        toolbar.set_content(Some(&toasts));
        toolbar.add_bottom_bar(&action_bar);
        window.set_content(Some(&toolbar));

        let (scanner, events) = Scanner::spawn();
        let win = Rc::new(Win {
            window,
            toasts,
            title,
            stack,
            none_page,
            scan_button,
            stop_button,
            progress,
            device_list,
            device_drop,
            page_list,
            picture,
            crop_area,
            action_bar,
            state: RefCell::default(),
            prefs: Prefs::new(SCHEMA),
            private: RefCell::new(Private::load_from(&Private::path())),
            scanner,
            updating: Cell::new(false),
            wanted_device: RefCell::new(device),
        });
        win.install_actions();
        win.connect_signals(&overlay);

        // Closures only hold weak references; the window keeps `Win` alive
        // until it is destroyed.
        let keep = RefCell::new(Some(win.clone()));
        win.window.connect_destroy(move |_| {
            keep.borrow_mut().take();
        });

        let weak = Rc::downgrade(&win);
        glib::spawn_future_local(async move {
            while let Ok(ev) = events.recv().await {
                match weak.upgrade() {
                    Some(w) => w.handle_event(ev),
                    None => break,
                }
            }
        });
        win.refresh_devices();
        win.update();
        win
    }

    fn weak(self: &Rc<Self>) -> Weak<Win> {
        Rc::downgrade(self)
    }

    fn toast(&self, text: &str) {
        self.toasts.add_toast(adw::Toast::new(text));
    }

    fn say(&self, text: &str) {
        helix_app_common::announce(&self.window, text, helix_app_common::Urgency::Polite);
    }

    /// Redraws the crop overlay and keeps its keyboard focusability and
    /// accessible name in step with the selected page's crop.
    fn crop_changed(&self) {
        let page = self.selected_page();
        let rect = page.as_ref().and_then(|p| p.crop.map(|r| (r, p.dpi)));
        match rect {
            Some((r, dpi)) => {
                self.crop_area.set_focusable(true);
                helix_app_common::set_accessible_label(&self.crop_area, &crop::describe(r, dpi));
            }
            None => {
                if self.crop_area.has_focus() {
                    self.page_list.child_focus(gtk::DirectionType::TabForward);
                }
                self.crop_area.set_focusable(false);
                helix_app_common::set_accessible_label(&self.crop_area, tr("No crop"));
            }
        }
        self.crop_area.queue_draw();
    }

    /// Arrow-key crop editing; returns whether the key was used.
    fn crop_key(&self, key: gdk::Key, mods: gdk::ModifierType) -> bool {
        let (dx, dy) = match key {
            gdk::Key::Left | gdk::Key::KP_Left => (-1.0, 0.0),
            gdk::Key::Right | gdk::Key::KP_Right => (1.0, 0.0),
            gdk::Key::Up | gdk::Key::KP_Up => (0.0, -1.0),
            gdk::Key::Down | gdk::Key::KP_Down => (0.0, 1.0),
            _ => return false,
        };
        let Some(page) = self.selected_page() else {
            return false;
        };
        let Some(r) = page.crop else { return false };
        let mm = if mods.contains(gdk::ModifierType::CONTROL_MASK) {
            10.0
        } else {
            1.0
        };
        let step = crop::mm_to_px(mm, page.dpi);
        let resize = mods.contains(gdk::ModifierType::SHIFT_MASK);
        let min = crop::mm_to_px(5.0, page.dpi).ceil() as u32;
        let n = crop::nudge(
            r,
            dx * step,
            dy * step,
            resize,
            page.width,
            page.height,
            min,
        );
        {
            let mut st = self.state.borrow_mut();
            let Some(i) = st.selected else { return false };
            st.doc.set_crop(i, Some(n));
        }
        self.crop_changed();
        true
    }

    fn dialog_open(&self) -> bool {
        self.window.visible_dialog().is_some()
    }

    fn document_type_text(&self) -> bool {
        self.prefs.get("document-type", "photo".to_string()) == "text"
    }

    // ---- actions ---------------------------------------------------------

    fn install_actions(self: &Rc<Self>) {
        let add = |name: &str, f: fn(&Rc<Win>)| {
            let action = gio::SimpleAction::new(name, None);
            let w = self.weak();
            action.connect_activate(move |_, _| {
                if let Some(w) = w.upgrade() {
                    f(&w);
                }
            });
            self.window.add_action(&action);
        };
        add("scan-single", |w| w.start_scan(ScanKind::Single));
        add("scan-feeder", |w| w.start_scan(ScanKind::Feeder));
        add("scan-batch", |w| w.start_scan(ScanKind::Batch));
        add("stop", |w| {
            if w.state.borrow().scanning {
                w.scanner.cancel();
            }
        });
        add("refresh", |w| w.refresh_devices());
        add("new", |w| w.new_document());
        add("save", |w| w.save(false));
        add("copy", |w| w.copy_page());
        add("print", |w| w.print());
        add("email", |w| w.email());
        add("reorder", |w| w.reorder_dialog());
        add("preferences", |w| w.preferences());
        add("shortcuts", |w| w.shortcuts());
        add("rotate-left", |w| {
            w.page_op(|d, i| d.rotate(i, false).then_some(i))
        });
        add("rotate-right", |w| {
            w.page_op(|d, i| d.rotate(i, true).then_some(i))
        });
        add("move-left", |w| {
            w.page_op(|d, i| if i > 0 { d.move_page(i, i - 1) } else { None })
        });
        add("move-right", |w| {
            w.page_op(|d, i| d.move_page(i, i + 1).filter(|&n| n != i))
        });
        add("delete", |w| {
            w.page_op(|d, i| {
                d.remove(i)?;
                Some(i.min(d.len().saturating_sub(1)))
            })
        });
        add("rotate-crop", |w| {
            w.page_op(|d, i| {
                let p = d.page(i)?;
                let r = crop::rotate_crop(p.crop?, p.width, p.height);
                d.set_crop(i, Some(r)).then_some(i)
            })
        });

        let crop = gio::SimpleAction::new_stateful(
            "crop",
            Some(glib::VariantTy::STRING),
            &"none".to_variant(),
        );
        let w = self.weak();
        crop.connect_activate(move |a, param| {
            let (Some(w), Some(kind)) = (w.upgrade(), param.and_then(|p| p.get::<String>())) else {
                return;
            };
            a.set_state(&kind.to_variant());
            w.set_crop(&kind);
        });
        self.window.add_action(&crop);

        let doc_type = gio::SimpleAction::new_stateful(
            "document-type",
            Some(glib::VariantTy::STRING),
            &(if self.document_type_text() {
                "text"
            } else {
                "photo"
            })
            .to_variant(),
        );
        let w = self.weak();
        doc_type.connect_activate(move |a, param| {
            let (Some(w), Some(kind)) = (w.upgrade(), param.and_then(|p| p.get::<String>())) else {
                return;
            };
            a.set_state(&kind.to_variant());
            w.prefs.set("document-type", kind);
        });
        self.window.add_action(&doc_type);
    }

    fn action(&self, name: &str) -> Option<gio::SimpleAction> {
        self.window
            .lookup_action(name)
            .and_downcast::<gio::SimpleAction>()
    }

    fn set_enabled(&self, name: &str, on: bool) {
        if let Some(a) = self.action(name) {
            a.set_enabled(on);
        }
    }

    /// Applies an operation to the selected page; `f` returns the index to
    /// select afterwards (or `None` when nothing changed).
    fn page_op(self: &Rc<Self>, f: impl FnOnce(&mut Document, usize) -> Option<usize>) {
        if self.dialog_open() {
            return;
        }
        let changed = {
            let mut st = self.state.borrow_mut();
            let Some(i) = st.selected else { return };
            let r = f(&mut st.doc, i);
            if let Some(n) = r {
                st.selected = if st.doc.is_empty() { None } else { Some(n) };
            }
            r.is_some()
        };
        if changed {
            self.rebuild_pages();
        }
    }

    fn set_crop(self: &Rc<Self>, kind: &str) {
        {
            let mut st = self.state.borrow_mut();
            let Some(i) = st.selected else { return };
            let Some(p) = st.doc.page(i) else { return };
            let rect = match kind {
                "none" => None,
                "custom" => Some(
                    p.crop
                        .unwrap_or_else(|| crop::custom_rect(p.width, p.height)),
                ),
                id => {
                    crop::paper(id).map(|paper| crop::preset_rect(paper, p.width, p.height, p.dpi))
                }
            };
            st.doc.set_crop(i, rect);
        }
        self.crop_changed();
        self.update();
    }

    // ---- devices ---------------------------------------------------------

    fn refresh_devices(&self) {
        let mut st = self.state.borrow_mut();
        if st.scanning {
            return;
        }
        st.searching = true;
        drop(st);
        self.scanner.send(Cmd::Refresh);
        self.update();
    }

    fn select_device(&self, name: &str) {
        {
            let mut st = self.state.borrow_mut();
            if st.device.as_deref() == Some(name) {
                return;
            }
            st.device = Some(name.to_string());
            st.caps = Capabilities::default();
        }
        self.prefs.set("selected-device", name.to_string());
        self.scanner.send(Cmd::Open(name.to_string()));
        self.update();
    }

    fn device_label(&self) -> String {
        let st = self.state.borrow();
        let Some(name) = &st.device else {
            return String::new();
        };
        st.devices
            .iter()
            .find(|d| &d.name == name)
            .map(|d| d.label())
            .unwrap_or_else(|| name.clone())
    }

    fn handle_event(self: &Rc<Self>, ev: Event) {
        match ev {
            Event::Unavailable(e) => {
                let mut st = self.state.borrow_mut();
                st.searching = false;
                st.unavailable = Some(e);
            }
            Event::Devices(devs) => {
                let current = {
                    let mut st = self.state.borrow_mut();
                    st.searching = false;
                    st.devices = devs;
                    st.device.clone()
                };
                self.updating.set(true);
                let labels: Vec<String> = self
                    .state
                    .borrow()
                    .devices
                    .iter()
                    .map(|d| d.label())
                    .collect();
                let labels: Vec<&str> = labels.iter().map(|s| s.as_str()).collect();
                self.device_list
                    .splice(0, self.device_list.n_items(), &labels);
                self.updating.set(false);
                let names: Vec<String> = self
                    .state
                    .borrow()
                    .devices
                    .iter()
                    .map(|d| d.name.clone())
                    .collect();
                let saved = self.prefs.get("selected-device", String::new());
                let wanted = self.wanted_device.borrow_mut().take();
                let pick = [wanted, current, Some(saved)]
                    .into_iter()
                    .flatten()
                    .find(|n| names.contains(n))
                    .or_else(|| names.first().cloned());
                match pick {
                    Some(n) => {
                        // Re-open on refresh even if unchanged.
                        self.state.borrow_mut().device = None;
                        self.select_device(&n);
                    }
                    None => self.state.borrow_mut().device = None,
                }
            }
            Event::Opened { device, caps } => {
                let mut st = self.state.borrow_mut();
                if st.device.as_deref() == Some(device.as_str()) {
                    st.caps = caps;
                }
            }
            Event::OpenFailed { device, error } => {
                self.toast(&format!("{device}: {error}"));
            }
            Event::ScanStarted => {
                self.progress.set_fraction(0.0);
                self.progress.set_visible(true);
            }
            Event::PageStarted => {
                self.progress.set_fraction(0.0);
                self.progress.set_visible(true);
                // Numbered as the page will be in the document (its row
                // name), not within this scan.
                let number = self.state.borrow().insert_at + 1;
                let text = trf!("Scanning page {number}", number = number);
                helix_app_common::set_accessible_label(&self.progress, &text);
                self.say(&format!("{text}…"));
            }
            Event::Progress(f) => {
                self.progress.set_fraction(f);
                return;
            }
            Event::Page(page) => {
                let (i, number) = {
                    let mut st = self.state.borrow_mut();
                    let at = st.insert_at;
                    let i = st.doc.insert(at, page);
                    st.insert_at = i + 1;
                    st.selected = Some(i);
                    (i, i + 1)
                };
                self.insert_page_row(i);
                self.say(&trf!("Page {number} scanned", number = number));
            }
            Event::ScanFinished { cancelled, error } => {
                self.state.borrow_mut().scanning = false;
                self.progress.set_visible(false);
                if cancelled {
                    self.toast(tr("Scan stopped"));
                } else if error.is_none() {
                    self.say(tr("Scan complete"));
                }
                if let Some(e) = error {
                    let d = adw::AlertDialog::new(Some(tr("Failed to Scan")), Some(&e));
                    d.add_response("close", tr("_Close"));
                    d.present(Some(&self.window));
                }
            }
        }
        self.update();
    }

    // ---- scanning --------------------------------------------------------

    fn scan_request(&self, kind: ScanKind) -> Option<ScanRequest> {
        let device = self.state.borrow().device.clone()?;
        let text = self.document_type_text();
        let dpi = if text {
            self.prefs.get("text-dpi", 150)
        } else {
            self.prefs.get("photo-dpi", 300)
        };
        let (pw, ph) = (
            self.prefs.get("paper-width", 0),
            self.prefs.get("paper-height", 0),
        );
        let private = self.private.borrow();
        Some(ScanRequest {
            device,
            source: Some(private.source.clone()).filter(|s| !s.is_empty()),
            kind,
            color: private.color_mode.resolve(text),
            dpi,
            paper: (pw > 0 && ph > 0).then(|| (pw as f64 / 10.0, ph as f64 / 10.0)),
            brightness: self.prefs.get("brightness", 0),
            contrast: self.prefs.get("contrast", 0),
            page_delay_ms: self.prefs.get("page-delay", 1000).clamp(0, 15000) as u32,
            page_side: PageSide::from_nick(&self.prefs.get_enum_nick("page-side", "both")),
            text,
        })
    }

    fn start_scan(self: &Rc<Self>, kind: ScanKind) {
        if self.state.borrow().scanning {
            return;
        }
        let Some(req) = self.scan_request(kind) else {
            self.toast(tr("No scanner selected"));
            return;
        };
        {
            let mut st = self.state.borrow_mut();
            st.scanning = true;
            st.insert_at = st.selected.map_or(st.doc.len(), |i| i + 1);
        }
        self.scanner.send(Cmd::Scan(req));
        self.update();
    }

    // ---- page list and view ---------------------------------------------

    fn rebuild_pages(self: &Rc<Self>) {
        self.updating.set(true);
        {
            let mut st = self.state.borrow_mut();
            st.textures = st.doc.pages().iter().map(texture).collect();
        }
        self.page_list.remove_all();
        let st = self.state.borrow();
        for (i, tex) in st.textures.iter().enumerate() {
            self.page_list.append(&page_row(i, tex));
        }
        if let Some(row) = st
            .selected
            .and_then(|i| self.page_list.row_at_index(i as i32))
        {
            self.page_list.select_row(Some(&row));
        }
        drop(st);
        self.updating.set(false);
        self.show_selected();
        self.update();
    }

    /// Adds the row for newly inserted page `i` without recreating the
    /// others (so keyboard focus stays where it was), renumbers the rows
    /// after it and selects it.
    fn insert_page_row(self: &Rc<Self>, i: usize) {
        let tex = {
            let mut st = self.state.borrow_mut();
            let Some(tex) = st.doc.page(i).map(texture) else {
                drop(st);
                return self.rebuild_pages();
            };
            st.textures.insert(i, tex.clone());
            tex
        };
        self.updating.set(true);
        self.page_list.insert(&page_row(i, &tex), i as i32);
        let mut n = i as i32 + 1;
        while let Some(row) = self.page_list.row_at_index(n) {
            name_page_row(&row, n as usize);
            n += 1;
        }
        if let Some(row) = self.page_list.row_at_index(i as i32) {
            self.page_list.select_row(Some(&row));
        }
        self.updating.set(false);
        self.show_selected();
        self.update();
    }

    fn show_selected(&self) {
        let st = self.state.borrow();
        let tex = st.selected.and_then(|i| st.textures.get(i));
        self.picture.set_paintable(tex);
        let kind = match st.selected.and_then(|i| st.doc.page(i)) {
            Some(p) if p.crop.is_some() => "custom",
            _ => "none",
        };
        drop(st);
        if let Some(a) = self.action("crop") {
            if a.state()
                .and_then(|s| s.get::<String>())
                .as_deref()
                .is_some_and(|s| (s == "none") != (kind == "none"))
            {
                a.set_state(&kind.to_variant());
            }
        }
        self.crop_changed();
    }

    /// Displayed image rectangle (x, y, scale) inside the crop area.
    fn view_geometry(&self, page: &Page) -> (f64, f64, f64) {
        let (aw, ah) = (
            self.crop_area.width() as f64,
            self.crop_area.height() as f64,
        );
        let scale = (aw / page.width as f64).min(ah / page.height as f64);
        let (dw, dh) = (page.width as f64 * scale, page.height as f64 * scale);
        ((aw - dw) / 2.0, (ah - dh) / 2.0, scale)
    }

    fn selected_page(&self) -> Option<Page> {
        let st = self.state.borrow();
        st.selected.and_then(|i| st.doc.page(i).cloned())
    }

    fn draw_crop(&self, cr: &cairo::Context) {
        let Some(page) = self.selected_page() else {
            return;
        };
        let Some(r) = page.crop else { return };
        let (ox, oy, s) = self.view_geometry(&page);
        let (x, y, w, h) = (
            ox + r.x as f64 * s,
            oy + r.y as f64 * s,
            r.width as f64 * s,
            r.height as f64 * s,
        );
        // Darken outside the crop.
        cr.set_source_rgba(0.0, 0.0, 0.0, 0.45);
        cr.rectangle(ox, oy, page.width as f64 * s, page.height as f64 * s);
        cr.rectangle(x, y, w, h);
        cr.set_fill_rule(cairo::FillRule::EvenOdd);
        let _ = cr.fill();
        // The crop frame in the system accent colour (it is drawn over the
        // scanned page, darkened around it).
        let accent = helix_app_common::style::accent_rgba();
        cr.set_source_rgb(
            accent.red() as f64,
            accent.green() as f64,
            accent.blue() as f64,
        );
        cr.set_line_width(if helix_app_common::style::is_high_contrast() {
            3.0
        } else {
            2.0
        });
        cr.rectangle(x, y, w, h);
        let _ = cr.stroke();
        for (hx, hy) in [(x, y), (x + w, y), (x, y + h), (x + w, y + h)] {
            cr.rectangle(hx - 5.0, hy - 5.0, 10.0, 10.0);
        }
        let _ = cr.fill();
    }

    fn crop_hit(&self, x: f64, y: f64) -> Option<(Rect, Handle)> {
        let page = self.selected_page()?;
        let r = page.crop?;
        let (ox, oy, s) = self.view_geometry(&page);
        if s <= 0.0 {
            return None;
        }
        crop::hit_test(r, (x - ox) / s, (y - oy) / s, 10.0 / s).map(|h| (r, h))
    }

    fn connect_signals(self: &Rc<Self>, overlay: &gtk::Overlay) {
        let w = self.weak();
        self.crop_area.set_draw_func(move |_, cr, _, _| {
            if let Some(w) = w.upgrade() {
                w.draw_crop(cr);
            }
        });
        helix_app_common::style::redraw_on_style_change(&self.crop_area);

        let drag = gtk::GestureDrag::new();
        let w = self.weak();
        drag.connect_drag_begin(move |g, x, y| {
            let Some(w) = w.upgrade() else { return };
            let hit = w.crop_hit(x, y);
            if hit.is_none() {
                g.set_state(gtk::EventSequenceState::Denied);
            }
            w.state.borrow_mut().drag = hit;
        });
        let w = self.weak();
        drag.connect_drag_update(move |_, dx, dy| {
            let Some(w) = w.upgrade() else { return };
            let Some(page) = w.selected_page() else {
                return;
            };
            let (_, _, s) = w.view_geometry(&page);
            let mut st = w.state.borrow_mut();
            let (Some((start, handle)), Some(i)) = (st.drag, st.selected) else {
                return;
            };
            let min = (20.0 / s.max(0.01)).ceil() as u32;
            let r = crop::drag(
                start,
                handle,
                dx / s,
                dy / s,
                page.width,
                page.height,
                min.max(1),
            );
            st.doc.set_crop(i, Some(r));
            drop(st);
            w.crop_changed();
        });
        let w = self.weak();
        drag.connect_drag_end(move |_, _, _| {
            if let Some(w) = w.upgrade() {
                w.state.borrow_mut().drag = None;
            }
        });
        self.crop_area.add_controller(drag);

        let motion = gtk::EventControllerMotion::new();
        let w = self.weak();
        motion.connect_motion(move |_, x, y| {
            let Some(w) = w.upgrade() else { return };
            let cursor = w.crop_hit(x, y).map(|(_, h)| h.cursor());
            w.crop_area.set_cursor_from_name(cursor);
        });
        self.crop_area.add_controller(motion);

        // Context menu on the page view.
        let popover = gtk::PopoverMenu::from_model(Some(&page_menu()));
        popover.set_parent(overlay);
        popover.set_has_arrow(false);
        popover.set_halign(gtk::Align::Start);
        let click = gtk::GestureClick::builder()
            .button(gdk::BUTTON_SECONDARY)
            .build();
        let w = self.weak();
        let p = popover.clone();
        click.connect_pressed(move |_, _, x, y| {
            let Some(w) = w.upgrade() else { return };
            if w.state.borrow().selected.is_none() {
                return;
            }
            p.set_pointing_to(Some(&gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
            p.popup();
        });
        overlay.add_controller(click);
        // Touch: a long press opens the same menu.
        let long = gtk::GestureLongPress::builder().touch_only(true).build();
        let w = self.weak();
        let p = popover.clone();
        long.connect_pressed(move |_, x, y| {
            let Some(w) = w.upgrade() else { return };
            if w.state.borrow().selected.is_none() {
                return;
            }
            p.set_pointing_to(Some(&gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
            p.popup();
        });
        overlay.add_controller(long);
        // Keyboard: Menu or Shift+F10 in the page list or the page view.
        let menu_keys = gtk::ShortcutController::new();
        let w = self.weak();
        let o = overlay.clone();
        menu_keys.add_shortcut(gtk::Shortcut::new(
            gtk::ShortcutTrigger::parse_string("<Shift>F10|Menu"),
            Some(gtk::CallbackAction::new(move |_, _| {
                let Some(w) = w.upgrade() else {
                    return glib::Propagation::Proceed;
                };
                if w.state.borrow().selected.is_none() {
                    return glib::Propagation::Proceed;
                }
                let (cx, cy) = (o.width() / 2, o.height() / 3);
                popover.set_pointing_to(Some(&gdk::Rectangle::new(cx, cy, 1, 1)));
                popover.popup();
                glib::Propagation::Stop
            })),
        ));
        if let Some(paned) = overlay.ancestor(gtk::Paned::static_type()) {
            paned.add_controller(menu_keys);
        }
        // Keyboard crop editing.
        let keys = gtk::EventControllerKey::new();
        let w = self.weak();
        keys.connect_key_pressed(move |_, key, _, mods| {
            let Some(w) = w.upgrade() else {
                return glib::Propagation::Proceed;
            };
            if w.crop_key(key, mods) {
                glib::Propagation::Stop
            } else {
                glib::Propagation::Proceed
            }
        });
        self.crop_area.add_controller(keys);

        let w = self.weak();
        self.page_list.connect_row_selected(move |_, row| {
            let Some(w) = w.upgrade() else { return };
            if w.updating.get() {
                return;
            }
            let Ok(mut st) = w.state.try_borrow_mut() else {
                return;
            };
            st.selected = row.map(|r| r.index() as usize);
            drop(st);
            w.show_selected();
            w.update();
        });

        let w = self.weak();
        self.device_drop.connect_selected_notify(move |d| {
            let Some(w) = w.upgrade() else { return };
            if w.updating.get() {
                return;
            }
            let name = w
                .state
                .borrow()
                .devices
                .get(d.selected() as usize)
                .map(|d| d.name.clone());
            if let Some(n) = name {
                w.select_device(&n);
            }
        });

        let w = self.weak();
        self.window.connect_close_request(move |window| {
            let Some(w) = w.upgrade() else {
                return glib::Propagation::Proceed;
            };
            if !w.state.borrow().doc.is_dirty() {
                return glib::Propagation::Proceed;
            }
            let d = adw::AlertDialog::new(
                Some(tr("Save current document?")),
                Some(tr("Documents that are not saved will be permanently lost.")),
            );
            d.add_responses(&[
                ("cancel", tr("_Cancel")),
                ("discard", tr("_Discard")),
                ("save", tr("_Save")),
            ]);
            d.set_response_appearance("discard", adw::ResponseAppearance::Destructive);
            d.set_response_appearance("save", adw::ResponseAppearance::Suggested);
            d.set_default_response(Some("save"));
            d.set_close_response("cancel");
            let ww = Rc::downgrade(&w);
            let win = window.clone();
            d.connect_response(None, move |_, resp| {
                let Some(w) = ww.upgrade() else { return };
                match resp {
                    "discard" => {
                        w.state.borrow_mut().doc.mark_saved();
                        win.close();
                    }
                    "save" => w.save(true),
                    _ => {}
                }
            });
            d.present(Some(window));
            glib::Propagation::Stop
        });
    }

    /// Refreshes visibility/sensitivity of everything from the state.
    fn update(&self) {
        // Gather everything first: widget changes below can emit signals
        // whose handlers borrow the state again.
        let st = self.state.borrow();
        let has_pages = !st.doc.is_empty();
        let page = st.selected.is_some();
        let has_device = st.device.is_some();
        let scanning = st.scanning;
        let view = if has_pages {
            "document"
        } else if st.searching {
            "searching"
        } else if st.devices.is_empty() {
            "none"
        } else {
            "ready"
        };
        let unavailable = st.unavailable.clone();
        let sel = st.selected.unwrap_or(0);
        let len = st.doc.len();
        let has_crop = st
            .selected
            .and_then(|i| st.doc.page(i))
            .is_some_and(|p| p.crop.is_some());
        let n_devices = st.devices.len();
        let device_index = st
            .device
            .as_ref()
            .and_then(|n| st.devices.iter().position(|d| &d.name == n));
        let dirty = st.doc.is_dirty();
        let subtitle = scanning.then(|| trf!("Scanning page {}…", st.insert_at + 1));
        drop(st);

        if let Some(e) = unavailable {
            self.none_page
                .set_description(Some(&trf!("Scanning is unavailable: {e}", e = e)));
        }
        let idle = !scanning;
        for a in ["scan-single", "scan-feeder", "scan-batch"] {
            self.set_enabled(a, idle && has_device);
        }
        self.set_enabled("stop", scanning);
        self.set_enabled("refresh", idle);
        for a in ["save", "print", "email", "new", "reorder"] {
            self.set_enabled(a, has_pages && idle);
        }
        for a in ["copy", "rotate-left", "rotate-right", "delete", "crop"] {
            self.set_enabled(a, page);
        }
        self.set_enabled("move-left", page && sel > 0);
        self.set_enabled("move-right", page && sel + 1 < len);
        self.set_enabled("rotate-crop", has_crop);
        self.updating.set(true);
        self.stack.set_visible_child_name(view);
        self.scan_button.set_visible(idle);
        self.stop_button.set_visible(scanning);
        self.action_bar.set_revealed(has_pages);
        self.device_drop.set_visible(n_devices > 1);
        if let Some(i) = device_index {
            if self.device_drop.selected() != i as u32 {
                self.device_drop.set_selected(i as u32);
            }
        }
        self.updating.set(false);
        self.title
            .set_subtitle(&subtitle.unwrap_or_else(|| self.device_label()));
        // A leading "*" marks unsaved changes, as in other GNOME apps.
        let title = if dirty {
            format!("*{}", tr(APP_NAME))
        } else {
            tr(APP_NAME).to_string()
        };
        self.window.set_title(Some(&title));
    }

    // ---- document operations --------------------------------------------

    fn confirm_discard(self: &Rc<Self>, then: impl Fn(&Rc<Win>) + 'static) {
        if !self.state.borrow().doc.is_dirty() {
            then(self);
            return;
        }
        let d = adw::AlertDialog::new(
            Some(tr("Discard Scanned Pages?")),
            Some(tr(
                "The current document has not been saved. Its pages will be permanently lost.",
            )),
        );
        d.add_responses(&[("cancel", tr("_Cancel")), ("discard", tr("_Discard"))]);
        d.set_response_appearance("discard", adw::ResponseAppearance::Destructive);
        d.set_close_response("cancel");
        let w = self.weak();
        d.connect_response(Some("discard"), move |_, _| {
            if let Some(w) = w.upgrade() {
                then(&w);
            }
        });
        d.present(Some(&self.window));
    }

    fn new_document(self: &Rc<Self>) {
        if self.state.borrow().scanning {
            return;
        }
        self.confirm_discard(|w| {
            {
                let mut st = w.state.borrow_mut();
                st.doc.clear();
                st.selected = None;
            }
            w.rebuild_pages();
        });
    }

    fn reorder_dialog(self: &Rc<Self>) {
        let d = adw::AlertDialog::new(
            Some(tr("Reorder Pages")),
            Some(tr(
                "Combine sides interleaves fronts (first half) with backs (second half).",
            )),
        );
        d.add_responses(&[
            ("combine", tr("Combine Sides")),
            ("combine-reverse", tr("Combine Sides (Reverse)")),
            ("reverse", tr("Reverse")),
            ("flip-odd", tr("Flip Odd Pages Upside-Down")),
            ("cancel", tr("_Cancel")),
        ]);
        d.set_close_response("cancel");
        let w = self.weak();
        d.connect_response(None, move |_, resp| {
            let Some(w) = w.upgrade() else { return };
            let op = match resp {
                "combine" => Reorder::Combine,
                "combine-reverse" => Reorder::CombineReversed,
                "reverse" => Reorder::Reverse,
                "flip-odd" => Reorder::FlipOdd,
                _ => return,
            };
            w.state.borrow_mut().doc.reorder(op);
            w.rebuild_pages();
        });
        d.present(Some(&self.window));
    }

    fn copy_page(&self) {
        if self.dialog_open() {
            return;
        }
        if let Some(p) = self.selected_page() {
            self.window.clipboard().set_texture(&texture(&p.cropped()));
            self.toast(tr("Page copied"));
        }
    }

    fn print(&self) {
        let pages: Vec<Page> = self.state.borrow().doc.pages().to_vec();
        if pages.is_empty() {
            return;
        }
        let op = gtk::PrintOperation::new();
        op.set_n_pages(pages.len() as i32);
        op.set_job_name(tr("Scanned Document"));
        op.connect_draw_page(move |_, ctx, n| {
            let Some(page) = pages.get(n as usize).map(|p| p.cropped()) else {
                return;
            };
            let Some(surf) = surface(&page) else { return };
            let cr = ctx.cairo_context();
            let s = (ctx.width() / page.width as f64).min(ctx.height() / page.height as f64);
            cr.scale(s, s);
            if cr.set_source_surface(&surf, 0.0, 0.0).is_ok() {
                let _ = cr.paint();
            }
        });
        if let Err(e) = op.run(gtk::PrintOperationAction::PrintDialog, Some(&self.window)) {
            self.toast(&trf!("Failed to print: {e}", e = e));
        }
    }

    fn save_folder(&self) -> Option<gio::File> {
        let dir = self.prefs.get("save-directory", String::new());
        if !dir.is_empty() {
            let f = if dir.contains("://") {
                gio::File::for_uri(&dir)
            } else {
                gio::File::for_path(&dir)
            };
            if f.query_exists(None::<&gio::Cancellable>) {
                return Some(f);
            }
        }
        glib::user_special_dir(glib::UserDirectory::Documents)
            .filter(|p| p.is_dir())
            .or_else(|| Some(glib::home_dir()))
            .map(gio::File::for_path)
    }

    fn save(self: &Rc<Self>, close_after: bool) {
        if self.state.borrow().doc.is_empty() {
            return;
        }
        let default =
            Format::from_mime(&self.prefs.get("save-format", "application/pdf".to_string()))
                .unwrap_or(Format::Pdf);
        let filters = gio::ListStore::new::<gtk::FileFilter>();
        let mut default_filter = None;
        for f in [default]
            .into_iter()
            .chain(Format::ALL.into_iter().filter(|f| *f != default))
        {
            let filter = gtk::FileFilter::new();
            filter.set_name(Some(f.label()));
            filter.add_mime_type(f.mime());
            filters.append(&filter);
            default_filter.get_or_insert(filter);
        }
        let dialog = gtk::FileDialog::builder()
            .title(tr("Save As…"))
            .modal(true)
            .initial_name(trf!("Scanned Document.{}", default.extension()))
            .filters(&filters)
            .build();
        if let Some(f) = default_filter {
            dialog.set_default_filter(Some(&f));
        }
        if let Some(folder) = self.save_folder() {
            dialog.set_initial_folder(Some(&folder));
        }
        let w = self.weak();
        let window = self.window.clone();
        glib::spawn_future_local(async move {
            let Ok(file) = dialog.save_future(Some(&window)).await else {
                return;
            };
            let Some(path) = file.path() else { return };
            let Some(w) = w.upgrade() else { return };
            w.write_document(path, default, close_after).await;
        });
    }

    async fn write_document(self: Rc<Self>, path: PathBuf, default: Format, close_after: bool) {
        let (path, format) = export::resolve_path(&path, default);
        let pages = self.state.borrow().doc.pages().to_vec();
        let quality = self.prefs.get("jpeg-quality", 75).clamp(0, 100) as u8;
        let p = path.clone();
        let result = gio::spawn_blocking(move || export::save(&pages, &p, format, quality))
            .await
            .unwrap_or_else(|_| Err(tr("Saving failed").into()));
        match result {
            Ok(files) => {
                self.state.borrow_mut().doc.mark_saved();
                if let Some(dir) = path.parent() {
                    self.prefs
                        .set("save-directory", gio::File::for_path(dir).uri().to_string());
                }
                self.prefs.set("save-format", format.mime().to_string());
                let name = files
                    .first()
                    .and_then(|f| f.file_name())
                    .map(|n| n.to_string_lossy().into_owned());
                self.toast(&match (files.len(), name) {
                    (1, Some(n)) => trf!("Saved “{n}”", n = n),
                    (n, _) => trnf!("Saved {n} file", "Saved {n} files", n),
                });
                self.post_process(format, files);
                self.update();
                if close_after {
                    self.window.close();
                }
            }
            Err(e) => {
                let d = adw::AlertDialog::new(Some(tr("Failed to Save File")), Some(&e));
                d.add_response("close", tr("_Close"));
                d.present(Some(&self.window));
            }
        }
    }

    fn postproc(&self) -> PostProc {
        PostProc {
            enabled: self.prefs.get("postproc-enabled", false),
            script: self.prefs.get("postproc-script", String::new()),
            arguments: self.prefs.get("postproc-arguments", String::new()),
            keep_original: self.prefs.get("postproc-keep-original", false),
        }
    }

    /// Runs the post-processing script on saved files (worker thread).
    fn post_process(self: &Rc<Self>, format: Format, files: Vec<PathBuf>) {
        let pp = self.postproc();
        if !pp.active() {
            return;
        }
        let w = self.weak();
        glib::spawn_future_local(async move {
            let res = gio::spawn_blocking(move || {
                crate::postproc::run(&pp, format.mime(), &files, &glib::home_dir())
            })
            .await
            .unwrap_or_else(|_| Err(tr("The post-processing script crashed").into()));
            if let (Some(w), Err(e)) = (w.upgrade(), res) {
                w.toast(&e);
            }
        });
    }

    /// Saves the document to a temporary file and opens the mail client
    /// with it attached (Email portal, else `xdg-email`).
    fn email(self: &Rc<Self>) {
        let pages = self.state.borrow().doc.pages().to_vec();
        if pages.is_empty() {
            return;
        }
        let format =
            Format::from_mime(&self.prefs.get("save-format", "application/pdf".to_string()))
                .unwrap_or(Format::Pdf);
        let quality = self.prefs.get("jpeg-quality", 75).clamp(0, 100) as u8;
        let w = self.weak();
        glib::spawn_future_local(async move {
            let res = gio::spawn_blocking(move || {
                let base = crate::email::base_dir();
                crate::email::prune(&base, std::time::SystemTime::now());
                let files = crate::email::write_attachments(&base, &pages, format, quality)?;
                crate::email::compose(&files, "")
            })
            .await
            .unwrap_or_else(|_| Err(tr("Sending failed").into()));
            if let (Some(w), Err(e)) = (w.upgrade(), res) {
                w.toast(&trf!("Could not open the mail client: {e}", e = e));
            }
        });
    }

    // ---- preferences -----------------------------------------------------

    fn save_private(&self) {
        let _ = self.private.borrow().save_to(&Private::path());
    }

    fn resolution_row(
        self: &Rc<Self>,
        title: &str,
        key: &'static str,
        default: i32,
    ) -> adw::ComboRow {
        let choices = {
            let st = self.state.borrow();
            if st.caps.resolutions.is_empty() {
                DEFAULT_DPI.to_vec()
            } else {
                st.caps.resolutions.clone()
            }
        };
        let labels: Vec<String> = choices.iter().map(|d| trf!("{d} dpi", d = d)).collect();
        let labels: Vec<&str> = labels.iter().map(|s| s.as_str()).collect();
        let row = adw::ComboRow::builder()
            .title(title)
            .model(&gtk::StringList::new(&labels))
            .build();
        let current = self.prefs.get(key, default);
        let nearest = crate::options::nearest_resolution(&choices, current);
        if let Some(i) = choices.iter().position(|&d| d == nearest) {
            row.set_selected(i as u32);
        }
        let w = self.weak();
        row.connect_selected_notify(move |r| {
            if let (Some(w), Some(d)) = (w.upgrade(), choices.get(r.selected() as usize)) {
                w.prefs.set(key, *d);
            }
        });
        row
    }

    fn slider_row(
        self: &Rc<Self>,
        title: &str,
        key: &'static str,
        min: f64,
        max: f64,
    ) -> adw::ActionRow {
        let scale = gtk::Scale::with_range(gtk::Orientation::Horizontal, min, max, 1.0);
        scale.set_value(self.prefs.get(key, 0) as f64);
        scale.set_width_request(200);
        scale.set_valign(gtk::Align::Center);
        scale.set_draw_value(true);
        scale.set_digits(0);
        helix_app_common::set_accessible_label(&scale, title);
        let w = self.weak();
        scale.connect_value_changed(move |s| {
            if let Some(w) = w.upgrade() {
                w.prefs.set(key, s.value().round() as i32);
            }
        });
        let row = adw::ActionRow::builder().title(title).build();
        row.add_suffix(&scale);
        row
    }

    fn preferences(self: &Rc<Self>) {
        let dialog = adw::PreferencesDialog::new();
        let page = adw::PreferencesPage::new();
        let scanning = adw::PreferencesGroup::builder()
            .title(tr("Scanning"))
            .build();

        // Scanner.
        let (names, labels, current, sources) = {
            let st = self.state.borrow();
            (
                st.devices
                    .iter()
                    .map(|d| d.name.clone())
                    .collect::<Vec<_>>(),
                st.devices.iter().map(|d| d.label()).collect::<Vec<_>>(),
                st.device.clone(),
                st.caps.sources.clone(),
            )
        };
        let label_refs: Vec<&str> = labels.iter().map(|s| s.as_str()).collect();
        let device_row = adw::ComboRow::builder()
            .title(tr("Scanner"))
            .model(&gtk::StringList::new(&label_refs))
            .sensitive(!names.is_empty() && !self.state.borrow().scanning)
            .build();
        if let Some(i) = current.and_then(|c| names.iter().position(|n| *n == c)) {
            device_row.set_selected(i as u32);
        }
        let w = self.weak();
        device_row.connect_selected_notify(move |r| {
            if let (Some(w), Some(n)) = (w.upgrade(), names.get(r.selected() as usize)) {
                w.select_device(n);
            }
        });
        scanning.add(&device_row);

        // Source.
        let mut source_labels = vec![tr("Automatic").to_string()];
        source_labels.extend(sources.iter().cloned());
        let saved = self.private.borrow().source.clone();
        if !saved.is_empty() && !sources.contains(&saved) {
            source_labels.push(saved.clone());
        }
        let refs: Vec<&str> = source_labels.iter().map(|s| s.as_str()).collect();
        let source_row = adw::ComboRow::builder()
            .title(tr("Source"))
            .model(&gtk::StringList::new(&refs))
            .build();
        if let Some(i) = source_labels.iter().skip(1).position(|s| *s == saved) {
            source_row.set_selected(i as u32 + 1);
        }
        let w = self.weak();
        source_row.connect_selected_notify(move |r| {
            let Some(w) = w.upgrade() else { return };
            let i = r.selected() as usize;
            w.private.borrow_mut().source = if i == 0 {
                String::new()
            } else {
                source_labels[i].clone()
            };
            w.save_private();
        });
        scanning.add(&source_row);

        // Colour mode.
        let refs: Vec<&str> = ColorChoice::ALL.iter().map(|c| c.label()).collect();
        let color_row = adw::ComboRow::builder()
            .title(tr("Color Mode"))
            .subtitle(tr("Automatic uses grayscale for text and color for images"))
            .model(&gtk::StringList::new(&refs))
            .build();
        let cur = self.private.borrow().color_mode;
        color_row.set_selected(ColorChoice::ALL.iter().position(|c| *c == cur).unwrap_or(0) as u32);
        let w = self.weak();
        color_row.connect_selected_notify(move |r| {
            let Some(w) = w.upgrade() else { return };
            w.private.borrow_mut().color_mode =
                ColorChoice::ALL[r.selected() as usize % ColorChoice::ALL.len()];
            w.save_private();
        });
        scanning.add(&color_row);

        // Sides.
        let sides = [PageSide::Front, PageSide::Back, PageSide::Both];
        let side_row = adw::ComboRow::builder()
            .title(tr("Scan Sides"))
            .subtitle(tr("For the document feeder"))
            .model(&gtk::StringList::new(&[
                tr("Front"),
                tr("Back"),
                tr("Both"),
            ]))
            .build();
        let cur = PageSide::from_nick(&self.prefs.get_enum_nick("page-side", "both"));
        side_row.set_selected(sides.iter().position(|s| *s == cur).unwrap_or(2) as u32);
        let w = self.weak();
        side_row.connect_selected_notify(move |r| {
            if let (Some(w), Some(s)) = (w.upgrade(), sides.get(r.selected() as usize)) {
                w.prefs.set_enum_nick("page-side", s.nick());
            }
        });
        scanning.add(&side_row);

        // Paper size.
        let mut paper_labels = vec![tr("Automatic")];
        paper_labels.extend(PAPERS.iter().map(|p| tr(p.label)));
        let paper_row = adw::ComboRow::builder()
            .title(tr("Page Size"))
            .model(&gtk::StringList::new(&paper_labels))
            .build();
        let cur = crop::paper_for_size(
            self.prefs.get("paper-width", 0),
            self.prefs.get("paper-height", 0),
        );
        paper_row.set_selected(cur.map_or(0, |i| i as u32 + 1));
        let w = self.weak();
        paper_row.connect_selected_notify(move |r| {
            let Some(w) = w.upgrade() else { return };
            let (pw, ph) = match r.selected() as usize {
                0 => (0, 0),
                i => PAPERS.get(i - 1).map_or((0, 0), |p| (p.width, p.height)),
            };
            w.prefs.set("paper-width", pw);
            w.prefs.set("paper-height", ph);
        });
        scanning.add(&paper_row);

        scanning.add(&self.resolution_row(tr("Text Resolution"), "text-dpi", 150));
        scanning.add(&self.resolution_row(tr("Image Resolution"), "photo-dpi", 300));
        scanning.add(&self.slider_row(tr("Brightness"), "brightness", -100.0, 100.0));
        scanning.add(&self.slider_row(tr("Contrast"), "contrast", -100.0, 100.0));

        let delay = adw::SpinRow::with_range(0.0, 15.0, 0.5);
        delay.set_title(tr("Delay Between Pages"));
        delay.set_subtitle(tr("Seconds, when scanning multiple pages from the flatbed"));
        delay.set_digits(1);
        delay.set_value(self.prefs.get("page-delay", 1000) as f64 / 1000.0);
        let w = self.weak();
        delay.connect_value_notify(move |s| {
            if let Some(w) = w.upgrade() {
                w.prefs
                    .set("page-delay", (s.value() * 1000.0).round() as i32);
            }
        });
        scanning.add(&delay);
        page.add(&scanning);

        let saving = adw::PreferencesGroup::builder().title(tr("Saving")).build();
        saving.add(&self.slider_row(tr("JPEG Quality"), "jpeg-quality", 0.0, 100.0));
        page.add(&saving);
        page.add(&self.postproc_group());

        dialog.add(&page);
        dialog.present(Some(&self.window));
    }

    /// Simple Scan's post-processing settings.
    fn postproc_group(self: &Rc<Self>) -> adw::PreferencesGroup {
        let group = adw::PreferencesGroup::builder()
            .title(tr("Post-processing"))
            .description(tr(
                "After saving, run SCRIPT MIME-TYPE KEEP-ORIGINAL FILE [ARGUMENTS…] \
                 (no shell; arguments are split like shell words but not expanded)",
            ))
            .build();
        let pp = self.postproc();
        let enabled = adw::SwitchRow::builder()
            .title(tr("Enable Post-processing"))
            .active(pp.enabled)
            .build();
        let w = self.weak();
        enabled.connect_active_notify(move |r| {
            if let Some(w) = w.upgrade() {
                w.prefs.set("postproc-enabled", r.is_active());
            }
        });
        group.add(&enabled);

        let script = adw::EntryRow::builder()
            .title(tr("Script"))
            .text(&pp.script)
            .show_apply_button(true)
            .build();
        let pick = gtk::Button::builder()
            .icon_name("document-open-symbolic")
            .tooltip_text(tr("Choose Script…"))
            .valign(gtk::Align::Center)
            .css_classes(["flat"])
            .build()
            .labelled(tr("Choose Script…"));
        script.add_suffix(&pick);
        let w = self.weak();
        script.connect_apply(move |e| {
            if let Some(w) = w.upgrade() {
                w.prefs.set("postproc-script", e.text().trim().to_string());
            }
        });
        let w = self.weak();
        let entry = script.clone();
        pick.connect_clicked(move |b| {
            let Some(w) = w.upgrade() else { return };
            let dialog = gtk::FileDialog::builder()
                .title(tr("Choose Script"))
                .modal(true)
                .build();
            let parent = b.root().and_downcast::<gtk::Window>();
            let entry = entry.clone();
            let w = Rc::downgrade(&w);
            dialog.open(parent.as_ref(), gio::Cancellable::NONE, move |res| {
                let (Some(w), Ok(file)) = (w.upgrade(), res) else {
                    return;
                };
                if let Some(path) = file.path() {
                    let s = path.to_string_lossy().into_owned();
                    entry.set_text(&s);
                    w.prefs.set("postproc-script", s);
                }
            });
        });
        group.add(&script);

        let args = adw::EntryRow::builder()
            .title(tr("Script Arguments"))
            .text(&pp.arguments)
            .show_apply_button(true)
            .build();
        let w = self.weak();
        args.connect_apply(move |e| {
            let Some(w) = w.upgrade() else { return };
            let text = e.text().to_string();
            match crate::postproc::split_arguments(&text) {
                Ok(_) => {
                    helix_app_common::set_input_error(e, None, None);
                    w.prefs.set("postproc-arguments", text);
                }
                Err(message) => {
                    // Shown inside the preferences dialog, not behind it.
                    helix_app_common::set_input_error(e, None, Some(&message));
                    if let Some(d) = e
                        .ancestor(adw::PreferencesDialog::static_type())
                        .and_downcast::<adw::PreferencesDialog>()
                    {
                        d.add_toast(adw::Toast::new(&message));
                    }
                }
            }
        });
        helix_app_common::clear_input_error_on_edit(&args, None);
        group.add(&args);

        let keep = adw::SwitchRow::builder()
            .title(tr("Keep Original File"))
            .subtitle(tr(
                "Saves an unprocessed copy with “_orig” before the extension",
            ))
            .active(pp.keep_original)
            .build();
        let w = self.weak();
        keep.connect_active_notify(move |r| {
            if let Some(w) = w.upgrade() {
                w.prefs.set("postproc-keep-original", r.is_active());
            }
        });
        group.add(&keep);
        for r in [
            script.upcast_ref::<gtk::Widget>(),
            args.upcast_ref(),
            keep.upcast_ref(),
        ] {
            enabled
                .bind_property("active", r, "sensitive")
                .sync_create()
                .build();
        }
        group
    }

    fn shortcuts(&self) {
        helix_app_common::shortcuts::show(&self.window, crate::shortcuts::GROUPS);
    }
}
