//! Track rows: the widgets shared by list-box rows (album, artist,
//! playlist and search pages) and recycled list-view rows (the queue and
//! full search results), their menu, context menu and drag-and-drop.

use adw::prelude::*;
use gtk::{gdk, gio, glib};
use helix_app_common::i18n::tr;
use helix_app_common::Labelled;
use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use crate::model::format_duration;
use crate::ui::{set_heart_icon, App, Live, RowHandle};

/// Where a track row lives (decides its menu and activation).
#[derive(Clone, Debug, PartialEq)]
pub enum RowCtx {
    Plain,
    Playlist(PathBuf),
    Queue,
}

/// A row's position in a reorderable list: `pos` is what removal uses
/// (playlist entry or queue item); `from`, `up` and `down` are the
/// positions moves use (playlist entries, or queue play order).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Place {
    pub pos: usize,
    pub from: usize,
    pub up: Option<usize>,
    pub down: Option<usize>,
}

impl Place {
    /// Row `n` of `len` rows whose positions are `positions` (or 0..len).
    pub fn in_list(n: usize, len: usize, positions: Option<&[usize]>) -> Place {
        let at = |i: usize| positions.map_or(i, |p| p[i]);
        Place {
            pos: at(n),
            from: at(n),
            up: n.checked_sub(1).map(at),
            down: (n + 1 < len).then(|| at(n + 1)),
        }
    }
}

/// What a row shows.
#[derive(Clone, Default)]
struct Bound {
    track: Option<usize>,
    ctx: Option<RowCtx>,
    place: Place,
}

/// Moves a row of a reorderable list view from one position to another.
pub type Reorder = Rc<dyn Fn(&Rc<App>, usize, usize)>;

/// The widgets of one track row.
pub struct TrackRow {
    pub root: gtk::Box,
    num: gtk::Label,
    title: gtk::Label,
    artist: Option<gtk::Label>,
    dur: gtk::Label,
    more: gtk::MenuButton,
    handles: [Rc<RowHandle>; 2],
    bound: RefCell<Bound>,
}

/// A track's menu: queue and playlist actions, where to go, details, and
/// for playlist or queue rows moving and removing.
pub fn track_menu(idx: usize, ctx: &RowCtx, place: Place, artist: Option<usize>) -> gio::Menu {
    let item = |label: &str, action: &str, target: glib::Variant| {
        let it = gio::MenuItem::new(Some(label), None);
        it.set_action_and_target_value(Some(action), Some(&target));
        it
    };
    let ti = (idx as u32).to_variant();
    let menu = gio::Menu::new();
    let queue = gio::Menu::new();
    if *ctx != RowCtx::Queue {
        queue.append_item(&item(tr("Play Next"), "win.play-next", ti.clone()));
        queue.append_item(&item(tr("Add to Queue"), "win.queue-add", ti.clone()));
    }
    queue.append_item(&item(
        tr("Add to Playlist…"),
        "win.add-to-playlist",
        ti.clone(),
    ));
    menu.append_section(None, &queue);
    let go = gio::Menu::new();
    go.append_item(&item(tr("Go to Album"), "win.go-album", ti.clone()));
    if artist.is_some() {
        go.append_item(&item(tr("Go to Artist"), "win.go-artist", ti.clone()));
    }
    menu.append_section(None, &go);
    let info = gio::Menu::new();
    info.append_item(&item(tr("Song Details"), "win.song-details", ti.clone()));
    info.append_item(&item(tr("Show in Files"), "win.show-in-files", ti));
    menu.append_section(None, &info);
    let edit = gio::Menu::new();
    match ctx {
        RowCtx::Playlist(file) => {
            let file = file.to_string_lossy().into_owned();
            for (label, to) in [(tr("Move Up"), place.up), (tr("Move Down"), place.down)] {
                if let Some(to) = to {
                    let target = (file.clone(), place.from as u32, to as u32).to_variant();
                    edit.append_item(&item(label, "win.playlist-move", target));
                }
            }
            let target = (file, place.pos as u32).to_variant();
            edit.append_item(&item(
                tr("Remove from Playlist"),
                "win.playlist-remove",
                target,
            ));
        }
        RowCtx::Queue => {
            for (label, to) in [(tr("Move Up"), place.up), (tr("Move Down"), place.down)] {
                if let Some(to) = to {
                    let target = (place.from as u32, to as u32).to_variant();
                    edit.append_item(&item(label, "win.queue-move", target));
                }
            }
            edit.append_item(&item(
                tr("Remove from Queue"),
                "win.queue-remove",
                (place.pos as u32).to_variant(),
            ));
        }
        RowCtx::Plain => {}
    }
    if edit.n_items() > 0 {
        menu.append_section(None, &edit);
    }
    menu
}

