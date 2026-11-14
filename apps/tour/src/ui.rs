//! GTK user interface.

use adw::prelude::*;
use gtk::{gdk, gio, glib};
use helix_app_common::Labelled;
use std::cell::Cell;
use std::path::PathBuf;
use std::rc::Rc;

use crate::illustrations::{self, Art};
use crate::pages::{self, PAGES};
use crate::state::{self, Outcome};

pub const APP_ID: &str = "org.helix.Tour";
const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Tracks whether the window was shown and whether an outcome was saved.
struct Session {
    state_path: PathBuf,
    shown: Cell<bool>,
    recorded: Cell<bool>,
}

impl Session {
    fn record(&self, outcome: Outcome) {
        if self.recorded.replace(true) {
            return;
        }
        if let Err(e) = state::record(&self.state_path, outcome, VERSION) {
            eprintln!(
                "helix-tour: could not save {}: {e}",
                self.state_path.display()
            );
        }
    }
}

pub fn run(state_path: PathBuf) -> glib::ExitCode {
    let app = adw::Application::builder().application_id(APP_ID).build();
    helix_app_common::install_standard_actions(&app, "Tour", "Get to know the Helix desktop");
    let session = Rc::new(Session {
        state_path,
        shown: Cell::new(false),
        recorded: Cell::new(false),
    });
    app.connect_startup(|_| load_css());
    {
        let session = session.clone();
        app.connect_activate(move |app| {
            if let Some(win) = app.active_window() {
                win.present();
                return;
            }
            session.shown.set(true);
            build(app, session.clone()).present();
        });
    }
    {
        // Closing the window or quitting at any point counts as skipping.
        let session = session.clone();
        app.connect_shutdown(move |_| {
            if session.shown.get() {
                session.record(Outcome::Skipped);
            }
        });
    }
    app.run_with_args(&[std::env::args().next().unwrap_or_default()])
}

