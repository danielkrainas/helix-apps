//! GTK user interface.

use adw::prelude::*;
use gtk::{gdk, gio, glib};
use helix_app_common::Labelled;
use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;

use crate::args::{Args, MAX_DELAY};
use crate::naming::{self, Stamp};
use crate::settings::Settings;
use crate::shell::{self, CaptureError, Mode, Request};

pub const APP_ID: &str = "org.helix.Screenshot";
const APP_NAME: &str = "Screenshot";
const COMMENTS: &str = "Save images of your screen or individual windows";

/// Exit status shared between the callbacks and `run`.
type Status = Rc<Cell<u8>>;

pub fn run(args: Args) -> glib::ExitCode {
    let app = adw::Application::builder()
        .application_id(APP_ID)
        .flags(gio::ApplicationFlags::NON_UNIQUE)
        .build();
    helix_app_common::install_standard_actions(&app, APP_NAME, COMMENTS);
    // Quit closes the windows instead, so that a screenshot on the
    // clipboard is kept (see `keep_clipboard`).
    let quit = gio::ActionEntry::builder("quit")
        .activate(|app: &adw::Application, _, _| {
            for w in app.windows() {
                w.close();
            }
        })
        .build();
    app.add_action_entries([quit]);

    let status: Status = Rc::default();
    let args = RefCell::new(Some(args));
    let st = status.clone();
    app.connect_activate(move |app| {
        let Some(args) = args.borrow_mut().take() else {
            return;
        };
        if args.clipboard {
            clipboard_only(app, &args, st.clone());
        } else {
            let win = Win::build(app, &args, st.clone());
            if args.capture_now() {
                win.exit_on_cancel.set(true);
                let req = request_from_args(&args);
                Win::take(&win, req);
            } else {
                win.window.present();
            }
        }
    });
    let code = app.run_with_args::<&str>(&[]);
    match status.get() {
        0 => code,
        n => glib::ExitCode::from(i32::from(n)),
    }
}

pub fn request_from_args(args: &Args) -> Request {
    Request {
        frame: args.frame,
        ..Request::new(
            args.mode.unwrap_or_default(),
            args.delay.unwrap_or(0),
            args.pointer,
        )
    }
}

/// Wayland drops the clipboard's content when its owner exits. While this
/// process owns the clipboard, keep it running (without windows) until
/// another app takes the clipboard.
fn keep_clipboard(app: &adw::Application) {
    thread_local! {
        static KEEPING: Cell<bool> = const { Cell::new(false) };
    }
    let Some(display) = gdk::Display::default() else {
        return;
    };
    let clipboard = display.clipboard();
    if !clipboard.is_local() || KEEPING.with(|k| k.replace(true)) {
        return;
    }
    let guard = RefCell::new(Some(app.hold()));
    clipboard.connect_changed(move |cb| {
        if !cb.is_local() {
            guard.borrow_mut().take();
            KEEPING.with(|k| k.set(false));
        }
    });
}

fn write_file(path: &std::path::Path, png: &[u8]) -> Result<(), String> {
    std::fs::write(path, png).map_err(|e| format!("Could not save {}: {e}", path.display()))
}

