//! A connection's IP settings as the editor shows them, and back
//! (NetworkManager's `ipv4` / `ipv6` settings groups). Pure logic.

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use zbus::zvariant::OwnedValue;

use crate::bus::get;
use crate::nm::val;

/// How a connection gets its addresses (`method`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    Auto,
    Manual,
    LinkLocal,
    Disabled,
    /// Sharing this computer's connection (hotspots); kept as it is.
    Shared,
}

impl Method {
    pub const CHOICES: [Method; 4] = [
        Method::Auto,
        Method::Manual,
        Method::LinkLocal,
        Method::Disabled,
    ];

    pub fn nick(self) -> &'static str {
        match self {
            Method::Auto => "auto",
            Method::Manual => "manual",
            Method::LinkLocal => "link-local",
            Method::Disabled => "disabled",
            Method::Shared => "shared",
        }
    }

    pub fn from_nick(nick: &str) -> Method {
        match nick {
            "manual" => Method::Manual,
            "link-local" => Method::LinkLocal,
            "disabled" | "ignore" => Method::Disabled,
            "shared" => Method::Shared,
            // "auto", "dhcp" (IPv6 DHCP only) and unknown ones.
            _ => Method::Auto,
        }
    }
}

/// One family's settings, as text fields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IpEdit {
    pub method: Method,
    /// `address/prefix`, comma separated.
    pub addresses: String,
    pub gateway: String,
    /// Comma separated.
    pub dns: String,
    pub auto_dns: bool,
    /// `destination/prefix gateway`, comma separated.
    pub routes: String,
    /// Only for resources on this network (never the default route).
    pub never_default: bool,
}

type Group = HashMap<String, OwnedValue>;

/// The fields of an `ipv4` / `ipv6` group (a missing group is automatic).
pub fn read(group: Option<&Group>, v6: bool) -> IpEdit {
    let empty = Group::new();
    let g = group.unwrap_or(&empty);
    let method = Method::from_nick(&get::<String>(g, "method").unwrap_or_default());
    let addresses: Vec<String> = get::<Vec<HashMap<String, OwnedValue>>>(g, "address-data")
        .unwrap_or_default()
        .iter()
        .filter_map(|a| {
            let address: String = get(a, "address")?;
            let prefix: u32 = get(a, "prefix")?;
            Some(format!("{address}/{prefix}"))
        })
        .collect();
    let dns: Vec<String> = if v6 {
        get::<Vec<Vec<u8>>>(g, "dns")
            .unwrap_or_default()
            .iter()
            .filter_map(|b| <[u8; 16]>::try_from(b.as_slice()).ok())
            .map(|b| Ipv6Addr::from(b).to_string())
            .collect()
    } else {
        get::<Vec<u32>>(g, "dns")
            .unwrap_or_default()
            .iter()
            .map(|n| Ipv4Addr::from(n.to_ne_bytes()).to_string())
            .collect()
    };
    let routes: Vec<String> = get::<Vec<HashMap<String, OwnedValue>>>(g, "route-data")
        .unwrap_or_default()
        .iter()
        .filter_map(|r| {
            let dest: String = get(r, "dest")?;
            let prefix: u32 = get(r, "prefix")?;
            Some(match get::<String>(r, "next-hop") {
                Some(hop) if !hop.is_empty() => format!("{dest}/{prefix} {hop}"),
                _ => format!("{dest}/{prefix}"),
            })
        })
        .collect();
    IpEdit {
        method,
        addresses: addresses.join(", "),
        gateway: get(g, "gateway").unwrap_or_default(),
        dns: dns.join(", "),
        auto_dns: !get::<bool>(g, "ignore-auto-dns").unwrap_or(false),
        routes: routes.join(", "),
        never_default: get(g, "never-default").unwrap_or(false),
    }
}

