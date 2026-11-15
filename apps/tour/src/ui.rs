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
use helix_app_common::i18n::tr;
use helix_app_common::trf;

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
    helix_app_common::install_standard_actions(
        &app,
        tr("Tour"),
        tr("Get to know the Helix desktop"),
    );
    let session = Rc::new(Session {
        state_path,
        shown: Cell::new(false),
        recorded: Cell::new(false),
    });
    app.connect_startup(|app| {
        load_css();
        helix_app_common::shortcuts::install(
            app,
            crate::shortcuts::ACCELS,
            crate::shortcuts::GROUPS,
        );
    });
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
    css.load_from_string(".tour-nav { min-width: 40px; min-height: 40px; }");
    if let Some(display) = gdk::Display::default() {
        gtk::style_context_add_provider_for_display(
            &display,
            &css,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    }
}

/// What is announced when tour page `idx` comes into view.
fn page_announcement(idx: usize) -> String {
    let title = PAGES.get(idx).map_or("", |p| tr(p.title));
    trf!(
        "{title}, page {idx} of {PAGES}",
        title = title,
        idx = idx + 1,
        PAGES = PAGES.len()
    )
}

fn illustration(art: Art, areas: &mut Vec<gtk::DrawingArea>) -> gtk::DrawingArea {
    let area = gtk::DrawingArea::builder()
        .content_width(illustrations::WIDTH)
        .content_height(illustrations::HEIGHT)
        .halign(gtk::Align::Center)
        .accessible_role(gtk::AccessibleRole::Presentation)
        .build();
    // Decorative artwork (presentation role) with its own light and dark
    // palettes; the text beside it carries the content.
    area.set_draw_func(move |_, cr, w, h| {
        illustrations::draw(art, cr, w, h, adw::StyleManager::default().is_dark());
    });
    helix_app_common::style::redraw_on_style_change(&area);
    areas.push(area.clone());
    area
}

fn title_label(text: &str) -> gtk::Label {
    gtk::Label::builder()
        .label(helix_app_common::i18n::tr_dyn(text))
        .wrap(true)
        .justify(gtk::Justification::Center)
        .css_classes(["title-1"])
        .build()
}

fn body_label(markup: &str) -> gtk::Label {
    gtk::Label::builder()
        .label(helix_app_common::i18n::tr_dyn(markup))
        .use_markup(true)
        .wrap(true)
        .justify(gtk::Justification::Center)
        .max_width_chars(52)
        .css_classes(["body"])
        .build()
}

fn pill(label: &str, suggested: bool) -> gtk::Button {
    // A wrapping label, so a long translation or large text cannot make
    // the button wider than a narrow window.
    let text = gtk::Label::builder()
        .label(label)
        .wrap(true)
        .justify(gtk::Justification::Center)
        .build();
    let b = gtk::Button::builder()
        .child(&text)
        .halign(gtk::Align::Center)
        .css_classes(["pill"])
        .build();
    if suggested {
        b.add_css_class("suggested-action");
    }
    b
}

/// Side margin of a page: room for the previous/next buttons.
const MARGIN: i32 = 72;
/// The same at narrow widths (40 px button + 18 px gap + 2).
const NARROW_MARGIN: i32 = 60;
/// Window width (sp) at or below which pages are laid out narrow.
const NARROW_WIDTH: f64 = 600.0;
/// Illustration size at narrow widths.
const NARROW_ART: (i32, i32) = (220, 134);

fn page_box(boxes: &mut Vec<gtk::Box>) -> gtk::Box {
    let b = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(18)
        .valign(gtk::Align::Center)
        .hexpand(true)
        .vexpand(true)
        .margin_start(MARGIN)
        .margin_end(MARGIN)
        .margin_top(12)
        .margin_bottom(12)
        .build();
    boxes.push(b.clone());
    b
}

/// Puts a page in a scrolled window, so that with large text, a long
/// translation or a small screen every part of it (the buttons too) can
/// still be reached.
fn scrollable(page: &gtk::Box) -> gtk::ScrolledWindow {
    gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .hexpand(true)
        .vexpand(true)
        .child(&adw::Clamp::builder().maximum_size(720).child(page).build())
        .build()
}

