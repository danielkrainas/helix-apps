//! GTK user interface.

use adw::prelude::*;
use gtk::{gdk, gio, glib};
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::{Rc, Weak};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::canvas::ImageCanvas;
use crate::decode::{self, Decoded, PixelFormat};
use crate::folder::{self, Extensions, Playlist, Source};
use crate::metadata::{self, ExifInfo};
use crate::save;
use crate::thumbs::{self, RawThumb};
use crate::trash;
use crate::view;
use crate::wallpaper;

pub const APP_ID: &str = "org.helix.ImageViewer";
const APP_NAME: &str = "Image Viewer";
const CONTROLS_TIMEOUT: Duration = Duration::from_secs(2);
const SLIDESHOW_INTERVAL: Duration = Duration::from_secs(5);
const LOADING_DELAY: Duration = Duration::from_millis(200);
/// GNOME Loupe's schema; only `show-properties` is used.
const LOUPE_SCHEMA: &str = "org.gnome.Loupe";
/// Thumbnails kept per window before the cache is cleared.
const THUMB_CACHE_MAX: usize = 2000;
const THUMB_WORKERS: usize = 2;

thread_local! {
    static PREFS: helix_app_common::Prefs = helix_app_common::Prefs::new(LOUPE_SCHEMA);
}

fn prefs() -> helix_app_common::Prefs {
    PREFS.with(|p| p.clone())
}

pub fn run(paths: Vec<PathBuf>) -> glib::ExitCode {
    let app = adw::Application::builder()
        .application_id(APP_ID)
        .flags(gio::ApplicationFlags::NON_UNIQUE)
        .build();
    helix_app_common::install_standard_actions(&app, APP_NAME, "View images");
    app.connect_startup(|app| {
        load_css();
        let new_window = gio::ActionEntry::builder("new-window")
            .activate(|app: &adw::Application, _, _| Viewer::new(app).window.present())
            .build();
        app.add_action_entries([new_window]);
        for (action, accels) in ACCELS {
            app.set_accels_for_action(action, accels);
        }
    });
    let initial = RefCell::new(Some(paths));
    app.connect_activate(move |app| {
        let viewer = Viewer::new(app);
        if let Some(paths) = initial.borrow_mut().take() {
            if !paths.is_empty() {
                viewer.open(paths);
            }
        }
        viewer.window.present();
        if std::env::var_os("HELIX_IMAGE_VIEWER_SMOKE").is_some() {
            smoke_script(&viewer);
        }
    });
    app.run_with_args(&[std::env::args().next().unwrap_or_default()])
}

const ACCELS: &[(&str, &[&str])] = &[
    ("app.new-window", &["<Control>n"]),
    ("win.open", &["<Control>o"]),
    ("win.open-with", &["<Control><Shift>o"]),
    ("win.copy", &["<Control>c"]),
    ("win.trash", &["Delete", "KP_Delete"]),
    ("win.prev", &["Left", "Page_Up", "KP_Left"]),
    ("win.next", &["Right", "Page_Down", "KP_Right"]),
    ("win.first", &["Home"]),
    ("win.last", &["End"]),
    (
        "win.zoom-in",
        &["<Control>plus", "<Control>equal", "<Control>KP_Add"],
    ),
    ("win.zoom-out", &["<Control>minus", "<Control>KP_Subtract"]),
    ("win.zoom-fit", &["<Control>0", "<Control>KP_0"]),
    ("win.zoom-100", &["<Control>1", "<Control>KP_1"]),
    ("win.zoom-200", &["<Control>2", "<Control>KP_2"]),
    ("win.rotate-right", &["<Control>r"]),
    ("win.rotate-left", &["<Control><Shift>r"]),
    ("win.save", &["<Control>s"]),
    ("win.properties", &["F9", "<Alt>Return"]),
    ("win.fullscreen", &["F11"]),
    ("win.leave-fullscreen", &["Escape"]),
    ("win.slideshow", &["F5"]),
];

fn load_css() {
    let css = gtk::CssProvider::new();
    css.load_from_string(
        ".iv-controls { padding: 6px; border-radius: 999px; }
         .iv-controls button, .iv-controls menubutton > button { min-width: 34px; min-height: 34px; }
         .iv-nav { min-width: 48px; min-height: 48px; }
         .iv-canvas { background: transparent; }
         .iv-strip { background: transparent; padding: 4px; }
         .iv-strip > row { padding: 3px; border-radius: 8px; }
         .iv-thumb { border-radius: 6px; }",
    );
    if let Some(display) = gdk::Display::default() {
        gtk::style_context_add_provider_for_display(
            &display,
            &css,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    }
}

fn toast(text: &str) -> adw::Toast {
    let t = adw::Toast::new(text);
    t.set_use_markup(false);
    t
}

/// Rows of the properties sidebar.
struct Props {
    dimensions: adw::ActionRow,
    format: adw::ActionRow,
    size: adw::ActionRow,
    folder: adw::ActionRow,
    created: adw::ActionRow,
    modified: adw::ActionRow,
    exif_group: adw::PreferencesGroup,
    taken: adw::ActionRow,
    camera: adw::ActionRow,
    exposure: adw::ActionRow,
    aperture: adw::ActionRow,
    iso: adw::ActionRow,
    focal: adw::ActionRow,
    gps: adw::ActionRow,
    gps_uri: Rc<RefCell<Option<String>>>,
}

fn prop_row(group: &adw::PreferencesGroup, title: &str) -> adw::ActionRow {
    let row = adw::ActionRow::builder()
        .title(title)
        .use_markup(false)
        .subtitle_selectable(true)
        .build();
    row.add_css_class("property");
    group.add(&row);
    row
}

fn set_row(row: &adw::ActionRow, value: Option<&str>) {
    row.set_subtitle(value.unwrap_or(""));
    row.set_visible(value.is_some());
}

/// File-system facts gathered on the worker thread.
struct FileDetails {
    size: Option<u64>,
    created: Option<i64>,
    modified: Option<i64>,
}

fn file_details(path: &Path) -> FileDetails {
    let md = std::fs::metadata(path).ok();
    let secs = |t: std::io::Result<std::time::SystemTime>| {
        t.ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs() as i64)
    };
    FileDetails {
        size: md.as_ref().map(|m| m.len()),
        created: md.as_ref().and_then(|m| secs(m.created())),
        modified: md.as_ref().and_then(|m| secs(m.modified())),
    }
}

fn format_time(secs: Option<i64>) -> Option<String> {
    let dt = glib::DateTime::from_unix_local(secs?).ok()?;
    dt.format("%x %X").ok().map(|s| s.to_string())
}

