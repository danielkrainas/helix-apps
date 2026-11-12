//! Appearance: style, accent colour and background.

use adw::prelude::*;
use gtk::{gdk, gio, glib};
use helix_app_common::Prefs;
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use super::widgets::{self, has_key, watch};
use super::{spawn_worker, Ctx, Panel};
use crate::values::{self, Style, ACCENT_COLORS};
use crate::wallpaper::{self, Wallpaper};

const INTERFACE: &str = "org.gnome.desktop.interface";
const BACKGROUND: &str = "org.gnome.desktop.background";
const THUMB_W: i32 = 160;
const THUMB_H: i32 = 100;

pub struct AppearancePanel;

fn install_css() {
    thread_local!(static DONE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) });
    if DONE.with(|d| d.replace(true)) {
        return;
    }
    let mut css = String::from(
        ".style-preview { min-width: 150px; min-height: 96px; border-radius: 9px; padding: 14px 18px 0 18px; }
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
        ",
    );
    for (nick, _, hex) in ACCENT_COLORS {
        css.push_str(&format!(".accent-swatch.accent-{nick} {{ background: {hex}; }}\n"));
    }
    let provider = gtk::CssProvider::new();
    provider.load_from_string(&css);
    if let Some(display) = gdk::Display::default() {
        gtk::style_context_add_provider_for_display(&display, &provider, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);
    }
}

impl Panel for AppearancePanel {
    fn build(&self, ctx: &Rc<Ctx>) -> gtk::Widget {
        install_css();
        let iface = ctx.prefs(INTERFACE);
        let page = widgets::page();
        page.add(&style_group(&iface));
        page.add(&accent_group(&iface));
        page.add(&background_group(ctx));
        page.upcast()
    }
}

fn style_card(style: Style, label: &str) -> gtk::ToggleButton {
    let preview = gtk::Box::new(gtk::Orientation::Vertical, 0);
    preview.add_css_class("style-preview");
    preview.add_css_class(if style == Style::Dark { "dark" } else { "light" });
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
    let button = gtk::ToggleButton::builder().child(&bx).css_classes(["flat", "style-card"]).build();
    button.update_property(&[gtk::accessible::Property::Label(label)]);
    button
}