fn load_css() {
    let css = gtk::CssProvider::new();
    css.load_from_string(
        ".tour-body { font-size: 12pt; }
         .tour-nav { min-width: 40px; min-height: 40px; }",
    );
    if let Some(display) = gdk::Display::default() {
        gtk::style_context_add_provider_for_display(
            &display,
            &css,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    }
}

fn illustration(art: Art, areas: &mut Vec<gtk::DrawingArea>) -> gtk::DrawingArea {
    let area = gtk::DrawingArea::builder()
        .content_width(illustrations::WIDTH)
        .content_height(illustrations::HEIGHT)
        .halign(gtk::Align::Center)
        .accessible_role(gtk::AccessibleRole::Presentation)
        .build();
    area.set_draw_func(move |_, cr, w, h| {
        illustrations::draw(art, cr, w, h, adw::StyleManager::default().is_dark());
    });
    areas.push(area.clone());
    area
}

fn title_label(text: &str) -> gtk::Label {
    gtk::Label::builder()
        .label(text)
        .wrap(true)
        .justify(gtk::Justification::Center)
        .css_classes(["title-1"])
        .build()
}

fn body_label(markup: &str) -> gtk::Label {
    gtk::Label::builder()
        .label(markup)
        .use_markup(true)
        .wrap(true)
        .justify(gtk::Justification::Center)
        .max_width_chars(52)
        .css_classes(["tour-body"])
        .build()
}

fn pill(label: &str, suggested: bool) -> gtk::Button {
    let b = gtk::Button::builder()
        .label(label)
        .halign(gtk::Align::Center)
        .css_classes(["pill"])
        .build();
    if suggested {
        b.add_css_class("suggested-action");
    }
    b
}

fn page_box() -> gtk::Box {
    gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(18)
        .valign(gtk::Align::Center)
        .hexpand(true)
        .vexpand(true)
        .margin_start(72)
        .margin_end(72)
        .build()
}

fn build(app: &adw::Application, session: Rc<Session>) -> adw::ApplicationWindow {
    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("Tour")
        .default_width(800)
        .default_height(640)
        .resizable(false)
        .build();
    let mut areas = Vec::new();

    // Welcome view.
    let welcome = page_box();
    welcome.append(&illustration(Art::Welcome, &mut areas));
    welcome.append(&title_label(pages::WELCOME_TITLE));
    if let Some(os) = crate::os_release::system_display_name() {
        let markup = format!("You are running <b>{}</b>.", glib::markup_escape_text(&os));
        let os_label = body_label(&markup);
        os_label.add_css_class("dim-label");
        welcome.append(&os_label);
    }
    welcome.append(&body_label(pages::WELCOME_BODY));
    let buttons = gtk::Box::builder()
        .spacing(12)
        .halign(gtk::Align::Center)
        .margin_top(12)
        .homogeneous(true)
        .build();
    let no_thanks = pill("No Thanks", false);
    let start = pill("Start Tour", true);
    buttons.append(&no_thanks);
    buttons.append(&start);
    welcome.append(&buttons);

    // Tour view.
    let carousel = adw::Carousel::builder()
        .hexpand(true)
        .vexpand(true)
        .allow_long_swipes(false)
        .build();
    for (i, page) in PAGES.iter().enumerate() {
        let b = page_box();
        b.append(&illustration(page.art, &mut areas));
        b.append(&title_label(page.title));
        b.append(&body_label(page.body));
        if i == pages::HELP_PAGE {
            let learn = pill("Learn More About Helix", false);
            learn.set_tooltip_text(Some(pages::PROJECT_URL));
            let win = window.downgrade();
            learn.connect_clicked(move |_| {
                let launcher = gtk::UriLauncher::new(pages::PROJECT_URL);
                launcher.launch(win.upgrade().as_ref(), None::<&gio::Cancellable>, |res| {
                    if let Err(e) = res {
                        eprintln!("helix-tour: could not open {}: {e}", pages::PROJECT_URL);
                    }
                });
            });
            b.append(&learn);
        }
        if i == PAGES.len() - 1 {
            let close = pill("Close", true);
            let session = session.clone();
            let win = window.downgrade();
            close.connect_clicked(move |_| {
                session.record(Outcome::Completed);
                if let Some(w) = win.upgrade() {
                    w.close();
                }
            });
            b.append(&close);
        }
        carousel.append(&b);
    }

    let prev = gtk::Button::builder()
        .icon_name("go-previous-symbolic")
        .tooltip_text("Previous")
        .halign(gtk::Align::Start)
        .valign(gtk::Align::Center)
        .margin_start(18)
        .css_classes(["circular", "tour-nav"])
        .build()
        .labelled("Previous");
    let next = gtk::Button::builder()
        .icon_name("go-next-symbolic")
        .tooltip_text("Next")
        .halign(gtk::Align::End)
        .valign(gtk::Align::Center)
        .margin_end(18)
        .css_classes(["circular", "tour-nav", "suggested-action"])
        .build()
        .labelled("Next");
    let overlay = gtk::Overlay::builder().child(&carousel).build();
    overlay.add_overlay(&prev);
    overlay.add_overlay(&next);
    let dots = adw::CarouselIndicatorDots::builder()
        .carousel(&carousel)
        .margin_bottom(18)
        .build();
    let tour = gtk::Box::new(gtk::Orientation::Vertical, 0);
    tour.append(&overlay);
    tour.append(&dots);

    let stack = gtk::Stack::builder()
        .transition_type(gtk::StackTransitionType::Crossfade)
        .build();
    stack.add_named(&welcome, Some("welcome"));
    stack.add_named(&tour, Some("tour"));

    let header = adw::HeaderBar::builder().show_title(false).build();
    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&header);
    toolbar.set_content(Some(&stack));
    window.set_content(Some(&toolbar));

    // Navigation. `current` is the page being shown or scrolled to, so
    // repeated presses advance even while an animation is running.
    let current = Rc::new(Cell::new(0u32));
    let update_arrows = {
        let (prev, next) = (prev.clone(), next.clone());
        move |idx: u32| {
            prev.set_visible(idx > 0);
            next.set_visible(idx + 1 < PAGES.len() as u32);
        }
    };
    update_arrows(0);
    let go = {
        let (carousel, current, update_arrows) =
            (carousel.clone(), current.clone(), update_arrows.clone());
        move |delta: i32| {
            let last = carousel.n_pages().saturating_sub(1) as i32;
            let target = (current.get() as i32 + delta).clamp(0, last) as u32;
            current.set(target);
            update_arrows(target);
            carousel.scroll_to(&carousel.nth_page(target), true);
        }
    };
    {
        let current = current.clone();
        carousel.connect_page_changed(move |_, idx| {
            current.set(idx);
            update_arrows(idx);
        });
    }
    {
        let go = go.clone();
        prev.connect_clicked(move |_| go(-1));
    }
    {
        let go = go.clone();
        next.connect_clicked(move |_| go(1));
    }

    let start_tour = {
        let (stack, next) = (stack.clone(), next.clone());
        move || {
            stack.set_visible_child_name("tour");
            next.grab_focus();
        }
    };
    {
        let start_tour = start_tour.clone();
        start.connect_clicked(move |_| start_tour());
    }
    let skip = {
        let session = session.clone();
        let win = window.downgrade();
        move || {
            session.record(Outcome::Skipped);
            if let Some(w) = win.upgrade() {
                w.close();
            }
        }
    };
    {
        let skip = skip.clone();
        no_thanks.connect_clicked(move |_| skip());
    }

    let keys = gtk::EventControllerKey::new();
    keys.set_propagation_phase(gtk::PropagationPhase::Capture);
    {
        let stack = stack.clone();
        keys.connect_key_pressed(move |_, key, _, mods| {
            if mods.intersects(
                gdk::ModifierType::CONTROL_MASK
                    | gdk::ModifierType::ALT_MASK
                    | gdk::ModifierType::SUPER_MASK,
            ) {
                return glib::Propagation::Proceed;
            }
            let on_tour = stack.visible_child_name().as_deref() == Some("tour");
            match key {
                gdk::Key::Escape => skip(),
                gdk::Key::Right | gdk::Key::KP_Right if on_tour => go(1),
                gdk::Key::Left | gdk::Key::KP_Left if on_tour => go(-1),
                gdk::Key::Right | gdk::Key::KP_Right => start_tour(),
                _ => return glib::Propagation::Proceed,
            }
            glib::Propagation::Stop
        });
    }
    window.add_controller(keys);

    // Redraw illustrations when the light/dark style changes.
    let style = adw::StyleManager::default();
    style.connect_dark_notify(move |_| {
        for a in &areas {
            a.queue_draw();
        }
    });

    window.set_default_widget(Some(&start));
    start.grab_focus();
    window
}