struct State {
    playlist: Playlist,
    exts: Extensions,
    generation: u64,
    textures: Vec<gdk::Texture>,
    delays: Vec<u32>,
    controls_timeout: Option<glib::SourceId>,
    over_controls: bool,
    slideshow: Option<glib::SourceId>,
    monitor: Option<(PathBuf, gio::FileMonitor)>,
    refresh_pending: bool,
    /// A decode for the current generation is in flight.
    loading: bool,
    /// The current image can be re-encoded (format supported, not animated).
    savable: bool,
    saving: bool,
}

/// The bottom thumbnail strip.
struct Strip {
    bar: gtk::ScrolledWindow,
    view: gtk::ListView,
    model: gtk::StringList,
    selection: gtk::SingleSelection,
    /// The paths the model currently mirrors (model strings are lossy).
    files: RefCell<Vec<PathBuf>>,
    /// Selection changes made by code, not the user.
    syncing: Cell<bool>,
    cache: RefCell<HashMap<PathBuf, gdk::Texture>>,
    bound: RefCell<HashMap<PathBuf, glib::WeakRef<gtk::Picture>>>,
    /// Paths whose thumbnail is wanted (bound and not cached); workers skip
    /// requests that are no longer wanted.
    wanted: Arc<Mutex<HashSet<PathBuf>>>,
    requests: async_channel::Sender<PathBuf>,
}

fn thumb_texture(t: &RawThumb) -> gdk::Texture {
    let fmt = if t.has_alpha {
        gdk::MemoryFormat::R8g8b8a8
    } else {
        gdk::MemoryFormat::R8g8b8
    };
    gdk::MemoryTexture::new(t.width, t.height, fmt, &t.bytes, t.stride).upcast()
}

pub struct Viewer {
    window: adw::ApplicationWindow,
    toasts: adw::ToastOverlay,
    split: adw::OverlaySplitView,
    toolbar: adw::ToolbarView,
    title: adw::WindowTitle,
    stack: gtk::Stack,
    error_page: adw::StatusPage,
    canvas: ImageCanvas,
    revealers: Vec<gtk::Revealer>,
    prev_btn: gtk::Button,
    next_btn: gtk::Button,
    zoom_btn: gtk::MenuButton,
    save_btn: gtk::Button,
    strip: Strip,
    props: Props,
    actions: RefCell<HashMap<&'static str, gio::SimpleAction>>,
    state: RefCell<State>,
    self_ref: RefCell<Weak<Viewer>>,
    fullscreen: Cell<bool>,
}