fn build(app: &adw::Application, session: Rc<Session>) -> adw::ApplicationWindow {
    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title(tr("Tour"))
        .default_width(800)
        .default_height(640)
        .width_request(360)
        .height_request(294)
        .build();
    helix_app_common::window_state::persist(
        &window,
        helix_app_common::window_state::Store::file("helix-tour"),
    );
    let mut areas = Vec::new();
    let mut boxes = Vec::new();

    // Welcome view.
    let welcome = page_box(&mut boxes);
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
    let no_thanks = pill(tr("No Thanks"), false);
    let start = pill(tr("Start Tour"), true);
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
        let b = page_box(&mut boxes);
        b.append(&illustration(page.art, &mut areas));
        b.append(&title_label(page.title));
        b.append(&body_label(page.body));
        if i == pages::HELP_PAGE {
            let learn = pill(tr("Learn More About Helix"), false);
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
            let close = pill(tr("Close"), true);
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
        carousel.append(&scrollable(&b));
    }

    let prev = gtk::Button::builder()
        .icon_name("go-previous-symbolic")
        .tooltip_text(tr("Previous"))
        .halign(gtk::Align::Start)
        .valign(gtk::Align::Center)
        .margin_start(18)
        .css_classes(["circular", "tour-nav"])
        .build()
        .labelled(tr("Previous"));
    let next = gtk::Button::builder()
        .icon_name("go-next-symbolic")
        .tooltip_text(tr("Next"))
        .halign(gtk::Align::End)
        .valign(gtk::Align::Center)
        .margin_end(18)
        .css_classes(["circular", "tour-nav", "suggested-action"])
        .build()
        .labelled(tr("Next"));
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
    stack.add_named(&scrollable(&welcome), Some("welcome"));
    stack.add_named(&tour, Some("tour"));

    let header = adw::HeaderBar::builder().show_title(false).build();
    let menu = gio::Menu::new();
    menu.append(
        Some(helix_app_common::i18n::tr(
            helix_app_common::shortcuts::MENU_LABEL,
        )),
        Some(helix_app_common::shortcuts::ACTION),
    );
    menu.append(Some(tr("_About Tour")), Some("app.about"));
    header.pack_end(
        &gtk::MenuButton::builder()
            .icon_name("open-menu-symbolic")
            .menu_model(&menu)
            .primary(true)
            .tooltip_text(tr("Main Menu"))
            .build()
            .labelled(tr("Main Menu")),
    );
    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&header);
    toolbar.set_content(Some(&stack));
    window.set_content(Some(&toolbar));

    // Narrow windows: smaller side margins and illustrations.
    let narrow = adw::Breakpoint::new(adw::BreakpointCondition::new_length(
        adw::BreakpointConditionLengthType::MaxWidth,
        NARROW_WIDTH,
        adw::LengthUnit::Sp,
    ));
    for b in &boxes {
        narrow.add_setter(b, "margin-start", Some(&NARROW_MARGIN.to_value()));
        narrow.add_setter(b, "margin-end", Some(&NARROW_MARGIN.to_value()));
    }
    for a in &areas {
        narrow.add_setter(a, "content-width", Some(&NARROW_ART.0.to_value()));
        narrow.add_setter(a, "content-height", Some(&NARROW_ART.1.to_value()));
    }
    // The welcome buttons stack up instead of sitting side by side.
    narrow.add_setter(
        &buttons,
        "orientation",
        Some(&gtk::Orientation::Vertical.to_value()),
    );
    narrow.add_setter(&buttons, "spacing", Some(&6.to_value()));
    window.add_breakpoint(narrow);

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
        carousel.connect_page_changed(move |c, idx| {
            current.set(idx);
            update_arrows(idx);
            // Focus usually stays on the Next button, so say where we are.
            helix_app_common::announce(
                c,
                &page_announcement(idx as usize),
                helix_app_common::Urgency::Polite,
            );
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
            helix_app_common::announce(
                &next,
                &page_announcement(0),
                helix_app_common::Urgency::Polite,
            );
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

    // Left/Right turn the pages of the tour, but only when focus is on the
    // previous/next buttons or on no button at all: on "No Thanks",
    // "Start Tour", "Learn More" or "Close" they keep their usual meaning
    // (moving focus). Escape skips the tour from anywhere.
    let keys = gtk::EventControllerKey::new();
    keys.set_propagation_phase(gtk::PropagationPhase::Capture);
    {
        let (stack, window_weak) = (stack.clone(), window.downgrade());
        let (prev, next) = (prev.clone(), next.clone());
        keys.connect_key_pressed(move |_, key, _, mods| {
            if mods.intersects(
                gdk::ModifierType::CONTROL_MASK
                    | gdk::ModifierType::ALT_MASK
                    | gdk::ModifierType::SUPER_MASK,
            ) {
                return glib::Propagation::Proceed;
            }
            if key == gdk::Key::Escape {
                skip();
                return glib::Propagation::Stop;
            }
            let forward = match key {
                gdk::Key::Right | gdk::Key::KP_Right => true,
                gdk::Key::Left | gdk::Key::KP_Left => false,
                _ => return glib::Propagation::Proceed,
            };
            if stack.visible_child_name().as_deref() != Some("tour") {
                return glib::Propagation::Proceed;
            }
            let Some(window) = window_weak.upgrade() else {
                return glib::Propagation::Proceed;
            };
            let focus = gtk::prelude::GtkWindowExt::focus(&window);
            let on_other_button = focus.as_ref().is_some_and(|f| {
                let nav =
                    f == prev.upcast_ref::<gtk::Widget>() || f == next.upcast_ref::<gtk::Widget>();
                !nav && (f.is::<gtk::Button>() || f.ancestor(gtk::Button::static_type()).is_some())
            });
            if on_other_button {
                return glib::Propagation::Proceed;
            }
            // The carousel runs right to left in right-to-left text.
            let rtl = window.direction() == gtk::TextDirection::Rtl;
            go(if forward != rtl { 1 } else { -1 });
            glib::Propagation::Stop
        });
    }
    window.add_controller(keys);

    window.set_default_widget(Some(&start));
    start.grab_focus();
    window
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_announcements() {
        assert_eq!(
            page_announcement(0),
            format!("{}, page 1 of {}", PAGES[0].title, PAGES.len())
        );
        assert!(
            page_announcement(PAGES.len() - 1).ends_with(&format!("page {0} of {0}", PAGES.len()))
        );
    }
}