fn items(text: &str) -> impl Iterator<Item = &str> {
    text.split([',', ';', '\n'])
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

fn addr(s: &str, v6: bool) -> Option<IpAddr> {
    let a: IpAddr = s.trim().parse().ok()?;
    (a.is_ipv6() == v6).then_some(a)
}

/// `address[/prefix]`; without a prefix, /24 (IPv4) or /64 (IPv6).
pub fn parse_cidr(s: &str, v6: bool) -> Option<(IpAddr, u32)> {
    let (a, p) = match s.split_once('/') {
        Some((a, p)) => (a, Some(p.trim().parse::<u32>().ok()?)),
        None => (s, None),
    };
    let a = addr(a, v6)?;
    let max = if v6 { 128 } else { 32 };
    let p = p.unwrap_or(if v6 { 64 } else { 24 });
    (p <= max).then_some((a, p))
}

/// Which field is wrong.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    Addresses,
    Gateway,
    Dns,
    Routes,
}

/// Checks the fields; the first wrong one.
pub fn check(e: &IpEdit, v6: bool) -> Result<(), Field> {
    if e.method == Method::Manual {
        let addresses: Vec<&str> = items(&e.addresses).collect();
        if addresses.is_empty() || addresses.iter().any(|a| parse_cidr(a, v6).is_none()) {
            return Err(Field::Addresses);
        }
        if !e.gateway.trim().is_empty() && addr(&e.gateway, v6).is_none() {
            return Err(Field::Gateway);
        }
    }
    if items(&e.dns).any(|d| addr(d, v6).is_none()) {
        return Err(Field::Dns);
    }
    for r in items(&e.routes) {
        let mut parts = r.split_whitespace().filter(|p| *p != "via");
        let dest_ok = parts.next().is_some_and(|d| parse_cidr(d, v6).is_some());
        let hop_ok = parts.next().is_none_or(|h| addr(h, v6).is_some());
        if !dest_ok || !hop_ok || parts.next().is_some() {
            return Err(Field::Routes);
        }
    }
    Ok(())
}

/// The group with the fields applied (other properties are kept, the
/// deprecated `addresses` / `routes` forms dropped). Call [`check`] first.
pub fn write(group: Option<&Group>, e: &IpEdit, v6: bool) -> Group {
    let mut g = group.cloned().unwrap_or_default();
    for k in [
        "addresses",
        "routes",
        "address-data",
        "route-data",
        "gateway",
        "dns",
    ] {
        g.remove(k);
    }
    if e.method != Method::Shared {
        g.insert("method".into(), val(e.method.nick()));
    }
    if e.method == Method::Manual {
        let data: Vec<HashMap<String, OwnedValue>> = items(&e.addresses)
            .filter_map(|a| parse_cidr(a, v6))
            .map(|(a, p)| {
                HashMap::from([
                    ("address".to_owned(), val(a.to_string())),
                    ("prefix".to_owned(), val(p)),
                ])
            })
            .collect();
        g.insert("address-data".into(), val(data));
        if let Some(gw) = addr(&e.gateway, v6) {
            g.insert("gateway".into(), val(gw.to_string()));
        }
    }
    let dns: Vec<IpAddr> = items(&e.dns).filter_map(|d| addr(d, v6)).collect();
    if v6 {
        let bytes: Vec<Vec<u8>> = dns
            .iter()
            .filter_map(|a| match a {
                IpAddr::V6(a) => Some(a.octets().to_vec()),
                IpAddr::V4(_) => None,
            })
            .collect();
        g.insert("dns".into(), val(bytes));
    } else {
        let nums: Vec<u32> = dns
            .iter()
            .filter_map(|a| match a {
                IpAddr::V4(a) => Some(u32::from_ne_bytes(a.octets())),
                IpAddr::V6(_) => None,
            })
            .collect();
        g.insert("dns".into(), val(nums));
    }
    g.insert("ignore-auto-dns".into(), val(!e.auto_dns));
    g.insert("never-default".into(), val(e.never_default));
    let routes: Vec<HashMap<String, OwnedValue>> = items(&e.routes)
        .filter_map(|r| {
            let mut parts = r.split_whitespace().filter(|p| *p != "via");
            let (dest, prefix) = parse_cidr(parts.next()?, v6)?;
            let mut m = HashMap::from([
                ("dest".to_owned(), val(dest.to_string())),
                ("prefix".to_owned(), val(prefix)),
            ]);
            if let Some(hop) = parts.next().and_then(|h| addr(h, v6)) {
                m.insert("next-hop".into(), val(hop.to_string()));
            }
            Some(m)
        })
        .collect();
    g.insert("route-data".into(), val(routes));
    g
}