impl Viewer {
    fn new(app: &adw::Application) -> Rc<Viewer> {
        let window = adw::ApplicationWindow::builder()
            .application(app)
            .title(APP_NAME)
            .default_width(1000)
            .default_height(700)
            .width_request(360)
            .height_request(294)
            .build();

        // Header.
        let header = adw::HeaderBar::new();
        let title = adw::WindowTitle::new(APP_NAME, "");
        header.set_title_widget(Some(&title));
        let open_btn = gtk::Button::builder()
            .label("Open…")
            .action_name("win.open")
            .tooltip_text("Open Images (Ctrl+O)")
            .build();
        header.pack_start(&open_btn);
        let menu = gio::Menu::new();
        let s1 = gio::Menu::new();
        s1.append(Some("_Open…"), Some("win.open"));
        s1.append(Some("Open _With…"), Some("win.open-with"));
        s1.append(Some("_New Window"), Some("app.new-window"));
        menu.append_section(None, &s1);
        let s2 = gio::Menu::new();
        s2.append(Some("_Copy"), Some("win.copy"));
        s2.append(Some("Set as _Background…"), Some("win.set-background"));
        s2.append(Some("Move to _Trash"), Some("win.trash"));
        menu.append_section(None, &s2);
        let s3 = gio::Menu::new();
        s3.append(Some("Rotate _Left"), Some("win.rotate-left"));
        s3.append(Some("Rotate _Right"), Some("win.rotate-right"));
        s3.append(Some("_Save Rotation"), Some("win.save"));
        s3.append(Some("_Fullscreen"), Some("win.fullscreen"));
        s3.append(Some("_Slideshow"), Some("win.slideshow"));
        menu.append_section(None, &s3);
        let s4 = gio::Menu::new();
        s4.append(Some("_About Image Viewer"), Some("app.about"));
        menu.append_section(None, &s4);
        let menu_btn = gtk::MenuButton::builder()
            .icon_name("open-menu-symbolic")
            .menu_model(&menu)
            .primary(true)
            .tooltip_text("Main Menu")
            .build();
        header.pack_end(&menu_btn);
        let props_btn = gtk::ToggleButton::builder()
            .icon_name("dialog-information-symbolic")
            .tooltip_text("Image Properties (F9)")
            .build();
        header.pack_end(&props_btn);

        // Image area.
        let canvas = ImageCanvas::default();
        canvas.add_css_class("iv-canvas");
        let overlay = gtk::Overlay::new();
        overlay.set_child(Some(&canvas));

        let nav_button = |icon: &str, action: &str, tip: &str, align: gtk::Align| {
            let b = gtk::Button::builder()
                .icon_name(icon)
                .action_name(action)
                .tooltip_text(tip)
                .build();
            b.add_css_class("osd");
            b.add_css_class("circular");
            b.add_css_class("iv-nav");
            let r = gtk::Revealer::builder()
                .transition_type(gtk::RevealerTransitionType::Crossfade)
                .halign(align)
                .valign(gtk::Align::Center)
                .margin_start(12)
                .margin_end(12)
                .child(&b)
                .build();
            (b, r)
        };
        let (prev_btn, prev_rev) = nav_button(
            "go-previous-symbolic",
            "win.prev",
            "Previous Image",
            gtk::Align::Start,
        );
        let (next_btn, next_rev) = nav_button(
            "go-next-symbolic",
            "win.next",
            "Next Image",
            gtk::Align::End,
        );
        overlay.add_overlay(&prev_rev);
        overlay.add_overlay(&next_rev);

        let controls = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        controls.add_css_class("osd");
        controls.add_css_class("toolbar");
        controls.add_css_class("iv-controls");
        let tool = |icon: &str, action: &str, tip: &str| {
            let b = gtk::Button::builder()
                .icon_name(icon)
                .action_name(action)
                .tooltip_text(tip)
                .build();
            b.add_css_class("flat");
            b
        };
        controls.append(&tool(
            "object-rotate-left-symbolic",
            "win.rotate-left",
            "Rotate Left (Ctrl+Shift+R)",
        ));
        controls.append(&tool(
            "object-rotate-right-symbolic",
            "win.rotate-right",
            "Rotate Right (Ctrl+R)",
        ));
        let save_btn = tool(
            "document-save-symbolic",
            "win.save",
            "Save Rotation (Ctrl+S)",
        );
        save_btn.set_visible(false);
        controls.append(&save_btn);
        controls.append(&gtk::Separator::new(gtk::Orientation::Vertical));
        controls.append(&tool(
            "zoom-out-symbolic",
            "win.zoom-out",
            "Zoom Out (Ctrl+−)",
        ));
        let zoom_menu = gio::Menu::new();
        zoom_menu.append(Some("Best Fit"), Some("win.zoom-fit"));
        zoom_menu.append(Some("100 %"), Some("win.zoom-100"));
        zoom_menu.append(Some("200 %"), Some("win.zoom-200"));
        let zoom_btn = gtk::MenuButton::builder()
            .label("100 %")
            .menu_model(&zoom_menu)
            .direction(gtk::ArrowType::Up)
            .tooltip_text("Zoom Level")
            .build();
        zoom_btn.add_css_class("flat");
        controls.append(&zoom_btn);
        controls.append(&tool("zoom-in-symbolic", "win.zoom-in", "Zoom In (Ctrl++)"));
        controls.append(&gtk::Separator::new(gtk::Orientation::Vertical));
        controls.append(&tool(
            "view-fullscreen-symbolic",
            "win.fullscreen",
            "Fullscreen (F11)",
        ));
        let controls_rev = gtk::Revealer::builder()
            .transition_type(gtk::RevealerTransitionType::Crossfade)
            .halign(gtk::Align::Center)
            .valign(gtk::Align::End)
            .margin_bottom(18)
            .child(&controls)
            .build();
        overlay.add_overlay(&controls_rev);

        // Pages.
        let empty = adw::StatusPage::builder()
            .icon_name("image-x-generic-symbolic")
            .title("View Images")
            .description("Drag and drop images here")
            .build();
        let empty_btn = gtk::Button::builder()
            .label("_Open Files…")
            .use_underline(true)
            .action_name("win.open")
            .halign(gtk::Align::Center)
            .build();
        empty_btn.add_css_class("pill");
        empty_btn.add_css_class("suggested-action");
        empty.set_child(Some(&empty_btn));
        let spinner = gtk::Spinner::builder()
            .spinning(true)
            .width_request(32)
            .height_request(32)
            .halign(gtk::Align::Center)
            .valign(gtk::Align::Center)
            .build();
        let error_page = adw::StatusPage::builder()
            .icon_name("dialog-error-symbolic")
            .title("Could Not Load Image")
            .build();
        let stack = gtk::Stack::builder()
            .transition_type(gtk::StackTransitionType::Crossfade)
            .build();
        stack.add_named(&empty, Some("empty"));
        stack.add_named(&spinner, Some("loading"));
        stack.add_named(&error_page, Some("error"));
        stack.add_named(&overlay, Some("image"));
        stack.set_visible_child_name("empty");

        // Thumbnail strip.
        let model = gtk::StringList::new(&[]);
        let selection = gtk::SingleSelection::builder()
            .model(&model)
            .autoselect(false)
            .can_unselect(true)
            .build();
        let factory = gtk::SignalListItemFactory::new();
        let view = gtk::ListView::builder()
            .model(&selection)
            .factory(&factory)
            .orientation(gtk::Orientation::Horizontal)
            .single_click_activate(false)
            .css_classes(["iv-strip"])
            .build();
        let bar = gtk::ScrolledWindow::builder()
            .child(&view)
            .vscrollbar_policy(gtk::PolicyType::Never)
            .hscrollbar_policy(gtk::PolicyType::Automatic)
            .height_request(84)
            .visible(false)
            .build();
        let (req_tx, req_rx) = async_channel::unbounded::<PathBuf>();
        let (res_tx, res_rx) = async_channel::unbounded::<(PathBuf, Option<RawThumb>)>();
        let wanted: Arc<Mutex<HashSet<PathBuf>>> = Arc::default();
        let cache_root = glib::user_cache_dir();
        for _ in 0..THUMB_WORKERS {
            let (rx, tx, wanted, root) = (
                req_rx.clone(),
                res_tx.clone(),
                wanted.clone(),
                cache_root.clone(),
            );
            std::thread::spawn(move || {
                while let Ok(path) = rx.recv_blocking() {
                    if !wanted.lock().map(|w| w.contains(&path)).unwrap_or(false) {
                        continue;
                    }
                    let thumb = thumbs::load(&path, &root, thumbs::SIZE);
                    if tx.send_blocking((path, thumb)).is_err() {
                        break;
                    }
                }
            });
        }
        let strip = Strip {
            bar,
            view,
            model,
            selection,
            files: RefCell::default(),
            syncing: Cell::new(false),
            cache: RefCell::default(),
            bound: RefCell::default(),
            wanted,
            requests: req_tx,
        };

        let toolbar = adw::ToolbarView::new();
        toolbar.add_top_bar(&header);
        toolbar.set_content(Some(&stack));
        toolbar.add_bottom_bar(&strip.bar);
        toolbar.set_extend_content_to_top_edge(false);

        // Properties sidebar.
        let props_page = adw::PreferencesPage::new();
        let image_group = adw::PreferencesGroup::builder().title("Image").build();
        let exif_group = adw::PreferencesGroup::builder().title("Camera").build();
        props_page.add(&image_group);
        props_page.add(&exif_group);
        let props = Props {
            dimensions: prop_row(&image_group, "Dimensions"),
            format: prop_row(&image_group, "Format"),
            size: prop_row(&image_group, "File Size"),
            folder: prop_row(&image_group, "Folder"),
            created: prop_row(&image_group, "Created"),
            modified: prop_row(&image_group, "Modified"),
            taken: prop_row(&exif_group, "Date Taken"),
            camera: prop_row(&exif_group, "Camera"),
            exposure: prop_row(&exif_group, "Exposure"),
            aperture: prop_row(&exif_group, "Aperture"),
            iso: prop_row(&exif_group, "ISO"),
            focal: prop_row(&exif_group, "Focal Length"),
            gps: prop_row(&exif_group, "Location"),
            exif_group,
            gps_uri: Rc::default(),
        };
        let folder_btn = gtk::Button::builder()
            .icon_name("folder-open-symbolic")
            .valign(gtk::Align::Center)
            .tooltip_text("Open Folder")
            .build();
        folder_btn.add_css_class("flat");
        props.folder.add_suffix(&folder_btn);
        let gps_btn = gtk::Button::builder()
            .icon_name("mark-location-symbolic")
            .valign(gtk::Align::Center)
            .tooltip_text("Show on Map")
            .build();
        gps_btn.add_css_class("flat");
        props.gps.add_suffix(&gps_btn);
        let sidebar_toolbar = adw::ToolbarView::new();
        let sidebar_header = adw::HeaderBar::builder()
            .show_title(true)
            .title_widget(&adw::WindowTitle::new("Properties", ""))
            .build();
        sidebar_toolbar.add_top_bar(&sidebar_header);
        sidebar_toolbar.set_content(Some(&props_page));

        let split = adw::OverlaySplitView::builder()
            .sidebar_position(gtk::PackType::End)
            .show_sidebar(false)
            .sidebar(&sidebar_toolbar)
            .content(&toolbar)
            .min_sidebar_width(260.0)
            .max_sidebar_width(340.0)
            .build();
        props_btn
            .bind_property("active", &split, "show-sidebar")
            .bidirectional()
            .sync_create()
            .build();
        // `show-properties` from org.gnome.Loupe (in memory if not installed).
        split.set_show_sidebar(prefs().get("show-properties", false));
        split.connect_show_sidebar_notify(|split| {
            let p = prefs();
            if p.get("show-properties", false) != split.shows_sidebar() {
                p.set("show-properties", split.shows_sidebar());
            }
        });
        let toasts = adw::ToastOverlay::new();
        toasts.set_child(Some(&split));
        window.set_content(Some(&toasts));

        let bp = adw::Breakpoint::new(adw::BreakpointCondition::new_length(
            adw::BreakpointConditionLengthType::MaxWidth,
            640.0,
            adw::LengthUnit::Sp,
        ));
        bp.add_setter(&split, "collapsed", Some(&true.to_value()));
        window.add_breakpoint(bp);

        let viewer = Rc::new(Viewer {
            window,
            toasts,
            split,
            toolbar,
            title,
            stack,
            error_page,
            canvas,
            revealers: vec![prev_rev, next_rev, controls_rev],
            prev_btn,
            next_btn,
            zoom_btn,
            save_btn,
            strip,
            props,
            actions: RefCell::default(),
            state: RefCell::new(State {
                playlist: Playlist::default(),
                exts: decode::supported_extensions(),
                generation: 0,
                textures: Vec::new(),
                delays: Vec::new(),
                controls_timeout: None,
                over_controls: false,
                slideshow: None,
                monitor: None,
                refresh_pending: false,
                loading: false,
                savable: false,
                saving: false,
            }),
            self_ref: RefCell::default(),
            fullscreen: Cell::new(false),
        });
        viewer.self_ref.replace(Rc::downgrade(&viewer));
        viewer.setup_actions();
        viewer.setup_signals(&overlay, &controls, &folder_btn, &gps_btn);
        viewer.setup_strip(&factory, res_rx);
        viewer.update_ui();
        // The window owns the viewer until it is closed.
        let keep = RefCell::new(Some(viewer.clone()));
        viewer.window.connect_close_request(move |_| {
            keep.borrow_mut().take();
            glib::Propagation::Proceed
        });
        viewer
    }

