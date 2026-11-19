//! Cached thumbnails for the grid, loaded a few at a time.
//!
//! A folder of thousands of pictures or videos binds its grid tiles many
//! times while it loads and scrolls. Starting a load for every bind kept
//! about a thousand thumbnail files open at once, which ran the process
//! out of file descriptors (GTK then failed to load icons and fonts, and
//! the Wayland connection broke). Here at most [`MAX_RUNNING`] thumbnails
//! are read at a time, each read and decoded on a worker thread; the
//! tiles bound most recently are served first, and a request whose tile
//! shows another file by the time its turn comes is dropped. Decoded
//! thumbnails are kept in a small cache, so scrolling back is instant.
//!
//! A file with no cached thumbnail gets one made ([`super::thumbnailer`]),
//! at most [`MAX_MAKING`] at a time beside the reads. One that could not
//! be made is not tried again while Files runs (unless it changes); the
//! next run tries again.

use gtk::prelude::*;
use gtk::{gdk, gio, glib};
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet, VecDeque};

use super::thumbnailer::{self, Source};

/// Thumbnails read at the same time.
pub const MAX_RUNNING: usize = 4;
/// Thumbnails made (thumbnailers run) at the same time.
pub const MAX_MAKING: usize = 2;
/// Decoded thumbnails kept (a large one is 256 × 256, about 256 KiB).
const CACHE: usize = 192;
/// Waiting requests above which those no longer wanted are pruned.
const MAX_QUEUED: usize = 512;

/// A request: show the thumbnail at `path` in `img` while `item` still
/// shows `expected`; first make it from `make` if that is set.
struct Request {
    path: String,
    make: Option<Source>,
    img: glib::WeakRef<gtk::Image>,
    item: glib::WeakRef<gtk::ListItem>,
    expected: glib::WeakRef<gio::FileInfo>,
}

impl Request {
    /// The tile still shows the file the thumbnail is for.
    fn wanted(&self) -> bool {
        match (self.item.upgrade(), self.expected.upgrade()) {
            (Some(item), Some(expected)) => item
                .item()
                .is_some_and(|i| i == expected.upcast::<glib::Object>()),
            _ => false,
        }
    }

    fn show(&self, texture: &gdk::Texture) {
        if let Some(img) = self.img.upgrade().filter(|_| self.wanted()) {
            img.set_paintable(Some(texture));
        }
    }
}

#[derive(Default)]
struct Loader {
    queue: RefCell<VecDeque<Request>>,
    running: Cell<usize>,
    making: Cell<usize>,
    cache: RefCell<Cache>,
    /// Files (URI, mtime) whose thumbnail could not be made this run.
    failed: RefCell<HashSet<(String, u64)>>,
}

impl Loader {
    fn has_slot(&self, req: &Request) -> bool {
        if req.make.is_some() {
            self.making.get() < MAX_MAKING
        } else {
            self.running.get() < MAX_RUNNING
        }
    }
}

/// Least-recently-used textures by path.
#[derive(Default)]
struct Cache {
    map: HashMap<String, (gdk::Texture, u64)>,
    tick: u64,
}

impl Cache {
    fn get(&mut self, path: &str) -> Option<gdk::Texture> {
        self.tick += 1;
        let tick = self.tick;
        self.map.get_mut(path).map(|(t, used)| {
            *used = tick;
            t.clone()
        })
    }

    fn put(&mut self, path: String, texture: gdk::Texture) {
        self.tick += 1;
        if self.map.len() >= CACHE && !self.map.contains_key(&path) {
            if let Some(oldest) = self
                .map
                .iter()
                .min_by_key(|(_, (_, used))| *used)
                .map(|(k, _)| k.clone())
            {
                self.map.remove(&oldest);
            }
        }
        self.map.insert(path, (texture, self.tick));
    }
}

thread_local! {
    static LOADER: Loader = Loader::default();
}

/// Show the cached thumbnail at `path` in `img` (the tile of `item`, now
/// showing `expected`), now if it is in the cache, else when its turn
/// comes.
pub fn request(path: &str, img: &gtk::Image, item: &gtk::ListItem, expected: &gio::FileInfo) {
    enqueue(Request {
        path: path.to_owned(),
        make: None,
        img: img.downgrade(),
        item: item.downgrade(),
        expected: expected.downgrade(),
    });
}

