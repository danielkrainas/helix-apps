//! Lyrics: a sidecar `.lrc` file next to the song, else lyrics embedded
//! in its tags (ID3 `USLT`, Vorbis `LYRICS`, MP4 `©lyr`…). Read for the
//! playing song only, never stored in the index. LRC time tags make them
//! synced: Now Playing highlights the current line.

use lofty::prelude::*;
use std::path::Path;

#[derive(Clone, Debug, PartialEq)]
pub enum Lyrics {
    Plain(String),
    /// Lines with their start times (ms), in time order.
    Synced(Vec<(u64, String)>),
}

/// Parses one `[mm:ss]`, `[mm:ss.xx]` or `[mm:ss:xx]` time tag's inside.
fn time_tag(tag: &str) -> Option<u64> {
    let (m, rest) = tag.split_once(':')?;
    let m: u64 = m.trim().parse().ok()?;
    let (s, frac) = match rest.split_once(['.', ':']) {
        Some((s, f)) => (s, f),
        None => (rest, ""),
    };
    let s: u64 = s.trim().parse().ok()?;
    if s >= 60 {
        return None;
    }
    let frac = frac.trim();
    let ms = if frac.is_empty() {
        0
    } else {
        if !frac.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        // Hundredths usually; tenths and thousandths too.
        let digits = &frac[..frac.len().min(3)];
        let v: u64 = digits.parse().ok()?;
        v * 10u64.pow(3 - digits.len() as u32)
    };
    Some(m * 60_000 + s * 1000 + ms)
}

/// Removes enhanced-LRC word timings (`<mm:ss.xx>`).
fn strip_word_times(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(i) = rest.find('<') {
        match rest[i..].find('>') {
            Some(j) if time_tag(&rest[i + 1..i + j]).is_some() => {
                out.push_str(&rest[..i]);
                rest = &rest[i + j + 1..];
            }
            _ => {
                out.push_str(&rest[..=i]);
                rest = &rest[i + 1..];
            }
        }
    }
    out.push_str(rest);
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Reads lyrics text: LRC when it has time tags, plain text otherwise.
pub fn parse(text: &str) -> Option<Lyrics> {
    let text = text.trim_start_matches('\u{feff}');
    let mut offset: i64 = 0;
    let mut lines: Vec<(u64, String)> = Vec::new();
    let mut plain: Vec<&str> = Vec::new();
    for raw in text.lines() {
        let mut rest = raw.trim();
        let mut times = Vec::new();
        let mut tagged = false;
        while let Some(inner) = rest.strip_prefix('[') {
            let Some(end) = inner.find(']') else { break };
            let tag = &inner[..end];
            match time_tag(tag) {
                Some(t) => times.push(t),
                None => {
                    // ID tags ([ar:…], [ti:…]); [offset:+ms] moves the times.
                    let Some((key, value)) = tag.split_once(':') else {
                        break;
                    };
                    if !key.chars().all(|c| c.is_ascii_alphabetic()) {
                        break;
                    }
                    if key.eq_ignore_ascii_case("offset") {
                        offset = value.trim().parse().unwrap_or(0);
                    }
                }
            }
            tagged = true;
            rest = inner[end + 1..].trim_start();
        }
        if !times.is_empty() {
            let line = strip_word_times(rest);
            for t in times {
                lines.push((t, line.clone()));
            }
        } else if !tagged {
            plain.push(raw.trim_end());
        }
    }
    if !lines.is_empty() {
        // A positive offset makes the lyrics appear sooner.
        for (t, _) in lines.iter_mut() {
            *t = (*t as i64 - offset).max(0) as u64;
        }
        lines.sort_by_key(|(t, _)| *t);
        return Some(Lyrics::Synced(lines));
    }
    let plain = plain.join("\n").trim().to_string();
    (!plain.is_empty()).then_some(Lyrics::Plain(plain))
}

/// The line showing at `pos_ms`: the last one that has started.
/// `starts` are the lines' start times, in order.
pub fn current_line(starts: &[u64], pos_ms: u64) -> Option<usize> {
    starts.partition_point(|t| *t <= pos_ms).checked_sub(1)
}

/// The song's lyrics: `<stem>.lrc` (any case) beside it, then its tags.
pub fn find(path: &Path) -> Option<Lyrics> {
    if let (Some(dir), Some(stem)) = (path.parent(), path.file_stem()) {
        let sidecar = std::fs::read_dir(dir).ok().and_then(|rd| {
            rd.flatten().map(|e| e.path()).find(|p| {
                p.file_stem() == Some(stem)
                    && p.extension()
                        .and_then(|e| e.to_str())
                        .is_some_and(|e| e.eq_ignore_ascii_case("lrc"))
            })
        });
        if let Some(text) = sidecar.and_then(|f| std::fs::read(f).ok()) {
            if let Some(l) = parse(&String::from_utf8_lossy(&text)) {
                return Some(l);
            }
        }
    }
    let file = lofty::read_from_path(path).ok()?;
    file.tags()
        .iter()
        .find_map(|tag| tag.get_string(ItemKey::Lyrics).and_then(parse))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_lrc() {
        let lrc = "\u{feff}[ar:Someone]\n[ti:Song]\n\n[00:01.50]First line\n\
                   [00:05.00][01:05.00]Chorus\n[00:03]Second <00:03.50>word\n[bad]tag\n";
        let Some(Lyrics::Synced(lines)) = parse(lrc) else {
            panic!("not synced");
        };
        assert_eq!(
            lines,
            [
                (1500, "First line".to_string()),
                (3000, "Second word".to_string()),
                (5000, "Chorus".to_string()),
                (65_000, "Chorus".to_string()),
            ]
        );
        let starts: Vec<u64> = lines.iter().map(|(t, _)| *t).collect();
        assert_eq!(current_line(&starts, 0), None);
        assert_eq!(current_line(&starts, 1500), Some(0));
        assert_eq!(current_line(&starts, 4999), Some(1));
        assert_eq!(current_line(&starts, 99_000), Some(3));
    }

    #[test]
    fn offset_and_fractions() {
        let Some(Lyrics::Synced(lines)) =
            parse("[offset:+500]\n[00:02.1]a\n[00:03.123]b\n[00:00.20]c")
        else {
            panic!("not synced");
        };
        assert_eq!(
            lines.iter().map(|(t, _)| *t).collect::<Vec<_>>(),
            [0, 1600, 2623]
        );
    }

    #[test]
    fn plain_lyrics() {
        assert_eq!(
            parse("  Line one\nLine two\n\n"),
            Some(Lyrics::Plain("Line one\nLine two".into()))
        );
        assert_eq!(parse(" \n "), None);
        // Only ID tags: nothing to show.
        assert_eq!(parse("[ar:X]\n[ti:Y]"), None);
        // Brackets that are not tags stay in plain text.
        assert_eq!(
            parse("[Chorus]\nla la"),
            Some(Lyrics::Plain("[Chorus]\nla la".into()))
        );
    }

    #[test]
    fn sidecar_file_first() {
        let dir = tempfile::tempdir().unwrap();
        let song = dir.path().join("Song.ogg");
        std::fs::write(&song, b"not audio").unwrap();
        assert_eq!(find(&song), None);
        std::fs::write(dir.path().join("Song.LRC"), "[00:01.00]Hello").unwrap();
        assert_eq!(
            find(&song),
            Some(Lyrics::Synced(vec![(1000, "Hello".into())]))
        );
    }
}