    fn weak(&self) -> Weak<Viewer> {
        self.self_ref.borrow().clone()
    }

    fn add_action(&self, name: &'static str, f: impl Fn(&Rc<Viewer>) + 'static) {
        let action = gio::SimpleAction::new(name, None);
        let weak = self.weak();
        action.connect_activate(move |_, _| {
            if let Some(v) = weak.upgrade() {
                f(&v);
            }
        });
        self.window.add_action(&action);
        self.actions.borrow_mut().insert(name, action);
    }

    fn setup_actions(&self) {
        self.add_action("open", |v| v.open_dialog());
        self.add_action("open-with", |v| v.open_with());
        self.add_action("copy", |v| v.copy());
        self.add_action("set-background", |v| v.set_background());
        self.add_action("trash", |v| v.trash_current());
        self.add_action("prev", |v| v.navigate(|p| p.step(-1)));
        self.add_action("next", |v| v.navigate(|p| p.step(1)));
        self.add_action("first", |v| v.navigate(Playlist::first));
        self.add_action("last", |v| v.navigate(Playlist::last));
        self.add_action("zoom-in", |v| v.canvas.update_view(|s| s.zoom_in(None)));
        self.add_action("zoom-out", |v| v.canvas.update_view(|s| s.zoom_out(None)));
        self.add_action("zoom-fit", |v| v.canvas.update_view(|s| s.best_fit()));
        self.add_action("zoom-100", |v| {
            v.canvas.update_view(|s| s.zoom_to(1.0, None))
        });
        self.add_action("zoom-200", |v| {
            v.canvas.update_view(|s| s.zoom_to(2.0, None))
        });
        self.add_action("rotate-right", |v| {
            v.canvas.update_view(|s| s.rotate(90));
            v.update_ui();
        });
        self.add_action("rotate-left", |v| {
            v.canvas.update_view(|s| s.rotate(-90));
            v.update_ui();
        });
        self.add_action("save", |v| v.save_rotation());
        self.add_action("properties", |v| {
            v.split.set_show_sidebar(!v.split.shows_sidebar())
        });
        self.add_action("fullscreen", |v| {
            v.window.set_fullscreened(!v.window.is_fullscreen())
        });
        self.add_action("leave-fullscreen", |v| {
            v.stop_slideshow();
            v.window.set_fullscreened(false);
        });
        self.add_action("slideshow", |v| v.toggle_slideshow());
    }

