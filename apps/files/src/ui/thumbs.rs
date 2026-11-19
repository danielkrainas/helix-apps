//! Cached thumbnails for the grid, loaded a few at a time.
//!
//! Thumbnails are found here, for the tiles being shown, not by asking
//! GIO for `thumbnail::path` while a folder lists: that hashes every
//! file's URI and looks in up to four cache folders for each, and was
//! more than half of a big folder's loading time (50 000 files: 916 ms
//! with it, 345 ms without; `tests/load_bench.rs`).
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

/// A request: show the thumbnail of `uri` (modified at `mtime`) in `img`
/// while `item` still shows `expected`. It is looked up in the cache
/// first; if there is none and `make` is set, one is made.
#[derive(Clone)]
struct Request {
    uri: String,
    mtime: u64,
    make: Option<Source>,
    /// Making the thumbnail (else looking it up).
    making: bool,
    img: glib::WeakRef<gtk::Image>,
    item: glib::WeakRef<gtk::ListItem>,
    expected: glib::WeakRef<gio::FileInfo>,
}

impl Request {
    /// Decoded thumbnails are kept by file and version.
    fn key(&self) -> String {
        format!("{}\n{}", self.uri, self.mtime)
    }

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
        if req.making {
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

/// Shows the thumbnail of `uri` (modified at `mtime`) in `img` (the tile
/// of `item`, now showing `expected`): now if it was decoded recently,
/// else when its turn comes, from the thumbnail cache, or made from
/// `make` if the cache has none (unless that failed before in this run).
pub fn request(
    uri: String,
    mtime: u64,
    make: Option<Source>,
    img: &gtk::Image,
    item: &gtk::ListItem,
    expected: &gio::FileInfo,
) {
    enqueue(Request {
        uri,
        mtime,
        make,
        making: false,
        img: img.downgrade(),
        item: item.downgrade(),
        expected: expected.downgrade(),
    });
}

fn enqueue(req: Request) {
    LOADER.with(|l| {
        if let Some(texture) = l.cache.borrow_mut().get(&req.key()) {
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

/// What a request's work came to.
enum Outcome {
    Texture(gdk::Texture),
    /// Not in the cache (a lookup).
    Missing,
    /// Could not be made or read.
    Failed,
}

/// Start queued work while slots are free (lookups: [`MAX_RUNNING`],
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
                if let Some(texture) = l.cache.borrow_mut().get(&queue[i].key()) {
                    if let Some(req) = queue.remove(i) {
                        req.show(&texture);
                    }
                    continue;
                }
                if !l.has_slot(&queue[i]) {
                    continue;
                }
                let req = queue.remove(i)?;
                if req.making {
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
            let (uri, mtime, making) = (req.uri.clone(), req.mtime, req.making);
            let make = req.make.clone();
            let outcome = gio::spawn_blocking(move || {
                let bytes = if making {
                    let Some(path) = make.as_ref().and_then(thumbnailer::make) else {
                        return Outcome::Failed;
                    };
                    match std::fs::read(path) {
                        Ok(b) => b,
                        Err(_) => return Outcome::Failed,
                    }
                } else {
                    match thumbnailer::lookup(&uri, mtime) {
                        Some((_, b)) => b,
                        None => return Outcome::Missing,
                    }
                };
                match gdk::Texture::from_bytes(&glib::Bytes::from_owned(bytes)) {
                    Ok(t) => Outcome::Texture(t),
                    Err(_) => Outcome::Failed,
                }
            })
            .await
            .unwrap_or(Outcome::Failed);
            let retry = LOADER.with(|l| {
                if req.making {
                    l.making.set(l.making.get() - 1);
                } else {
                    l.running.set(l.running.get() - 1);
                }
                let id = (req.uri.clone(), req.mtime);
                match &outcome {
                    Outcome::Texture(t) => {
                        l.cache.borrow_mut().put(req.key(), t.clone());
                        None
                    }
                    Outcome::Failed if req.making => {
                        l.failed.borrow_mut().insert(id);
                        None
                    }
                    // None cached (or unreadable): make one.
                    _ if !req.making && req.make.is_some() && !l.failed.borrow().contains(&id) => {
                        Some(Request {
                            making: true,
                            ..req.clone()
                        })
                    }
                    _ => None,
                }
            });
            if let Outcome::Texture(t) = &outcome {
                req.show(t);
            }
            if let Some(r) = retry {
                if r.wanted() {
                    LOADER.with(|l| l.queue.borrow_mut().push_back(r));
                }
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
