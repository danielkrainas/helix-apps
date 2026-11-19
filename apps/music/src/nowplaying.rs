//! The Now Playing page: the playing song (cover, title, links to its
//! artist and album, lyrics) and the queue beside it, in a sidebar that
//! overlays the song on narrow windows.

use adw::prelude::*;
use gtk::{gio, glib};
use helix_app_common::i18n::tr;
use helix_app_common::trnf;
use helix_app_common::Labelled;
use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;

use crate::lyrics::{self, Lyrics};
use crate::pages::{label, page_with_header, scrolled};
use crate::rows::{Place, RowCtx};
use crate::ui::{cover_widget, App};

/// The open page's widgets (weak: the page may be closed).
pub struct NowPlaying {
    split: glib::WeakRef<adw::OverlaySplitView>,
    cover: glib::WeakRef<gtk::Overlay>,
    title: glib::WeakRef<gtk::Label>,
    artist: glib::WeakRef<gtk::Button>,
    album: glib::WeakRef<gtk::Button>,
    count: glib::WeakRef<gtk::Label>,
    pane: glib::WeakRef<gtk::Stack>,
    lyrics_button: glib::WeakRef<gtk::ToggleButton>,
    lyrics_box: glib::WeakRef<gtk::Box>,
    lyrics_scroll: glib::WeakRef<gtk::ScrolledWindow>,
    /// Synced lyrics: each line's start and label.
    lines: RefCell<Vec<(u64, glib::WeakRef<gtk::Label>)>>,
    current_line: Cell<Option<usize>>,
    /// The song whose lyrics are shown (or being read).
    lyrics_for: RefCell<Option<PathBuf>>,
}

fn link_button(tip: &str) -> (gtk::Button, gtk::Label) {
    let l = gtk::Label::builder()
        .wrap(true)
        .justify(gtk::Justification::Center)
        .build();
    let b = gtk::Button::builder()
        .child(&l)
        .halign(gtk::Align::Center)
        .tooltip_text(tip)
        .css_classes(["flat", "hm-np-link"])
        .build();
    (b, l)
}