    fn setup_signals(
        &self,
        overlay: &gtk::Overlay,
        controls: &gtk::Box,
        folder_btn: &gtk::Button,
        gps_btn: &gtk::Button,
    ) {
        let weak = self.weak();
        self.canvas.connect_zoom_changed(move |zoom, _fit| {
            if let Some(v) = weak.upgrade() {
                v.zoom_btn.set_label(&view::zoom_label(zoom));
            }
        });

        // Reveal controls on pointer motion.
        let motion = gtk::EventControllerMotion::new();
        let weak = self.weak();
        let last = Cell::new((f64::NAN, f64::NAN));
        motion.connect_motion(move |_, x, y| {
            // Ignore synthetic motion with no actual movement.
            if last.get() == (x, y) {
                return;
            }
            last.set((x, y));
            if let Some(v) = weak.upgrade() {
                v.show_controls();
            }
        });
        overlay.add_controller(motion);
        let over = gtk::EventControllerMotion::new();
        let weak = self.weak();
        over.connect_enter(move |_, _, _| {
            if let Some(v) = weak.upgrade() {
                v.state.borrow_mut().over_controls = true;
            }
        });
        let weak = self.weak();
        over.connect_leave(move |_| {
            if let Some(v) = weak.upgrade() {
                v.state.borrow_mut().over_controls = false;
                v.show_controls();
            }
        });
        controls.add_controller(over);

        let weak = self.weak();
        self.window.connect_fullscreened_notify(move |w| {
            if let Some(v) = weak.upgrade() {
                let fs = w.is_fullscreen();
                v.fullscreen.set(fs);
                if !fs {
                    v.stop_slideshow();
                    v.toolbar.set_reveal_top_bars(true);
                    v.canvas.set_cursor(None);
                }
                v.update_ui();
                v.show_controls();
            }
        });

        // Drag and drop.
        let drop = gtk::DropTarget::new(gdk::FileList::static_type(), gdk::DragAction::COPY);
        let weak = self.weak();
        drop.connect_drop(move |_, value, _, _| {
            let Some(v) = weak.upgrade() else {
                return false;
            };
            let Ok(list) = value.get::<gdk::FileList>() else {
                return false;
            };
            let paths: Vec<PathBuf> = list.files().iter().filter_map(|f| f.path()).collect();
            if paths.is_empty() {
                return false;
            }
            v.open(paths);
            true
        });
        self.toasts.add_controller(drop);

        let weak = self.weak();
        folder_btn.connect_clicked(move |_| {
            let (Some(v), Some(p)) = (weak.upgrade(), v_current(&weak)) else {
                return;
            };
            let launcher = gtk::FileLauncher::new(Some(&gio::File::for_path(&p)));
            let toasts = v.toasts.clone();
            launcher.open_containing_folder(Some(&v.window), gio::Cancellable::NONE, move |r| {
                if let Err(e) = r {
                    toasts.add_toast(toast(&format!("Could not open folder: {}", e.message())));
                }
            });
        });
        let uri = self.props.gps_uri.clone();
        let weak = self.weak();
        gps_btn.connect_clicked(move |_| {
            let (Some(v), Some(u)) = (weak.upgrade(), uri.borrow().clone()) else {
                return;
            };
            let toasts = v.toasts.clone();
            gtk::UriLauncher::new(&u).launch(Some(&v.window), gio::Cancellable::NONE, move |r| {
                if let Err(e) = r {
                    toasts.add_toast(toast(&format!("Could not open map: {}", e.message())));
                }
            });
        });

        // Stop timers when the window goes away.
        let weak = self.weak();
        self.window.connect_close_request(move |_| {
            if let Some(v) = weak.upgrade() {
                let mut st = v.state.borrow_mut();
                st.generation += 1;
                if let Some(id) = st.controls_timeout.take() {
                    id.remove();
                }
                if let Some(id) = st.slideshow.take() {
                    id.remove();
                }
                st.monitor = None;
            }
            glib::Propagation::Proceed
        });
    }

    fn current(&self) -> Option<PathBuf> {
        self.state
            .borrow()
            .playlist
            .current()
            .map(Path::to_path_buf)
    }

    // ---- Opening and loading ------------------------------------------

    pub fn open(&self, paths: Vec<PathBuf>) {
        let exts = self.state.borrow().exts.clone();
        let playlist = Playlist::open(paths, &exts);
        self.state.borrow_mut().playlist = playlist;
        self.update_monitor();
        self.load_current();
    }

    fn open_dialog(&self) {
        let filter = gtk::FileFilter::new();
        filter.set_name(Some("Supported Images"));
        for m in decode::supported_mime_types() {
            filter.add_mime_type(&m);
        }
        for e in self.state.borrow().exts.iter() {
            filter.add_suffix(e);
        }
        let filters = gio::ListStore::new::<gtk::FileFilter>();
        filters.append(&filter);
        let dialog = gtk::FileDialog::builder()
            .title("Open Images")
            .modal(true)
            .filters(&filters)
            .default_filter(&filter)
            .build();
        if let Some(dir) = self
            .current()
            .and_then(|p| p.parent().map(Path::to_path_buf))
        {
            dialog.set_initial_folder(Some(&gio::File::for_path(dir)));
        }
        let weak = self.weak();
        dialog.open_multiple(Some(&self.window), gio::Cancellable::NONE, move |res| {
            let (Some(v), Ok(model)) = (weak.upgrade(), res) else {
                return;
            };
            let paths: Vec<PathBuf> = model
                .iter::<gio::File>()
                .filter_map(Result::ok)
                .filter_map(|f| f.path())
                .collect();
            if !paths.is_empty() {
                v.open(paths);
            }
        });
    }

    fn navigate(&self, f: impl FnOnce(&mut Playlist) -> bool) {
        let changed = f(&mut self.state.borrow_mut().playlist);
        if changed {
            self.load_current();
            self.restart_slideshow_timer();
        }
    }

    fn load_current(&self) {
        let (gen, path) = {
            let mut st = self.state.borrow_mut();
            st.generation += 1;
            st.textures.clear();
            st.delays.clear();
            st.loading = st.playlist.current().is_some();
            st.savable = false;
            (st.generation, st.playlist.current().map(Path::to_path_buf))
        };
        self.update_ui();
        let Some(path) = path else {
            self.canvas.set_texture(None, true);
            self.stack.set_visible_child_name("empty");
            self.clear_props();
            return;
        };

        // Show a spinner only if decoding takes a while.
        let weak = self.weak();
        glib::timeout_add_local_once(LOADING_DELAY, move || {
            if let Some(v) = weak.upgrade() {
                let st = v.state.borrow();
                if st.generation == gen && st.loading {
                    drop(st);
                    v.stack.set_visible_child_name("loading");
                }
            }
        });

        let weak = self.weak();
        glib::MainContext::default().spawn_local(async move {
            let p = path.clone();
            let result = gio::spawn_blocking(move || (decode::decode(&p), file_details(&p))).await;
            let Some(v) = weak.upgrade() else { return };
            if v.state.borrow().generation != gen {
                return;
            }
            v.state.borrow_mut().loading = false;
            match result {
                Ok((Ok(decoded), details)) => v.show_decoded(gen, &path, decoded, details),
                Ok((Err(e), details)) => v.show_error(&path, &e, details),
                Err(_) => v.show_error(&path, "The decoder crashed", file_details(&path)),
            }
        });
    }

    fn show_decoded(&self, gen: u64, path: &Path, decoded: Decoded, details: FileDetails) {
        let textures: Vec<gdk::Texture> = decoded
            .frames
            .iter()
            .map(|f| {
                let fmt = match f.format {
                    PixelFormat::Rgb8 => gdk::MemoryFormat::R8g8b8,
                    PixelFormat::Rgba8 => gdk::MemoryFormat::R8g8b8a8,
                };
                gdk::MemoryTexture::new(f.width as i32, f.height as i32, fmt, &f.bytes, f.stride)
                    .upcast()
            })
            .collect();
        let delays: Vec<u32> = decoded.frames.iter().map(|f| f.delay_ms).collect();
        self.canvas.set_texture(textures.first(), true);
        {
            let mut st = self.state.borrow_mut();
            st.textures = textures;
            st.delays = delays;
            st.savable = save::can_save(&decoded.format) && !decoded.is_animated();
        }
        self.stack.set_visible_child_name("image");
        self.canvas.grab_focus();
        self.fill_props(path, Some(&decoded), &details);
        self.update_ui();
        self.show_controls();
        if decoded.is_animated() {
            self.schedule_frame(gen, 0);
        }
    }

