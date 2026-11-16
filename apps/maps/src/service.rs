//! Geocoding / routing worker. All requests go through one thread that
//! enforces at least one second between requests (Nominatim and OSRM demo
//! usage policies) and drops requests superseded by newer ones.

use crate::geo::LatLon;
use crate::http::{self, RateLimiter};
use crate::nominatim::{self, Place};
use crate::osrm::{self, Mode, Route};
use helix_app_common::trf;
use std::sync::mpsc;
use std::time::Duration;

pub const MIN_INTERVAL: Duration = Duration::from_secs(1);

#[derive(Debug, Clone)]
pub enum Endpoint {
    Resolved(Box<Place>),
    Text(String),
}

#[derive(Debug, Clone)]
pub struct Endpoints {
    pub geocoder: String,
    pub router: String,
    pub language: String,
}

#[derive(Debug, Clone)]
pub enum Job {
    Search {
        id: u64,
        query: String,
        limit: u32,
        viewbox: Option<(f64, f64, f64, f64)>,
    },
    Reverse {
        id: u64,
        at: LatLon,
        zoom: u32,
    },
    Route {
        id: u64,
        mode: Mode,
        from: Endpoint,
        to: Endpoint,
    },
}

impl Job {
    fn slot(&self) -> u8 {
        match self {
            Job::Search { .. } => 0,
            Job::Reverse { .. } => 1,
            Job::Route { .. } => 2,
        }
    }
}

#[derive(Debug)]
pub enum Reply {
    Search {
        id: u64,
        result: Result<Vec<Place>, String>,
    },
    Reverse {
        id: u64,
        at: LatLon,
        result: Result<Option<Place>, String>,
    },
    Route {
        id: u64,
        result: Result<Box<(Route, Place, Place)>, String>,
    },
}

/// Keeps only the newest job of each kind, preserving arrival order.
pub fn collapse(jobs: Vec<(Job, Endpoints)>) -> Vec<(Job, Endpoints)> {
    let mut out: Vec<(Job, Endpoints)> = Vec::new();
    for j in jobs {
        out.retain(|(o, _)| o.slot() != j.0.slot());
        out.push(j);
    }
    out
}

/// Builds an Accept-Language header value from glib language names
/// (e.g. `["de_DE.UTF-8", "de_DE", "de", "C"]`).
pub fn accept_language(names: &[String]) -> String {
    let mut tags: Vec<String> = Vec::new();
    for n in names {
        let base = n.split(['.', '@']).next().unwrap_or("");
        if base.is_empty() || base == "C" || base == "POSIX" {
            continue;
        }
        let tag = base.replace('_', "-");
        if !tags.contains(&tag) {
            tags.push(tag);
        }
    }
    if tags.is_empty() {
        return "en".to_string();
    }
    tags.iter()
        .enumerate()
        .map(|(i, t)| {
            if i == 0 {
                t.clone()
            } else {
                format!("{t};q={:.1}", (1.0 - 0.1 * i as f64).max(0.1))
            }
        })
        .collect::<Vec<_>>()
        .join(",")
}

pub struct Service {
    tx: mpsc::Sender<(Job, Endpoints)>,
}

impl Service {
    pub fn start(reply: async_channel::Sender<Reply>) -> Service {
        let (tx, rx) = mpsc::channel::<(Job, Endpoints)>();
        std::thread::Builder::new()
            .name("maps-service".into())
            .spawn(move || worker(rx, reply))
            .expect("spawn service thread");
        Service { tx }
    }

    pub fn submit(&self, job: Job, endpoints: Endpoints) {
        let _ = self.tx.send((job, endpoints));
    }
}

fn worker(rx: mpsc::Receiver<(Job, Endpoints)>, reply: async_channel::Sender<Reply>) {
    let agent = http::agent();
    let mut limiter = RateLimiter::new(MIN_INTERVAL);
    let mut pending: Vec<(Job, Endpoints)> = Vec::new();
    loop {
        if pending.is_empty() {
            match rx.recv() {
                Ok(j) => pending.push(j),
                Err(_) => return,
            }
        }
        pending.extend(rx.try_iter());
        pending = collapse(pending);
        let (job, ep) = pending.remove(0);
        let r = run(&agent, &mut limiter, job, &ep, &rx, &mut pending);
        if let Some(r) = r {
            if reply.send_blocking(r).is_err() {
                return;
            }
        }
    }
}

/// Waits for the rate limiter; returns false if, meanwhile, a newer job of
/// the same kind arrived (the current one is then dropped).
fn wait_turn(
    limiter: &mut RateLimiter,
    slot: u8,
    rx: &mpsc::Receiver<(Job, Endpoints)>,
    pending: &mut Vec<(Job, Endpoints)>,
) -> bool {
    limiter.acquire();
    pending.extend(rx.try_iter());
    !pending.iter().any(|(j, _)| j.slot() == slot)
}