impl App {
    /// The widgets of a track row, not yet showing a track (see
    /// [`App::bind_track_row`]).
    pub fn new_track_row(self: &Rc<Self>, show_artist: bool) -> Rc<TrackRow> {
        let num = gtk::Label::builder()
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
        let title = label(&[]);
        text.append(&title);
        let artist = show_artist.then(|| {
            let l = label(&["dim-label", "caption"]);
            text.append(&l);
            l
        });
        let dur = gtk::Label::builder()
            .css_classes(["dim-label", "numeric"])
            .build();
        let heart = gtk::ToggleButton::builder()
            .valign(gtk::Align::Center)
            .css_classes(["flat"])
            .build();
        set_heart_icon(&heart);
        let more = gtk::MenuButton::builder()
            .icon_name("view-more-symbolic")
            .valign(gtk::Align::Center)
            .css_classes(["flat"])
            .tooltip_text(tr("More"))
            .build()
            .labelled(tr("More"));
        let root = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        root.set_margin_top(6);
        root.set_margin_bottom(6);
        root.set_margin_start(12);
        root.set_margin_end(6);
        root.append(&indicator);
        root.append(&text);
        root.append(&dur);
        root.append(&heart);
        root.append(&more);
        let row = Rc::new(TrackRow {
            root,
            num,
            title,
            artist,
            dur,
            more,
            handles: [
                RowHandle::new(None, Live::Indicator(indicator.downgrade())),
                RowHandle::new(None, Live::Heart(heart.downgrade())),
            ],
            bound: RefCell::default(),
        });
        for h in &row.handles {
            self.register_row(h.clone());
        }
        // The menu is built when it opens, for the track shown then.
        let (w, r) = (Rc::downgrade(self), Rc::downgrade(&row));
        row.more.set_create_popup_func(move |mb| {
            if let (Some(app), Some(row)) = (w.upgrade(), r.upgrade()) {
                mb.set_menu_model(app.row_menu(&row).as_ref());
            }
        });
        let (w, r) = (Rc::downgrade(self), Rc::downgrade(&row));
        heart.connect_toggled(move |h| {
            set_heart_icon(h);
            let (Some(app), Some(row)) = (w.upgrade(), r.upgrade()) else {
                return;
            };
            let Some(t) = row.bound.borrow().track else {
                return;
            };
            let path = app.lib().tracks.get(t).map(|t| t.path.clone());
            if let Some(p) = path {
                app.set_favorite(&p, h.is_active());
            }
        });
        // Right-click, long-press, Menu or Shift+F10 open the row's menu.
        let more = row.more.downgrade();
        crate::pages::on_context_click(&row.root, move |_, _, _| {
            if let Some(m) = more.upgrade() {
                m.popup();
            }
        });
        row
    }

    fn row_menu(&self, row: &TrackRow) -> Option<gio::Menu> {
        let b = row.bound.borrow();
        let t = b.track?;
        let lib = self.lib();
        let artist = lib.artist_index(lib.tracks.get(t)?.artist_name());
        Some(track_menu(t, b.ctx.as_ref()?, b.place, artist))
    }

    /// Shows track `idx` in `row`.
    pub fn bind_track_row(
        &self,
        row: &TrackRow,
        idx: usize,
        number: &str,
        ctx: &RowCtx,
        place: Place,
    ) {
        let lib = self.lib();
        let Some(t) = lib.tracks.get(idx) else {
            return;
        };
        row.num.set_label(number);
        row.title.set_label(&t.title);
        if let Some(a) = &row.artist {
            a.set_label(t.artist_name());
        }
        row.dur.set_label(&format_duration(t.duration_ms));
        *row.bound.borrow_mut() = Bound {
            track: Some(idx),
            ctx: Some(ctx.clone()),
            place,
        };
        for h in &row.handles {
            *h.path.borrow_mut() = Some(t.path.clone());
            self.refresh_row(h);
        }
    }

    /// A list-box row for track `idx`.
    pub fn track_row(
        self: &Rc<Self>,
        idx: usize,
        number: &str,
        show_artist: bool,
        ctx: &RowCtx,
        place: Place,
    ) -> gtk::ListBoxRow {
        let row = self.new_track_row(show_artist);
        self.bind_track_row(&row, idx, number, ctx, place);
        let lbr = gtk::ListBoxRow::builder()
            .child(&row.root)
            .activatable(true)
            .build();
        add_menu_keys(&lbr, &row.more);
        // The parts live as long as the row.
        let keep = RefCell::new(Some(row));
        lbr.connect_destroy(move |_| {
            keep.borrow_mut().take();
        });
        lbr
    }