    fn schedule_frame(&self, gen: u64, index: usize) {
        let delay = self
            .state
            .borrow()
            .delays
            .get(index)
            .copied()
            .unwrap_or(100);
        let weak = self.weak();
        glib::timeout_add_local_once(Duration::from_millis(delay as u64), move || {
            let Some(v) = weak.upgrade() else { return };
            let next = {
                let st = v.state.borrow();
                if st.generation != gen || st.textures.is_empty() {
                    return;
                }
                let next = (index + 1) % st.textures.len();
                v.canvas.set_texture(st.textures.get(next), false);
                next
            };
            v.schedule_frame(gen, next);
        });
    }

    fn show_error(&self, path: &Path, msg: &str, details: FileDetails) {
        self.canvas.set_texture(None, true);
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        self.error_page
            .set_description(Some(&glib::markup_escape_text(&format!("{name}: {msg}"))));
        self.stack.set_visible_child_name("error");
        self.fill_props(path, None, &details);
        self.update_ui();
    }

    // ---- Folder monitoring ----------------------------------------------

    fn update_monitor(&self) {
        let dir = match &self.state.borrow().playlist.source {
            Source::Folder(d) => Some(d.clone()),
            Source::Files => None,
        };
        let same =
            matches!((&self.state.borrow().monitor, &dir), (Some((m, _)), Some(d)) if m == d);
        if same {
            return;
        }
        let monitor = dir.and_then(|d| {
            let m = gio::File::for_path(&d)
                .monitor_directory(gio::FileMonitorFlags::WATCH_MOVES, gio::Cancellable::NONE)
                .ok()?;
            let weak = self.weak();
            m.connect_changed(move |_, _, _, event| {
                if matches!(
                    event,
                    gio::FileMonitorEvent::ChangesDoneHint
                        | gio::FileMonitorEvent::AttributeChanged
                ) {
                    return;
                }
                if let Some(v) = weak.upgrade() {
                    v.queue_refresh();
                }
            });
            Some((d, m))
        });
        self.state.borrow_mut().monitor = monitor;
    }

    fn queue_refresh(&self) {
        if std::mem::replace(&mut self.state.borrow_mut().refresh_pending, true) {
            return;
        }
        let weak = self.weak();
        glib::timeout_add_local_once(Duration::from_millis(300), move || {
            let Some(v) = weak.upgrade() else { return };
            v.state.borrow_mut().refresh_pending = false;
            let (dir, exts, keep) = {
                let st = v.state.borrow();
                let Source::Folder(dir) = &st.playlist.source else {
                    return;
                };
                (
                    dir.clone(),
                    st.exts.clone(),
                    st.playlist.current().map(Path::to_path_buf),
                )
            };
            let weak = v.weak();
            glib::MainContext::default().spawn_local(async move {
                let d = dir.clone();
                let Ok(list) =
                    gio::spawn_blocking(move || folder::list_images(&d, &exts, keep.as_deref()))
                        .await
                else {
                    return;
                };
                let Some(v) = weak.upgrade() else { return };
                let changed = {
                    let mut st = v.state.borrow_mut();
                    if st.playlist.source != Source::Folder(dir) {
                        return;
                    }
                    st.playlist.refresh(list)
                };
                if changed {
                    v.load_current();
                } else {
                    v.update_ui();
                }
            });
        });
    }

    // ---- UI state -------------------------------------------------------

    fn update_ui(&self) {
        let st = self.state.borrow();
        let has_image = !st.textures.is_empty();
        let has_file = st.playlist.current().is_some();
        match st.playlist.current() {
            Some(p) => {
                let name = p
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                self.window.set_title(Some(&format!("{name} — {APP_NAME}")));
                self.title.set_title(&name);
                let pos = st
                    .playlist
                    .index()
                    .map(|i| format!("{} / {}", i + 1, st.playlist.len()));
                self.title.set_subtitle(if st.playlist.len() > 1 {
                    pos.as_deref().unwrap_or("")
                } else {
                    ""
                });
            }
            None => {
                self.window.set_title(Some(APP_NAME));
                self.title.set_title(APP_NAME);
                self.title.set_subtitle("");
            }
        }
        let has_prev = st.playlist.has_prev();
        let has_next = st.playlist.has_next();
        let can_save = has_image && st.savable && !st.saving && self.canvas.view().rotation != 0;
        drop(st);
        let actions = self.actions.borrow();
        let enable = |name: &str, on: bool| {
            if let Some(a) = actions.get(name) {
                a.set_enabled(on);
            }
        };
        for name in [
            "zoom-in",
            "zoom-out",
            "zoom-fit",
            "zoom-100",
            "zoom-200",
            "rotate-right",
            "rotate-left",
            "copy",
        ] {
            enable(name, has_image);
        }
        for name in ["open-with", "set-background", "trash"] {
            enable(name, has_file);
        }
        enable("prev", has_prev);
        enable("next", has_next);
        enable("first", has_prev);
        enable("last", has_next);
        enable("slideshow", has_file);
        enable("save", can_save);
        self.save_btn.set_visible(can_save);
        self.prev_btn.set_visible(has_prev);
        self.next_btn.set_visible(has_next);
        drop(actions);
        self.sync_strip();
    }

    fn show_controls(&self) {
        for r in &self.revealers {
            r.set_reveal_child(true);
        }
        if self.fullscreen.get() {
            self.toolbar.set_reveal_top_bars(true);
        }
        self.canvas.set_cursor(None);
        if let Some(id) = self.state.borrow_mut().controls_timeout.take() {
            id.remove();
        }
        let weak = self.weak();
        let id = glib::timeout_add_local_once(CONTROLS_TIMEOUT, move || {
            let Some(v) = weak.upgrade() else { return };
            v.state.borrow_mut().controls_timeout = None;
            if v.state.borrow().over_controls {
                return;
            }
            for r in &v.revealers {
                r.set_reveal_child(false);
            }
            if v.fullscreen.get() {
                v.toolbar.set_reveal_top_bars(false);
                v.canvas.set_cursor_from_name(Some("none"));
            }
        });
        self.state.borrow_mut().controls_timeout = Some(id);
    }

    // ---- Properties -----------------------------------------------------

    fn clear_props(&self) {
        let p = &self.props;
        for row in [
            &p.dimensions,
            &p.format,
            &p.size,
            &p.folder,
            &p.created,
            &p.modified,
        ] {
            set_row(row, None);
        }
        p.exif_group.set_visible(false);
    }