fn style_group(prefs: &Prefs) -> adw::PreferencesGroup {
    let group = widgets::group("Style");
    let default = style_card(Style::Default, "Default");
    let dark = style_card(Style::Dark, "Dark");
    dark.set_group(Some(&default));
    let sync = {
        let (d, k, p) = (default.downgrade(), dark.downgrade(), prefs.clone());
        move || {
            let (Some(d), Some(k)) = (d.upgrade(), k.upgrade()) else { return };
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
    let bx = gtk::Box::builder().spacing(24).halign(gtk::Align::Center).build();
    bx.append(&default);
    bx.append(&dark);
    group.add(&bx);
    group
}

fn accent_group(prefs: &Prefs) -> adw::PreferencesGroup {
    let group = widgets::group("Accent Color");
    if !has_key(prefs, "accent-color") {
        group.set_description(Some(widgets::UNSUPPORTED));
    }
    let bx = gtk::Box::builder().spacing(10).halign(gtk::Align::Center).margin_top(6).margin_bottom(6).build();
    let mut buttons: Vec<(gtk::ToggleButton, &'static str)> = Vec::new();
    for (nick, label, _) in ACCENT_COLORS {
        let b = gtk::ToggleButton::builder()
            .css_classes(["accent-swatch", &format!("accent-{nick}")])
            .tooltip_text(*label)
            .valign(gtk::Align::Center)
            .build();
        b.update_property(&[gtk::accessible::Property::Label(label)]);
        if let Some((first, _)) = buttons.first() {
            b.set_group(Some(first));
        }
        bx.append(&b);
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

fn uri_path(uri: &str) -> Option<PathBuf> {
    if uri.is_empty() {
        None
    } else {
        gio::File::for_uri(uri).path()
    }
}

fn path_uri(path: &Path) -> String {
    glib::filename_to_uri(path, None).map(|s| s.to_string()).unwrap_or_default()
}

struct Background {
    prefs: Prefs,
    flow: gtk::FlowBox,
    items: RefCell<Vec<(Wallpaper, gtk::Button, gtk::Picture)>>,
    thumbs: async_channel::Sender<PathBuf>,
}

impl Background {
    fn refresh_current(&self) {
        let light = uri_path(&self.prefs.get::<String>("picture-uri", String::new()));
        let dark = uri_path(&self.prefs.get::<String>("picture-uri-dark", String::new()));
        for (wp, button, _) in self.items.borrow().iter() {
            if wallpaper::is_current(wp, light.as_deref(), dark.as_deref()) {
                button.add_css_class("current");
            } else {
                button.remove_css_class("current");
            }
        }
    }

    fn apply(&self, wp: &Wallpaper) {
        self.prefs.set("picture-uri", path_uri(&wp.light));
        self.prefs.set("picture-uri-dark", path_uri(wp.dark_or_light()));
        self.refresh_current();
    }

    fn add(self: &Rc<Self>, wp: Wallpaper) {
        let picture = gtk::Picture::builder()
            .content_fit(gtk::ContentFit::Cover)
            .width_request(THUMB_W)
            .height_request(THUMB_H)
            .can_shrink(true)
            .build();
        let name = wp.light.file_stem().and_then(|s| s.to_str()).unwrap_or("Wallpaper").replace('_', " ");
        let button = gtk::Button::builder()
            .child(&picture)
            .css_classes(["flat", "wallpaper-button"])
            .tooltip_text(&name)
            .build();
        button.update_property(&[gtk::accessible::Property::Label(&name)]);
        let weak = Rc::downgrade(self);
        let wp2 = wp.clone();
        button.connect_clicked(move |_| {
            if let Some(this) = weak.upgrade() {
                this.apply(&wp2);
            }
        });
        self.flow.append(&button);
        let _ = self.thumbs.send_blocking(wp.light.clone());
        self.items.borrow_mut().push((wp, button, picture));
    }

    fn set_thumbnail(&self, path: &Path, texture: &gdk::Texture) {
        for (wp, _, picture) in self.items.borrow().iter() {
            if wp.light == path {
                picture.set_paintable(Some(texture));
            }
        }
    }
}

type Thumb = (PathBuf, Option<(i32, i32, bool, usize, glib::Bytes)>);

fn load_thumbnail(path: &Path) -> Option<(i32, i32, bool, usize, glib::Bytes)> {
    let pb = gtk::gdk_pixbuf::Pixbuf::from_file_at_scale(path, THUMB_W * 2, THUMB_H * 2, true).ok()?;
    Some((pb.width(), pb.height(), pb.has_alpha(), pb.rowstride() as usize, pb.read_pixel_bytes()))
}

fn background_group(ctx: &Rc<Ctx>) -> adw::PreferencesGroup {
    let prefs = ctx.prefs(BACKGROUND);
    let group = widgets::group("Background");
    let add_button = gtk::Button::builder().label("Add Picture…").css_classes(["flat"]).build();
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
    let bg = Rc::new(Background { prefs: prefs.clone(), flow, items: RefCell::default(), thumbs: path_tx });
    let weak = Rc::downgrade(&bg);
    glib::spawn_future_local(async move {
        while let Ok((path, data)) = thumb_rx.recv().await {
            let Some(bg) = weak.upgrade() else { break };
            if let Some((w, h, alpha, stride, bytes)) = data {
                let format = if alpha { gdk::MemoryFormat::R8g8b8a8 } else { gdk::MemoryFormat::R8g8b8 };
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
        filter.set_name(Some("Images"));
        filter.add_pixbuf_formats();
        let filters = gio::ListStore::new::<gtk::FileFilter>();
        filters.append(&filter);
        let dialog = gtk::FileDialog::builder().title("Add Picture").modal(true).filters(&filters).build();
        let bg = bg.clone();
        let user_dir = user_dir.clone();
        let ctx_weak = ctx_weak.clone();
        dialog.open(Some(&window), gio::Cancellable::NONE, move |res| {
            let Ok(file) = res else { return };
            let Some(src) = file.path() else { return };
            spawn_worker(
                move || -> Result<PathBuf, String> {
                    std::fs::create_dir_all(&user_dir).map_err(|e| e.to_string())?;
                    let name = src.file_name().and_then(|n| n.to_str()).unwrap_or("picture").to_string();
                    let dest = wallpaper::unique_destination(&user_dir, &name, |p| p.exists());
                    std::fs::copy(&src, &dest).map_err(|e| e.to_string())?;
                    Ok(dest)
                },
                move |res| match res {
                    Ok(dest) => {
                        let wp = Wallpaper { light: dest, dark: None };
                        bg.add(wp.clone());
                        bg.apply(&wp);
                    }
                    Err(e) => {
                        if let Some(ctx) = ctx_weak.upgrade() {
                            ctx.toast(&format!("Could not add picture: {e}"));
                        }
                    }
                },
            );
        });
    });
    group
}
