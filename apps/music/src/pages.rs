//! Views and pages: albums grid, artists, songs, playlists, search, album /
//! artist / playlist pages, Now Playing and preferences.

use adw::prelude::*;
use gtk::{gdk, gio, glib};
use helix_app_common::Labelled;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use crate::model::{self, format_duration, format_total};
use crate::playlist::{self, EntryInfo, Playlist, Smart};
use crate::rows::{Place, RowCtx};
use crate::ui::{
    cover_widget, music_dir, resize_cover, set_heart_icon, toast, App, Live, RowHandle,
};
use helix_app_common::i18n::tr;
use helix_app_common::trf;
use helix_app_common::trnf;

/// Which playlist a page shows.
#[derive(Clone, Debug, PartialEq)]
pub enum PlRef {
    Smart(Smart),
    User(PathBuf),
}

pub fn label(text: &str, classes: &[&str]) -> gtk::Label {
    gtk::Label::builder()
        .label(text)
        .xalign(0.0)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .css_classes(classes.to_vec())
        .build()
}

pub fn scrolled(child: &impl IsA<gtk::Widget>, max: i32) -> gtk::ScrolledWindow {
    let clamp = adw::Clamp::builder()
        .maximum_size(max)
        .child(child)
        .margin_top(18)
        .margin_bottom(24)
        .margin_start(12)
        .margin_end(12)
        .build();
    gtk::ScrolledWindow::builder()
        .child(&clamp)
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vexpand(true)
        .build()
}

/// Adds a button to a header's button row; the button itself, not the
/// flow box slot around it, takes keyboard focus.
fn add_header_button(buttons: &gtk::FlowBox, button: &impl IsA<gtk::Widget>) {
    button.as_ref().set_halign(gtk::Align::Start);
    buttons.append(button);
    // The slot is only a layout cell: no focus, hover or activation of
    // its own (see `.hm-header-buttons` in the CSS).
    if let Some(slot) = button.as_ref().parent().and_downcast::<gtk::FlowBoxChild>() {
        slot.set_focusable(false);
    }
}

/// Lays out an album or playlist header (cover, then text) side by side,
/// or stacked with a smaller cover when the window is narrow.
pub fn layout_header(header: &gtk::Box, narrow: bool) {
    header.set_orientation(if narrow {
        gtk::Orientation::Vertical
    } else {
        gtk::Orientation::Horizontal
    });
    if let Some(cover) = header.first_child() {
        let size = if narrow { 128 } else { 160 };
        if let Some(c) = cover.downcast_ref::<gtk::Overlay>() {
            resize_cover(c, size);
        } else if let Some(a) = cover.downcast_ref::<adw::Avatar>() {
            a.set_size(size);
        }
        cover.set_halign(if narrow {
            gtk::Align::Start
        } else {
            gtk::Align::Center
        });
    }
}

fn page(title: &str, tag: Option<&str>, content: &impl IsA<gtk::Widget>) -> adw::NavigationPage {
    page_with_header(title, tag, content).0
}

/// A pushed page and its header bar (for page-specific buttons).
pub fn page_with_header(
    title: &str,
    tag: Option<&str>,
    content: &impl IsA<gtk::Widget>,
) -> (adw::NavigationPage, adw::HeaderBar) {
    let tv = adw::ToolbarView::new();
    let header = adw::HeaderBar::new();
    tv.add_top_bar(&header);
    tv.set_content(Some(content));
    let p = adw::NavigationPage::builder()
        .title(title)
        .child(&tv)
        .build();
    if let Some(t) = tag {
        p.set_tag(Some(t));
    }
    (p, header)
}

fn pill(label: &str, icon: &str, suggested: bool) -> gtk::Button {
    let content = adw::ButtonContent::builder()
        .label(label)
        .icon_name(icon)
        .build();
    let b = gtk::Button::builder()
        .child(&content)
        .css_classes(["pill"])
        .build();
    if suggested {
        b.add_css_class("suggested-action");
    }
    b
}

/// Registers a recycled list cell's widget: whenever the cell is bound to
/// another track it shows that track's playing, favourite or play-count
/// state, and it follows later changes (see `App::refresh_rows`).
fn live_cell(app: &std::rc::Weak<App>, item: &gtk::ListItem, live: Live) {
    let Some(a) = app.upgrade() else { return };
    let handle = RowHandle::new(None, live);
    a.register_row(handle.clone());
    let app = app.clone();
    item.connect_item_notify(move |item| {
        let Some(app) = app.upgrade() else { return };
        let lib = app.lib();
        *handle.path.borrow_mut() = item
            .item()
            .and_then(|o| lib.tracks.get(unboxed(&o)))
            .map(|t| t.path.clone());
        app.refresh_row(&handle);
    });
}

/// A search section's heading with its number of matches, and a Show
/// All button when only some are listed (it disappears once used).
fn section_header(title: &str, total: usize, show_all: Option<Box<dyn Fn()>>) -> gtk::Box {
    let header = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    header.append(&label(title, &["heading"]));
    let count = label(&total.to_string(), &["dim-label", "numeric"]);
    count.set_hexpand(true);
    header.append(&count);
    if let Some(f) = show_all {
        let b = gtk::Button::builder()
            .label(tr("Show All"))
            .css_classes(["flat"])
            .build();
        b.connect_clicked(move |b| {
            f();
            b.set_visible(false);
        });
        header.append(&b);
    }
    header
}

/// The menu button of the Songs row that `focus` is in (or is).
fn row_menu_button(focus: &gtk::Widget) -> Option<gtk::MenuButton> {
    fn find(w: &gtk::Widget) -> Option<gtk::MenuButton> {
        if w.widget_name() == "hm-song-menu" {
            return w.clone().downcast().ok();
        }
        let mut c = w.first_child();
        while let Some(child) = c {
            if let Some(found) = find(&child) {
                return Some(found);
            }
            c = child.next_sibling();
        }
        None
    }
    let mut w = Some(focus.clone());
    while let Some(cur) = w {
        if cur.css_name() == "row" {
            return find(&cur);
        }
        w = cur.parent();
    }
    None
}

/// A context-menu popover for children of `host`, removed with it.
pub fn context_menu_host(host: &impl IsA<gtk::Widget>) -> gtk::PopoverMenu {
    let pop = gtk::PopoverMenu::from_model(None::<&gio::MenuModel>);
    pop.set_has_arrow(false);
    pop.set_halign(gtk::Align::Start);
    pop.set_parent(host);
    let p = pop.clone();
    host.as_ref().connect_destroy(move |_| p.unparent());
    pop
}

/// Shows `menu` from `pop` pointing at (`x`, `y`) in `widget`.
pub fn popup_at(
    pop: &gtk::PopoverMenu,
    widget: &impl IsA<gtk::Widget>,
    x: f64,
    y: f64,
    menu: &gio::Menu,
) {
    let Some(host) = pop.parent() else { return };
    let Some(p) = widget
        .as_ref()
        .compute_point(&host, &gtk::graphene::Point::new(x as f32, y as f32))
    else {
        return;
    };
    pop.set_menu_model(Some(menu));
    pop.set_pointing_to(Some(&gdk::Rectangle::new(p.x() as i32, p.y() as i32, 1, 1)));
    pop.popup();
}

/// Calls `f` with the position of a right-click or a touch long-press on
/// `widget`.
pub fn on_context_click<W: IsA<gtk::Widget>>(
    widget: &W,
    f: impl Fn(&W, f64, f64) + Clone + 'static,
) {
    let click = gtk::GestureClick::builder()
        .button(gdk::BUTTON_SECONDARY)
        .build();
    let (w, f1) = (widget.downgrade(), f.clone());
    click.connect_pressed(move |g, _, x, y| {
        g.set_state(gtk::EventSequenceState::Claimed);
        if let Some(w) = w.upgrade() {
            f1(&w, x, y);
        }
    });
    widget.add_controller(click);
    let long = gtk::GestureLongPress::builder().touch_only(true).build();
    let w = widget.downgrade();
    long.connect_pressed(move |g, x, y| {
        g.set_state(gtk::EventSequenceState::Claimed);
        if let Some(w) = w.upgrade() {
            f(&w, x, y);
        }
    });
    widget.add_controller(long);
}

/// An artist's initials in a coloured circle (decorative: the row says
/// the name).
fn artist_avatar(name: &str, size: i32) -> adw::Avatar {
    adw::Avatar::builder()
        .text(name)
        .show_initials(true)
        .size(size)
        .accessible_role(gtk::AccessibleRole::Presentation)
        .build()
}

