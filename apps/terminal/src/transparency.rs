//! Background transparency setting (`org.helix.Terminal`).

use helix_app_common::Prefs;

/// Helix's own schema for settings `org.gnome.Console` lacks.
pub const SCHEMA: &str = "org.helix.Terminal";
/// GNOME Terminal profile key names, same meaning and types.
pub const KEY_ENABLED: &str = "use-transparent-background";
pub const KEY_PERCENT: &str = "background-transparency-percent";
/// Transparency used when the setting is first turned on.
pub const DEFAULT_PERCENT: i32 = 20;

/// The stored transparency state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Transparency {
    pub enabled: bool,
    /// 0 (opaque) – 100 (fully transparent).
    pub percent: i32,
}

impl Transparency {
    pub fn load(prefs: &Prefs) -> Transparency {
        Transparency {
            enabled: prefs.get(KEY_ENABLED, false),
            percent: prefs.get(KEY_PERCENT, DEFAULT_PERCENT).clamp(0, 100),
        }
    }

    pub fn set_enabled(prefs: &Prefs, enabled: bool) {
        prefs.set(KEY_ENABLED, enabled);
    }

    pub fn set_percent(prefs: &Prefs, percent: i32) {
        prefs.set(KEY_PERCENT, percent.clamp(0, 100));
    }

    /// Alpha for the terminal's default background colour.
    pub fn background_alpha(self) -> f32 {
        if self.enabled {
            1.0 - self.percent.clamp(0, 100) as f32 / 100.0
        } else {
            1.0
        }
    }

    /// Whether windows must stop painting their own opaque background.
    pub fn is_see_through(self) -> bool {
        self.background_alpha() < 1.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gtk::gio::prelude::*;

    const XML: &str = include_str!("../data/org.helix.Terminal.gschema.xml");

    #[test]
    fn alpha() {
        let t = |enabled, percent| Transparency { enabled, percent };
        assert_eq!(t(false, 20).background_alpha(), 1.0);
        assert_eq!(t(false, 100).background_alpha(), 1.0);
        assert!((t(true, 20).background_alpha() - 0.8).abs() < 1e-6);
        assert_eq!(t(true, 0).background_alpha(), 1.0);
        assert_eq!(t(true, 100).background_alpha(), 0.0);
        assert_eq!(t(true, 150).background_alpha(), 0.0);
        assert_eq!(t(true, -5).background_alpha(), 1.0);
        assert!(t(true, 20).is_see_through());
        assert!(!t(true, 0).is_see_through());
        assert!(!t(false, 50).is_see_through());
    }

    #[test]
    fn schema_matches_code() {
        assert!(XML.contains(&format!(r#"<schema id="{SCHEMA}""#)));
        assert!(XML.contains(&format!(r#"<key name="{KEY_ENABLED}" type="b">"#)));
        assert!(XML.contains(&format!(r#"<key name="{KEY_PERCENT}" type="i">"#)));
        assert!(XML.contains(&format!("<default>{DEFAULT_PERCENT}</default>")));
        assert!(XML.contains(r#"<range min="0" max="100"/>"#));
    }

    /// Without the schema, values live in memory with the documented
    /// defaults.
    #[test]
    fn memory_fallback() {
        let prefs = Prefs::new("org.helix.Terminal.DoesNotExist");
        assert_eq!(
            Transparency::load(&prefs),
            Transparency {
                enabled: false,
                percent: DEFAULT_PERCENT
            }
        );
        Transparency::set_enabled(&prefs, true);
        Transparency::set_percent(&prefs, 140);
        assert_eq!(
            Transparency::load(&prefs),
            Transparency {
                enabled: true,
                percent: 100
            }
        );
    }

    /// Read and write through the compiled bundled schema (memory backend).
    #[test]
    fn gsettings_round_trip() {
        let dir =
            std::env::temp_dir().join(format!("helix-terminal-schemas-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("org.helix.Terminal.gschema.xml"), XML).unwrap();
        let compiled = std::process::Command::new("glib-compile-schemas")
            .arg("--strict")
            .arg(&dir)
            .status();
        if !matches!(compiled, Ok(s) if s.success()) {
            eprintln!("glib-compile-schemas unavailable; skipping");
            std::fs::remove_dir_all(&dir).ok();
            return;
        }
        // The bundled schema source is looked up once per thread.
        std::env::set_var("HELIX_APPS_SCHEMA_DIR", &dir);
        std::env::set_var("GSETTINGS_BACKEND", "memory");
        let prefs = Prefs::new(SCHEMA);
        std::fs::remove_dir_all(&dir).ok();
        if helix_app_common::lookup_schema(SCHEMA).is_none() {
            eprintln!("bundled schema not picked up; skipping");
            return;
        }
        let s = prefs.gsettings().expect("schema installed");
        assert!(!s.boolean(KEY_ENABLED));
        assert_eq!(s.int(KEY_PERCENT), DEFAULT_PERCENT);
        Transparency::set_enabled(&prefs, true);
        Transparency::set_percent(&prefs, 35);
        assert!(s.boolean(KEY_ENABLED));
        assert_eq!(s.int(KEY_PERCENT), 35);
        assert_eq!(
            Transparency::load(&prefs),
            Transparency {
                enabled: true,
                percent: 35
            }
        );
    }
}
