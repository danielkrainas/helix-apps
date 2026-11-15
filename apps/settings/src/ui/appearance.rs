//! Appearance: style, accent colour, icon theme and background.

use adw::prelude::*;
use gtk::{gdk, gio, glib};
use helix_app_common::Prefs;
use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use super::widgets::{self, has_key, watch};
use super::{spawn_worker, Ctx, Panel};
use crate::values::{self, Style, ACCENT_COLORS};
use crate::wallpaper::{self, Wallpaper};
use helix_app_common::i18n::tr;
use helix_app_common::trf;

const INTERFACE: &str = "org.gnome.desktop.interface";
const BACKGROUND: &str = "org.gnome.desktop.background";
const THUMB_W: i32 = 160;
const THUMB_H: i32 = 100;
/// Narrowest a thumbnail gets, so two fit side by side at 360 px.
const THUMB_MIN_W: i32 = 136;

pub struct AppearancePanel;

fn install_css() {
    thread_local!(static DONE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) });
    if DONE.with(|d| d.replace(true)) {
        return;
    }
    let mut css = String::from(
        ".style-preview { min-width: 120px; min-height: 80px; border-radius: 9px; padding: 14px 18px 0 18px; }
         .style-preview.light { background: #deddda; }
         .style-preview.dark { background: #3d3846; }
         .style-window { border-radius: 6px 6px 0 0; }
         .style-preview.light .style-window { background: #ffffff; }
         .style-preview.dark .style-window { background: #222226; }
         .style-titlebar { min-height: 14px; border-radius: 6px 6px 0 0; }
         .style-preview.light .style-titlebar { background: #ebebed; }
         .style-preview.dark .style-titlebar { background: #2e2e32; }
         .style-card { padding: 4px; border-radius: 12px; }
         .accent-swatch { min-width: 30px; min-height: 30px; padding: 0; border-radius: 9999px; }
         .accent-swatch:checked { box-shadow: 0 0 0 3px @window_bg_color, 0 0 0 5px alpha(@window_fg_color, 0.6); }
         .wallpaper-button { padding: 3px; border-radius: 10px; }
         .wallpaper-button picture { border-radius: 8px; }
         .wallpaper-button.current { box-shadow: 0 0 0 3px @accent_bg_color; }
         .wallpaper-check { background: @accent_bg_color; color: @accent_fg_color; border-radius: 9999px; padding: 4px; margin: 8px; }
        ",
    );
    for (nick, _, hex) in ACCENT_COLORS {
        css.push_str(&format!(
            ".accent-swatch.accent-{nick} {{ background: {hex}; }}\n"
        ));
    }
    let provider = gtk::CssProvider::new();
    provider.load_from_string(&css);
    if let Some(display) = gdk::Display::default() {
        gtk::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    }
}

impl Panel for AppearancePanel {
    fn build(&self, ctx: &Rc<Ctx>) -> gtk::Widget {
        install_css();
        let iface = ctx.prefs(INTERFACE);
        let page = widgets::page();
        page.add(&style_group(&iface));
        page.add(&accent_group(&iface));
        page.add(&icons_group(&iface));
        page.add(&background_group(ctx));
        page.upcast()
    }
}

fn style_card(style: Style, label: &str) -> gtk::ToggleButton {
    let preview = gtk::Box::new(gtk::Orientation::Vertical, 0);
    preview.add_css_class("style-preview");
    preview.add_css_class(if style == Style::Dark {
        "dark"
    } else {
        "light"
    });
    let win = gtk::Box::new(gtk::Orientation::Vertical, 0);
    win.add_css_class("style-window");
    win.set_vexpand(true);
    let bar = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    bar.add_css_class("style-titlebar");
    win.append(&bar);
    preview.append(&win);
    let bx = gtk::Box::new(gtk::Orientation::Vertical, 6);
    bx.append(&preview);
    bx.append(&gtk::Label::new(Some(label)));
    let button = gtk::ToggleButton::builder()
        .child(&bx)
        .css_classes(["flat", "style-card"])
        .build();
    helix_app_common::set_accessible_label(&button, label);
    button
}

fn style_group(prefs: &Prefs) -> adw::PreferencesGroup {
    let group = widgets::group(tr("Style"));
    let default = style_card(Style::Default, tr("Default"));
    let dark = style_card(Style::Dark, tr("Dark"));
    dark.set_group(Some(&default));
    let sync = {
        let (d, k, p) = (default.downgrade(), dark.downgrade(), prefs.clone());
        move || {
            let (Some(d), Some(k)) = (d.upgrade(), k.upgrade()) else {
                return;
            };
            match values::style_from_scheme(&p.get_enum_nick("color-scheme", "default")) {
                Style::Default => d.set_active(true),
                Style::Dark => k.set_active(true),
            }
        }
    };
    sync();
    for (button, style) in [(&default, Style::Default), (&dark, Style::Dark)] {
        let p = prefs.clone();
        button.connect_toggled(move |b| {
            let current = p.get_enum_nick("color-scheme", "default");
            if b.is_active() && values::style_from_scheme(&current) != style {
                p.set_enum_nick("color-scheme", values::scheme_for_style(style));
            }
        });
    }
    watch(prefs, "color-scheme", sync);
    // Side by side, stacked when the window is too narrow for both.
    let flow = wrapping_box(2, 24);
    flow_append(&flow, &default);
    flow_append(&flow, &dark);
    group.add(&flow);
    group
}

/// A centred box that wraps its children onto more lines when narrow (at
/// most `per_line` on one line).
fn wrapping_box(per_line: u32, spacing: u32) -> gtk::FlowBox {
    gtk::FlowBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .homogeneous(true)
        .halign(gtk::Align::Center)
        .column_spacing(spacing)
        .row_spacing(spacing)
        .min_children_per_line(1)
        .max_children_per_line(per_line)
        .build()
}

/// Appends `child` to `flow` so that the button itself, not the flow box
/// slot around it, takes keyboard focus.
fn flow_append(flow: &gtk::FlowBox, child: &impl IsA<gtk::Widget>) {
    flow.append(child);
    if let Some(slot) = child.as_ref().parent() {
        slot.set_focusable(false);
    }
}

fn accent_group(prefs: &Prefs) -> adw::PreferencesGroup {
    let group = widgets::group(tr("Accent Color"));
    if !has_key(prefs, "accent-color") {
        group.set_description(Some(widgets::UNSUPPORTED));
    }
    let bx = wrapping_box(ACCENT_COLORS.len() as u32, 10);
    bx.set_margin_top(6);
    bx.set_margin_bottom(6);
    let mut buttons: Vec<(gtk::ToggleButton, &'static str)> = Vec::new();
    for (nick, label, _) in ACCENT_COLORS {
        let b = gtk::ToggleButton::builder()
            .css_classes(["accent-swatch", &format!("accent-{nick}")])
            .tooltip_text(tr(label))
            .valign(gtk::Align::Center)
            .halign(gtk::Align::Center)
            .build();
        helix_app_common::set_accessible_label(&b, tr(label));
        if let Some((first, _)) = buttons.first() {
            b.set_group(Some(first));
        }
        flow_append(&bx, &b);
        buttons.push((b, nick));
    }
    let buttons = Rc::new(buttons);
    let sync = {
        let (buttons, p) = (buttons.clone(), prefs.clone());
        move || {
            let nick = values::accent_nick(&p.get_enum_nick("accent-color", "blue"));
            if let Some((b, _)) = buttons.iter().find(|(_, n)| *n == nick) {
                b.set_active(true);
            }
        }
    };
    sync();
    for (b, nick) in buttons.iter() {
        let p = prefs.clone();
        let nick: &'static str = nick;
        b.connect_toggled(move |b| {
            if b.is_active() && p.get_enum_nick("accent-color", "blue") != nick {
                p.set_enum_nick("accent-color", nick);
            }
        });
    }
    watch(prefs, "accent-color", sync);
    group.add(&bx);
    group
}

/// *Icon Theme*: the installed themes, by name; one set by other tools
/// that is not installed stays listed under its id.
fn icons_group(prefs: &Prefs) -> adw::PreferencesGroup {
    let group = widgets::group(tr("Icons"));
    let row = adw::ComboRow::builder().title(tr("Icon Theme")).build();
    group.add(&row);
    if !has_key(prefs, "icon-theme") {
        group.set_description(Some(widgets::UNSUPPORTED));
        row.set_sensitive(false);
        return group;
    }
    let mut themes = crate::icon_themes::installed(&crate::icon_themes::search_dirs());
    let current: String = prefs.get("icon-theme", String::new());
    if !current.is_empty() && !themes.iter().any(|t| t.id == current) {
        themes.push(crate::icon_themes::IconTheme {
            id: current.clone(),
            name: current,
        });
    }
    let labels: Vec<&str> = themes.iter().map(|t| t.name.as_str()).collect();
    row.set_model(Some(&gtk::StringList::new(&labels)));
    let themes = Rc::new(themes);
    let sync = {
        let (row, themes, p) = (row.downgrade(), themes.clone(), prefs.clone());
        move || {
            let id: String = p.get("icon-theme", String::new());
            if let (Some(row), Some(i)) = (row.upgrade(), themes.iter().position(|t| t.id == id)) {
                row.set_selected(i as u32);
            }
        }
    };
    sync();
    row.connect_selected_notify({
        let (themes, p) = (themes.clone(), prefs.clone());
        move |r| {
            if let Some(t) = themes.get(r.selected() as usize) {
                if p.get("icon-theme", String::new()) != t.id {
                    p.set("icon-theme", t.id.as_str());
                }
            }
        }
    });
    watch(prefs, "icon-theme", sync);
    group
}

fn uri_path(uri: &str) -> Option<PathBuf> {
    if uri.is_empty() {
        None
    } else {
        gio::File::for_uri(uri).path()
    }
}

fn path_uri(path: &Path) -> String {
    glib::filename_to_uri(path, None)
        .map(|s| s.to_string())
        .unwrap_or_default()
}

/// One background tile.
struct Tile {
    wp: Wallpaper,
    /// A toggle button, so assistive technology reads the current
    /// background as "pressed".
    button: gtk::ToggleButton,
    picture: gtk::Picture,
    /// Check mark on the current background (not shown by colour alone).
    check: gtk::Image,
}

struct Background {
    prefs: Prefs,
    flow: gtk::FlowBox,
    /// "No Backgrounds Found", shown when the grid is empty.
    empty: gtk::Label,
    items: RefCell<Vec<Tile>>,
    thumbs: async_channel::Sender<PathBuf>,
    /// Set while the tiles' states are updated from the settings.
    updating: Cell<bool>,
    /// The background folders have been read.
    scanned: Cell<bool>,
}

/// Accessible name of a background tile: its name, and whether it comes
/// as a light and dark pair.
fn tile_name(name: &str, pair: bool) -> String {
    if pair {
        trf!("{name}, light and dark variants", name = name)
    } else {
        name.to_string()
    }
}

impl Background {
    fn refresh_current(&self) {
        let light = uri_path(&self.prefs.get::<String>("picture-uri", String::new()));
        let dark = uri_path(&self.prefs.get::<String>("picture-uri-dark", String::new()));
        self.updating.set(true);
        for t in self.items.borrow().iter() {
            let current = wallpaper::is_current(&t.wp, light.as_deref(), dark.as_deref());
            t.button.set_active(current);
            t.check.set_visible(current);
            if current {
                t.button.add_css_class("current");
            } else {
                t.button.remove_css_class("current");
            }
        }
        self.updating.set(false);
        let empty = self.scanned.get() && self.items.borrow().is_empty();
        self.empty.set_visible(empty);
        self.flow.set_visible(!empty);
    }

    fn apply(&self, wp: &Wallpaper) {
        self.prefs.set("picture-uri", path_uri(&wp.light));
        self.prefs
            .set("picture-uri-dark", path_uri(wp.dark_or_light()));
        self.refresh_current();
    }

    fn add(self: &Rc<Self>, wp: Wallpaper) {
        let picture = gtk::Picture::builder()
            .content_fit(gtk::ContentFit::Cover)
            .width_request(THUMB_MIN_W)
            .height_request(THUMB_H)
            .can_shrink(true)
            .build();
        let name = wp
            .light
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or(tr("Wallpaper"))
            .replace('_', " ");
        let check = gtk::Image::builder()
            .icon_name("object-select-symbolic")
            .halign(gtk::Align::End)
            .valign(gtk::Align::Start)
            .css_classes(["wallpaper-check"])
            .accessible_role(gtk::AccessibleRole::Presentation)
            .visible(false)
            .build();
        let overlay = gtk::Overlay::builder().child(&picture).build();
        overlay.add_overlay(&check);
        let button = gtk::ToggleButton::builder()
            .child(&overlay)
            .css_classes(["flat", "wallpaper-button"])
            .tooltip_text(&name)
            .build();
        helix_app_common::set_accessible_label(&button, &tile_name(&name, wp.dark.is_some()));
        let weak = Rc::downgrade(self);
        let wp2 = wp.clone();
        button.connect_toggled(move |b| {
            let Some(this) = weak.upgrade() else { return };
            if this.updating.get() {
                return;
            }
            if b.is_active() {
                this.apply(&wp2);
            } else {
                // Pressing the current background keeps it current.
                this.refresh_current();
            }
        });
        flow_append(&self.flow, &button);
        let _ = self.thumbs.send_blocking(wp.light.clone());
        self.items.borrow_mut().push(Tile {
            wp,
            button,
            picture,
            check,
        });
    }

    fn set_thumbnail(&self, path: &Path, texture: &gdk::Texture) {
        for t in self.items.borrow().iter() {
            if t.wp.light == path {
                t.picture.set_paintable(Some(texture));
            }
        }
    }
}

type Thumb = (PathBuf, Option<(i32, i32, bool, usize, glib::Bytes)>);

fn load_thumbnail(path: &Path) -> Option<(i32, i32, bool, usize, glib::Bytes)> {
    let pb =
        gtk::gdk_pixbuf::Pixbuf::from_file_at_scale(path, THUMB_W * 2, THUMB_H * 2, true).ok()?;
    Some((
        pb.width(),
        pb.height(),
        pb.has_alpha(),
        pb.rowstride() as usize,
        pb.read_pixel_bytes(),
    ))
}

fn background_group(ctx: &Rc<Ctx>) -> adw::PreferencesGroup {
    let prefs = ctx.prefs(BACKGROUND);
    let group = widgets::group(tr("Background"));
    let add_button = gtk::Button::builder()
        .label(tr("Add Picture…"))
        .css_classes(["flat"])
        .build();
    group.set_header_suffix(Some(&add_button));
    let flow = gtk::FlowBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .homogeneous(true)
        .column_spacing(8)
        .row_spacing(8)
        .min_children_per_line(2)
        .max_children_per_line(6)
        .build();
    group.add(&flow);
    let empty = gtk::Label::builder()
        .label(tr("No Backgrounds Found"))
        .wrap(true)
        .margin_top(12)
        .margin_bottom(12)
        .css_classes(["dim-label"])
        .visible(false)
        .build();
    empty.update_property(&[gtk::accessible::Property::Description(tr(
        "Add a picture with Add Picture…",
    ))]);
    group.add(&empty);

    // Thumbnail worker: paths in, pixel data out.
    let (path_tx, path_rx) = async_channel::unbounded::<PathBuf>();
    let (thumb_tx, thumb_rx) = async_channel::unbounded::<Thumb>();
    std::thread::spawn(move || {
        while let Ok(path) = path_rx.recv_blocking() {
            let t = load_thumbnail(&path);
            if thumb_tx.send_blocking((path, t)).is_err() {
                break;
            }
        }
    });
    let bg = Rc::new(Background {
        prefs: prefs.clone(),
        flow,
        empty,
        items: RefCell::default(),
        thumbs: path_tx,
        updating: Cell::new(false),
        scanned: Cell::new(false),
    });
    let weak = Rc::downgrade(&bg);
    glib::spawn_future_local(async move {
        while let Ok((path, data)) = thumb_rx.recv().await {
            let Some(bg) = weak.upgrade() else { break };
            if let Some((w, h, alpha, stride, bytes)) = data {
                let format = if alpha {
                    gdk::MemoryFormat::R8g8b8a8
                } else {
                    gdk::MemoryFormat::R8g8b8
                };
                let texture = gdk::MemoryTexture::new(w, h, format, &bytes, stride);
                bg.set_thumbnail(&path, texture.upcast_ref());
            }
        }
    });

    let user_dir = glib::user_data_dir().join("backgrounds");
    {
        let user_dir = user_dir.clone();
        let bg = bg.clone();
        spawn_worker(
            move || {
                let mut paths = wallpaper::scan(Path::new(wallpaper::SYSTEM_DIR));
                paths.extend(wallpaper::scan(&user_dir));
                wallpaper::pair(&paths)
            },
            move |wps| {
                for wp in wps {
                    bg.add(wp);
                }
                bg.scanned.set(true);
                bg.refresh_current();
            },
        );
    }
    for key in ["picture-uri", "picture-uri-dark"] {
        let weak = Rc::downgrade(&bg);
        watch(&prefs, key, move || {
            if let Some(bg) = weak.upgrade() {
                bg.refresh_current();
            }
        });
    }

    let window = ctx.window.clone();
    let ctx_weak = Rc::downgrade(ctx);
    add_button.connect_clicked(move |_| {
        let filter = gtk::FileFilter::new();
        filter.set_name(Some(tr("Images")));
        filter.add_pixbuf_formats();
        let filters = gio::ListStore::new::<gtk::FileFilter>();
        filters.append(&filter);
        let dialog = gtk::FileDialog::builder()
            .title(tr("Add Picture"))
            .modal(true)
            .filters(&filters)
            .build();
        let bg = bg.clone();
        let user_dir = user_dir.clone();
        let ctx_weak = ctx_weak.clone();
        dialog.open(Some(&window), gio::Cancellable::NONE, move |res| {
            let Ok(file) = res else { return };
            let Some(src) = file.path() else { return };
            spawn_worker(
                move || -> Result<PathBuf, String> {
                    std::fs::create_dir_all(&user_dir).map_err(|e| e.to_string())?;
                    let name = src
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("picture")
                        .to_string();
                    let dest = wallpaper::unique_destination(&user_dir, &name, |p| p.exists());
                    std::fs::copy(&src, &dest).map_err(|e| e.to_string())?;
                    Ok(dest)
                },
                move |res| match res {
                    Ok(dest) => {
                        let wp = Wallpaper {
                            light: dest,
                            dark: None,
                        };
                        bg.add(wp.clone());
                        bg.apply(&wp);
                    }
                    Err(e) => {
                        if let Some(ctx) = ctx_weak.upgrade() {
                            ctx.toast(&trf!("Could not add picture: {e}", e = e));
                        }
                    }
                },
            );
        });
    });
    group
}

#[cfg(test)]
mod tests {
    #[test]
    fn pairs_are_described() {
        assert_eq!(
            super::tile_name("Adwaita", true),
            "Adwaita, light and dark variants"
        );
        assert_eq!(super::tile_name("Beach", false), "Beach");
    }
}