fn run(
    agent: &ureq::Agent,
    limiter: &mut RateLimiter,
    job: Job,
    ep: &Endpoints,
    rx: &mpsc::Receiver<(Job, Endpoints)>,
    pending: &mut Vec<(Job, Endpoints)>,
) -> Option<Reply> {
    let slot = job.slot();
    let lang = Some(ep.language.as_str());
    match job {
        Job::Search {
            id,
            query,
            limit,
            viewbox,
        } => {
            if !wait_turn(limiter, slot, rx, pending) {
                return None;
            }
            let url = nominatim::search_url(&ep.geocoder, &query, limit, viewbox);
            let result = http::get(agent, &url, lang)
                .and_then(|b| nominatim::parse_search(&String::from_utf8_lossy(&b)));
            Some(Reply::Search { id, result })
        }
        Job::Reverse { id, at, zoom } => {
            if !wait_turn(limiter, slot, rx, pending) {
                return None;
            }
            let url = nominatim::reverse_url(&ep.geocoder, at, zoom);
            let result = http::get(agent, &url, lang)
                .and_then(|b| nominatim::parse_reverse(&String::from_utf8_lossy(&b)));
            Some(Reply::Reverse { id, at, result })
        }
        Job::Route { id, mode, from, to } => {
            let mut resolve = |e: Endpoint, start: bool| -> Result<Option<Place>, String> {
                match e {
                    Endpoint::Resolved(p) => Ok(Some(*p)),
                    Endpoint::Text(t) => {
                        if let Some(p) = crate::geo::parse_coordinates(&t) {
                            return Ok(Some(Place::at(p, &t)));
                        }
                        if !wait_turn(limiter, slot, rx, pending) {
                            return Ok(None);
                        }
                        let url = nominatim::search_url(&ep.geocoder, &t, 1, None);
                        let places = http::get(agent, &url, lang)
                            .and_then(|b| nominatim::parse_search(&String::from_utf8_lossy(&b)))?;
                        places.into_iter().next().map(Some).ok_or_else(|| {
                            if start {
                                trf!("Could not find the start “{t}”", t = t)
                            } else {
                                trf!("Could not find the destination “{t}”", t = t)
                            }
                        })
                    }
                }
            };
            let resolved = resolve(from, true).and_then(|a| match a {
                Some(a) => Ok(resolve(to, false)?.map(|b| (a, b))),
                None => Ok(None),
            });
            let result = match resolved {
                Ok(Some((a, b))) => {
                    if wait_turn(limiter, slot, rx, pending) {
                        let url = osrm::route_url(&ep.router, mode, a.location(), b.location());
                        http::get(agent, &url, None)
                            .and_then(|body| osrm::parse_route(&String::from_utf8_lossy(&body)))
                            .map(|route| Some((route, a, b)))
                    } else {
                        Ok(None)
                    }
                }
                Ok(None) => Ok(None),
                Err(e) => Err(e),
            };
            match result {
                Ok(Some(r)) => Some(Reply::Route {
                    id,
                    result: Ok(Box::new(r)),
                }),
                Ok(None) => None,
                Err(e) => Some(Reply::Route { id, result: Err(e) }),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ep() -> Endpoints {
        Endpoints {
            geocoder: String::new(),
            router: String::new(),
            language: String::new(),
        }
    }

    #[test]
    fn collapse_keeps_newest_per_kind() {
        let jobs = vec![
            (
                Job::Search {
                    id: 1,
                    query: "a".into(),
                    limit: 1,
                    viewbox: None,
                },
                ep(),
            ),
            (
                Job::Reverse {
                    id: 2,
                    at: LatLon { lat: 0.0, lon: 0.0 },
                    zoom: 18,
                },
                ep(),
            ),
            (
                Job::Search {
                    id: 3,
                    query: "b".into(),
                    limit: 1,
                    viewbox: None,
                },
                ep(),
            ),
        ];
        let ids: Vec<u64> = collapse(jobs)
            .into_iter()
            .map(|(j, _)| match j {
                Job::Search { id, .. } | Job::Reverse { id, .. } | Job::Route { id, .. } => id,
            })
            .collect();
        assert_eq!(ids, [2, 3]);
    }

    #[test]
    fn languages() {
        let names: Vec<String> = ["de_DE.UTF-8", "de_DE", "de", "C"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(accept_language(&names), "de-DE,de;q=0.9");
        assert_eq!(accept_language(&["C".to_string()]), "en");
    }
}