    /// A list view of tracks whose rows are recycled: `model` holds
    /// `BoxedAnyObject`s, `item` maps one to (track, number, place).
    pub fn track_list_view(
        self: &Rc<Self>,
        model: &impl IsA<gio::ListModel>,
        show_artist: bool,
        ctx: RowCtx,
        item: impl Fn(&glib::Object, u32) -> Option<(usize, String, Place)> + 'static,
        reorder: Option<Reorder>,
    ) -> gtk::ListView {
        let item = Rc::new(item);
        let factory = gtk::SignalListItemFactory::new();
        let w = Rc::downgrade(self);
        factory.connect_setup(move |_, li| {
            let Some(app) = w.upgrade() else { return };
            let li = li.downcast_ref::<gtk::ListItem>().unwrap();
            let row = app.new_track_row(show_artist);
            li.set_child(Some(&row.root));
            add_menu_keys(&row.root, &row.more);
            if let Some(reorder) = &reorder {
                let (r, w, reorder) = (Rc::downgrade(&row), w.clone(), reorder.clone());
                reorderable(
                    &row.root,
                    move || {
                        r.upgrade().and_then(|r| {
                            r.bound.borrow().track.map(|_| r.bound.borrow().place.from)
                        })
                    },
                    move |from, to| {
                        if let Some(app) = w.upgrade() {
                            reorder(&app, from, to);
                        }
                    },
                );
            }
            // Says "Now playing" on the playing row (like list-box rows).
            let desc = RowHandle::new(None, Live::ItemDescription(li.downgrade()));
            app.register_row(desc.clone());
            let (w, item, ctx) = (w.clone(), item.clone(), ctx.clone());
            let rebind = move |li: &gtk::ListItem| {
                let (Some(app), Some(obj)) = (w.upgrade(), li.item()) else {
                    return;
                };
                if let Some((t, number, place)) = item(&obj, li.position()) {
                    app.bind_track_row(&row, t, &number, &ctx, place);
                    *desc.path.borrow_mut() = app.lib().tracks.get(t).map(|t| t.path.clone());
                    app.refresh_row(&desc);
                }
            };
            let r2 = rebind.clone();
            li.connect_item_notify(move |li| r2(li));
            li.connect_position_notify(move |li| rebind(li));
        });
        gtk::ListView::builder()
            .model(&gtk::NoSelection::new(Some(
                model.clone().upcast::<gio::ListModel>(),
            )))
            .factory(&factory)
            .single_click_activate(true)
            .css_classes(["hm-track-list"])
            .build()
    }
}

/// Opens `more`'s menu with the Menu key or Shift+F10 on `widget`.
fn add_menu_keys(widget: &impl IsA<gtk::Widget>, more: &gtk::MenuButton) {
    let keys = gtk::EventControllerKey::new();
    let more = more.downgrade();
    keys.connect_key_pressed(move |_, key, _, mods| {
        let menu_key = key == gdk::Key::Menu
            || (key == gdk::Key::F10 && mods.contains(gdk::ModifierType::SHIFT_MASK));
        if let (true, Some(m)) = (menu_key, more.upgrade()) {
            m.popup();
            return glib::Propagation::Stop;
        }
        glib::Propagation::Proceed
    });
    widget.add_controller(keys);
}

/// Lets rows of a list be dragged onto each other to reorder it: `from`
/// gives a row's current position, `moved(from, to)` makes the move.
pub fn reorderable(
    widget: &impl IsA<gtk::Widget>,
    from: impl Fn() -> Option<usize> + 'static,
    moved: impl Fn(usize, usize) + 'static,
) {
    let from = Rc::new(from);
    let source = gtk::DragSource::new();
    source.set_actions(gdk::DragAction::MOVE);
    let f = from.clone();
    source.connect_prepare(move |_, _, _| {
        f().map(|p| gdk::ContentProvider::for_value(&(p as u32).to_value()))
    });
    let w = widget.downgrade();
    source.connect_drag_begin(move |s, _| {
        if let Some(w) = w.upgrade() {
            let paintable = gtk::WidgetPaintable::new(Some(&w));
            s.set_icon(Some(&paintable), 0, 0);
        }
    });
    widget.add_controller(source);
    let target = gtk::DropTarget::new(u32::static_type(), gdk::DragAction::MOVE);
    let w = widget.downgrade();
    target.connect_enter(move |_, _, _| {
        if let Some(w) = w.upgrade() {
            w.add_css_class("hm-drop-target");
        }
        gdk::DragAction::MOVE
    });
    let w = widget.downgrade();
    target.connect_leave(move |_| {
        if let Some(w) = w.upgrade() {
            w.remove_css_class("hm-drop-target");
        }
    });
    let w = widget.downgrade();
    target.connect_drop(move |_, value, _, _| {
        if let Some(w) = w.upgrade() {
            w.remove_css_class("hm-drop-target");
        }
        let (Ok(src), Some(to)) = (value.get::<u32>(), from()) else {
            return false;
        };
        if src as usize != to {
            moved(src as usize, to);
        }
        true
    });
    widget.add_controller(target);
}

fn label(classes: &[&str]) -> gtk::Label {
    gtk::Label::builder()
        .xalign(0.0)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .css_classes(classes.to_vec())
        .build()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn places_in_a_list() {
        assert_eq!(
            Place::in_list(0, 3, None),
            Place {
                pos: 0,
                from: 0,
                up: None,
                down: Some(1)
            }
        );
        // Playlist rows skip entries that are not in the library.
        let p = Place::in_list(1, 3, Some(&[0, 4, 9]));
        assert_eq!((p.pos, p.from, p.up, p.down), (4, 4, Some(0), Some(9)));
        assert_eq!(Place::in_list(2, 3, Some(&[0, 4, 9])).down, None);
    }
}