impl App {
    pub fn show_now_playing(self: &Rc<Self>) {
        if self.nav.visible_page().and_then(|p| p.tag()).as_deref() == Some("now-playing") {
            return;
        }
        if self.nav.find_page("now-playing").is_some() {
            self.nav.pop_to_tag("now-playing");
            return;
        }
        // The song.
        let cover = cover_widget(256);
        let title = gtk::Label::builder()
            .css_classes(["title-2"])
            .wrap(true)
            .justify(gtk::Justification::Center)
            .build();
        let (artist, _) = link_button(tr("Go to Artist"));
        artist.add_css_class("hm-np-artist");
        let (album, _) = link_button(tr("Go to Album"));
        album.add_css_class("dim-label");
        let song = gtk::Box::new(gtk::Orientation::Vertical, 6);
        song.set_valign(gtk::Align::Center);
        song.append(&cover);
        title.set_margin_top(12);
        song.append(&title);
        song.append(&artist);
        song.append(&album);
        let w = Rc::downgrade(self);
        artist.connect_clicked(move |_| {
            let Some(app) = w.upgrade() else { return };
            let lib = app.lib();
            let ar = app
                .current_track()
                .and_then(|t| lib.artist_index(t.artist_name()));
            if let Some(ar) = ar {
                app.show_artist(ar);
            }
        });
        let w = Rc::downgrade(self);
        album.connect_clicked(move |_| {
            let Some(app) = w.upgrade() else { return };
            let lib = app.lib();
            let ai = app
                .current_track()
                .and_then(|t| lib.index_of(&t.path))
                .and_then(|i| lib.album_of(i));
            if let Some(ai) = ai {
                app.show_album(ai);
            }
        });
        // Lyrics, instead of the song when the Lyrics button is on.
        let lyrics_box = gtk::Box::new(gtk::Orientation::Vertical, 12);
        lyrics_box.add_css_class("hm-lyrics");
        let lyrics_scroll = scrolled(&lyrics_box, 600);
        let pane = gtk::Stack::builder()
            .transition_type(gtk::StackTransitionType::Crossfade)
            .vexpand(true)
            .hexpand(true)
            .build();
        pane.add_named(&scrolled(&song, 500), Some("song"));
        pane.add_named(&lyrics_scroll, Some("lyrics"));

        // The queue.
        let count = label("", &["dim-label", "caption"]);
        let heading = gtk::Box::new(gtk::Orientation::Vertical, 2);
        heading.set_hexpand(true);
        heading.set_valign(gtk::Align::Center);
        heading.append(&label(tr("Queue"), &["heading"]));
        heading.append(&count);
        let save = gtk::Button::builder()
            .icon_name("document-save-symbolic")
            .tooltip_text(tr("Save as Playlist…"))
            .action_name("win.queue-save")
            .css_classes(["flat"])
            .valign(gtk::Align::Center)
            .build()
            .labelled(tr("Save as Playlist…"));
        let clear = gtk::Button::builder()
            .icon_name("edit-clear-all-symbolic")
            .tooltip_text(tr("Clear Queue"))
            .action_name("win.queue-clear")
            .css_classes(["flat"])
            .valign(gtk::Align::Center)
            .build()
            .labelled(tr("Clear Queue"));
        let bar = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        bar.add_css_class("hm-queue-bar");
        bar.append(&heading);
        bar.append(&save);
        bar.append(&clear);
        let list = self.track_list_view(
            &self.queue_model,
            true,
            RowCtx::Queue,
            {
                let w = Rc::downgrade(self);
                move |obj, n| {
                    let app = w.upgrade()?;
                    let (item, path) = obj
                        .downcast_ref::<glib::BoxedAnyObject>()?
                        .borrow::<(usize, PathBuf)>()
                        .clone();
                    let t = app.lib().index_of(&path)?;
                    let n = n as usize;
                    let len = app.queue_model.n_items() as usize;
                    let place = Place {
                        pos: item,
                        from: n,
                        up: n.checked_sub(1),
                        down: (n + 1 < len).then_some(n + 1),
                    };
                    Some((t, (n + 1).to_string(), place))
                }
            },
            Some(Rc::new(|app: &Rc<App>, from, to| {
                app.move_in_queue(from, to)
            })),
        );
        let w = Rc::downgrade(self);
        list.connect_activate(move |_, pos| {
            let Some(app) = w.upgrade() else { return };
            let item = app
                .queue_model
                .item(pos)
                .and_downcast::<glib::BoxedAnyObject>()
                .map(|b| b.borrow::<(usize, PathBuf)>().0);
            if let Some(i) = item {
                app.queue.borrow_mut().jump(i);
                app.start_current(true);
            }
        });
        let queue_scroll = gtk::ScrolledWindow::builder()
            .child(&list)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .build();
        let side = gtk::Box::new(gtk::Orientation::Vertical, 0);
        side.append(&bar);
        side.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
        side.append(&queue_scroll);

        let split = adw::OverlaySplitView::builder()
            .content(&pane)
            .sidebar(&side)
            .sidebar_position(gtk::PackType::End)
            .min_sidebar_width(300.0)
            .max_sidebar_width(460.0)
            .sidebar_width_fraction(0.42)
            .collapsed(self.is_narrow())
            .show_sidebar(!self.is_narrow())
            .build();
        let (p, header) = page_with_header(tr("Now Playing"), Some("now-playing"), &split);
        let queue_btn = gtk::ToggleButton::builder()
            .icon_name("view-list-symbolic")
            .tooltip_text(tr("Queue"))
            .build()
            .labelled(tr("Queue"));
        split
            .bind_property("show-sidebar", &queue_btn, "active")
            .bidirectional()
            .sync_create()
            .build();
        header.pack_end(&queue_btn);
        let lyrics_button = gtk::ToggleButton::builder()
            .icon_name("media-view-subtitles-symbolic")
            .tooltip_text(tr("Lyrics"))
            .visible(false)
            .active(self.show_lyrics.get())
            .build()
            .labelled(tr("Lyrics"));
        let w = Rc::downgrade(self);
        lyrics_button.connect_toggled(move |b| {
            if let Some(app) = w.upgrade() {
                app.show_lyrics.set(b.is_active());
                app.update_lyrics_pane();
            }
        });
        header.pack_end(&lyrics_button);

        *self.now_playing.borrow_mut() = Some(Rc::new(NowPlaying {
            split: split.downgrade(),
            cover: cover.downgrade(),
            title: title.downgrade(),
            artist: artist.downgrade(),
            album: album.downgrade(),
            count: count.downgrade(),
            pane: pane.downgrade(),
            lyrics_button: lyrics_button.downgrade(),
            lyrics_box: lyrics_box.downgrade(),
            lyrics_scroll: lyrics_scroll.downgrade(),
            lines: RefCell::default(),
            current_line: Cell::new(None),
            lyrics_for: RefCell::default(),
        }));
        self.nav.push(&p);
        self.refresh_queue_view();
    }

