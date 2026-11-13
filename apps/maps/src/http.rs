//! HTTP plumbing shared by the tile, geocoding and routing clients.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

/// Generic, non-identifying User-Agent required by the OSM usage policies.
pub const USER_AGENT: &str = concat!("helix-maps/", env!("CARGO_PKG_VERSION"));

const MAX_BODY: u64 = 8 * 1024 * 1024;

pub fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(10))
        .timeout(Duration::from_secs(20))
        .user_agent(USER_AGENT)
        .build()
}

/// GETs `url` and returns the body; `accept_language` is sent if given.
pub fn get(agent: &ureq::Agent, url: &str, accept_language: Option<&str>) -> Result<Vec<u8>, String> {
    let mut req = agent.get(url);
    if let Some(lang) = accept_language {
        req = req.set("Accept-Language", lang);
    }
    match req.call() {
        Ok(resp) => {
            let mut body = Vec::new();
            resp.into_reader()
                .take(MAX_BODY)
                .read_to_end(&mut body)
                .map_err(|e| format!("Network error: {e}"))?;
            Ok(body)
        }
        Err(ureq::Error::Status(code, resp)) => {
            let mut text = String::new();
            let _ = resp.into_reader().take(4096).read_to_string(&mut text);
            let detail = serde_json::from_str::<serde_json::Value>(&text)
                .ok()
                .and_then(|v| v.get("message").and_then(|m| m.as_str()).map(String::from));
            Err(match detail {
                Some(m) => format!("The server returned an error ({code}): {m}"),
                None => format!("The server returned an error ({code})"),
            })
        }
        Err(ureq::Error::Transport(t)) => Err(format!("Could not connect to the server: {t}")),
    }
}

/// Enforces a minimum interval between successive requests.
#[derive(Debug)]
pub struct RateLimiter {
    interval: Duration,
    last: Option<Instant>,
}

impl RateLimiter {
    pub fn new(interval: Duration) -> Self {
        RateLimiter { interval, last: None }
    }

    /// How long to wait at `now` before the next request may be sent.
    pub fn wait_time(&self, now: Instant) -> Duration {
        match self.last {
            Some(last) => (last + self.interval).saturating_duration_since(now),
            None => Duration::ZERO,
        }
    }

    /// Records that a request is being sent at `now`.
    pub fn mark(&mut self, now: Instant) {
        self.last = Some(now);
    }

    /// Blocks until a request may be sent and records it.
    pub fn acquire(&mut self) {
        let wait = self.wait_time(Instant::now());
        if !wait.is_zero() {
            std::thread::sleep(wait);
        }
        self.mark(Instant::now());
    }
}

/// On-disk tile cache: `<root>/<template hash>/<z>/<x>/<y>`.
#[derive(Debug, Clone)]
pub struct TileCache {
    dir: PathBuf,
}

pub const TILE_MAX_AGE: Duration = Duration::from_secs(7 * 24 * 3600);

pub enum Cached {
    Fresh(Vec<u8>),
    Stale(Vec<u8>),
    Missing,
}

impl TileCache {
    pub fn new(root: &Path, template: &str) -> Self {
        TileCache { dir: root.join(format!("{:016x}", crate::settings::fnv1a(template))) }
    }

    pub fn path(&self, z: i32, x: i32, y: i32) -> PathBuf {
        self.dir.join(z.to_string()).join(x.to_string()).join(y.to_string())
    }

    pub fn lookup(&self, z: i32, x: i32, y: i32, now: SystemTime) -> Cached {
        let path = self.path(z, x, y);
        let Ok(meta) = std::fs::metadata(&path) else { return Cached::Missing };
        let Ok(data) = std::fs::read(&path) else { return Cached::Missing };
        if data.is_empty() {
            return Cached::Missing;
        }
        let age = meta.modified().ok().and_then(|m| now.duration_since(m).ok());
        match age {
            Some(age) if age <= TILE_MAX_AGE => Cached::Fresh(data),
            _ => Cached::Stale(data),
        }
    }

    pub fn store(&self, z: i32, x: i32, y: i32, data: &[u8]) {
        let path = self.path(z, x, y);
        if let Some(parent) = path.parent() {
            if std::fs::create_dir_all(parent).is_err() {
                return;
            }
        }
        let tmp = path.with_extension("part");
        if std::fs::write(&tmp, data).is_ok() {
            let _ = std::fs::rename(&tmp, &path);
        }
    }
}

/// Recognises PNG / JPEG / WebP payloads so error pages are never cached.
pub fn looks_like_image(data: &[u8]) -> bool {
    data.starts_with(b"\x89PNG\r\n\x1a\n")
        || data.starts_with(&[0xFF, 0xD8, 0xFF])
        || (data.len() >= 12 && &data[..4] == b"RIFF" && &data[8..12] == b"WEBP")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_agent_is_generic() {
        assert!(USER_AGENT.starts_with("helix-maps/"));
        assert!(!USER_AGENT.contains('@'));
    }

    #[test]
    fn rate_limiter() {
        let t0 = Instant::now();
        let mut rl = RateLimiter::new(Duration::from_secs(1));
        assert_eq!(rl.wait_time(t0), Duration::ZERO);
        rl.mark(t0);
        assert_eq!(rl.wait_time(t0), Duration::from_secs(1));
        assert_eq!(rl.wait_time(t0 + Duration::from_millis(400)), Duration::from_millis(600));
        assert_eq!(rl.wait_time(t0 + Duration::from_secs(2)), Duration::ZERO);
    }

    #[test]
    fn cache_roundtrip() {
        let root = std::env::temp_dir().join(format!("helix-maps-cache-{}", std::process::id()));
        let cache = TileCache::new(&root, "https://t/{z}/{x}/{y}.png");
        let other = TileCache::new(&root, "https://u/{z}/{x}/{y}.png");
        assert_ne!(cache.path(1, 2, 3), other.path(1, 2, 3));
        assert!(matches!(cache.lookup(1, 2, 3, SystemTime::now()), Cached::Missing));
        cache.store(1, 2, 3, b"tile");
        assert!(matches!(cache.lookup(1, 2, 3, SystemTime::now()), Cached::Fresh(d) if d == b"tile"));
        let later = SystemTime::now() + TILE_MAX_AGE + Duration::from_secs(60);
        assert!(matches!(cache.lookup(1, 2, 3, later), Cached::Stale(_)));
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn image_sniffing() {
        assert!(looks_like_image(b"\x89PNG\r\n\x1a\n...."));
        assert!(looks_like_image(&[0xFF, 0xD8, 0xFF, 0xE0]));
        assert!(looks_like_image(b"RIFF\0\0\0\0WEBPVP8 "));
        assert!(!looks_like_image(b"<html>"));
    }
}
