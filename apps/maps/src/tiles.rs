//! Raster tile data source: a `ShumateDataSource` subclass that downloads
//! tiles with the app's own User-Agent, a small LIFO download pool and an
//! on-disk cache.

use crate::http::{self, Cached, TileCache};
use crate::settings::expand_tile_url;
use gtk::{gio, glib};
use shumate::prelude::*;
use shumate::subclass::prelude::*;
use std::cell::RefCell;
use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex};
use std::time::SystemTime;

/// Concurrent downloads (the OSM tile policy asks for few connections).
const WORKERS: usize = 2;
/// Oldest queued jobs are dropped beyond this (they are off-screen by now).
const MAX_QUEUE: usize = 256;

type Reply = async_channel::Sender<Result<Vec<u8>, String>>;

struct Job {
    url: String,
    z: i32,
    x: i32,
    y: i32,
    cache: TileCache,
    stale: Option<Vec<u8>>,
    cancel: Option<gio::Cancellable>,
    reply: Reply,
}

#[derive(Clone)]
struct Pool {
    shared: Arc<(Mutex<Vec<Job>>, Condvar)>,
}

impl Pool {
    fn new() -> Pool {
        let pool = Pool { shared: Arc::new((Mutex::new(Vec::new()), Condvar::new())) };
        for i in 0..WORKERS {
            let p = pool.clone();
            std::thread::Builder::new()
                .name(format!("maps-tiles-{i}"))
                .spawn(move || p.work())
                .expect("spawn tile worker");
        }
        pool
    }

    fn push(&self, job: Job) {
        let (lock, cv) = &*self.shared;
        let mut q = lock.lock().unwrap();
        q.push(job);
        if q.len() > MAX_QUEUE {
            q.remove(0);
        }
        cv.notify_one();
    }

    fn work(&self) {
        let agent = http::agent();
        loop {
            let job = {
                let (lock, cv) = &*self.shared;
                let mut q = lock.lock().unwrap();
                loop {
                    if let Some(j) = q.pop() {
                        break j;
                    }
                    q = cv.wait(q).unwrap();
                }
            };
            if job.cancel.as_ref().is_some_and(|c| c.is_cancelled()) {
                continue;
            }
            let result = match http::get(&agent, &job.url, None) {
                Ok(data) if http::looks_like_image(&data) => {
                    job.cache.store(job.z, job.x, job.y, &data);
                    Ok(data)
                }
                Ok(_) => Err("The tile server did not return an image".to_string()),
                Err(e) => Err(e),
            };
            let result = match (result, job.stale) {
                (Err(_), Some(stale)) => Ok(stale),
                (r, _) => r,
            };
            let _ = job.reply.send_blocking(result);
        }
    }
}

mod imp {
    use super::*;
    use std::future::Future;
    use std::pin::Pin;
    use std::sync::OnceLock;

    #[derive(Default)]
    pub struct TileSource {
        pub template: RefCell<String>,
        pub cache: RefCell<Option<TileCache>>,
    }

    pub(super) fn pool() -> &'static Pool {
        static POOL: OnceLock<Pool> = OnceLock::new();
        POOL.get_or_init(Pool::new)
    }

    #[glib::object_subclass]
    impl ObjectSubclass for TileSource {
        const NAME: &'static str = "HelixMapsTileSource";
        type Type = super::TileSource;
        type ParentType = shumate::DataSource;
    }

    impl ObjectImpl for TileSource {}

    impl DataSourceImpl for TileSource {
        fn tile_data_future(
            &self,
            x: i32,
            y: i32,
            zoom_level: i32,
        ) -> Pin<Box<dyn Future<Output = Result<glib::Bytes, glib::Error>> + 'static>> {
            let obj = self.obj().clone();
            Box::pin(async move { obj.fetch(x, y, zoom_level, None).await })
        }

        fn start_request(
            &self,
            x: i32,
            y: i32,
            zoom_level: i32,
            cancellable: Option<gio::Cancellable>,
        ) -> Option<shumate::DataSourceRequest> {
            let req = shumate::DataSourceRequest::new(x, y, zoom_level);
            let obj = self.obj().clone();
            let r = req.clone();
            glib::spawn_future_local(async move {
                match obj.fetch(x, y, zoom_level, cancellable).await {
                    Ok(bytes) => r.emit_data(&bytes, true),
                    Err(e) => r.emit_error(&e),
                }
            });
            Some(req)
        }
    }
}

glib::wrapper! {
    pub struct TileSource(ObjectSubclass<imp::TileSource>) @extends shumate::DataSource;
}

pub fn cache_root() -> PathBuf {
    glib::user_cache_dir().join("helix-maps").join("tiles")
}

impl TileSource {
    pub fn new(template: &str) -> TileSource {
        let obj: TileSource = glib::Object::new();
        *obj.imp().template.borrow_mut() = template.to_string();
        *obj.imp().cache.borrow_mut() = Some(TileCache::new(&cache_root(), template));
        obj
    }

    async fn fetch(
        &self,
        x: i32,
        y: i32,
        z: i32,
        cancel: Option<gio::Cancellable>,
    ) -> Result<glib::Bytes, glib::Error> {
        let err = |kind: gio::IOErrorEnum, msg: &str| glib::Error::new(kind, msg);
        let cache = self.imp().cache.borrow().clone().expect("cache configured");
        let url = expand_tile_url(&self.imp().template.borrow(), z, x, y);

        let c = cache.clone();
        let cached = gio::spawn_blocking(move || c.lookup(z, x, y, SystemTime::now()))
            .await
            .map_err(|_| err(gio::IOErrorEnum::Failed, "cache lookup failed"))?;
        let stale = match cached {
            Cached::Fresh(data) => return Ok(glib::Bytes::from_owned(data)),
            Cached::Stale(data) => Some(data),
            Cached::Missing => None,
        };
        if cancel.as_ref().is_some_and(|c| c.is_cancelled()) {
            return Err(err(gio::IOErrorEnum::Cancelled, "cancelled"));
        }
        let (tx, rx) = async_channel::bounded(1);
        imp::pool().push(Job { url, z, x, y, cache, stale, cancel, reply: tx });
        match rx.recv().await {
            Ok(Ok(data)) => Ok(glib::Bytes::from_owned(data)),
            Ok(Err(msg)) => Err(err(gio::IOErrorEnum::Failed, &msg)),
            Err(_) => Err(err(gio::IOErrorEnum::Cancelled, "cancelled")),
        }
    }
}

/// Removes every cached tile (all templates). Blocking.
pub fn clear_cache() -> std::io::Result<()> {
    match std::fs::remove_dir_all(cache_root()) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}