/// `--clipboard` (maybe with `--file`): capture, deliver, no main window.
fn clipboard_only(app: &adw::Application, args: &Args, status: Status) {
    let hold = app.hold();
    let req = request_from_args(args);
    let file = args.file.as_deref().map(naming::absolute);
    let app = app.clone();
    glib::spawn_future_local(async move {
        let _hold = hold;
        let png = match gio::spawn_blocking(move || shell::take(&req)).await {
            Ok(Ok(png)) => png,
            Ok(Err(CaptureError::Cancelled)) => {
                status.set(1);
                return;
            }
            Ok(Err(CaptureError::Failed(reason))) => {
                eprintln!("helix-screenshot: {reason}");
                status.set(1);
                return;
            }
            Err(_) => {
                eprintln!("helix-screenshot: the capture thread failed");
                status.set(1);
                return;
            }
        };
        if let Some(file) = file {
            if let Err(e) = write_file(&file, &png) {
                eprintln!("helix-screenshot: {e}");
                status.set(1);
            }
        }
        let texture = match gdk::Texture::from_bytes(&glib::Bytes::from_owned(png)) {
            Ok(t) => t,
            Err(e) => {
                eprintln!("helix-screenshot: could not load the screenshot: {e}");
                status.set(1);
                return;
            }
        };
        // Only the focused client may set the Wayland selection, so show a
        // small window for a moment and copy once it is active.
        let label = gtk::Label::builder()
            .label("Copying screenshot…")
            .margin_top(24)
            .margin_bottom(24)
            .margin_start(24)
            .margin_end(24)
            .build();
        let win = adw::Window::builder()
            .application(&app)
            .title(APP_NAME)
            .resizable(false)
            .content(&label)
            .build();
        let done = Rc::new(Cell::new(false));
        let copy = {
            let app = app.clone();
            let win = win.clone();
            move || {
                if done.replace(true) {
                    return;
                }
                win.clipboard().set_texture(&texture);
                keep_clipboard(&app);
                win.close();
            }
        };
        let copy = Rc::new(copy);
        let c = copy.clone();
        win.connect_is_active_notify(move |w| {
            if w.is_active() {
                // Let the focus event (and its serial) reach GDK first.
                let c = c.clone();
                glib::idle_add_local_once(move || c());
            }
        });
        glib::timeout_add_local_once(std::time::Duration::from_secs(2), move || copy());
        win.present();
    });
}

/// A capture held in memory.
struct Shot {
    png: glib::Bytes,
    texture: gdk::Texture,
    stamp: Stamp,
    mode: Mode,
}

/// The preview's alternative text: what was captured and its size —
/// "Screenshot of the whole screen, 1920 × 1080".
fn preview_text(mode: Mode, width: i32, height: i32) -> String {
    let what = match mode {
        Mode::Screen => "the whole screen",
        Mode::Window => "a window",
        Mode::Area => "a selected area",
    };
    format!("Screenshot of {what}, {width} × {height}")
}

struct Win {
    app: adw::Application,
    window: adw::ApplicationWindow,
    stack: gtk::Stack,
    toasts: adw::ToastOverlay,
    modes: [gtk::ToggleButton; 3],
    delay: adw::SpinRow,
    pointer: adw::SwitchRow,
    take_button: gtk::Button,
    actions: gio::SimpleActionGroup,
    picture: gtk::Picture,
    preview_title: adw::WindowTitle,
    settings: RefCell<Settings>,
    shot: RefCell<Option<Shot>>,
    busy: Cell<bool>,
    /// Started from the command line with a capture: a cancelled
    /// selection ends the app instead of showing the options.
    exit_on_cancel: Cell<bool>,
    status: Status,
}

const MODES: [Mode; 3] = [Mode::Screen, Mode::Window, Mode::Area];

fn mode_button(label: &str, icon: &str) -> gtk::ToggleButton {
    let content = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(6)
        .margin_top(6)
        .margin_bottom(6)
        .build();
    content.append(&gtk::Image::builder().icon_name(icon).pixel_size(32).build());
    let label = gtk::Label::with_mnemonic(label);
    content.append(&label);
    let button = gtk::ToggleButton::builder()
        .child(&content)
        .hexpand(true)
        .build();
    label.set_mnemonic_widget(Some(&button));
    button
}