    fn fill_props(&self, path: &Path, decoded: Option<&Decoded>, details: &FileDetails) {
        let p = &self.props;
        let dims = decoded.map(|d| {
            let (w, h) = d.size();
            metadata::format_dimensions(w, h)
        });
        set_row(&p.dimensions, dims.as_deref());
        let format = decoded.map(|d| {
            if d.is_animated() {
                format!("{} (animated)", d.format)
            } else {
                d.format.clone()
            }
        });
        set_row(&p.format, format.as_deref());
        set_row(
            &p.size,
            details
                .size
                .map(|s| glib::format_size(s).to_string())
                .as_deref(),
        );
        set_row(
            &p.folder,
            path.parent()
                .map(|d| d.to_string_lossy().into_owned())
                .as_deref(),
        );
        set_row(&p.created, format_time(details.created).as_deref());
        set_row(&p.modified, format_time(details.modified).as_deref());

        let exif: Option<&ExifInfo> = decoded
            .and_then(|d| d.exif.as_ref())
            .filter(|e| e.has_details());
        p.exif_group.set_visible(exif.is_some());
        let e = exif.cloned().unwrap_or_default();
        set_row(&p.taken, e.date_taken.as_deref());
        set_row(&p.camera, e.camera.as_deref());
        set_row(&p.exposure, e.exposure.as_deref());
        set_row(&p.aperture, e.aperture.as_deref());
        set_row(&p.iso, e.iso.as_deref());
        set_row(&p.focal, e.focal_length.as_deref());
        set_row(
            &p.gps,
            e.gps
                .map(|(la, lo)| metadata::format_gps(la, lo))
                .as_deref(),
        );
        p.gps_uri
            .replace(e.gps.map(|(la, lo)| metadata::geo_uri(la, lo)));
    }

    // ---- Actions ----------------------------------------------------------

    fn open_with(&self) {
        let Some(path) = self.current() else { return };
        let launcher = gtk::FileLauncher::new(Some(&gio::File::for_path(path)));
        launcher.set_always_ask(true);
        let toasts = self.toasts.clone();
        launcher.launch(Some(&self.window), gio::Cancellable::NONE, move |r| {
            if let Err(e) = r {
                if !e.matches(gtk::DialogError::Dismissed)
                    && !e.matches(gtk::DialogError::Cancelled)
                {
                    toasts.add_toast(toast(&format!("Could not open: {}", e.message())));
                }
            }
        });
    }

    fn copy(&self) {
        let Some(texture) = self.canvas.texture() else {
            return;
        };
        self.window.clipboard().set_texture(&texture);
        self.toasts.add_toast(toast("Image copied to clipboard"));
    }

    fn set_background(&self) {
        let Some(path) = self.current() else { return };
        let uri = gio::File::for_path(&path).uri().to_string();
        let weak = self.weak();
        glib::MainContext::default().spawn_local(async move {
            let result = gio::spawn_blocking(move || wallpaper::set_wallpaper(&uri, "")).await;
            let Some(v) = weak.upgrade() else { return };
            let msg = match result {
                Ok(Ok(wallpaper::Outcome::Set)) => Some("Background set".to_string()),
                Ok(Ok(wallpaper::Outcome::Cancelled)) => None,
                Ok(Ok(wallpaper::Outcome::Failed)) => {
                    Some("Could not set the background".to_string())
                }
                Ok(Err(e)) => Some(format!("Could not set the background: {e}")),
                Err(_) => Some("Could not set the background".to_string()),
            };
            if let Some(m) = msg {
                v.toasts.add_toast(toast(&m));
            }
        });
    }

    fn trash_current(&self) {
        let Some(path) = self.current() else { return };
        let weak = self.weak();
        glib::MainContext::default().spawn_local(async move {
            let p = path.clone();
            let result = gio::spawn_blocking(move || trash::trash(&p)).await;
            let Some(v) = weak.upgrade() else { return };
            match result {
                Ok(Ok(())) => {
                    let index = {
                        let mut st = v.state.borrow_mut();
                        let i = st.playlist.position_of(&path).unwrap_or(0);
                        st.playlist.remove(&path);
                        i
                    };
                    v.load_current();
                    let t = toast("Image moved to trash");
                    t.set_button_label(Some("_Undo"));
                    let weak = v.weak();
                    t.connect_button_clicked(move |_| {
                        if let Some(v) = weak.upgrade() {
                            v.undo_trash(path.clone(), index);
                        }
                    });
                    v.toasts.add_toast(t);
                }
                Ok(Err(e)) => v
                    .toasts
                    .add_toast(toast(&format!("Could not move to trash: {e}"))),
                Err(_) => v.toasts.add_toast(toast("Could not move to trash")),
            }
        });
    }

    fn undo_trash(&self, path: PathBuf, index: usize) {
        let weak = self.weak();
        glib::MainContext::default().spawn_local(async move {
            let p = path.clone();
            let result =
                gio::spawn_blocking(move || trash::restore(&p, &trash::candidate_trash_dirs(&p)))
                    .await;
            let Some(v) = weak.upgrade() else { return };
            match result {
                Ok(Ok(())) => {
                    {
                        let mut st = v.state.borrow_mut();
                        st.playlist.insert(path.clone(), index);
                        st.playlist.select(&path);
                    }
                    v.load_current();
                }
                Ok(Err(e)) => v
                    .toasts
                    .add_toast(toast(&format!("Could not restore: {e}"))),
                Err(_) => v.toasts.add_toast(toast("Could not restore")),
            }
        });
    }

    // ---- Saving rotation ----------------------------------------------------

    fn save_rotation(&self) {
        let Some(path) = self.current() else { return };
        let degrees = self.canvas.view().rotation;
        if degrees == 0 || !self.state.borrow().savable {
            return;
        }
        self.state.borrow_mut().saving = true;
        self.update_ui();
        let weak = self.weak();
        glib::MainContext::default().spawn_local(async move {
            let p = path.clone();
            let result = gio::spawn_blocking(move || save::save_rotated(&p, degrees)).await;
            let Some(v) = weak.upgrade() else { return };
            v.state.borrow_mut().saving = false;
            match result {
                Ok(Ok(())) => {
                    v.invalidate_thumbnail(&path);
                    v.toasts.add_toast(toast("Image saved"));
                    if v.current().as_deref() == Some(path.as_path()) {
                        v.load_current();
                    } else {
                        v.update_ui();
                    }
                }
                Ok(Err(e)) => {
                    v.toasts
                        .add_toast(toast(&format!("Could not save image: {e}")));
                    v.update_ui();
                }
                Err(_) => {
                    v.toasts.add_toast(toast("Could not save image"));
                    v.update_ui();
                }
            }
        });
    }

    // ---- Thumbnail strip ----------------------------------------------------

