//! Camera device model and selection rules.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Location {
    Front,
    Back,
    External,
    Unknown,
}

impl Location {
    pub fn parse(s: Option<&str>) -> Location {
        match s.map(|s| s.to_ascii_lowercase()).as_deref() {
            Some("front") => Location::Front,
            Some("back") => Location::Back,
            Some("external") => Location::External,
            _ => Location::Unknown,
        }
    }
}

/// Front cameras and webcams of unknown placement face the user, so their
/// preview is mirrored.
pub fn should_mirror(loc: Location) -> bool {
    matches!(loc, Location::Front | Location::Unknown)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Provider {
    PipeWire,
    Libcamera,
    V4l2,
    Other,
}

impl Provider {
    /// Classifies a provider factory name (`pipewiredeviceprovider`) or a
    /// device type name (`GstPipeWireDevice`, `GstV4l2Device`, …).
    pub fn classify(name: &str) -> Provider {
        let n = name.to_ascii_lowercase();
        if n.contains("pipewire") {
            Provider::PipeWire
        } else if n.contains("libcamera") {
            Provider::Libcamera
        } else if n.contains("v4l2") {
            Provider::V4l2
        } else {
            Provider::Other
        }
    }
}

/// The same physical camera shows up through several providers; keep only
/// the devices of the most preferred provider that has any.
pub fn filter_by_provider<T>(devices: Vec<(Provider, T)>) -> Vec<T> {
    let Some(best) = devices.iter().map(|(p, _)| *p).min() else {
        return Vec::new();
    };
    devices
        .into_iter()
        .filter(|(p, _)| *p == best)
        .map(|(_, d)| d)
        .collect()
}

/// How the app obtains video.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceMode {
    /// Real cameras (portal or device monitor).
    Real,
    /// `videotestsrc` virtual cameras (`HELIX_CAMERA_SOURCE=test`).
    Test,
    /// Pretend there is no camera.
    None,
    /// Pretend the portal denied access.
    Denied,
}

impl SourceMode {
    pub fn parse(v: Option<&str>) -> SourceMode {
        match v {
            Some("test") => SourceMode::Test,
            Some("none") => SourceMode::None,
            Some("denied") => SourceMode::Denied,
            _ => SourceMode::Real,
        }
    }

    pub fn from_env() -> SourceMode {
        SourceMode::parse(std::env::var("HELIX_CAMERA_SOURCE").ok().as_deref())
    }
}

/// Index of the camera to open: the remembered one, else the first.
pub fn pick_index(names: &[String], remembered: Option<&str>) -> Option<usize> {
    if names.is_empty() {
        return None;
    }
    Some(
        remembered
            .and_then(|r| names.iter().position(|n| n == r))
            .unwrap_or(0),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mirror_rules() {
        assert!(should_mirror(Location::parse(Some("front"))));
        assert!(should_mirror(Location::parse(None)));
        assert!(!should_mirror(Location::parse(Some("back"))));
        assert!(!should_mirror(Location::parse(Some("External"))));
    }

    #[test]
    fn provider_dedup_prefers_pipewire() {
        let devs = vec![
            (Provider::V4l2, "v4l2-a"),
            (Provider::PipeWire, "pw-a"),
            (Provider::Libcamera, "lc-a"),
            (Provider::PipeWire, "pw-b"),
        ];
        assert_eq!(filter_by_provider(devs), vec!["pw-a", "pw-b"]);
        let devs = vec![(Provider::V4l2, "a"), (Provider::Other, "b")];
        assert_eq!(filter_by_provider(devs), vec!["a"]);
        assert!(filter_by_provider::<&str>(vec![]).is_empty());
        assert_eq!(Provider::classify("libcameraprovider"), Provider::Libcamera);
        assert_eq!(Provider::classify("GstPipeWireDevice"), Provider::PipeWire);
        assert_eq!(Provider::classify("GstV4l2Device"), Provider::V4l2);
        assert_eq!(Provider::classify("GstFooDevice"), Provider::Other);
    }

    #[test]
    fn source_mode_parsing() {
        assert_eq!(SourceMode::parse(Some("test")), SourceMode::Test);
        assert_eq!(SourceMode::parse(Some("none")), SourceMode::None);
        assert_eq!(SourceMode::parse(Some("denied")), SourceMode::Denied);
        assert_eq!(SourceMode::parse(None), SourceMode::Real);
    }

    #[test]
    fn picks_remembered_camera() {
        let names = vec!["A".to_string(), "B".to_string()];
        assert_eq!(pick_index(&names, Some("B")), Some(1));
        assert_eq!(pick_index(&names, Some("Z")), Some(0));
        assert_eq!(pick_index(&[], Some("A")), None);
    }
}