impl Win {
    fn build(app: &adw::Application, args: &Args, status: Status) -> Rc<Win> {
        let mut settings = Settings::load(&Settings::path());
        if let Some(m) = args.mode {
            settings.mode = m;
        }
        if let Some(d) = args.delay {
            settings.delay = d;
        }
        if args.pointer {
            settings.pointer = true;
        }

        // Options page.
        let menu = gio::Menu::new();
        menu.append(Some("_About Screenshot"), Some("app.about"));
        menu.append(Some("_Quit"), Some("app.quit"));
        let header = adw::HeaderBar::new();
        header.pack_end(
            &gtk::MenuButton::builder()
                .icon_name("open-menu-symbolic")
                .menu_model(&menu)
                .primary(true)
                .tooltip_text("Main Menu")
                .build()
                .labelled("Main Menu"),
        );

        let modes = [
            mode_button("_Screen", "video-single-display-symbolic"),
            mode_button("_Window", "focus-windows-symbolic"),
            mode_button("_Area", "edit-select-all-symbolic"),
        ];
        for b in &modes[1..] {
            b.set_group(Some(&modes[0]));
        }
        // One labelled group of exclusive toggles (libadwaita 1.5 has no
        // toggle group widget).
        let mode_box = gtk::Box::builder()
            .homogeneous(true)
            .css_classes(["linked"])
            .accessible_role(gtk::AccessibleRole::Group)
            .build()
            .labelled("Take");
        for b in &modes {
            mode_box.append(b);
        }
        let mode_group = adw::PreferencesGroup::builder().title("Take").build();
        mode_group.add(&mode_box);

        let delay = adw::SpinRow::with_range(0.0, MAX_DELAY as f64, 1.0);
        delay.set_title("_Delay in Seconds");
        delay.set_use_underline(true);
        delay.set_digits(0);
        let pointer = adw::SwitchRow::builder()
            .title("Show _Pointer")
            .use_underline(true)
            .build();
        let options_group = adw::PreferencesGroup::builder().title("Options").build();
        options_group.add(&delay);
        options_group.add(&pointer);

        let take_button = gtk::Button::builder()
            .label("_Take Screenshot")
            .use_underline(true)
            .halign(gtk::Align::Center)
            .css_classes(["pill", "suggested-action"])
            .build();

        let options = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(24)
            .margin_top(18)
            .margin_bottom(24)
            .margin_start(18)
            .margin_end(18)
            .build();
        options.append(&mode_group);
        options.append(&options_group);
        options.append(&take_button);
        let options_page = adw::ToolbarView::new();
        options_page.add_top_bar(&header);
        options_page.set_content(Some(&adw::Clamp::builder().child(&options).build()));

        // Preview page.
        let back = gtk::Button::builder()
            .icon_name("go-previous-symbolic")
            .tooltip_text("New Screenshot")
            .action_name("win.new")
            .build()
            .labelled("New Screenshot");
        let preview_title = adw::WindowTitle::new("Screenshot", "");
        let preview_header = adw::HeaderBar::builder()
            .title_widget(&preview_title)
            .build();
        preview_header.pack_start(&back);
        let picture = gtk::Picture::builder()
            .can_shrink(true)
            .content_fit(gtk::ContentFit::ScaleDown)
            .hexpand(true)
            .vexpand(true)
            .build();
        let frame = gtk::Frame::builder()
            .child(&picture)
            .margin_top(12)
            .margin_bottom(12)
            .margin_start(12)
            .margin_end(12)
            .css_classes(["card"])
            .build();
        let copy_button = gtk::Button::builder()
            .label("_Copy to Clipboard")
            .use_underline(true)
            .action_name("win.copy")
            .css_classes(["pill"])
            .build();
        let save_button = gtk::Button::builder()
            .label("_Save…")
            .use_underline(true)
            .action_name("win.save")
            .css_classes(["pill", "suggested-action"])
            .build();
        let actions = gtk::Box::builder()
            .spacing(12)
            .halign(gtk::Align::Center)
            .margin_bottom(18)
            .build();
        actions.append(&copy_button);
        actions.append(&save_button);
        let preview_page = adw::ToolbarView::new();
        preview_page.add_top_bar(&preview_header);
        preview_page.set_content(Some(&frame));
        preview_page.add_bottom_bar(&actions);

        let stack = gtk::Stack::builder()
            .transition_type(gtk::StackTransitionType::Crossfade)
            .build();
        stack.add_named(&options_page, Some("options"));
        stack.add_named(&preview_page, Some("preview"));
        let toasts = adw::ToastOverlay::new();
        toasts.set_child(Some(&stack));

        let window = adw::ApplicationWindow::builder()
            .application(app)
            .title(APP_NAME)
            .default_width(560)
            .default_height(520)
            .width_request(360)
            .height_request(400)
            .content(&toasts)
            .build();
        window.set_default_widget(Some(&take_button));

        let win = Rc::new(Win {
            app: app.clone(),
            window,
            stack,
            toasts,
            modes,
            delay,
            pointer,
            take_button,
            actions: gio::SimpleActionGroup::new(),
            picture,
            preview_title,
            settings: RefCell::new(settings),
            shot: RefCell::new(None),
            busy: Cell::new(false),
            exit_on_cancel: Cell::new(false),
            status,
        });
        win.load_settings();
        win.connect();
        win
    }