    fn setup_strip(
        &self,
        factory: &gtk::SignalListItemFactory,
        results: async_channel::Receiver<(PathBuf, Option<RawThumb>)>,
    ) {
        factory.connect_setup(|_, item| {
            let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
                return;
            };
            let pic = gtk::Picture::builder()
                .content_fit(gtk::ContentFit::Cover)
                .width_request(64)
                .height_request(64)
                .css_classes(["iv-thumb"])
                .build();
            pic.set_overflow(gtk::Overflow::Hidden);
            item.set_child(Some(&pic));
        });
        let weak = self.weak();
        factory.connect_bind(move |_, item| {
            let (Some(v), Some(item)) = (weak.upgrade(), item.downcast_ref::<gtk::ListItem>())
            else {
                return;
            };
            let Some(pic) = item.child().and_downcast::<gtk::Picture>() else {
                return;
            };
            let Some(path) = v
                .strip
                .files
                .borrow()
                .get(item.position() as usize)
                .cloned()
            else {
                return;
            };
            pic.set_tooltip_text(
                path.file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .as_deref(),
            );
            v.strip
                .bound
                .borrow_mut()
                .insert(path.clone(), pic.downgrade());
            let cached = v.strip.cache.borrow().get(&path).cloned();
            match cached {
                Some(t) => pic.set_paintable(Some(&t)),
                None => {
                    pic.set_paintable(None::<&gdk::Paintable>);
                    v.request_thumbnail(path);
                }
            }
        });
        let weak = self.weak();
        factory.connect_unbind(move |_, item| {
            let (Some(v), Some(item)) = (weak.upgrade(), item.downcast_ref::<gtk::ListItem>())
            else {
                return;
            };
            let Some(pic) = item.child().and_downcast::<gtk::Picture>() else {
                return;
            };
            let mut bound = v.strip.bound.borrow_mut();
            let path = bound
                .iter()
                .find(|(_, w)| w.upgrade().as_ref() == Some(&pic))
                .map(|(p, _)| p.clone());
            if let Some(p) = path {
                bound.remove(&p);
                if let Ok(mut w) = v.strip.wanted.lock() {
                    w.remove(&p);
                }
            }
        });

        let weak = self.weak();
        self.strip.selection.connect_selected_notify(move |sel| {
            let Some(v) = weak.upgrade() else { return };
            if v.strip.syncing.get() || sel.selected() == gtk::INVALID_LIST_POSITION {
                return;
            }
            let i = sel.selected() as usize;
            v.navigate(|p| p.go_to(i));
        });

        let weak = self.weak();
        glib::MainContext::default().spawn_local(async move {
            while let Ok((path, thumb)) = results.recv().await {
                let Some(v) = weak.upgrade() else { break };
                if let Ok(mut w) = v.strip.wanted.lock() {
                    w.remove(&path);
                }
                let Some(thumb) = thumb else { continue };
                let tex = thumb_texture(&thumb);
                {
                    let mut cache = v.strip.cache.borrow_mut();
                    if cache.len() >= THUMB_CACHE_MAX {
                        cache.clear();
                    }
                    cache.insert(path.clone(), tex.clone());
                }
                let pic = v.strip.bound.borrow().get(&path).and_then(|w| w.upgrade());
                if let Some(pic) = pic {
                    pic.set_paintable(Some(&tex));
                }
            }
        });
    }

    fn request_thumbnail(&self, path: PathBuf) {
        let fresh = self
            .strip
            .wanted
            .lock()
            .map(|mut w| w.insert(path.clone()))
            .unwrap_or(false);
        if fresh {
            let _ = self.strip.requests.try_send(path);
        }
    }

    fn invalidate_thumbnail(&self, path: &Path) {
        self.strip.cache.borrow_mut().remove(path);
        let bound = self.strip.bound.borrow().contains_key(path);
        if bound {
            self.request_thumbnail(path.to_path_buf());
        }
    }

    /// Mirrors the playlist into the strip and selects the current image.
    fn sync_strip(&self) {
        let (files, index) = {
            let st = self.state.borrow();
            (st.playlist.files().to_vec(), st.playlist.index())
        };
        let strip = &self.strip;
        strip.syncing.set(true);
        if *strip.files.borrow() != files {
            let names: Vec<String> = files
                .iter()
                .map(|p| p.to_string_lossy().into_owned())
                .collect();
            let names: Vec<&str> = names.iter().map(String::as_str).collect();
            strip.files.replace(files.clone());
            strip.model.splice(0, strip.model.n_items(), &names);
        }
        let pos = index
            .map(|i| i as u32)
            .unwrap_or(gtk::INVALID_LIST_POSITION);
        if strip.selection.selected() != pos {
            strip.selection.set_selected(pos);
        }
        strip.syncing.set(false);
        let show = files.len() > 1 && !self.fullscreen.get();
        strip.bar.set_visible(show);
        if let (true, Some(i)) = (show, index) {
            strip
                .view
                .scroll_to(i as u32, gtk::ListScrollFlags::NONE, None);
        }
    }

    // ---- Slideshow --------------------------------------------------------

    fn toggle_slideshow(&self) {
        if self.state.borrow().slideshow.is_some() {
            self.stop_slideshow();
            return;
        }
        self.window.set_fullscreened(true);
        self.restart_slideshow_timer_force();
    }

    fn stop_slideshow(&self) {
        if let Some(id) = self.state.borrow_mut().slideshow.take() {
            id.remove();
        }
    }

    fn restart_slideshow_timer(&self) {
        if self.state.borrow().slideshow.is_some() {
            self.restart_slideshow_timer_force();
        }
    }

    fn restart_slideshow_timer_force(&self) {
        self.stop_slideshow();
        let weak = self.weak();
        let id = glib::timeout_add_local(SLIDESHOW_INTERVAL, move || {
            let Some(v) = weak.upgrade() else {
                return glib::ControlFlow::Break;
            };
            let moved = v.state.borrow_mut().playlist.step(1);
            if moved {
                v.load_current();
                glib::ControlFlow::Continue
            } else {
                v.state.borrow_mut().slideshow = None;
                glib::ControlFlow::Break
            }
        });
        self.state.borrow_mut().slideshow = Some(id);
    }
}

/// Scripted walk-through for headless smoke tests
/// (`HELIX_IMAGE_VIEWER_SMOKE=1`): rotate, save, navigate, properties, quit.
fn smoke_script(viewer: &Rc<Viewer>) {
    let steps: &[(u64, &str)] = &[
        (1000, "win.rotate-right"),
        (1300, "win.save"),
        (2300, "win.next"),
        (2800, "win.properties"),
        (3300, "win.last"),
        (3800, "win.first"),
        (4500, "app.quit"),
    ];
    for (ms, action) in steps.iter().copied() {
        let weak = Rc::downgrade(viewer);
        glib::timeout_add_local_once(Duration::from_millis(ms), move || {
            let Some(v) = weak.upgrade() else { return };
            eprintln!("smoke: {action}");
            let _ = WidgetExt::activate_action(&v.window, action, None);
        });
    }
}

fn v_current(weak: &Weak<Viewer>) -> Option<PathBuf> {
    weak.upgrade().and_then(|v| v.current())
}