#[cfg(test)]
mod tests {
    use super::*;

    fn edit(method: Method, addresses: &str, gateway: &str, dns: &str, routes: &str) -> IpEdit {
        IpEdit {
            method,
            addresses: addresses.into(),
            gateway: gateway.into(),
            dns: dns.into(),
            auto_dns: dns.is_empty(),
            routes: routes.into(),
            never_default: false,
        }
    }

    #[test]
    fn round_trip_v4() {
        let e = edit(
            Method::Manual,
            "192.168.1.5/24, 10.0.0.2",
            "192.168.1.1",
            "1.1.1.1, 9.9.9.9",
            "10.8.0.0/16 via 192.168.1.254",
        );
        assert_eq!(check(&e, false), Ok(()));
        let mut old = Group::new();
        old.insert("may-fail".into(), val(false));
        old.insert("addresses".into(), val(vec![vec![1u32, 2, 3]]));
        let g = write(Some(&old), &e, false);
        assert!(g.contains_key("may-fail"), "other properties kept");
        assert!(
            !g.contains_key("addresses"),
            "the deprecated form is dropped"
        );
        let back = read(Some(&g), false);
        assert_eq!(back.addresses, "192.168.1.5/24, 10.0.0.2/24");
        assert_eq!(back.dns, "1.1.1.1, 9.9.9.9");
        assert_eq!(back.routes, "10.8.0.0/16 192.168.1.254");
        assert_eq!(back.gateway, "192.168.1.1");
        assert!(!back.auto_dns);
        // DNS is stored in network order.
        let dns: Vec<u32> = get(&g, "dns").unwrap();
        assert_eq!(dns[0].to_ne_bytes(), [1, 1, 1, 1]);
    }

    #[test]
    fn round_trip_v6_and_auto() {
        let e = edit(Method::Auto, "", "", "2606:4700::1111", "");
        assert_eq!(check(&e, true), Ok(()));
        let g = write(None, &e, true);
        let back = read(Some(&g), true);
        assert_eq!(
            (back.method, back.dns.as_str()),
            (Method::Auto, "2606:4700::1111")
        );
        assert!(!g.contains_key("address-data"));
        assert_eq!(read(None, true).method, Method::Auto);
        assert_eq!(Method::from_nick("ignore"), Method::Disabled);
    }

    #[test]
    fn bad_fields() {
        let bad = |e: IpEdit, v6| check(&e, v6).unwrap_err();
        assert_eq!(
            bad(edit(Method::Manual, "", "", "", ""), false),
            Field::Addresses
        );
        assert_eq!(
            bad(edit(Method::Manual, "300.1.1.1", "", "", ""), false),
            Field::Addresses
        );
        assert_eq!(
            bad(edit(Method::Manual, "10.0.0.1/33", "", "", ""), false),
            Field::Addresses
        );
        assert_eq!(
            bad(edit(Method::Manual, "fe80::1", "", "", ""), false),
            Field::Addresses
        );
        assert_eq!(
            bad(edit(Method::Manual, "10.0.0.1", "x", "", ""), false),
            Field::Gateway
        );
        assert_eq!(
            bad(edit(Method::Auto, "", "", "1.1.1", ""), false),
            Field::Dns
        );
        assert_eq!(
            bad(edit(Method::Auto, "", "", "", "10.0.0.0/8 x"), false),
            Field::Routes
        );
        // Addresses are not looked at unless manual.
        assert_eq!(
            check(&edit(Method::Auto, "junk", "junk", "", ""), false),
            Ok(())
        );
    }
}
