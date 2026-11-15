//! The About window every app shows from its primary menu.
//!
//! It links to the issue tracker ("Report an Issue"), offers
//! troubleshooting information to attach to a report, and shows the
//! release notes of the current version from the suite's release history
//! (`data/releases.xml`, which `tools/metainfo/make_metainfo.py` also
//! copies into every app's AppStream metainfo).

use crate::i18n::tr;
use adw::prelude::*;
use std::cell::RefCell;

pub const WEBSITE: &str = "https://github.com/danielkrainas/helix-apps";
pub const ISSUE_URL: &str = "https://github.com/danielkrainas/helix-apps/issues";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The suite's release history, as it appears in every metainfo file.
pub const RELEASES_XML: &str = include_str!("../data/releases.xml");

const LICENSE_TEXT: &str = "To the extent possible under law, the authors have dedicated all \
copyright and related and neighboring rights to this software to the public domain worldwide. \
This software is distributed without any warranty.\n\n\
See the CC0 1.0 Universal Public Domain Dedication: \
https://creativecommons.org/publicdomain/zero/1.0/";

thread_local! {
    /// GSettings schemas the app opened through [`crate::Prefs`], with
    /// whether each was installed.
    static SCHEMAS: RefCell<Vec<(String, bool)>> = const { RefCell::new(Vec::new()) };
}

/// Records that `schema_id` was opened (for the troubleshooting text).
pub(crate) fn note_schema(schema_id: &str, installed: bool) {
    SCHEMAS.with(|s| {
        let mut s = s.borrow_mut();
        if !s.iter().any(|(id, _)| id == schema_id) {
            s.push((schema_id.to_string(), installed));
        }
    });
}

/// The release notes of `version` from [`RELEASES_XML`]: the markup inside
/// its `<description>`, which libadwaita's About window understands.
pub fn release_notes(version: &str) -> Option<String> {
    let start = RELEASES_XML.find(&format!("<release version=\"{version}\""))?;
    let rest = &RELEASES_XML[start..];
    let rest = &rest[..rest.find("</release>")?];
    let body = rest.split_once("<description>")?.1;
    let body = body.split_once("</description>")?.0;
    let lines: Vec<&str> = body
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    Some(lines.join("\n"))
}

/// Troubleshooting information: versions, session and settings, nothing
/// personal (no user names, paths or host names).
pub fn debug_info(app_name: &str, app_id: &str) -> String {
    let mut out = format!("{app_name} ({app_id}) {VERSION}\n\n");
    out.push_str(&format!(
        "GTK {}.{}.{}\nlibadwaita {}.{}.{}\n",
        gtk::major_version(),
        gtk::minor_version(),
        gtk::micro_version(),
        adw::major_version(),
        adw::minor_version(),
        adw::micro_version(),
    ));
    let display = gtk::gdk::Display::default()
        .map(|d| d.type_().name().to_string())
        .unwrap_or_else(|| "none".into());
    out.push_str(&format!("Display: {display}\n"));
    for var in ["XDG_CURRENT_DESKTOP", "XDG_SESSION_TYPE", "LANG"] {
        let value = std::env::var(var).unwrap_or_else(|_| tr("(unset)").into());
        out.push_str(&format!("{var}: {value}\n"));
    }
    let style = adw::StyleManager::default();
    out.push_str(&format!(
        "Style: {}{}\n",
        if style.is_dark() { "dark" } else { "light" },
        if style.is_high_contrast() {
            tr(", high contrast")
        } else {
            ""
        }
    ));
    if let Some(settings) = gtk::Settings::default() {
        let dpi = settings.gtk_xft_dpi();
        if dpi > 0 {
            // gtk-xft-dpi is 96 × 1024 at a text scaling factor of 1.
            out.push_str(&format!(
                "Text scaling: {:.2}\n",
                dpi as f64 / (96.0 * 1024.0)
            ));
        }
    }
    out.push_str("\nSettings schemas:\n");
    let schemas = SCHEMAS.with(|s| s.borrow().clone());
    if schemas.is_empty() {
        out.push_str("  (none)\n");
    }
    for (id, installed) in schemas {
        let how = if installed {
            "saved"
        } else {
            "not installed, settings last until the app quits"
        };
        out.push_str(&format!("  {id}: {how}\n"));
    }
    out
}

/// Shows the standard About window over `parent`.
pub fn show(parent: &impl IsA<gtk::Widget>, app_name: &str, app_id: &str, comments: &str) {
    let developers: Vec<&str> = env!("CARGO_PKG_AUTHORS")
        .split(':')
        .filter(|s| !s.is_empty())
        .collect();
    let about = adw::AboutDialog::builder()
        .application_name(crate::i18n::tr_dyn(app_name))
        .application_icon(app_id)
        .developer_name(tr("The Helix Project"))
        .version(VERSION)
        .comments(crate::i18n::tr_dyn(comments))
        .website(WEBSITE)
        .issue_url(ISSUE_URL)
        .debug_info(debug_info(app_name, app_id))
        .debug_info_filename(format!("{app_id}-troubleshooting.txt"))
        .developers(developers)
        .copyright(tr("Dedicated to the public domain (CC0 1.0)"))
        .license_type(gtk::License::Custom)
        .license(LICENSE_TEXT)
        .build();
    if let Some(notes) = release_notes(VERSION) {
        about.set_release_notes_version(VERSION);
        about.set_release_notes(&notes);
    }
    about.present(Some(parent));
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn current_release_has_notes() {
        let notes = release_notes(VERSION).expect("releases.xml lists the current version");
        assert!(notes.starts_with("<p>"), "{notes}");
        assert!(!notes.contains("<description>"));
        assert!(release_notes("0.0.0-none").is_none());
    }

    /// Every app ships metainfo with the suite's release history and a
    /// symbolic icon next to its full-colour one.
    #[test]
    fn every_app_has_metainfo_and_symbolic_icon() {
        let apps = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../apps");
        let mut checked = 0;
        for entry in std::fs::read_dir(&apps).unwrap() {
            let dir = entry.unwrap().path();
            if !dir.join("src").is_dir() {
                continue;
            }
            let data = dir.join("data");
            let desktop = std::fs::read_dir(&data)
                .unwrap()
                .filter_map(|e| e.ok())
                .map(|e| e.file_name().to_string_lossy().to_string())
                .find(|n| n.ends_with(".desktop") && !n.ends_with("-autostart.desktop"))
                .unwrap_or_else(|| panic!("{} has no .desktop file", dir.display()));
            let id = desktop.trim_end_matches(".desktop");
            let metainfo = std::fs::read_to_string(data.join(format!("{id}.metainfo.xml")))
                .unwrap_or_else(|_| panic!("{id} has no metainfo"));
            assert!(
                metainfo.contains(RELEASES_XML.trim_end()),
                "{id}: metainfo releases differ from data/releases.xml; run tools/metainfo/make_metainfo.py"
            );
            assert!(metainfo.contains(&format!("<launchable type=\"desktop-id\">{desktop}")));
            assert!(
                data.join("icons")
                    .join(format!("{id}-symbolic.svg"))
                    .is_file(),
                "{id} has no symbolic icon"
            );
            checked += 1;
        }
        assert!(checked >= 20, "only {checked} apps found");
    }
}