/// Like [`request`] for a file with no cached thumbnail: one is made
/// first (unless that failed before in this run). On success `expected`
/// gets the new `thumbnail::path`, so its tile reads it from then on.
pub fn request_made(src: Source, img: &gtk::Image, item: &gtk::ListItem, expected: &gio::FileInfo) {
    let failed = LOADER.with(|l| l.failed.borrow().contains(&(src.uri.clone(), src.mtime)));
    if failed {
        return;
    }
    enqueue(Request {
        path: thumbnailer::cache_path(&src.uri)
            .to_string_lossy()
            .into_owned(),
        make: Some(src),
        img: img.downgrade(),
        item: item.downgrade(),
        expected: expected.downgrade(),
    });
}

fn enqueue(req: Request) {
    LOADER.with(|l| {
        if let Some(texture) = l.cache.borrow_mut().get(&req.path) {
            req.show(&texture);
            return;
        }
        let mut queue = l.queue.borrow_mut();
        // Forget requests for tiles that show other files by now.
        if queue.len() >= MAX_QUEUED {
            queue.retain(Request::wanted);
        }
        queue.push_back(req);
    });
    pump();
}

/// Start queued work while slots are free (reads: [`MAX_RUNNING`],
/// thumbnails made: [`MAX_MAKING`]): the newest request first (the tiles
/// on screen now), skipping ones no longer wanted.
fn pump() {
    loop {
        let next = LOADER.with(|l| {
            let mut queue = l.queue.borrow_mut();
            let mut i = queue.len();
            while i > 0 {
                i -= 1;
                if !queue[i].wanted() {
                    queue.remove(i);
                    continue;
                }
                if let Some(texture) = l.cache.borrow_mut().get(&queue[i].path) {
                    if let Some(req) = queue.remove(i) {
                        req.show(&texture);
                    }
                    continue;
                }
                if !l.has_slot(&queue[i]) {
                    continue;
                }
                let req = queue.remove(i)?;
                if req.make.is_some() {
                    l.making.set(l.making.get() + 1);
                } else {
                    l.running.set(l.running.get() + 1);
                }
                return Some(req);
            }
            None
        });
        let Some(req) = next else { return };
        glib::spawn_future_local(async move {
            let path = req.path.clone();
            let make = req.make.clone();
            let result = gio::spawn_blocking(move || {
                let path = match &make {
                    Some(src) => thumbnailer::make(src)?.to_string_lossy().into_owned(),
                    None => path,
                };
                let bytes = std::fs::read(&path).ok()?;
                let texture = gdk::Texture::from_bytes(&glib::Bytes::from_owned(bytes)).ok()?;
                Some((path, texture))
            })
            .await
            .ok()
            .flatten();
            LOADER.with(|l| {
                match &req.make {
                    Some(src) => {
                        l.making.set(l.making.get() - 1);
                        if result.is_none() {
                            l.failed.borrow_mut().insert((src.uri.clone(), src.mtime));
                        }
                    }
                    None => l.running.set(l.running.get() - 1),
                }
                if let Some((_, texture)) = &result {
                    l.cache.borrow_mut().put(req.path.clone(), texture.clone());
                }
            });
            if let Some((path, texture)) = result {
                if req.make.is_some() {
                    if let Some(info) = req.expected.upgrade() {
                        info.set_attribute_byte_string("thumbnail::path", path.as_str());
                        info.set_attribute_boolean("thumbnail::is-valid", true);
                    }
                }
                req.show(&texture);
            }
            pump();
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_drops_the_least_recently_used() {
        let tex = || -> gdk::Texture {
            gdk::MemoryTexture::new(
                1,
                1,
                gdk::MemoryFormat::R8g8b8a8,
                &glib::Bytes::from_static(&[0, 0, 0, 255]),
                4,
            )
            .upcast()
        };
        let mut cache = Cache::default();
        for i in 0..CACHE {
            cache.put(format!("{i}"), tex());
        }
        // "0" used again: "1" is now the oldest.
        assert!(cache.get("0").is_some());
        cache.put("new".into(), tex());
        assert_eq!(cache.map.len(), CACHE);
        assert!(cache.get("0").is_some());
        assert!(cache.get("1").is_none());
        assert!(cache.get("new").is_some());
    }
}