    fn np(&self) -> Option<Rc<NowPlaying>> {
        let np = self.now_playing.borrow().clone()?;
        if np.split.upgrade().is_none() {
            self.now_playing.borrow_mut().take();
            return None;
        }
        Some(np)
    }

    /// Narrow windows: the queue overlays the song (hidden at first).
    pub fn layout_now_playing(&self, narrow: bool) {
        if let Some(split) = self.np().and_then(|np| np.split.upgrade()) {
            split.set_collapsed(narrow);
            split.set_show_sidebar(!narrow);
        }
    }

    /// Updates the queue model (only when the queue changed) and the open
    /// Now Playing page.
    pub fn refresh_queue_view(self: &Rc<Self>) {
        let order: Vec<(usize, PathBuf)> = self
            .queue
            .borrow()
            .in_play_order()
            .into_iter()
            .map(|(i, p)| (i, p.clone()))
            .collect();
        let model = &self.queue_model;
        let same = model.n_items() as usize == order.len()
            && order.iter().enumerate().all(|(n, e)| {
                model
                    .item(n as u32)
                    .and_downcast::<glib::BoxedAnyObject>()
                    .is_some_and(|b| *b.borrow::<(usize, PathBuf)>() == *e)
            });
        if !same {
            let objs: Vec<glib::BoxedAnyObject> = order
                .iter()
                .cloned()
                .map(glib::BoxedAnyObject::new)
                .collect();
            model.splice(0, model.n_items(), &objs);
        }
        let Some(np) = self.np() else { return };
        let track = self.current_track();
        if let Some(c) = np.cover.upgrade() {
            self.set_cover(&c, track.as_ref().and_then(|t| t.cover.as_deref()));
        }
        if let Some(l) = np.title.upgrade() {
            l.set_label(
                track
                    .as_ref()
                    .map_or(tr("Nothing Playing"), |t| t.title.as_str()),
            );
        }
        let lib = self.lib();
        for (button, text, target) in [
            (
                np.artist.upgrade(),
                track.as_ref().map(|t| t.artist_name().to_string()),
                track
                    .as_ref()
                    .and_then(|t| lib.artist_index(t.artist_name())),
            ),
            (
                np.album.upgrade(),
                track.as_ref().map(|t| t.album_name().to_string()),
                track
                    .as_ref()
                    .and_then(|t| lib.index_of(&t.path))
                    .and_then(|i| lib.album_of(i)),
            ),
        ] {
            let Some(b) = button else { continue };
            if let Some(l) = b.child().and_downcast::<gtk::Label>() {
                l.set_label(text.as_deref().unwrap_or(""));
            }
            b.set_visible(text.is_some());
            // A plain label when there is nowhere to go.
            b.set_sensitive(target.is_some());
        }
        if let Some(c) = np.count.upgrade() {
            c.set_label(&trnf!("{n} song", "{n} songs", order.len()));
        }
        self.load_lyrics(&np, track.map(|t| t.path));
    }

