//! Views and pages: albums grid, artists, songs, playlists, search, album /
//! artist / playlist pages, Now Playing and preferences.

use adw::prelude::*;
use gtk::{gio, glib};
use helix_app_common::Labelled;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use crate::model::{self, format_duration, format_total};
use crate::playlist::{self, EntryInfo, Playlist, Smart};
use crate::ui::{music_dir, set_heart_icon, sync_heart, toast, App, RowHandle};
use helix_app_common::i18n::tr;
use helix_app_common::trf;
use helix_app_common::trnf;

/// Which playlist a page shows.
#[derive(Clone, Debug, PartialEq)]
pub enum PlRef {
    Smart(Smart),
    User(PathBuf),
}

/// Where a track row lives (decides its menu and activation).
#[derive(Clone)]
pub enum RowCtx {
    Plain,
    Playlist(PathBuf),
    Queue,
}

fn label(text: &str, classes: &[&str]) -> gtk::Label {
    gtk::Label::builder()
        .label(text)
        .xalign(0.0)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .css_classes(classes.to_vec())
        .build()
}

fn scrolled(child: &impl IsA<gtk::Widget>, max: i32) -> gtk::ScrolledWindow {
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
    if let Some(slot) = button.as_ref().parent() {
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
    if let Some(img) = header.first_child().and_downcast::<gtk::Image>() {
        img.set_pixel_size(if narrow { 128 } else { 160 });
        img.set_halign(if narrow {
            gtk::Align::Start
        } else {
            gtk::Align::Fill
        });
    }
}

fn page(title: &str, tag: Option<&str>, content: &impl IsA<gtk::Widget>) -> adw::NavigationPage {
    let tv = adw::ToolbarView::new();
    tv.add_top_bar(&adw::HeaderBar::new());
    tv.set_content(Some(content));
    let p = adw::NavigationPage::builder()
        .title(title)
        .child(&tv)
        .build();
    if let Some(t) = tag {
        p.set_tag(Some(t));
    }
    p
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

fn boxed(i: usize) -> glib::BoxedAnyObject {
    glib::BoxedAnyObject::new(i)
}

fn unboxed(o: &glib::Object) -> usize {
    o.downcast_ref::<glib::BoxedAnyObject>()
        .map(|b| *b.borrow::<usize>())
        .unwrap_or(0)
}

impl App {
    pub fn build_views(self: &Rc<Self>) {
        // Albums grid.
        let factory = gtk::SignalListItemFactory::new();
        factory.connect_setup(|_, item| {
            let item = item.downcast_ref::<gtk::ListItem>().unwrap();
            let img = gtk::Image::builder()
                .pixel_size(160)
                .css_classes(["hm-cover"])
                // Decorative: the album or track title next to it names it.
                .accessible_role(gtk::AccessibleRole::Presentation)
                .build();
            let title = label("", &["heading"]);
            let artist = label("", &["dim-label"]);
            title.set_max_width_chars(18);
            artist.set_max_width_chars(18);
            let b = gtk::Box::new(gtk::Orientation::Vertical, 4);
            b.add_css_class("hm-tile");
            b.append(&img);
            b.append(&title);
            b.append(&artist);
            item.set_child(Some(&b));
        });
        let w = Rc::downgrade(self);
        factory.connect_bind(move |_, item| {
            let Some(app) = w.upgrade() else { return };
            let item = item.downcast_ref::<gtk::ListItem>().unwrap();
            let ai = unboxed(&item.item().unwrap());
            let lib = app.lib();
            let Some(album) = lib.albums.get(ai) else {
                return;
            };
            let b = item.child().and_downcast::<gtk::Box>().unwrap();
            let img = b.first_child().and_downcast::<gtk::Image>().unwrap();
            let title = img.next_sibling().and_downcast::<gtk::Label>().unwrap();
            let artist = title.next_sibling().and_downcast::<gtk::Label>().unwrap();
            app.set_cover(&img, album.cover.as_deref());
            title.set_label(&album.title);
            artist.set_label(&album.artist);
            b.set_tooltip_text(Some(&format!("{} — {}", album.title, album.artist)));
        });
        let grid = gtk::GridView::builder()
            .model(&gtk::NoSelection::new(Some(self.albums_model.clone())))
            .factory(&factory)
            .single_click_activate(true)
            .max_columns(12)
            .min_columns(1)
            .build();
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
                app.show_artist(row.index() as usize);
            }
        });
        self.stack.add_titled_with_icon(
            &scrolled(&self.artists_list, 700),
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
        let pl_box = gtk::Box::new(gtk::Orientation::Vertical, 12);
        let new_btn = gtk::Button::builder()
            .label(tr("New Playlist…"))
            .action_name("win.new-playlist")
            .halign(gtk::Align::End)
            .css_classes(["pill"])
            .build();
        pl_box.append(&new_btn);
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

        let empty_btn = gtk::Button::builder()
            .label(tr("Add Folder…"))
            .action_name("win.add-folder")
            .halign(gtk::Align::Center)
            .css_classes(["pill", "suggested-action"])
            .build();
        self.empty_page.set_child(Some(&empty_btn));

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

    fn build_songs_view(self: &Rc<Self>) -> gtk::ScrolledWindow {
        let cv = gtk::ColumnView::builder()
            .single_click_activate(true)
            .show_row_separators(false)
            .css_classes(["data-table"])
            .build();
        let w = Rc::downgrade(self);
        let text_col = |title: &str, expand: bool, get: fn(&model::Track, &App) -> String| {
            let f = gtk::SignalListItemFactory::new();
            f.connect_setup(|_, item| {
                let item = item.downcast_ref::<gtk::ListItem>().unwrap();
                item.set_child(Some(&label("", &[])));
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

        // Playing indicator.
        let f = gtk::SignalListItemFactory::new();
        f.connect_setup(|_, item| {
            let item = item.downcast_ref::<gtk::ListItem>().unwrap();
            item.set_child(Some(&gtk::Image::new()));
        });
        let wi = w.clone();
        f.connect_bind(move |_, item| {
            let Some(app) = wi.upgrade() else { return };
            let item = item.downcast_ref::<gtk::ListItem>().unwrap();
            let i = unboxed(&item.item().unwrap());
            let lib = app.lib();
            let playing = lib
                .tracks
                .get(i)
                .is_some_and(|t| app.queue.borrow().current() == Some(&t.path));
            if let Some(img) = item.child().and_downcast::<gtk::Image>() {
                img.set_icon_name(playing.then_some("media-playback-start-symbolic"));
                if playing {
                    img.update_property(&[gtk::accessible::Property::Label(tr("Now Playing"))]);
                } else {
                    img.reset_property(gtk::AccessibleProperty::Label);
                }
            }
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
        let plays = text_col(tr("Plays"), false, |t, app| {
            app.stats.borrow().plays(&t.path).to_string()
        });
        plays.set_sorter(Some(&sorter(|t, app| {
            SortKey::Num(app.stats.borrow().plays(&t.path) as u64)
        })));
        for c in [&title, &artist, &album, &length, &plays] {
            cv.append_column(c);
        }
        // Narrow windows show title, artist and length only.
        for c in [&album, &plays] {
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
            let ws = ws.clone();
            let it = item.downgrade();
            heart.connect_toggled(move |h| {
                set_heart_icon(h);
                let (Some(app), Some(item)) = (ws.upgrade(), it.upgrade()) else {
                    return;
                };
                let Some(obj) = item.item() else { return };
                let lib = app.lib();
                if let Some(t) = lib.tracks.get(unboxed(&obj)) {
                    app.set_favorite(&t.path, h.is_active());
                }
            });
            item.set_child(Some(&heart));
        });
        let wb = w.clone();
        f.connect_bind(move |_, item| {
            let Some(app) = wb.upgrade() else { return };
            let item = item.downcast_ref::<gtk::ListItem>().unwrap();
            let i = unboxed(&item.item().unwrap());
            let fav = app
                .lib()
                .tracks
                .get(i)
                .is_some_and(|t| app.stats.borrow().is_favorite(&t.path));
            if let Some(h) = item.child().and_downcast::<gtk::ToggleButton>() {
                sync_heart(&h, fav);
            }
        });
        cv.append_column(
            &gtk::ColumnViewColumn::builder()
                .title(tr("Favorite"))
                .factory(&f)
                .build(),
        );

        self.songs_sorted.set_sorter(cv.sorter().as_ref());
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
        gtk::ScrolledWindow::builder()
            .child(&cv)
            .hscrollbar_policy(gtk::PolicyType::Automatic)
            .vexpand(true)
            .build()
    }

    /// Rebuilds all library views from the current library.
    pub fn refresh_library_views(self: &Rc<Self>) {
        let lib = self.lib();
        let albums: Vec<glib::BoxedAnyObject> = (0..lib.albums.len()).map(boxed).collect();
        self.albums_model
            .splice(0, self.albums_model.n_items(), &albums);
        let songs: Vec<glib::BoxedAnyObject> =
            lib.all_tracks_by_album().into_iter().map(boxed).collect();
        self.songs_model
            .splice(0, self.songs_model.n_items(), &songs);

        self.artists_list.remove_all();
        for a in &lib.artists {
            let n = a.albums.len();
            let row = adw::ActionRow::builder()
                .title(glib::markup_escape_text(&a.name))
                .subtitle(trnf!("{n} album", "{n} albums", n))
                .activatable(true)
                .build();
            row.add_prefix(&gtk::Image::from_icon_name("avatar-default-symbolic"));
            row.add_suffix(&gtk::Image::from_icon_name("go-next-symbolic"));
            self.artists_list.append(&row);
        }
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

    pub fn track_row(
        self: &Rc<Self>,
        idx: usize,
        number: &str,
        show_artist: bool,
        ctx: &RowCtx,
        pos: usize,
    ) -> gtk::ListBoxRow {
        let lib = self.lib();
        let t = &lib.tracks[idx];
        let num = gtk::Label::builder()
            .label(number)
            .css_classes(["dim-label", "numeric", "hm-num"])
            .build();
        let indicator = gtk::Stack::new();
        indicator.add_named(&num, Some("number"));
        // The row's description also says it is playing.
        indicator.add_named(
            &gtk::Image::builder()
                .icon_name("media-playback-start-symbolic")
                .accessible_role(gtk::AccessibleRole::Presentation)
                .build(),
            Some("playing"),
        );
        let text = gtk::Box::new(gtk::Orientation::Vertical, 2);
        text.set_hexpand(true);
        text.set_valign(gtk::Align::Center);
        text.append(&label(&t.title, &[]));
        if show_artist {
            text.append(&label(t.artist_name(), &["dim-label", "caption"]));
        }
        let dur = gtk::Label::builder()
            .label(format_duration(t.duration_ms))
            .css_classes(["dim-label", "numeric"])
            .build();
        let heart = gtk::ToggleButton::builder()
            .valign(gtk::Align::Center)
            .css_classes(["flat"])
            .build();
        sync_heart(&heart, self.stats.borrow().is_favorite(&t.path));
        let path = t.path.clone();
        let w = Rc::downgrade(self);
        heart.connect_toggled(move |h| {
            set_heart_icon(h);
            if let Some(app) = w.upgrade() {
                app.set_favorite(&path, h.is_active());
            }
        });

        let menu = gio::Menu::new();
        let item = |label: &str, action: &str, target: glib::Variant| {
            let it = gio::MenuItem::new(Some(label), None);
            it.set_action_and_target_value(Some(action), Some(&target));
            it
        };
        let ti = (idx as u32).to_variant();
        let s1 = gio::Menu::new();
        if !matches!(ctx, RowCtx::Queue) {
            s1.append_item(&item(tr("Play Next"), "win.play-next", ti.clone()));
            s1.append_item(&item(tr("Add to Queue"), "win.queue-add", ti.clone()));
        }
        s1.append_item(&item(
            tr("Add to Playlist…"),
            "win.add-to-playlist",
            ti.clone(),
        ));
        s1.append_item(&item(tr("Go to Album"), "win.go-album", ti));
        menu.append_section(None, &s1);
        match ctx {
            RowCtx::Playlist(file) => {
                let target = (file.to_string_lossy().into_owned(), pos as u32).to_variant();
                menu.append_item(&item(
                    tr("Remove from Playlist"),
                    "win.playlist-remove",
                    target,
                ));
            }
            RowCtx::Queue => menu.append_item(&item(
                tr("Remove from Queue"),
                "win.queue-remove",
                (pos as u32).to_variant(),
            )),
            RowCtx::Plain => {}
        }
        let more = gtk::MenuButton::builder()
            .icon_name("view-more-symbolic")
            .valign(gtk::Align::Center)
            .css_classes(["flat"])
            .tooltip_text(tr("More"))
            .build()
            .labelled(tr("More"));
        more.set_create_popup_func(move |mb| {
            if mb.menu_model().is_none() {
                mb.set_menu_model(Some(&menu));
            }
        });

        let b = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        b.set_margin_top(6);
        b.set_margin_bottom(6);
        b.set_margin_start(12);
        b.set_margin_end(6);
        b.append(&indicator);
        b.append(&text);
        b.append(&dur);
        b.append(&heart);
        b.append(&more);
        let row = gtk::ListBoxRow::builder()
            .child(&b)
            .activatable(true)
            .build();
        let playing = self.queue.borrow().current() == Some(&t.path);
        crate::ui::show_playing(&indicator, playing);
        self.register_row(RowHandle {
            path: t.path.clone(),
            indicator: indicator.downgrade(),
            heart: heart.downgrade(),
        });
        row
    }

    /// A boxed list of track rows; activating one plays `tracks` from there.
    pub fn track_list(
        self: &Rc<Self>,
        tracks: &[usize],
        numbers: impl Fn(usize, usize) -> String,
        show_artist: bool,
        ctx: RowCtx,
        positions: Option<&[usize]>,
        play_from: Option<(Vec<usize>, usize)>,
    ) -> gtk::ListBox {
        let list = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .css_classes(["boxed-list"])
            .build();
        for (n, &t) in tracks.iter().enumerate() {
            let pos = positions.map_or(n, |p| p[n]);
            list.append(&self.track_row(t, &numbers(n, t), show_artist, &ctx, pos));
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

    fn header_block(
        self: &Rc<Self>,
        cover: Option<&str>,
        title: &str,
        subtitle: &str,
        detail: &str,
        tracks: Vec<usize>,
    ) -> (gtk::Widget, gtk::FlowBox) {
        let img = gtk::Image::builder()
            .pixel_size(160)
            .css_classes(["hm-cover"])
            // Decorative: the album or track title next to it names it.
            .accessible_role(gtk::AccessibleRole::Presentation)
            .build();
        self.set_cover(&img, cover);
        let info = gtk::Box::new(gtk::Orientation::Vertical, 6);
        info.set_valign(gtk::Align::Center);
        let t = label(title, &["title-1"]);
        t.set_wrap(true);
        t.set_ellipsize(gtk::pango::EllipsizeMode::None);
        info.append(&t);
        if !subtitle.is_empty() {
            info.append(&label(subtitle, &["title-4"]));
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
        let w = Rc::downgrade(self);
        shuffle.connect_clicked(move |_| {
            if let Some(app) = w.upgrade() {
                app.play_tracks(&tracks, 0, Some(true));
            }
        });
        add_header_button(&buttons, &play);
        add_header_button(&buttons, &shuffle);
        info.append(&buttons);
        let header = gtk::Box::new(gtk::Orientation::Horizontal, 18);
        header.append(&img);
        header.append(&info);
        // Narrow windows put the cover above the text (see
        // `App::layout_headers`).
        self.headers.borrow_mut().push(header.downgrade());
        layout_header(&header, self.is_narrow());
        (header.upcast(), buttons)
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
        let (header, _) = self.header_block(
            album.cover.as_deref(),
            &album.title,
            &album.artist,
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
        let first_cover = artist
            .albums
            .iter()
            .find_map(|&a| lib.albums[a].cover.clone());
        let (header, _) = self.header_block(
            first_cover.as_deref(),
            &artist.name,
            "",
            &format!(
                "{} · {} · {}",
                trnf!("{n} album", "{n} albums", artist.albums.len()),
                songs_count(all.len()),
                format_total(total)
            ),
            all,
        );
        content.append(&header);
        for &ai in &artist.albums {
            let album = &lib.albums[ai];
            let tracks = lib.artist_tracks_in_album(&artist.name, ai);
            let img = gtk::Image::builder()
                .pixel_size(48)
                .css_classes(["hm-cover"])
                // Decorative: the album or track title next to it names it.
                .accessible_role(gtk::AccessibleRole::Presentation)
                .build();
            self.set_cover(&img, album.cover.as_deref());
            let text = gtk::Box::new(gtk::Orientation::Vertical, 2);
            text.set_valign(gtk::Align::Center);
            text.append(&label(&album.title, &["heading"]));
            text.append(&label(
                &album.year.map(|y| y.to_string()).unwrap_or_default(),
                &["dim-label", "caption"],
            ));
            let hb = gtk::Box::new(gtk::Orientation::Horizontal, 12);
            hb.append(&img);
            hb.append(&text);
            let btn = gtk::Button::builder()
                .child(&hb)
                .css_classes(["flat"])
                .halign(gtk::Align::Start)
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
        let p = page(&title, Some("playlist"), &scrolled(&content, 800));
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
        let (header, buttons) = self.header_block(
            cover.as_deref(),
            &name,
            "",
            &format!("{} · {}", songs_count(tracks.len()), format_total(total)),
            tracks.clone(),
        );
        if let PlRef::User(file) = r {
            let rename = gtk::Button::builder()
                .icon_name("document-edit-symbolic")
                .tooltip_text(tr("Rename Playlist"))
                .css_classes(["circular"])
                .build()
                .labelled(tr("Rename Playlist"));
            let delete = gtk::Button::builder()
                .icon_name("user-trash-symbolic")
                .tooltip_text(tr("Delete Playlist"))
                .css_classes(["circular", "destructive-action"])
                .build()
                .labelled(tr("Delete Playlist"));
            let (w, f) = (Rc::downgrade(self), file.clone());
            rename.connect_clicked(move |_| {
                if let Some(app) = w.upgrade() {
                    app.new_playlist_dialog(Some(f.clone()));
                }
            });
            let (w, f) = (Rc::downgrade(self), file.clone());
            delete.connect_clicked(move |_| {
                if let Some(app) = w.upgrade() {
                    app.delete_playlist_dialog(&f);
                }
            });
            let export = gtk::Button::builder()
                .icon_name("document-save-as-symbolic")
                .tooltip_text(tr("Export Playlist…"))
                .css_classes(["circular"])
                .build()
                .labelled(tr("Export Playlist…"));
            let (w, f) = (Rc::downgrade(self), file.clone());
            export.connect_clicked(move |_| {
                if let Some(app) = w.upgrade() {
                    app.export_playlist_dialog(&f);
                }
            });
            add_header_button(&buttons, &rename);
            add_header_button(&buttons, &export);
            add_header_button(&buttons, &delete);
        }
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

    pub fn show_now_playing(self: &Rc<Self>) {
        if self.nav.visible_page().and_then(|p| p.tag()).as_deref() == Some("now-playing") {
            return;
        }
        if self.nav.find_page("now-playing").is_some() {
            self.nav.pop_to_tag("now-playing");
            return;
        }
        let content = gtk::Box::new(gtk::Orientation::Vertical, 12);
        let img = gtk::Image::builder()
            .pixel_size(256)
            .css_classes(["hm-cover"])
            // Decorative: the album or track title next to it names it.
            .accessible_role(gtk::AccessibleRole::Presentation)
            .halign(gtk::Align::Center)
            .build();
        let title = gtk::Label::builder()
            .css_classes(["title-2"])
            .wrap(true)
            .justify(gtk::Justification::Center)
            .build();
        let sub = gtk::Label::builder()
            .css_classes(["dim-label"])
            .wrap(true)
            .justify(gtk::Justification::Center)
            .build();
        content.append(&img);
        content.append(&title);
        content.append(&sub);
        content.append(&label(tr("Queue"), &["heading", "hm-disc"]));
        let list = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .css_classes(["boxed-list"])
            .build();
        content.append(&list);
        let w = Rc::downgrade(self);
        list.connect_row_activated(move |_, row| {
            let Some(app) = w.upgrade() else { return };
            let idx = row.widget_name().parse::<usize>().ok();
            if let Some(i) = idx {
                app.queue.borrow_mut().jump(i);
                app.start_current(true);
            }
        });
        // The header labels live in the list's parent; refresh_queue_view
        // reaches them through these widget names.
        img.set_widget_name("np-cover");
        title.set_widget_name("np-title");
        sub.set_widget_name("np-sub");
        *self.queue_view.borrow_mut() = Some(list.downgrade());
        self.nav.push(&page(
            tr("Now Playing"),
            Some("now-playing"),
            &scrolled(&content, 700),
        ));
        self.refresh_queue_view();
    }

    pub fn refresh_queue_view(self: &Rc<Self>) {
        let Some(list) = self.queue_view.borrow().as_ref().and_then(|w| w.upgrade()) else {
            return;
        };
        let Some(content) = list.parent().and_downcast::<gtk::Box>() else {
            return;
        };
        let track = self.current_track();
        let mut child = content.first_child();
        while let Some(c) = child {
            match c.widget_name().as_str() {
                "np-cover" => {
                    if let Some(img) = c.downcast_ref::<gtk::Image>() {
                        self.set_cover(img, track.as_ref().and_then(|t| t.cover.as_deref()));
                    }
                }
                "np-title" => {
                    if let Some(l) = c.downcast_ref::<gtk::Label>() {
                        l.set_label(
                            track
                                .as_ref()
                                .map_or(tr("Nothing Playing"), |t| t.title.as_str()),
                        );
                    }
                }
                "np-sub" => {
                    if let Some(l) = c.downcast_ref::<gtk::Label>() {
                        l.set_label(
                            &track
                                .as_ref()
                                .map(|t| format!("{} — {}", t.artist_name(), t.album_name()))
                                .unwrap_or_default(),
                        );
                    }
                }
                _ => {}
            }
            child = c.next_sibling();
        }
        list.remove_all();
        let lib = self.lib();
        let order: Vec<(usize, PathBuf)> = self
            .queue
            .borrow()
            .in_play_order()
            .into_iter()
            .map(|(i, p)| (i, p.clone()))
            .collect();
        for (n, (item_idx, path)) in order.iter().enumerate().take(500) {
            let Some(t) = lib.index_of(path) else {
                continue;
            };
            let row = self.track_row(t, &(n + 1).to_string(), true, &RowCtx::Queue, *item_idx);
            row.set_widget_name(&item_idx.to_string());
            list.append(&row);
        }
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
        if !r.albums.is_empty() {
            b.append(&label(tr("Albums"), &["heading"]));
            let list = gtk::ListBox::builder()
                .selection_mode(gtk::SelectionMode::None)
                .css_classes(["boxed-list"])
                .build();
            for &ai in r.albums.iter().take(50) {
                let a = &lib.albums[ai];
                let row = adw::ActionRow::builder()
                    .title(glib::markup_escape_text(&a.title))
                    .subtitle(glib::markup_escape_text(&a.artist))
                    .activatable(true)
                    .build();
                let img = gtk::Image::builder()
                    .pixel_size(40)
                    .css_classes(["hm-cover"])
                    // Decorative: the album or track title next to it names it.
                    .accessible_role(gtk::AccessibleRole::Presentation)
                    .build();
                self.set_cover(&img, a.cover.as_deref());
                row.add_prefix(&img);
                list.append(&row);
            }
            let (w, albums) = (Rc::downgrade(self), r.albums.clone());
            list.connect_row_activated(move |_, row| {
                if let Some(app) = w.upgrade() {
                    app.show_album(albums[row.index() as usize]);
                }
            });
            b.append(&list);
        }
        if !r.artists.is_empty() {
            b.append(&label(tr("Artists"), &["heading"]));
            let list = gtk::ListBox::builder()
                .selection_mode(gtk::SelectionMode::None)
                .css_classes(["boxed-list"])
                .build();
            for &ar in r.artists.iter().take(50) {
                let row = adw::ActionRow::builder()
                    .title(glib::markup_escape_text(&lib.artists[ar].name))
                    .activatable(true)
                    .build();
                row.add_prefix(&gtk::Image::from_icon_name("avatar-default-symbolic"));
                list.append(&row);
            }
            let (w, artists) = (Rc::downgrade(self), r.artists.clone());
            list.connect_row_activated(move |_, row| {
                if let Some(app) = w.upgrade() {
                    app.show_artist(artists[row.index() as usize]);
                }
            });
            b.append(&list);
        }
        if !r.tracks.is_empty() {
            b.append(&label(tr("Songs"), &["heading"]));
            let tracks: Vec<usize> = r.tracks.iter().copied().take(200).collect();
            b.append(&self.track_list(
                &tracks,
                |_, _| String::new(),
                true,
                RowCtx::Plain,
                None,
                None,
            ));
        }
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
                let n = app.songs_model.n_items();
                app.songs_model.items_changed(0, n, n);
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
