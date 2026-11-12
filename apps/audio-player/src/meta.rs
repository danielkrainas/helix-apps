//! Track metadata gathered from tags, with file-name fallbacks.

#[derive(Debug, Clone, Default, PartialEq)]
pub struct TrackInfo {
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    /// Encoded cover image (PNG/JPEG/…) from the `image` tag.
    pub cover: Option<Vec<u8>>,
}

fn clean(s: Option<String>) -> Option<String> {
    s.map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

impl TrackInfo {
    pub fn new(
        title: Option<String>,
        artist: Option<String>,
        album: Option<String>,
        cover: Option<Vec<u8>>,
    ) -> Self {
        TrackInfo {
            title: clean(title),
            artist: clean(artist),
            album: clean(album),
            cover,
        }
    }

    /// Fills fields that are still empty from `other`. Returns whether
    /// anything changed.
    pub fn merge(&mut self, other: TrackInfo) -> bool {
        let mut changed = false;
        macro_rules! fill {
            ($f:ident) => {
                if self.$f.is_none() && other.$f.is_some() {
                    self.$f = other.$f;
                    changed = true;
                }
            };
        }
        fill!(title);
        fill!(artist);
        fill!(album);
        fill!(cover);
        changed
    }

    /// Tag title or the file name without extension.
    pub fn display_title(&self, uri: &str) -> String {
        self.title.clone().unwrap_or_else(|| title_from_uri(uri))
    }

    /// "artist — album" with whichever parts are known.
    pub fn subtitle(&self) -> Option<String> {
        match (&self.artist, &self.album) {
            (Some(a), Some(b)) => Some(format!("{a} \u{2014} {b}")),
            (Some(a), None) => Some(a.clone()),
            (None, Some(b)) => Some(b.clone()),
            (None, None) => None,
        }
    }
}

/// Decoded last path segment of `uri` without its extension.
pub fn title_from_uri(uri: &str) -> String {
    let path = uri.split(['?', '#']).next().unwrap_or(uri);
    let last = path
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or(path);
    let name = gst::glib::Uri::unescape_string(last, None::<&str>)
        .map(|s| s.to_string())
        .unwrap_or_else(|| last.to_string());
    let stem = match name.rfind('.') {
        Some(i) if i > 0 => &name[..i],
        _ => &name,
    };
    if stem.is_empty() {
        uri.to_string()
    } else {
        stem.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filename_fallback() {
        assert_eq!(title_from_uri("file:///home/a/My%20Song.flac"), "My Song");
        assert_eq!(title_from_uri("file:///home/a/.hidden"), ".hidden");
        assert_eq!(title_from_uri("file:///a/b/track.01.mp3"), "track.01");
        assert_eq!(title_from_uri("file:///a/b/noext"), "noext");
        assert_eq!(title_from_uri("https://x/y/z.ogg?a=1"), "z");
    }

    #[test]
    fn merge_and_display() {
        let mut info = TrackInfo::new(None, Some(" Band ".into()), Some("".into()), None);
        assert_eq!(info.artist.as_deref(), Some("Band"));
        assert_eq!(info.album, None);
        assert_eq!(info.display_title("file:///x/y.ogg"), "y");
        assert_eq!(info.subtitle().as_deref(), Some("Band"));
        let changed = info.merge(TrackInfo::new(
            Some("T".into()),
            Some("Other".into()),
            Some("LP".into()),
            None,
        ));
        assert!(changed);
        assert_eq!(info.title.as_deref(), Some("T"));
        assert_eq!(info.artist.as_deref(), Some("Band"), "first value wins");
        assert_eq!(info.subtitle().as_deref(), Some("Band \u{2014} LP"));
        assert!(!info.merge(TrackInfo::default()));
    }
}