    /// Reads the playing song's lyrics (on a worker thread) when it changed.
    fn load_lyrics(self: &Rc<Self>, np: &Rc<NowPlaying>, path: Option<PathBuf>) {
        if *np.lyrics_for.borrow() == path {
            return;
        }
        *np.lyrics_for.borrow_mut() = path.clone();
        self.set_lyrics(np, None);
        let Some(path) = path else { return };
        let (w, npw) = (Rc::downgrade(self), Rc::downgrade(np));
        glib::spawn_future_local(async move {
            let p = path.clone();
            let found = gio::spawn_blocking(move || lyrics::find(&p))
                .await
                .ok()
                .flatten();
            let (Some(app), Some(np)) = (w.upgrade(), npw.upgrade()) else {
                return;
            };
            if np.lyrics_for.borrow().as_ref() == Some(&path) {
                app.set_lyrics(&np, found.as_ref());
            }
        });
    }

    fn set_lyrics(&self, np: &NowPlaying, found: Option<&Lyrics>) {
        let Some(b) = np.lyrics_box.upgrade() else {
            return;
        };
        while let Some(c) = b.first_child() {
            b.remove(&c);
        }
        np.lines.borrow_mut().clear();
        np.current_line.set(None);
        match found {
            Some(Lyrics::Plain(text)) => {
                let l = gtk::Label::builder()
                    .label(text)
                    .wrap(true)
                    .selectable(true)
                    .justify(gtk::Justification::Center)
                    .css_classes(["hm-lyric", "title-4"])
                    .build();
                b.append(&l);
            }
            Some(Lyrics::Synced(lines)) => {
                let mut v = Vec::new();
                for (t, text) in lines {
                    let l = gtk::Label::builder()
                        .label(text.as_str())
                        .wrap(true)
                        .justify(gtk::Justification::Center)
                        .css_classes(["hm-lyric", "title-4", "dim-label"])
                        .build();
                    b.append(&l);
                    v.push((*t, l.downgrade()));
                }
                *np.lines.borrow_mut() = v;
            }
            None => {}
        }
        if let Some(btn) = np.lyrics_button.upgrade() {
            btn.set_visible(found.is_some());
        }
        self.update_lyrics_pane();
        self.update_lyrics_position(self.position_ms());
    }

    /// Shows the lyrics or the song, as the Lyrics button says.
    fn update_lyrics_pane(&self) {
        let Some(np) = self.np() else { return };
        let has = np.lyrics_button.upgrade().is_some_and(|b| b.is_visible());
        if let Some(pane) = np.pane.upgrade() {
            pane.set_visible_child_name(if has && self.show_lyrics.get() {
                "lyrics"
            } else {
                "song"
            });
        }
    }

    /// Highlights the synced lyrics line at `pos_ms` and keeps it in view.
    pub fn update_lyrics_position(&self, pos_ms: u64) {
        let Some(np) = self.np() else { return };
        let lines = np.lines.borrow();
        if lines.is_empty() {
            return;
        }
        let starts: Vec<u64> = lines.iter().map(|(t, _)| *t).collect();
        let now = lyrics::current_line(&starts, pos_ms);
        let before = np.current_line.replace(now);
        if before == now {
            return;
        }
        for (i, on) in [(before, false), (now, true)] {
            let Some(l) = i.and_then(|i| lines.get(i)).and_then(|(_, l)| l.upgrade()) else {
                continue;
            };
            if on {
                l.remove_css_class("dim-label");
                l.add_css_class("hm-lyric-current");
            } else {
                l.add_css_class("dim-label");
                l.remove_css_class("hm-lyric-current");
            }
        }
        // Scroll the current line to the middle of the view.
        let (Some(l), Some(sw)) = (
            now.and_then(|i| lines.get(i))
                .and_then(|(_, l)| l.upgrade()),
            np.lyrics_scroll.upgrade(),
        ) else {
            return;
        };
        if let Some(child) = sw.child() {
            if let Some(p) = l.compute_point(&child, &gtk::graphene::Point::new(0.0, 0.0)) {
                let adj = sw.vadjustment();
                let target = p.y() as f64 + l.height() as f64 / 2.0 - adj.page_size() / 2.0;
                adj.set_value(target.clamp(adj.lower(), adj.upper() - adj.page_size()));
            }
        }
    }
}