/// What a page header shows next to its title.
pub enum Art<'a> {
    Cover(Option<&'a str>),
    /// An artist: their initials.
    Avatar(&'a str),
    /// A placeholder tile with this icon.
    Icon(&'static str),
}

/// Play Next / Add to Queue / Add to Playlist… for several tracks.
pub fn tracks_menu(tracks: &[usize]) -> gio::Menu {
    let target: Vec<u32> = tracks.iter().map(|&t| t as u32).collect();
    let target = target.to_variant();
    let menu = gio::Menu::new();
    for (label, action) in [
        (tr("Play Next"), "win.tracks-play-next"),
        (tr("Add to Queue"), "win.tracks-queue"),
        (tr("Add to Playlist…"), "win.tracks-add-to-playlist"),
    ] {
        let it = gio::MenuItem::new(Some(label), None);
        it.set_action_and_target_value(Some(action), Some(&target));
        menu.append_item(&it);
    }
    menu
}

/// An album tile: cover, then title and artist, all on the cover's left
/// edge and no wider than the cover.
fn album_tile() -> gtk::Box {
    const SIZE: i32 = 160;
    let tile = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(4)
        .halign(gtk::Align::Center)
        .valign(gtk::Align::Start)
        .build();
    tile.append(&cover_widget(SIZE));
    for classes in [&["heading"][..], &["dim-label"][..]] {
        let l = label("", classes);
        // Ellipsized at the cover's width rather than widening the tile.
        l.set_width_request(SIZE);
        l.set_max_width_chars(1);
        l.set_margin_top(if classes[0] == "heading" { 6 } else { 0 });
        tile.append(&l);
    }
    tile
}

fn boxed(i: usize) -> glib::BoxedAnyObject {
    glib::BoxedAnyObject::new(i)
}

fn unboxed(o: &glib::Object) -> usize {
    o.downcast_ref::<glib::BoxedAnyObject>()
        .map(|b| *b.borrow::<usize>())
        .unwrap_or(0)
}

impl App {
    /// Shows album `ai` in a tile from [`album_tile`].
    pub fn bind_album_tile(&self, tile: &gtk::Box, ai: usize) {
        let lib = self.lib();
        let Some(album) = lib.albums.get(ai) else {
            return;
        };
        let Some(cover) = tile.first_child().and_downcast::<gtk::Overlay>() else {
            return;
        };
        self.set_cover(&cover, album.cover.as_deref());
        let title = cover.next_sibling().and_downcast::<gtk::Label>();
        let artist = title
            .as_ref()
            .and_then(|t| t.next_sibling())
            .and_downcast::<gtk::Label>();
        if let (Some(title), Some(artist)) = (title, artist) {
            title.set_label(&album.title);
            artist.set_label(&album.artist);
        }
        tile.set_tooltip_text(Some(&format!("{} — {}", album.title, album.artist)));
    }

    pub fn build_views(self: &Rc<Self>) {
        // Albums grid.
        let grid = gtk::GridView::builder()
            .model(&gtk::NoSelection::new(Some(self.albums_model.clone())))
            .single_click_activate(true)
            .max_columns(12)
            .min_columns(1)
            .css_classes(["hm-albums"])
            .build();
        let menu = context_menu_host(&grid);
        let factory = gtk::SignalListItemFactory::new();
        let w = Rc::downgrade(self);
        factory.connect_setup(move |_, item| {
            let item = item.downcast_ref::<gtk::ListItem>().unwrap();
            let tile = album_tile();
            let (w, m, it) = (w.clone(), menu.clone(), item.downgrade());
            on_context_click(&tile, move |tile, x, y| {
                let (Some(app), Some(obj)) = (w.upgrade(), it.upgrade().and_then(|i| i.item()))
                else {
                    return;
                };
                popup_at(&m, tile, x, y, &app.album_menu(unboxed(&obj)));
            });
            item.set_child(Some(&tile));
        });
        let w = Rc::downgrade(self);
        factory.connect_bind(move |_, item| {
            let Some(app) = w.upgrade() else { return };
            let item = item.downcast_ref::<gtk::ListItem>().unwrap();
            if let (Some(obj), Some(tile)) = (item.item(), item.child().and_downcast()) {
                app.bind_album_tile(&tile, unboxed(&obj));
            }
        });
        grid.set_factory(Some(&factory));
        let w = Rc::downgrade(self);
        grid.connect_activate(move |g, pos| {
            if let (Some(app), Some(obj)) = (w.upgrade(), g.model().and_then(|m| m.item(pos))) {
                app.show_album(unboxed(&obj));
            }
        });
        let albums_scroll = gtk::ScrolledWindow::builder()
            .child(&grid)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .build();
        self.stack.add_titled_with_icon(
            &albums_scroll,
            Some("albums"),
            tr("Albums"),
            "media-optical-cd-audio-symbolic",
        );

        // Artists.
        let w = Rc::downgrade(self);
        self.artists_list.connect_row_activated(move |_, row| {
            if let Some(app) = w.upgrade() {
                let ar = app.artist_order.borrow().get(row.index() as usize).copied();
                if let Some(ar) = ar {
                    app.show_artist(ar);
                }
            }
        });
        // Narrow windows have no room for a fifth view in the bottom bar:
        // Genres is then a row above the artists.
        let genres_entry = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .css_classes(["boxed-list"])
            .visible(false)
            .build();
        let row = adw::ActionRow::builder()
            .title(tr("Genres"))
            .activatable(true)
            .build();
        row.add_prefix(&gtk::Image::from_icon_name("emoji-symbols-symbolic"));
        row.add_suffix(&gtk::Image::from_icon_name("go-next-symbolic"));
        genres_entry.append(&row);
        let w = Rc::downgrade(self);
        genres_entry.connect_row_activated(move |_, _| {
            if let Some(app) = w.upgrade() {
                app.show_genres_page();
            }
        });
        self.narrow
            .add_setter(&genres_entry, "visible", Some(&true.to_value()));
        let artists_box = gtk::Box::new(gtk::Orientation::Vertical, 12);
        artists_box.set_valign(gtk::Align::Start);
        artists_box.append(&genres_entry);
        artists_box.append(&self.artists_list);
        self.stack.add_titled_with_icon(
            &scrolled(&artists_box, 700),
            Some("artists"),
            tr("Artists"),
            "system-users-symbolic",
        );

        // Songs.
        let songs = self.build_songs_view();
        self.stack.add_titled_with_icon(
            &songs,
            Some("songs"),
            tr("Songs"),
            "audio-x-generic-symbolic",
        );

        // Playlists.
        // New Playlist is a header button while this view is shown.
        let pl_box = gtk::Box::new(gtk::Orientation::Vertical, 12);
        pl_box.append(&self.playlists_list);
        let w = Rc::downgrade(self);
        self.playlists_list.connect_row_activated(move |_, row| {
            let Some(app) = w.upgrade() else { return };
            let i = row.index() as usize;
            let r = if i < Smart::ALL.len() {
                PlRef::Smart(Smart::ALL[i])
            } else {
                match app.playlists.borrow().get(i - Smart::ALL.len()) {
                    Some(p) => PlRef::User(p.file.clone()),
                    None => return,
                }
            };
            app.show_playlist(r);
        });
        self.stack.add_titled_with_icon(
            &scrolled(&pl_box, 700),
            Some("playlists"),
            tr("Playlists"),
            "view-list-symbolic",
        );

        // Genres.
        let w = Rc::downgrade(self);
        self.genres_list.connect_row_activated(move |_, row| {
            if let Some(app) = w.upgrade() {
                app.show_genre(row.index() as usize);
            }
        });
        let genres = self.stack.add_titled_with_icon(
            &scrolled(&self.genres_list, 700),
            Some("genres"),
            tr("Genres"),
            // Shapes for kinds of music (the note is Songs').
            "emoji-symbols-symbolic",
        );
        self.narrow
            .add_setter(&genres, "visible", Some(&false.to_value()));

        let empty_btn = gtk::Button::builder()
            .label(tr("Add Folder…"))
            .action_name("win.add-folder")
            .halign(gtk::Align::Center)
            .css_classes(["pill", "suggested-action"])
            .build();
        self.empty_page.set_child(Some(&empty_btn));

        // User playlist actions (target: the playlist file).
        let playlist_action = |name: &str, f: fn(&Rc<App>, &Path)| {
            let a = gio::SimpleAction::new(name, Some(glib::VariantTy::STRING));
            let w = Rc::downgrade(self);
            a.connect_activate(move |_, v| {
                if let (Some(app), Some(file)) = (w.upgrade(), v.and_then(|v| v.get::<String>())) {
                    f(&app, Path::new(&file));
                }
            });
            self.window.add_action(&a);
        };
        playlist_action("playlist-rename", |app, f| {
            app.new_playlist_dialog(Some(f.to_path_buf()))
        });
        playlist_action("playlist-export", |app, f| app.export_playlist_dialog(f));
        playlist_action("playlist-delete", |app, f| app.delete_playlist_dialog(f));

        let a = gio::SimpleAction::new(
            "playlist-move",
            Some(&<(String, u32, u32)>::static_variant_type()),
        );
        let w = Rc::downgrade(self);
        a.connect_activate(move |_, v| {
            if let (Some(app), Some((file, from, to))) =
                (w.upgrade(), v.and_then(|v| v.get::<(String, u32, u32)>()))
            {
                app.move_in_playlist(Path::new(&file), from as usize, to as usize);
            }
        });
        self.window.add_action(&a);
        let a = gio::SimpleAction::new("queue-move", Some(&<(u32, u32)>::static_variant_type()));
        let w = Rc::downgrade(self);
        a.connect_activate(move |_, v| {
            if let (Some(app), Some((from, to))) =
                (w.upgrade(), v.and_then(|v| v.get::<(u32, u32)>()))
            {
                app.move_in_queue(from as usize, to as usize);
            }
        });
        self.window.add_action(&a);

        let a = gio::SimpleAction::new("queue-remove", Some(glib::VariantTy::UINT32));
        let w = Rc::downgrade(self);
        a.connect_activate(move |_, v| {
            if let (Some(app), Some(i)) = (w.upgrade(), v.and_then(|v| v.get::<u32>())) {
                let was_current = app.queue.borrow_mut().remove(i as usize);
                if was_current {
                    let playing = app.playing.get();
                    if app.queue.borrow().is_empty() {
                        app.stop();
                    } else {
                        app.start_current(playing);
                    }
                }
                app.update_next_uri();
                app.update_now_playing();
            }
        });
        self.window.add_action(&a);
    }

    fn build_songs_view(self: &Rc<Self>) -> gtk::Box {
        let cv = gtk::ColumnView::builder()
            .single_click_activate(true)
            .show_row_separators(false)
            .css_classes(["data-table"])
            .build();
        let w = Rc::downgrade(self);
        // Right-click or long-press on a row: its menu, at the pointer.
        let host = context_menu_host(&cv);
        let with_menu = {
            let w = w.clone();
            move |widget: &gtk::Widget, item: &gtk::ListItem| {
                let (w, host, it) = (w.clone(), host.clone(), item.downgrade());
                on_context_click(widget, move |widget, x, y| {
                    let (Some(app), Some(obj)) = (w.upgrade(), it.upgrade().and_then(|i| i.item()))
                    else {
                        return;
                    };
                    if let Some(menu) = app.song_menu(unboxed(&obj)) {
                        popup_at(&host, widget, x, y, &menu);
                    }
                });
            }
        };
        let text_col = |title: &str, expand: bool, get: fn(&model::Track, &App) -> String| {
            let f = gtk::SignalListItemFactory::new();
            let with_menu = with_menu.clone();
            f.connect_setup(move |_, item| {
                let item = item.downcast_ref::<gtk::ListItem>().unwrap();
                let l = label("", &[]);
                with_menu(l.upcast_ref(), item);
                item.set_child(Some(&l));
            });
            let w = w.clone();
            f.connect_bind(move |_, item| {
                let Some(app) = w.upgrade() else { return };
                let item = item.downcast_ref::<gtk::ListItem>().unwrap();
                let i = unboxed(&item.item().unwrap());
                let lib = app.lib();
                if let (Some(t), Some(l)) =
                    (lib.tracks.get(i), item.child().and_downcast::<gtk::Label>())
                {
                    l.set_label(&get(t, &app));
                }
            });
            gtk::ColumnViewColumn::builder()
                .title(title)
                .factory(&f)
                .expand(expand)
                .resizable(true)
                .build()
        };
        let sorter = |key: fn(&model::Track, &App) -> SortKey| {
            let w = w.clone();
            gtk::CustomSorter::new(move |a, b| {
                let Some(app) = w.upgrade() else {
                    return gtk::Ordering::Equal;
                };
                let lib = app.lib();
                let (ta, tb) = (lib.tracks.get(unboxed(a)), lib.tracks.get(unboxed(b)));
                match (ta, tb) {
                    (Some(ta), Some(tb)) => key(ta, &app).cmp(&key(tb, &app)).into(),
                    _ => gtk::Ordering::Equal,
                }
            })
        };

        // Playing indicator: registered so it follows the playing track.
        let f = gtk::SignalListItemFactory::new();
        let (wi, wm) = (w.clone(), with_menu.clone());
        f.connect_setup(move |_, item| {
            let item = item.downcast_ref::<gtk::ListItem>().unwrap();
            let img = gtk::Image::new();
            wm(img.upcast_ref(), item);
            item.set_child(Some(&img));
            live_cell(&wi, item, Live::PlayingIcon(img.downgrade()));
        });
        cv.append_column(
            &gtk::ColumnViewColumn::builder()
                .title(tr("Playing"))
                .factory(&f)
                .build(),
        );

        let title = text_col(tr("Title"), true, |t, _| t.title.clone());
        title.set_sorter(Some(&sorter(|t, _| {
            SortKey::Text(model::sort_key(&t.title))
        })));
        let artist = text_col(tr("Artist"), true, |t, _| t.artist_name().to_string());
        artist.set_sorter(Some(&sorter(|t, _| {
            SortKey::Text(model::sort_key(t.artist_name()))
        })));
        let album = text_col(tr("Album"), true, |t, _| t.album_name().to_string());
        album.set_sorter(Some(&sorter(|t, _| {
            SortKey::Text(model::sort_key(t.album_name()))
        })));
        let length = text_col(tr("Length"), false, |t, _| format_duration(t.duration_ms));
        length.set_sorter(Some(&sorter(|t, _| SortKey::Num(t.duration_ms))));
        // Play counts change while the table is shown.
        let f = gtk::SignalListItemFactory::new();
        let (wp, wm) = (w.clone(), with_menu.clone());
        f.connect_setup(move |_, item| {
            let item = item.downcast_ref::<gtk::ListItem>().unwrap();
            let l = label("", &["numeric"]);
            wm(l.upcast_ref(), item);
            item.set_child(Some(&l));
            live_cell(&wp, item, Live::Plays(l.downgrade()));
        });
        let plays = gtk::ColumnViewColumn::builder()
            .title(tr("Plays"))
            .factory(&f)
            .resizable(true)
            .build();
        plays.set_sorter(Some(&sorter(|t, app| {
            SortKey::Num(app.stats.borrow().plays(&t.path) as u64)
        })));
        let genre = text_col(tr("Genre"), true, |t, _| {
            t.genre.clone().unwrap_or_default()
        });
        genre.set_sorter(Some(&sorter(|t, _| {
            // Songs without a genre sort last.
            SortKey::Text(
                t.genre
                    .as_deref()
                    .map_or_else(|| "\u{10ffff}".into(), model::sort_key),
            )
        })));
        for c in [&title, &artist, &album, &genre, &length, &plays] {
            cv.append_column(c);
        }
        // Narrow windows show title, artist and length only.
        for c in [&album, &genre, &plays] {
            self.narrow
                .add_setter(c, "visible", Some(&false.to_value()));
        }

        // Favourite toggle.
        let f = gtk::SignalListItemFactory::new();
        let ws = w.clone();
        f.connect_setup(move |_, item| {
            let item = item.downcast_ref::<gtk::ListItem>().unwrap();
            let heart = gtk::ToggleButton::builder()
                .icon_name("non-starred-symbolic")
                .css_classes(["flat"])
                .build()
                .labelled(tr("Add to Favorites"));
            let w2 = ws.clone();
            let it = item.downgrade();
            heart.connect_toggled(move |h| {
                set_heart_icon(h);
                let (Some(app), Some(item)) = (w2.upgrade(), it.upgrade()) else {
                    return;
                };
                let Some(obj) = item.item() else { return };
                let lib = app.lib();
                if let Some(t) = lib.tracks.get(unboxed(&obj)) {
                    app.set_favorite(&t.path, h.is_active());
                }
            });
            item.set_child(Some(&heart));
            live_cell(&ws, item, Live::Heart(heart.downgrade()));
        });
        cv.append_column(
            &gtk::ColumnViewColumn::builder()
                .title(tr("Favorite"))
                .factory(&f)
                .build(),
        );

        // The row menu, also for the keyboard (Tab to it, or Menu).
        let f = gtk::SignalListItemFactory::new();
        let wm = w.clone();
        f.connect_setup(move |_, item| {
            let item = item.downcast_ref::<gtk::ListItem>().unwrap();
            let more = gtk::MenuButton::builder()
                .icon_name("view-more-symbolic")
                .valign(gtk::Align::Center)
                .css_classes(["flat"])
                .tooltip_text(tr("More"))
                .name("hm-song-menu")
                .build()
                .labelled(tr("More"));
            let (w, it) = (wm.clone(), item.downgrade());
            more.set_create_popup_func(move |mb| {
                let (Some(app), Some(obj)) = (w.upgrade(), it.upgrade().and_then(|i| i.item()))
                else {
                    return;
                };
                mb.set_menu_model(app.song_menu(unboxed(&obj)).as_ref());
            });
            item.set_child(Some(&more));
        });
        cv.append_column(&gtk::ColumnViewColumn::builder().factory(&f).build());
        let keys = gtk::EventControllerKey::new();
        let wk = w.clone();
        keys.connect_key_pressed(move |_, key, _, mods| {
            let menu_key = key == gdk::Key::Menu
                || (key == gdk::Key::F10 && mods.contains(gdk::ModifierType::SHIFT_MASK));
            let Some(app) = wk.upgrade().filter(|_| menu_key) else {
                return glib::Propagation::Proceed;
            };
            match gtk::prelude::GtkWindowExt::focus(&app.window).and_then(|f| row_menu_button(&f)) {
                Some(mb) => {
                    mb.popup();
                    glib::Propagation::Stop
                }
                None => glib::Propagation::Proceed,
            }
        });
        cv.add_controller(keys);

        self.songs_sorted.set_sorter(cv.sorter().as_ref());
        self.songs_selection.set_model(Some(&self.songs_sorted));
        cv.set_model(Some(&gtk::NoSelection::new(Some(
            self.songs_sorted.clone(),
        ))));
        let w = Rc::downgrade(self);
        cv.connect_activate(move |_, pos| {
            let Some(app) = w.upgrade() else { return };
            let model = &app.songs_sorted;
            let order: Vec<usize> = (0..model.n_items())
                .filter_map(|i| model.item(i))
                .map(|o| unboxed(&o))
                .collect();
            app.play_tracks(&order, pos as usize, None);
        });
        // Columns never make the window wider than it is.
        let scroll = gtk::ScrolledWindow::builder()
            .child(&cv)
            .hscrollbar_policy(gtk::PolicyType::Automatic)
            .vexpand(true)
            .build();

        // Selection mode (the header's Select button): click, Ctrl- and
        // Shift-click select; the action bar acts on the selected songs.
        // Everything here can shrink: a hidden action bar still asks for
        // its width, and narrow windows are 360 px wide.
        let count = gtk::Label::builder()
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .width_chars(4)
            .build();
        let bar = gtk::ActionBar::builder().revealed(false).build();
        bar.set_center_widget(Some(&count));
        let buttons: Vec<gtk::Button> = [
            (tr("Play"), "media-playback-start-symbolic", "play"),
            (tr("Add to Queue"), "list-add-symbolic", "queue"),
            (tr("Add to Playlist…"), "view-list-symbolic", "playlist"),
        ]
        .into_iter()
        .map(|(text, icon, what)| {
            let b = gtk::Button::builder()
                .child(
                    &adw::ButtonContent::builder()
                        .label(text)
                        .icon_name(icon)
                        .can_shrink(true)
                        .build(),
                )
                .tooltip_text(text)
                .build();
            let w = Rc::downgrade(self);
            b.connect_clicked(move |_| {
                if let Some(app) = w.upgrade() {
                    app.act_on_selection(what);
                }
            });
            b
        })
        .collect();
        buttons[0].add_css_class("suggested-action");
        bar.pack_start(&buttons[0]);
        bar.pack_end(&buttons[2]);
        bar.pack_end(&buttons[1]);
        let (wc, cl) = (count.downgrade(), buttons.clone());
        let update = move |sel: &gtk::MultiSelection| {
            let n = sel.selection().size() as usize;
            if let Some(c) = wc.upgrade() {
                c.set_label(&if n == 0 {
                    tr("No Songs Selected").to_string()
                } else {
                    trnf!("{n} song selected", "{n} songs selected", n)
                });
            }
            for b in &cl {
                b.set_sensitive(n > 0);
            }
        };
        update(&self.songs_selection);
        self.songs_selection
            .connect_selection_changed(move |sel, _, _| update(sel));
        let (w, cvw, barw) = (Rc::downgrade(self), cv.downgrade(), bar.downgrade());
        self.songs_select.connect_toggled(move |b| {
            let (Some(app), Some(cv)) = (w.upgrade(), cvw.upgrade()) else {
                return;
            };
            let on = b.is_active();
            app.songs_selection.unselect_all();
            if on {
                cv.set_model(Some(&app.songs_selection));
            } else {
                cv.set_model(Some(&gtk::NoSelection::new(Some(app.songs_sorted.clone()))));
            }
            cv.set_single_click_activate(!on);
            if let Some(bar) = barw.upgrade() {
                bar.set_revealed(on);
            }
        });
        let page = gtk::Box::new(gtk::Orientation::Vertical, 0);
        page.append(&scroll);
        page.append(&bar);
        page
    }

    /// The Songs menu for track `idx`.
    fn song_menu(&self, idx: usize) -> Option<gio::Menu> {
        let lib = self.lib();
        let artist = lib.artist_index(lib.tracks.get(idx)?.artist_name());
        Some(crate::rows::track_menu(
            idx,
            &RowCtx::Plain,
            Place::default(),
            artist,
        ))
    }

    /// The selected songs, in the table's order.
    pub fn selected_songs(&self) -> Vec<usize> {
        let sel = self.songs_selection.selection();
        let mut out = Vec::new();
        if let Some((mut it, first)) = gtk::BitsetIter::init_first(&sel) {
            let mut push = |pos: u32| {
                if let Some(o) = self.songs_sorted.item(pos) {
                    out.push(unboxed(&o));
                }
            };
            push(first);
            for pos in &mut it {
                push(pos);
            }
        }
        out
    }

    fn act_on_selection(self: &Rc<Self>, what: &str) {
        let tracks = self.selected_songs();
        if tracks.is_empty() {
            return;
        }
        self.songs_select.set_active(false);
        match what {
            "play" => self.play_tracks(&tracks, 0, Some(false)),
            "queue" => self.queue_tracks(&tracks, false),
            _ => self.add_to_playlist_dialog(tracks),
        }
    }

    /// Rebuilds all library views from the current library.
    pub fn refresh_library_views(self: &Rc<Self>) {
        let lib = self.lib();
        self.sort_albums();
        let songs: Vec<glib::BoxedAnyObject> =
            lib.all_tracks_by_album().into_iter().map(boxed).collect();
        self.songs_model
            .splice(0, self.songs_model.n_items(), &songs);
        self.sort_artists();
        self.fill_genres(&self.genres_list);
        self.refresh_playlists_list();
        self.update_main_stack();
        if self.search_bar.is_search_mode() {
            self.show_search(&self.search_entry.text());
        }
        // Pages showing stale indices are closed.
        if self.nav.visible_page().and_then(|p| p.tag()).as_deref() != Some("now-playing") {
            self.nav.pop_to_tag("root");
        }
        self.update_now_playing();
    }

    /// Fills the Albums grid in the chosen order.
    pub fn sort_albums(&self) {
        let lib = self.lib();
        let sort = model::AlbumSort::from_nick(&self.state.borrow().album_sort).unwrap_or_default();
        let order = {
            let stats = self.stats.borrow();
            lib.album_order(sort, |p| stats.plays(p))
        };
        let albums: Vec<glib::BoxedAnyObject> = order.into_iter().map(boxed).collect();
        self.albums_model
            .splice(0, self.albums_model.n_items(), &albums);
    }

    /// Fills the Artists list in the chosen order.
    pub fn sort_artists(&self) {
        let lib = self.lib();
        let sort =
            model::ArtistSort::from_nick(&self.state.borrow().artist_sort).unwrap_or_default();
        let order = {
            let stats = self.stats.borrow();
            lib.artist_order(sort, |p| stats.plays(p))
        };
        self.artists_list.remove_all();
        for &ar in &order {
            let a = &lib.artists[ar];
            let n = a.albums.len();
            let row = adw::ActionRow::builder()
                .title(glib::markup_escape_text(&a.name))
                .subtitle(trnf!("{n} album", "{n} albums", n))
                .activatable(true)
                .build();
            row.add_prefix(&artist_avatar(&a.name, 40));
            row.add_suffix(&gtk::Image::from_icon_name("go-next-symbolic"));
            self.artists_list.append(&row);
        }
        *self.artist_order.borrow_mut() = order;
    }

    pub fn refresh_playlists_list(&self) {
        self.playlists_list.remove_all();
        let lib = self.lib();
        let stats = self.stats.borrow();
        for s in Smart::ALL {
            let n = s.tracks(&lib, &stats).len();
            let row = adw::ActionRow::builder()
                .title(s.title())
                .subtitle(songs_count(n))
                .activatable(true)
                .build();
            row.add_prefix(&gtk::Image::from_icon_name(s.icon()));
            row.add_suffix(&gtk::Image::from_icon_name("go-next-symbolic"));
            if n == 0 {
                // Still listed (its page says how it fills up), but quiet.
                row.add_css_class("dim-label");
            }
            self.playlists_list.append(&row);
        }
        for p in self.playlists.borrow().iter() {
            let row = adw::ActionRow::builder()
                .title(glib::markup_escape_text(&p.name))
                .subtitle(songs_count(p.entries.len()))
                .activatable(true)
                .build();
            row.add_prefix(&gtk::Image::from_icon_name("view-list-symbolic"));
            row.add_suffix(&gtk::Image::from_icon_name("go-next-symbolic"));
            self.playlists_list.append(&row);
        }
    }

    // ----- track rows ------------------------------------------------------

    /// A boxed list of track rows; activating one plays `tracks` from there.
    /// Long lists get their first rows at once and the rest while idle.
    /// Playlist rows can be dragged to reorder the playlist.
    pub fn track_list(
        self: &Rc<Self>,
        tracks: &[usize],
        numbers: impl Fn(usize, usize) -> String + 'static,
        show_artist: bool,
        ctx: RowCtx,
        positions: Option<&[usize]>,
        play_from: Option<(Vec<usize>, usize)>,
    ) -> gtk::ListBox {
        const FIRST: usize = 100;
        const BATCH: usize = 200;
        let list = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .css_classes(["boxed-list"])
            .build();
        let rows: Rc<Vec<(usize, Place)>> = Rc::new(
            tracks
                .iter()
                .enumerate()
                .map(|(n, &t)| (t, Place::in_list(n, tracks.len(), positions)))
                .collect(),
        );
        let add = {
            let (w, l, rows, ctx) = (Rc::downgrade(self), list.downgrade(), rows.clone(), ctx);
            move |range: std::ops::Range<usize>| {
                let (Some(app), Some(list)) = (w.upgrade(), l.upgrade()) else {
                    return;
                };
                for n in range {
                    let (t, place) = rows[n];
                    let row = app.track_row(t, &numbers(n, t), show_artist, &ctx, place);
                    if let RowCtx::Playlist(file) = &ctx {
                        let (w, f) = (Rc::downgrade(&app), file.clone());
                        crate::rows::reorderable(
                            &row,
                            move || Some(place.from),
                            move |from, to| {
                                if let Some(app) = w.upgrade() {
                                    app.move_in_playlist(&f, from, to);
                                }
                            },
                        );
                    }
                    list.append(&row);
                }
            }
        };
        add(0..rows.len().min(FIRST));
        if rows.len() > FIRST {
            let next = std::cell::Cell::new(FIRST);
            let total = rows.len();
            let l = list.downgrade();
            glib::idle_add_local(move || {
                if l.upgrade().is_none() {
                    return glib::ControlFlow::Break;
                }
                let start = next.get();
                let end = (start + BATCH).min(total);
                add(start..end);
                next.set(end);
                if end < total {
                    glib::ControlFlow::Continue
                } else {
                    glib::ControlFlow::Break
                }
            });
        }
        let (tracks, offset) = play_from.unwrap_or_else(|| (tracks.to_vec(), 0));
        let w = Rc::downgrade(self);
        list.connect_row_activated(move |_, row| {
            if let Some(app) = w.upgrade() {
                app.play_tracks(&tracks, offset + row.index() as usize, None);
            }
        });
        list
    }

    /// The header of an album, artist, genre or playlist page: art, title,
    /// subtitle (a link to the artist when `artist` is given), details,
    /// Play and Shuffle, and a menu to queue or add all of `tracks`.
    fn header_block(
        self: &Rc<Self>,
        art: Art,
        title: &str,
        subtitle: &str,
        artist: Option<usize>,
        detail: &str,
        tracks: Vec<usize>,
    ) -> gtk::Widget {
        let art: gtk::Widget = match art {
            Art::Cover(key) => {
                let c = cover_widget(160);
                self.set_cover(&c, key);
                c.upcast()
            }
            Art::Avatar(name) => adw::Avatar::builder()
                .text(name)
                .show_initials(true)
                .size(160)
                .halign(gtk::Align::Center)
                .valign(gtk::Align::Center)
                .accessible_role(gtk::AccessibleRole::Presentation)
                .build()
                .upcast(),
            Art::Icon(icon) => {
                let c = cover_widget(160);
                self.set_cover(&c, None);
                if let Some(img) = c.child().and_downcast::<gtk::Image>() {
                    img.set_icon_name(Some(icon));
                }
                c.upcast()
            }
        };
        let info = gtk::Box::new(gtk::Orientation::Vertical, 6);
        info.set_valign(gtk::Align::Center);
        let t = label(title, &["title-1"]);
        t.set_wrap(true);
        t.set_ellipsize(gtk::pango::EllipsizeMode::None);
        info.append(&t);
        match artist {
            Some(ar) if !subtitle.is_empty() => {
                info.append(&self.artist_link(subtitle, ar, &["title-4"]));
            }
            _ if !subtitle.is_empty() => info.append(&label(subtitle, &["title-4"])),
            _ => {}
        }
        info.append(&label(detail, &["dim-label"]));
        // Wraps onto a second line when the window is narrow.
        let buttons = gtk::FlowBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .column_spacing(6)
            .row_spacing(6)
            .margin_top(6)
            .halign(gtk::Align::Start)
            .max_children_per_line(8)
            .css_classes(["hm-header-buttons"])
            .build();
        let play = pill(tr("Play"), "media-playback-start-symbolic", true);
        let shuffle = pill(tr("Shuffle"), "media-playlist-shuffle-symbolic", false);
        play.set_sensitive(!tracks.is_empty());
        shuffle.set_sensitive(!tracks.is_empty());
        let (w, t1) = (Rc::downgrade(self), tracks.clone());
        play.connect_clicked(move |_| {
            if let Some(app) = w.upgrade() {
                app.play_tracks(&t1, 0, Some(false));
            }
        });
        let (w, t2) = (Rc::downgrade(self), tracks.clone());
        shuffle.connect_clicked(move |_| {
            if let Some(app) = w.upgrade() {
                app.play_tracks(&t2, 0, Some(true));
            }
        });
        add_header_button(&buttons, &play);
        add_header_button(&buttons, &shuffle);
        if !tracks.is_empty() {
            let more = gtk::MenuButton::builder()
                .icon_name("view-more-symbolic")
                .menu_model(&tracks_menu(&tracks))
                .valign(gtk::Align::Center)
                .css_classes(["circular"])
                .tooltip_text(tr("More"))
                .build()
                .labelled(tr("More"));
            add_header_button(&buttons, &more);
        }
        info.append(&buttons);
        let header = gtk::Box::new(gtk::Orientation::Horizontal, 18);
        header.append(&art);
        header.append(&info);
        // Narrow windows put the cover above the text (see
        // `App::layout_headers`).
        self.headers.borrow_mut().push(header.downgrade());
        layout_header(&header, self.is_narrow());
        header.upcast()
    }

    /// An artist's name as a link to their page.
    pub fn artist_link(
        self: &Rc<Self>,
        name: &str,
        artist: usize,
        classes: &[&str],
    ) -> gtk::Button {
        let l = label(name, classes);
        let b = gtk::Button::builder()
            .child(&l)
            .halign(gtk::Align::Start)
            .tooltip_text(tr("Go to Artist"))
            .css_classes(["flat", "hm-link"])
            .build();
        let w = Rc::downgrade(self);
        b.connect_clicked(move |_| {
            if let Some(app) = w.upgrade() {
                app.show_artist(artist);
            }
        });
        b
    }

    // ----- pages -------------------------------------------------------------

    pub fn show_album(self: &Rc<Self>, ai: usize) {
        let lib = self.lib();
        let Some(album) = lib.albums.get(ai) else {
            return;
        };
        let mut detail = Vec::new();
        if let Some(y) = album.year {
            detail.push(y.to_string());
        }
        detail.push(songs_count(album.tracks.len()));
        detail.push(format_total(album.duration_ms));
        let content = gtk::Box::new(gtk::Orientation::Vertical, 12);
        let artist = (!album.various)
            .then(|| lib.artist_index(&album.artist))
            .flatten();
        let header = self.header_block(
            Art::Cover(album.cover.as_deref()),
            &album.title,
            &album.artist,
            artist,
            &detail.join(" · "),
            album.tracks.clone(),
        );
        content.append(&header);

        let discs: Vec<u32> = {
            let mut d: Vec<u32> = album
                .tracks
                .iter()
                .map(|&t| lib.tracks[t].disc.unwrap_or(1))
                .collect();
            d.dedup();
            d
        };
        let mut offset = 0;
        for disc in &discs {
            let part: Vec<usize> = album
                .tracks
                .iter()
                .copied()
                .filter(|&t| lib.tracks[t].disc.unwrap_or(1) == *disc)
                .collect();
            if discs.len() > 1 {
                content.append(&label(
                    &trf!("Disc {disc}", disc = disc),
                    &["heading", "hm-disc"],
                ));
            }
            let lib2 = lib.clone();
            let list = self.track_list(
                &part,
                move |n, t| {
                    lib2.tracks[t]
                        .track
                        .map_or_else(|| (n + 1).to_string(), |x| x.to_string())
                },
                album.various
                    || album
                        .tracks
                        .iter()
                        .any(|&t| lib.tracks[t].artist_name() != album.artist),
                RowCtx::Plain,
                None,
                // Activation plays the whole album from this disc's offset.
                Some((album.tracks.clone(), offset)),
            );
            offset += part.len();
            content.append(&list);
        }
        self.nav
            .push(&page(&album.title, None, &scrolled(&content, 800)));
    }

    pub fn show_artist(self: &Rc<Self>, ar: usize) {
        let lib = self.lib();
        let Some(artist) = lib.artists.get(ar) else {
            return;
        };
        let content = gtk::Box::new(gtk::Orientation::Vertical, 12);
        let all: Vec<usize> = artist
            .albums
            .iter()
            .flat_map(|&a| lib.artist_tracks_in_album(&artist.name, a))
            .collect();
        let total: u64 = all.iter().map(|&t| lib.tracks[t].duration_ms).sum();
        // Initials rather than one album's cover, which is not a picture of
        // the artist.
        let header = self.header_block(
            Art::Avatar(&artist.name),
            &artist.name,
            "",
            None,
            &format!(
                "{} · {} · {}",
                trnf!("{n} album", "{n} albums", artist.albums.len()),
                songs_count(all.len()),
                format_total(total)
            ),
            all,
        );
        content.append(&header);
        content.append(&label(tr("Albums"), &["heading", "hm-disc"]));
        content.append(&self.album_tiles(&artist.albums));
        for &ai in &artist.albums {
            let album = &lib.albums[ai];
            let tracks = lib.artist_tracks_in_album(&artist.name, ai);
            let text = gtk::Box::new(gtk::Orientation::Horizontal, 12);
            text.append(&label(&album.title, &["heading"]));
            if let Some(y) = album.year {
                text.append(&label(&y.to_string(), &["dim-label"]));
            }
            let btn = gtk::Button::builder()
                .child(&text)
                .css_classes(["flat", "hm-link"])
                .halign(gtk::Align::Start)
                .tooltip_text(tr("Go to Album"))
                .margin_top(12)
                .build();
            let w = Rc::downgrade(self);
            btn.connect_clicked(move |_| {
                if let Some(app) = w.upgrade() {
                    app.show_album(ai);
                }
            });
            content.append(&btn);
            let lib2 = lib.clone();
            content.append(&self.track_list(
                &tracks,
                move |n, t| {
                    lib2.tracks[t]
                        .track
                        .map_or_else(|| (n + 1).to_string(), |x| x.to_string())
                },
                album.various,
                RowCtx::Plain,
                None,
                None,
            ));
        }
        self.nav
            .push(&page(&artist.name, None, &scrolled(&content, 800)));
    }

    /// The genres as a page of their own (narrow windows, and Alt+5 there).
    pub fn show_genres_page(self: &Rc<Self>) {
        let list = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .css_classes(["boxed-list"])
            .valign(gtk::Align::Start)
            .build();
        self.fill_genres(&list);
        let w = Rc::downgrade(self);
        list.connect_row_activated(move |_, row| {
            if let Some(app) = w.upgrade() {
                app.show_genre(row.index() as usize);
            }
        });
        self.nav
            .push(&page(tr("Genres"), None, &scrolled(&list, 700)));
    }

    /// One row per genre: name, album and song counts.
    fn fill_genres(&self, list: &gtk::ListBox) {
        list.remove_all();
        for g in &self.lib().genres {
            let row = adw::ActionRow::builder()
                .title(glib::markup_escape_text(&g.name))
                .subtitle(format!(
                    "{} · {}",
                    trnf!("{n} album", "{n} albums", g.albums.len()),
                    songs_count(g.tracks.len())
                ))
                .activatable(true)
                .build();
            row.add_suffix(&gtk::Image::from_icon_name("go-next-symbolic"));
            list.append(&row);
        }
    }

    pub fn show_genre(self: &Rc<Self>, gi: usize) {
        let lib = self.lib();
        let Some(genre) = lib.genres.get(gi) else {
            return;
        };
        let total: u64 = genre
            .tracks
            .iter()
            .map(|&t| lib.tracks[t].duration_ms)
            .sum();
        let content = gtk::Box::new(gtk::Orientation::Vertical, 12);
        content.append(&self.header_block(
            Art::Icon("emoji-symbols-symbolic"),
            &genre.name,
            "",
            None,
            &format!(
                "{} · {} · {}",
                trnf!("{n} album", "{n} albums", genre.albums.len()),
                songs_count(genre.tracks.len()),
                format_total(total)
            ),
            genre.tracks.clone(),
        ));
        content.append(&label(tr("Albums"), &["heading", "hm-disc"]));
        content.append(&self.album_tiles(&genre.albums));
        content.append(&label(tr("Songs"), &["heading", "hm-disc"]));
        content.append(&self.track_list(
            &genre.tracks,
            |_, _| String::new(),
            true,
            RowCtx::Plain,
            None,
            None,
        ));
        self.nav
            .push(&page(&genre.name, None, &scrolled(&content, 800)));
    }

    /// A wrapping grid of album tiles (artist and genre pages); a tile
    /// opens its album, right-click or long-press shows its menu.
    pub fn album_tiles(self: &Rc<Self>, albums: &[usize]) -> gtk::FlowBox {
        let flow = gtk::FlowBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .homogeneous(true)
            .column_spacing(6)
            .row_spacing(6)
            .max_children_per_line(8)
            .min_children_per_line(1)
            .css_classes(["hm-album-tiles"])
            .build();
        let menu = context_menu_host(&flow);
        for &ai in albums {
            let tile = album_tile();
            self.bind_album_tile(&tile, ai);
            flow.append(&tile);
            let (w, m) = (Rc::downgrade(self), menu.clone());
            on_context_click(&tile, move |tile, x, y| {
                if let Some(app) = w.upgrade() {
                    popup_at(&m, tile, x, y, &app.album_menu(ai));
                }
            });
        }
        let (w, albums) = (Rc::downgrade(self), albums.to_vec());
        flow.connect_child_activated(move |_, child| {
            if let (Some(app), Some(&ai)) = (w.upgrade(), albums.get(child.index() as usize)) {
                app.show_album(ai);
            }
        });
        flow
    }

    /// An album's context menu: play it, queue it, add it to a playlist,
    /// go to its artist.
    pub fn album_menu(&self, ai: usize) -> gio::Menu {
        let lib = self.lib();
        let menu = gio::Menu::new();
        let Some(album) = lib.albums.get(ai) else {
            return menu;
        };
        let target: Vec<u32> = album.tracks.iter().map(|&t| t as u32).collect();
        let target = target.to_variant();
        let play = gio::Menu::new();
        for (label, action) in [
            (tr("Play"), "win.play-tracks"),
            (tr("Shuffle"), "win.shuffle-tracks"),
        ] {
            let it = gio::MenuItem::new(Some(label), None);
            it.set_action_and_target_value(Some(action), Some(&target));
            play.append_item(&it);
        }
        menu.append_section(None, &play);
        menu.append_section(None, &tracks_menu(&album.tracks));
        if let Some(ar) = (!album.various)
            .then(|| lib.artist_index(&album.artist))
            .flatten()
        {
            let go = gio::Menu::new();
            let it = gio::MenuItem::new(Some(tr("Go to Artist")), None);
            it.set_action_and_target_value(
                Some("win.show-artist"),
                Some(&(ar as u32).to_variant()),
            );
            go.append_item(&it);
            menu.append_section(None, &go);
        }
        menu
    }

    pub fn show_playlist(self: &Rc<Self>, r: PlRef) {
        let title = match &r {
            PlRef::Smart(s) => s.title().to_string(),
            PlRef::User(f) => self
                .playlists
                .borrow()
                .iter()
                .find(|p| &p.file == f)
                .map(|p| p.name.clone())
                .unwrap_or_default(),
        };
        let content = gtk::Box::new(gtk::Orientation::Vertical, 12);
        *self.playlist_view.borrow_mut() = Some((content.downgrade(), r.clone()));
        self.fill_playlist(&content, &r);
        let (p, header) = page_with_header(&title, Some("playlist"), &scrolled(&content, 800));
        if let PlRef::User(file) = &r {
            // Secondary and destructive actions stay out of the Play row.
            let target = file.to_string_lossy().into_owned().to_variant();
            let menu = gio::Menu::new();
            let item = |label: &str, action: &str| {
                let it = gio::MenuItem::new(Some(label), None);
                it.set_action_and_target_value(Some(action), Some(&target));
                it
            };
            let edit = gio::Menu::new();
            edit.append_item(&item(tr("_Rename…"), "win.playlist-rename"));
            edit.append_item(&item(tr("_Export…"), "win.playlist-export"));
            menu.append_section(None, &edit);
            let danger = gio::Menu::new();
            danger.append_item(&item(tr("_Delete…"), "win.playlist-delete"));
            menu.append_section(None, &danger);
            header.pack_end(
                &gtk::MenuButton::builder()
                    .icon_name("view-more-symbolic")
                    .menu_model(&menu)
                    .tooltip_text(tr("Playlist Menu"))
                    .build()
                    .labelled(tr("Playlist Menu")),
            );
        }
        self.nav.push(&p);
    }

    fn fill_playlist(self: &Rc<Self>, content: &gtk::Box, r: &PlRef) {
        while let Some(c) = content.first_child() {
            content.remove(&c);
        }
        let lib = self.lib();
        let (name, tracks, positions, ctx) = match r {
            PlRef::Smart(s) => {
                let t = s.tracks(&lib, &self.stats.borrow());
                let n = t.len();
                (
                    s.title().to_string(),
                    t,
                    (0..n).collect::<Vec<_>>(),
                    RowCtx::Plain,
                )
            }
            PlRef::User(file) => {
                let pls = self.playlists.borrow();
                let Some(pl) = pls.iter().find(|p| &p.file == file) else {
                    return;
                };
                let mut tracks = Vec::new();
                let mut positions = Vec::new();
                for (i, e) in pl.entries.iter().enumerate() {
                    if let Some(t) = lib.index_of(e) {
                        tracks.push(t);
                        positions.push(i);
                    }
                }
                (
                    pl.name.clone(),
                    tracks,
                    positions,
                    RowCtx::Playlist(file.clone()),
                )
            }
        };
        let total: u64 = tracks.iter().map(|&t| lib.tracks[t].duration_ms).sum();
        let cover = tracks.iter().find_map(|&t| lib.tracks[t].cover.clone());
        let header = self.header_block(
            Art::Cover(cover.as_deref()),
            &name,
            "",
            None,
            &format!("{} · {}", songs_count(tracks.len()), format_total(total)),
            tracks.clone(),
        );
        content.append(&header);
        if tracks.is_empty() {
            let empty = adw::StatusPage::builder()
                .icon_name("view-list-symbolic")
                .title(tr("No Songs"))
                .build();
            empty.set_description(Some(match r {
                PlRef::User(_) => tr("Add songs with “Add to Playlist…” in a song’s menu."),
                PlRef::Smart(Smart::Favorites) => {
                    tr("Mark songs as favorites with the star button.")
                }
                _ => tr("Songs will appear here as you listen."),
            }));
            content.append(&empty);
        } else {
            let show_numbers = matches!(r, PlRef::User(_));
            content.append(&self.track_list(
                &tracks,
                move |n, _| {
                    if show_numbers {
                        (n + 1).to_string()
                    } else {
                        String::new()
                    }
                },
                true,
                ctx,
                Some(&positions),
                None,
            ));
        }
    }

    pub fn refresh_playlist_view(self: &Rc<Self>) {
        let cur = self.playlist_view.borrow().clone();
        if let Some((w, r)) = cur {
            if let Some(content) = w.upgrade() {
                self.fill_playlist(&content, &r);
            }
        }
        self.refresh_playlists_list();
    }

    pub fn show_search(self: &Rc<Self>, query: &str) {
        let b = &self.search_box;
        while let Some(c) = b.first_child() {
            b.remove(&c);
        }
        let lib = self.lib();
        let r = lib.search(query);
        self.update_main_stack();
        if query.trim().is_empty() {
            return;
        }
        if r.is_empty() {
            let s = adw::StatusPage::builder()
                .icon_name("system-search-symbolic")
                .title(tr("No Results"))
                .description(tr("Try a different search"))
                .build();
            b.append(&s);
            return;
        }
        // Long sections show their first matches; Show All adds the rest
        // (songs open a page that lists them all).
        const ALBUMS: usize = 50;
        const ARTISTS: usize = 50;
        const SONGS: usize = 200;
        if !r.albums.is_empty() {
            let list = gtk::ListBox::builder()
                .selection_mode(gtk::SelectionMode::None)
                .css_classes(["boxed-list"])
                .build();
            let add = {
                let (w, l, albums) = (Rc::downgrade(self), list.downgrade(), r.albums.clone());
                move |range: std::ops::Range<usize>| {
                    let (Some(app), Some(list)) = (w.upgrade(), l.upgrade()) else {
                        return;
                    };
                    let lib = app.lib();
                    for &ai in &albums[range] {
                        let a = &lib.albums[ai];
                        let row = adw::ActionRow::builder()
                            .title(glib::markup_escape_text(&a.title))
                            .subtitle(glib::markup_escape_text(&a.artist))
                            .activatable(true)
                            .build();
                        let img = cover_widget(40);
                        app.set_cover(&img, a.cover.as_deref());
                        row.add_prefix(&img);
                        list.append(&row);
                    }
                }
            };
            add(0..r.albums.len().min(ALBUMS));
            let n = r.albums.len();
            b.append(&section_header(
                tr("Albums"),
                n,
                (n > ALBUMS).then(|| Box::new(move || add(ALBUMS..n)) as Box<dyn Fn()>),
            ));
            let (w, albums) = (Rc::downgrade(self), r.albums.clone());
            list.connect_row_activated(move |_, row| {
                if let Some(app) = w.upgrade() {
                    app.show_album(albums[row.index() as usize]);
                }
            });
            b.append(&list);
        }
        if !r.artists.is_empty() {
            let list = gtk::ListBox::builder()
                .selection_mode(gtk::SelectionMode::None)
                .css_classes(["boxed-list"])
                .build();
            let add = {
                let (w, l, artists) = (Rc::downgrade(self), list.downgrade(), r.artists.clone());
                move |range: std::ops::Range<usize>| {
                    let (Some(app), Some(list)) = (w.upgrade(), l.upgrade()) else {
                        return;
                    };
                    let lib = app.lib();
                    for &ar in &artists[range] {
                        let row = adw::ActionRow::builder()
                            .title(glib::markup_escape_text(&lib.artists[ar].name))
                            .activatable(true)
                            .build();
                        row.add_prefix(&artist_avatar(&lib.artists[ar].name, 32));
                        list.append(&row);
                    }
                }
            };
            add(0..r.artists.len().min(ARTISTS));
            let n = r.artists.len();
            b.append(&section_header(
                tr("Artists"),
                n,
                (n > ARTISTS).then(|| Box::new(move || add(ARTISTS..n)) as Box<dyn Fn()>),
            ));
            let (w, artists) = (Rc::downgrade(self), r.artists.clone());
            list.connect_row_activated(move |_, row| {
                if let Some(app) = w.upgrade() {
                    app.show_artist(artists[row.index() as usize]);
                }
            });
            b.append(&list);
        }
        if !r.tracks.is_empty() {
            let n = r.tracks.len();
            let show_all = (n > SONGS).then(|| {
                let (w, all, q) = (
                    Rc::downgrade(self),
                    r.tracks.clone(),
                    query.trim().to_string(),
                );
                Box::new(move || {
                    if let Some(app) = w.upgrade() {
                        app.show_all_songs(&q, all.clone());
                    }
                }) as Box<dyn Fn()>
            });
            b.append(&section_header(tr("Songs"), n, show_all));
            let first: Vec<usize> = r.tracks.iter().copied().take(SONGS).collect();
            // Playing one plays from all the matches, not just those shown.
            b.append(&self.track_list(
                &first,
                |_, _| String::new(),
                true,
                RowCtx::Plain,
                None,
                Some((r.tracks.clone(), 0)),
            ));
        }
    }

    /// Every song matching a search, in a list that only builds the rows
    /// on screen.
    fn show_all_songs(self: &Rc<Self>, query: &str, tracks: Vec<usize>) {
        let model = gio::ListStore::new::<glib::BoxedAnyObject>();
        let objs: Vec<glib::BoxedAnyObject> = tracks.iter().copied().map(boxed).collect();
        model.extend_from_slice(&objs);
        let list = self.track_list_view(
            &model,
            true,
            RowCtx::Plain,
            |obj, _| Some((unboxed(obj), String::new(), Place::default())),
            None,
        );
        list.add_css_class("hm-all-songs");
        let w = Rc::downgrade(self);
        list.connect_activate(move |_, pos| {
            if let Some(app) = w.upgrade() {
                app.play_tracks(&tracks, pos as usize, None);
            }
        });
        let clamp = adw::Clamp::builder().maximum_size(900).child(&list).build();
        let sw = gtk::ScrolledWindow::builder()
            .child(&clamp)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .build();
        // Translators: the title of a page with every song matching a search.
        let title = trf!("Songs Matching “{query}”", query = query);
        self.nav.push(&page(&title, None, &sw));
    }

    // ----- playlist editing ----------------------------------------------------

    fn entry_info(&self) -> impl Fn(&Path) -> Option<EntryInfo> {
        let lib = self.lib();
        move |p: &Path| {
            lib.track_by_path(p).map(|t| EntryInfo {
                seconds: t.duration_ms / 1000,
                artist: t.artist_name().into(),
                title: t.title.clone(),
            })
        }
    }

    fn save_playlist(&self, pl: &Playlist) {
        if let Err(e) = playlist::save(pl, self.entry_info()) {
            self.toasts
                .add_toast(toast(&trf!("Cannot save playlist: {e}", e = e)));
        }
    }

    fn reload_playlists(self: &Rc<Self>) {
        *self.playlists.borrow_mut() = playlist::load_all(&App::data_dir().join("playlists"));
        self.refresh_playlist_view();
    }

    fn playlist_filters(save: bool) -> gio::ListStore {
        let filters = gio::ListStore::new::<gtk::FileFilter>();
        let all = gtk::FileFilter::new();
        all.set_name(Some(tr("Playlists")));
        let kinds: &[(&str, &[&str], &str)] = &[
            (tr("M3U Playlist"), &["m3u", "m3u8"], "audio/x-mpegurl"),
            (tr("XSPF Playlist"), &["xspf"], "application/xspf+xml"),
            (tr("PLS Playlist"), &["pls"], "audio/x-scpls"),
        ];
        let mut singles = Vec::new();
        for (name, exts, mime) in kinds {
            if save && *mime == "audio/x-scpls" {
                continue;
            }
            let f = gtk::FileFilter::new();
            f.set_name(Some(name));
            for e in *exts {
                f.add_suffix(e);
                all.add_suffix(e);
            }
            f.add_mime_type(mime);
            all.add_mime_type(mime);
            singles.push(f);
        }
        if !save {
            filters.append(&all);
        }
        for f in singles {
            filters.append(&f);
        }
        filters
    }

    /// Imports an M3U/XSPF/PLS file as a new playlist (entries outside the
    /// library are dropped).
    pub fn import_playlist_dialog(self: &Rc<Self>) {
        let dialog = gtk::FileDialog::builder()
            .title(tr("Import Playlist"))
            .modal(true)
            .filters(&Self::playlist_filters(false))
            .build();
        let w = Rc::downgrade(self);
        dialog.open(Some(&self.window), gio::Cancellable::NONE, move |res| {
            let (Some(app), Ok(file)) = (w.upgrade(), res) else {
                return;
            };
            let Some(path) = file.path() else { return };
            let read = playlist::read_any(&path);
            let lib = app.lib();
            let imported = read.as_ref().ok().and_then(|(name, entries)| {
                playlist::import(
                    name.clone(),
                    &path,
                    entries.clone(),
                    &App::data_dir().join("playlists"),
                    |p| lib.index_of(p).is_some(),
                )
            });
            match (read, imported) {
                (Err(e), _) => app
                    .toasts
                    .add_toast(toast(&trf!("Cannot read playlist: {e}", e = e))),
                (Ok(_), None) => app
                    .toasts
                    .add_toast(toast(tr("No songs from this playlist are in your library"))),
                (Ok(_), Some(pl)) => {
                    app.save_playlist(&pl);
                    app.toasts.add_toast(toast(&trf!("Imported “{}”", pl.name)));
                    app.reload_playlists();
                }
            }
        });
    }

    /// Imports the playlist files LocalSearch has indexed (read-only query).
    /// Playlists already imported (same name and songs) are skipped.
    pub fn import_index_playlists(self: &Rc<Self>) {
        let (tx, rx) =
            async_channel::bounded::<Result<Vec<crate::tracker::IndexedPlaylist>, String>>(1);
        std::thread::spawn(move || {
            let res = match crate::tracker::Endpoint::connect() {
                Some(ep) => crate::tracker::playlists(&ep),
                None => Err(tr("the LocalSearch index is not available").into()),
            };
            let _ = tx.send_blocking(res);
        });
        let w = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let Ok(res) = rx.recv().await else { return };
            let Some(app) = w.upgrade() else { return };
            let found = match res {
                Ok(f) => f,
                Err(e) => {
                    app.toasts
                        .add_toast(toast(&trf!("Cannot import: {e}", e = e)));
                    return;
                }
            };
            let lib = app.lib();
            let dir = App::data_dir().join("playlists");
            let mut count = 0;
            for ip in found {
                let Some(pl) = playlist::import(ip.title, &ip.file, ip.entries, &dir, |p| {
                    lib.index_of(p).is_some()
                }) else {
                    continue;
                };
                let dup = app.playlists.borrow().iter().any(|p| {
                    crate::model::sort_key(&p.name) == crate::model::sort_key(&pl.name)
                        && p.entries == pl.entries
                });
                if dup {
                    continue;
                }
                app.save_playlist(&pl);
                app.playlists.borrow_mut().push(pl);
                count += 1;
            }
            app.toasts.add_toast(toast(&match count {
                0 => tr("No new playlists with songs from your library were found").to_string(),
                n => trnf!("Imported {n} playlist", "Imported {n} playlists", n),
            }));
            if count > 0 {
                app.reload_playlists();
            }
        });
    }

    /// Imports GNOME Music's favourites and user playlists from its private
    /// Tracker database once (read-only, on a worker thread). Without that
    /// database Helix just keeps using its own store.
    pub fn import_gnome_music_once(self: &Rc<Self>) {
        if self.state.borrow().gnome_music_imported
            || std::env::var_os("HELIX_MUSIC_NO_GNOME_IMPORT").is_some()
        {
            return;
        }
        let (tx, rx) =
            async_channel::bounded::<Result<Option<crate::gnome_import::Found>, String>>(1);
        let spawned = std::thread::Builder::new()
            .name("gnome-music-import".into())
            .spawn(move || {
                let _ = tx.send_blocking(crate::gnome_import::load());
            });
        if spawned.is_err() {
            return;
        }
        let w = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let Ok(res) = rx.recv().await else { return };
            let Some(app) = w.upgrade() else { return };
            let found = match res {
                Ok(Some(found)) => found,
                Ok(None) => return, // no GNOME Music database: nothing to import
                Err(e) => {
                    eprintln!("helix-music: cannot read GNOME Music's database: {e}");
                    return;
                }
            };
            let lib = app.lib();
            let mut favs = 0;
            {
                let mut stats = app.stats.borrow_mut();
                for p in found.favorites.iter().filter(|p| lib.index_of(p).is_some()) {
                    if !stats.is_favorite(p) {
                        stats.set_favorite(p, true);
                        favs += 1;
                    }
                }
            }
            if favs > 0 {
                app.save_stats();
            }
            let dir = App::data_dir().join("playlists");
            let mut lists = 0;
            for (title, entries) in found.playlists {
                let Some(pl) = playlist::import(Some(title), Path::new(""), entries, &dir, |p| {
                    lib.index_of(p).is_some()
                }) else {
                    continue;
                };
                let dup = app.playlists.borrow().iter().any(|p| {
                    crate::model::sort_key(&p.name) == crate::model::sort_key(&pl.name)
                        && p.entries == pl.entries
                });
                if dup {
                    continue;
                }
                app.save_playlist(&pl);
                app.playlists.borrow_mut().push(pl);
                lists += 1;
            }
            app.state.borrow_mut().gnome_music_imported = true;
            app.save_state();
            if lists > 0 {
                app.reload_playlists();
            }
            if favs > 0 {
                app.refresh_rows();
            }
            if favs + lists > 0 {
                app.toasts
                    .add_toast(toast(&crate::gnome_import::summary(favs, lists)));
            }
        });
    }

    /// Saves a copy of a playlist as M3U or XSPF (by the chosen extension).
    pub fn export_playlist_dialog(self: &Rc<Self>, file: &Path) {
        let Some(pl) = self
            .playlists
            .borrow()
            .iter()
            .find(|p| p.file == file)
            .cloned()
        else {
            return;
        };
        let dialog = gtk::FileDialog::builder()
            .title(tr("Export Playlist"))
            .modal(true)
            .initial_name(format!("{}.m3u", pl.name.replace('/', "_")))
            .filters(&Self::playlist_filters(true))
            .build();
        let w = Rc::downgrade(self);
        dialog.save(Some(&self.window), gio::Cancellable::NONE, move |res| {
            let (Some(app), Ok(dest)) = (w.upgrade(), res) else {
                return;
            };
            let Some(mut path) = dest.path() else { return };
            if playlist::Format::of(&path).is_none() {
                path.set_extension("m3u");
            }
            match playlist::export(&pl, &path, app.entry_info()) {
                Ok(()) => app.toasts.add_toast(toast(tr("Playlist exported"))),
                Err(e) => app
                    .toasts
                    .add_toast(toast(&trf!("Cannot export playlist: {e}", e = e))),
            }
        });
    }

    /// Creates a playlist (`rename == None`) or renames one.
    pub fn new_playlist_dialog(self: &Rc<Self>, rename: Option<PathBuf>) {
        self.playlist_name_dialog(rename, Vec::new());
    }

    fn playlist_name_dialog(self: &Rc<Self>, rename: Option<PathBuf>, add: Vec<usize>) {
        let current = rename.as_ref().and_then(|f| {
            self.playlists
                .borrow()
                .iter()
                .find(|p| &p.file == f)
                .map(|p| p.name.clone())
        });
        let dialog = adw::AlertDialog::new(
            Some(if rename.is_some() {
                tr("Rename Playlist")
            } else {
                tr("New Playlist")
            }),
            None,
        );
        let entry = gtk::Entry::builder()
            .placeholder_text(tr("Playlist Name"))
            .activates_default(true)
            .text(current.unwrap_or_default())
            .build()
            .labelled(tr("Playlist Name"));
        dialog.set_extra_child(Some(&entry));
        dialog.add_response("cancel", tr("_Cancel"));
        let ok = if rename.is_some() {
            tr("_Rename")
        } else {
            tr("_Create")
        };
        dialog.add_response("ok", ok);
        dialog.set_response_appearance("ok", adw::ResponseAppearance::Suggested);
        dialog.set_default_response(Some("ok"));
        dialog.set_close_response("cancel");
        dialog.set_response_enabled("ok", !entry.text().trim().is_empty());
        let d = dialog.clone();
        entry.connect_changed(move |e| d.set_response_enabled("ok", !e.text().trim().is_empty()));
        let w = Rc::downgrade(self);
        let entry2 = entry.clone();
        dialog.connect_response(Some("ok"), move |_, _| {
            let entry = &entry2;
            let Some(app) = w.upgrade() else { return };
            let name = entry.text().trim().to_string();
            if name.is_empty() {
                return;
            }
            let lib = app.lib();
            match &rename {
                Some(file) => {
                    let found = app
                        .playlists
                        .borrow()
                        .iter()
                        .find(|p| &p.file == file)
                        .cloned();
                    if let Some(mut pl) = found {
                        pl.name = name.clone();
                        app.save_playlist(&pl);
                    }
                    app.reload_playlists();
                    if let Some(p) = app
                        .nav
                        .visible_page()
                        .filter(|p| p.tag().as_deref() == Some("playlist"))
                    {
                        p.set_title(&name);
                    }
                }
                None => {
                    let dir = App::data_dir().join("playlists");
                    let _ = std::fs::create_dir_all(&dir);
                    let pl = Playlist {
                        file: playlist::new_file_for(&dir, &name),
                        name,
                        entries: add.iter().map(|&i| lib.tracks[i].path.clone()).collect(),
                    };
                    app.save_playlist(&pl);
                    app.reload_playlists();
                    if !add.is_empty() {
                        app.toasts.add_toast(toast(&trf!("Added to “{}”", pl.name)));
                    }
                }
            }
        });
        dialog.present(Some(&self.window));
        entry.grab_focus();
    }

    /// Asks for a name and saves `tracks` as a new playlist.
    pub fn save_as_playlist_dialog(self: &Rc<Self>, tracks: Vec<usize>) {
        self.playlist_name_dialog(None, tracks);
    }

    pub fn add_to_playlist_dialog(self: &Rc<Self>, tracks: Vec<usize>) {
        let dialog = adw::AlertDialog::new(Some(tr("Add to Playlist")), None);
        let list = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .css_classes(["boxed-list"])
            .build();
        let new_row = adw::ActionRow::builder()
            .title(tr("New Playlist…"))
            .activatable(true)
            .build();
        new_row.add_prefix(&gtk::Image::from_icon_name("list-add-symbolic"));
        list.append(&new_row);
        let files: Vec<PathBuf> = self
            .playlists
            .borrow()
            .iter()
            .map(|p| p.file.clone())
            .collect();
        for p in self.playlists.borrow().iter() {
            let row = adw::ActionRow::builder()
                .title(glib::markup_escape_text(&p.name))
                .subtitle(songs_count(p.entries.len()))
                .activatable(true)
                .build();
            row.add_prefix(&gtk::Image::from_icon_name("view-list-symbolic"));
            list.append(&row);
        }
        let sw = gtk::ScrolledWindow::builder()
            .child(&list)
            .propagate_natural_height(true)
            .max_content_height(360)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .build();
        dialog.set_extra_child(Some(&sw));
        dialog.add_response("cancel", tr("_Cancel"));
        dialog.set_close_response("cancel");
        let w = Rc::downgrade(self);
        let d = dialog.clone();
        list.connect_row_activated(move |_, row| {
            let Some(app) = w.upgrade() else { return };
            d.close();
            let i = row.index() as usize;
            if i == 0 {
                app.playlist_name_dialog(None, tracks.clone());
                return;
            }
            let Some(file) = files.get(i - 1) else { return };
            let found = app
                .playlists
                .borrow()
                .iter()
                .find(|p| &p.file == file)
                .cloned();
            if let Some(mut pl) = found {
                let lib = app.lib();
                pl.entries
                    .extend(tracks.iter().map(|&t| lib.tracks[t].path.clone()));
                app.save_playlist(&pl);
                app.toasts.add_toast(toast(&trf!("Added to “{}”", pl.name)));
                app.reload_playlists();
            }
        });
        dialog.present(Some(&self.window));
    }

    /// Moves entry `from` of a user playlist to position `to`.
    pub fn move_in_playlist(self: &Rc<Self>, file: &Path, from: usize, to: usize) {
        let found = self
            .playlists
            .borrow()
            .iter()
            .find(|p| p.file == file)
            .cloned();
        if let Some(mut pl) = found {
            if playlist::move_entry(&mut pl.entries, from, to) {
                self.save_playlist(&pl);
                self.reload_playlists();
            }
        }
    }

    /// Moves the song at play-order position `from` of the queue to `to`.
    pub fn move_in_queue(self: &Rc<Self>, from: usize, to: usize) {
        self.queue.borrow_mut().move_item(from, to);
        self.update_next_uri();
        self.update_now_playing();
    }

    pub fn remove_from_playlist(self: &Rc<Self>, file: &Path, pos: usize) {
        let found = self
            .playlists
            .borrow()
            .iter()
            .find(|p| p.file == file)
            .cloned();
        if let Some(mut pl) = found {
            if pos < pl.entries.len() {
                pl.entries.remove(pos);
                self.save_playlist(&pl);
                self.reload_playlists();
            }
        }
    }

    fn delete_playlist_dialog(self: &Rc<Self>, file: &Path) {
        let name = self
            .playlists
            .borrow()
            .iter()
            .find(|p| p.file == file)
            .map(|p| p.name.clone())
            .unwrap_or_default();
        let dialog = adw::AlertDialog::new(
            Some(tr("Delete Playlist?")),
            Some(&trf!(
                "“{name}” will be permanently deleted. The songs stay in your library.",
                name = name
            )),
        );
        dialog.add_response("cancel", tr("_Cancel"));
        dialog.add_response("delete", tr("_Delete"));
        dialog.set_response_appearance("delete", adw::ResponseAppearance::Destructive);
        dialog.set_close_response("cancel");
        let (w, f) = (Rc::downgrade(self), file.to_path_buf());
        dialog.connect_response(Some("delete"), move |_, _| {
            let Some(app) = w.upgrade() else { return };
            if let Err(e) = std::fs::remove_file(&f) {
                app.toasts
                    .add_toast(toast(&trf!("Cannot delete playlist: {e}", e = e)));
                return;
            }
            *app.playlist_view.borrow_mut() = None;
            app.nav.pop();
            app.reload_playlists();
        });
        dialog.present(Some(&self.window));
    }

    // ----- preferences ---------------------------------------------------------

    pub fn show_preferences(self: &Rc<Self>) {
        let dialog = adw::PreferencesDialog::new();
        let page = adw::PreferencesPage::new();
        let description = if self.using_tracker.get() {
            tr("Music in these folders appears in your library. Folders indexed by LocalSearch are read from its index.")
        } else {
            tr("Music in these folders appears in your library")
        };
        let folders = adw::PreferencesGroup::builder()
            .title(tr("Library Folders"))
            .description(description)
            .build();
        let add = gtk::Button::builder()
            .icon_name("list-add-symbolic")
            .tooltip_text(tr("Add Folder…"))
            .css_classes(["flat"])
            .valign(gtk::Align::Center)
            .build()
            .labelled(tr("Add Folder…"));
        folders.set_header_suffix(Some(&add));
        let main = adw::ActionRow::builder()
            .title(glib::markup_escape_text(&music_dir().display().to_string()))
            .subtitle(tr("Music folder"))
            .build();
        main.add_prefix(&gtk::Image::from_icon_name("folder-music-symbolic"));
        folders.add(&main);
        for f in self.state.borrow().folders.clone() {
            let row = adw::ActionRow::builder()
                .title(glib::markup_escape_text(&f.display().to_string()))
                .build();
            row.add_prefix(&gtk::Image::from_icon_name("folder-symbolic"));
            let rm = gtk::Button::builder()
                .icon_name("user-trash-symbolic")
                .tooltip_text(tr("Remove Folder"))
                .css_classes(["flat"])
                .valign(gtk::Align::Center)
                .build()
                .labelled(tr("Remove Folder"));
            let (w, g, r) = (Rc::downgrade(self), folders.downgrade(), row.downgrade());
            rm.connect_clicked(move |_| {
                if let (Some(app), Some(g), Some(r)) = (w.upgrade(), g.upgrade(), r.upgrade()) {
                    g.remove(&r);
                    app.remove_folder(&f);
                }
            });
            row.add_suffix(&rm);
            folders.add(&row);
        }
        let (w, d) = (Rc::downgrade(self), dialog.downgrade());
        add.connect_clicked(move |_| {
            if let (Some(app), Some(d)) = (w.upgrade(), d.upgrade()) {
                d.close();
                app.add_folder_dialog();
            }
        });
        page.add(&folders);

        let playback = adw::PreferencesGroup::builder()
            .title(tr("Playback"))
            .build();
        let inhibit = adw::SwitchRow::builder()
            .title(tr("Inhibit Suspend"))
            .subtitle(tr("Keep the computer awake while music plays"))
            .active(self.prefs.get("inhibit-suspend", false))
            .build();
        let w = Rc::downgrade(self);
        inhibit.connect_active_notify(move |r| {
            if let Some(app) = w.upgrade() {
                app.prefs.set("inhibit-suspend", r.is_active());
                app.update_inhibit();
            }
        });
        playback.add(&inhibit);

        let modes = crate::player::ReplayGain::ALL;
        let labels: Vec<&str> = modes.iter().map(|m| m.label()).collect();
        let rg = adw::ComboRow::builder()
            .title(tr("ReplayGain"))
            .subtitle(tr(
                "Even out loudness using the volume tags of songs or albums",
            ))
            .model(&gtk::StringList::new(&labels))
            .selected(
                modes
                    .iter()
                    .position(|m| *m == self.replaygain())
                    .unwrap_or(0) as u32,
            )
            .build();
        let w = Rc::downgrade(self);
        rg.connect_selected_notify(move |r| {
            if let (Some(app), Some(m)) = (w.upgrade(), modes.get(r.selected() as usize)) {
                app.set_replaygain(*m);
            }
        });
        playback.add(&rg);

        let notify = adw::SwitchRow::builder()
            .title(tr("Notifications"))
            .subtitle(tr(
                "Announce new songs while the window is in the background",
            ))
            .active(self.state.borrow().notifications)
            .build();
        let w = Rc::downgrade(self);
        notify.connect_active_notify(move |r| {
            if let Some(app) = w.upgrade() {
                app.state.borrow_mut().notifications = r.is_active();
                app.save_state();
            }
        });
        playback.add(&notify);
        let background = adw::SwitchRow::builder()
            .title(tr("Background Playback"))
            .subtitle(tr("Keep playing when the window is closed"))
            .active(self.state.borrow().background_playback)
            .build();
        let w = Rc::downgrade(self);
        background.connect_active_notify(move |r| {
            if let Some(app) = w.upgrade() {
                app.state.borrow_mut().background_playback = r.is_active();
                app.save_state();
            }
        });
        playback.add(&background);
        page.add(&playback);
        dialog.add(&page);
        dialog.present(Some(&self.window));
    }
}

#[derive(PartialEq, Eq, PartialOrd, Ord)]
enum SortKey {
    Text(String),
    Num(u64),
}

fn songs_count(n: usize) -> String {
    trnf!("{n} song", "{n} songs", n)
}