    fn load_settings(&self) {
        let s = self.settings.borrow().clone();
        let i = MODES.iter().position(|m| *m == s.mode).unwrap_or(0);
        self.modes[i].set_active(true);
        self.delay.set_value(s.delay as f64);
        self.pointer.set_active(s.pointer);
        self.pointer.set_sensitive(s.mode != Mode::Area);
    }

    fn mode(&self) -> Mode {
        self.modes
            .iter()
            .position(|b| b.is_active())
            .map_or(Mode::Screen, |i| MODES[i])
    }

    /// Stores the current choices in the settings file.
    fn store_settings(&self) {
        let mut s = self.settings.borrow_mut();
        s.mode = self.mode();
        s.delay = self.delay.value().round().clamp(0.0, MAX_DELAY as f64) as u32;
        s.pointer = self.pointer.is_active();
        if let Err(e) = s.save(&Settings::path()) {
            eprintln!("helix-screenshot: could not save settings: {e}");
        }
    }

    fn connect(self: &Rc<Self>) {
        for b in &self.modes {
            let w = Rc::downgrade(self);
            b.connect_toggled(move |b| {
                if let Some(w) = w.upgrade() {
                    if b.is_active() {
                        w.pointer.set_sensitive(w.mode() != Mode::Area);
                    }
                }
            });
        }
        let w = Rc::downgrade(self);
        self.take_button.connect_clicked(move |_| {
            let Some(w) = w.upgrade() else { return };
            w.store_settings();
            let s = w.settings.borrow().clone();
            let req = Request {
                pick_window: true,
                ..Request::new(s.mode, s.delay, s.pointer)
            };
            Win::take(&w, req);
        });

        let actions = &self.actions;
        let add = |name: &str, f: fn(&Rc<Win>)| {
            let action = gio::SimpleAction::new(name, None);
            let w = Rc::downgrade(self);
            action.connect_activate(move |_, _| {
                if let Some(w) = w.upgrade() {
                    f(&w);
                }
            });
            actions.add_action(&action);
        };
        add("new", |w| w.show_options());
        add("copy", |w| w.copy());
        add("save", Win::save);
        self.window.insert_action_group("win", Some(actions));
        self.app
            .set_accels_for_action("win.new", &["Escape", "<Alt>Left"]);
        self.app.set_accels_for_action("win.copy", &["<Control>c"]);
        self.app.set_accels_for_action("win.save", &["<Control>s"]);
        self.set_preview_actions(false);

        let w = Rc::downgrade(self);
        self.window.connect_close_request(move |_| {
            if let Some(w) = w.upgrade() {
                keep_clipboard(&w.app);
            }
            glib::Propagation::Proceed
        });
    }

    fn set_preview_actions(&self, on: bool) {
        for name in ["new", "copy", "save"] {
            if let Some(a) = self
                .actions
                .lookup_action(name)
                .and_then(|a| a.downcast::<gio::SimpleAction>().ok())
            {
                a.set_enabled(on);
            }
        }
    }

    fn show_options(&self) {
        self.shot.replace(None);
        self.picture.set_paintable(None::<&gdk::Paintable>);
        self.set_preview_actions(false);
        self.stack.set_visible_child_name("options");
        self.window.set_default_widget(Some(&self.take_button));
    }

    fn show_preview(&self, shot: Shot) {
        self.picture.set_paintable(Some(&shot.texture));
        self.picture.set_alternative_text(Some(&preview_text(
            shot.mode,
            shot.texture.width(),
            shot.texture.height(),
        )));
        self.preview_title.set_subtitle(&format!(
            "{} × {}",
            shot.texture.width(),
            shot.texture.height()
        ));
        self.shot.replace(Some(shot));
        self.set_preview_actions(true);
        self.window.set_default_widget(None::<&gtk::Widget>);
        self.stack.set_visible_child_name("preview");
    }

    /// Hides the window, captures off the main thread, then shows the
    /// preview (or the reason it failed).
    fn take(self: &Rc<Self>, req: Request) {
        if self.busy.replace(true) {
            return;
        }
        let hold = self.app.hold();
        self.window.set_visible(false);
        let w = self.clone();
        let mode = req.mode;
        glib::spawn_future_local(async move {
            let _hold = hold;
            let result = gio::spawn_blocking(move || shell::take(&req))
                .await
                .unwrap_or_else(|_| Err(CaptureError::Failed("The capture thread failed.".into())));
            w.busy.set(false);
            let exit_on_cancel = w.exit_on_cancel.replace(false);
            match result.and_then(|png| {
                let png = glib::Bytes::from_owned(png);
                gdk::Texture::from_bytes(&png)
                    .map(|texture| Shot {
                        png,
                        texture,
                        stamp: Stamp::now(),
                        mode,
                    })
                    .map_err(|e| {
                        CaptureError::Failed(format!("Could not load the screenshot: {e}"))
                    })
            }) {
                Ok(shot) => {
                    w.show_preview(shot);
                    w.window.present();
                }
                Err(CaptureError::Cancelled) if exit_on_cancel => {
                    w.status.set(1);
                    w.window.close();
                }
                Err(CaptureError::Cancelled) => w.window.present(),
                Err(CaptureError::Failed(reason)) => {
                    w.show_options();
                    w.window.present();
                    let dialog =
                        adw::AlertDialog::new(Some("Could Not Take Screenshot"), Some(&reason));
                    dialog.add_response("close", "_Close");
                    dialog.present(Some(&w.window));
                }
            }
        });
    }

    fn copy(&self) {
        let Some(shot) = &*self.shot.borrow() else {
            return;
        };
        self.window.clipboard().set_texture(&shot.texture);
        self.toasts
            .add_toast(adw::Toast::new("Screenshot copied to clipboard"));
    }

    fn save(self: &Rc<Self>) {
        let Some(stamp) = self.shot.borrow().as_ref().map(|s| s.stamp) else {
            return;
        };
        let folder = naming::save_folder(self.settings.borrow().last_folder.as_deref());
        let filter = gtk::FileFilter::new();
        filter.set_name(Some("PNG Images"));
        filter.add_mime_type("image/png");
        let filters = gio::ListStore::new::<gtk::FileFilter>();
        filters.append(&filter);
        let dialog = gtk::FileDialog::builder()
            .title("Save Screenshot")
            .modal(true)
            .initial_folder(&gio::File::for_path(&folder))
            .initial_name(naming::default_file_name(stamp))
            .filters(&filters)
            .accept_label("_Save")
            .build();
        let w = self.clone();
        dialog.save(Some(&self.window), gio::Cancellable::NONE, move |res| {
            let Some(path) = res.ok().and_then(|f| f.path()) else {
                return;
            };
            let Some(png) = w.shot.borrow().as_ref().map(|s| s.png.clone()) else {
                return;
            };
            match write_file(&path, &png) {
                Ok(()) => {
                    let folder: Option<PathBuf> = path.parent().map(Into::into);
                    w.settings.borrow_mut().last_folder = folder;
                    w.store_settings();
                    w.toasts.add_toast(adw::Toast::new("Screenshot saved"));
                }
                Err(reason) => {
                    let dialog =
                        adw::AlertDialog::new(Some("Could Not Save Screenshot"), Some(&reason));
                    dialog.add_response("close", "_Close");
                    dialog.present(Some(&w.window));
                }
            }
        });
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preview_says_what_was_captured() {
        assert_eq!(
            preview_text(Mode::Screen, 1920, 1080),
            "Screenshot of the whole screen, 1920 × 1080"
        );
        assert_eq!(
            preview_text(Mode::Area, 300, 200),
            "Screenshot of a selected area, 300 × 200"
        );
    }
}
