//! Song Details: a dialog with a song's tags, file properties and
//! listening statistics, and Show in Files.

use adw::prelude::*;
use gtk::{gio, glib};
use helix_app_common::i18n::{n_, tr};
use helix_app_common::{trf, trnf};
use lofty::file::FileType;
use lofty::prelude::*;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use crate::model::{format_duration, Track};
use crate::playlist::Stat;
use crate::ui::{toast, App};

/// Technical properties of an audio file (read on a worker thread).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Technical {
    pub format: Option<&'static str>,
    pub bitrate_kbps: Option<u32>,
    pub sample_rate: Option<u32>,
    pub bit_depth: Option<u8>,
    pub channels: Option<u8>,
    pub size: Option<u64>,
}

pub fn read_technical(path: &Path) -> Technical {
    let size = std::fs::metadata(path).ok().map(|m| m.len());
    let Ok(file) = lofty::read_from_path(path) else {
        return Technical {
            size,
            ..Default::default()
        };
    };
    let p = file.properties();
    Technical {
        format: format_name(file.file_type()),
        bitrate_kbps: p.audio_bitrate().or(p.overall_bitrate()).filter(|b| *b > 0),
        sample_rate: p.sample_rate().filter(|r| *r > 0),
        bit_depth: p.bit_depth().filter(|b| *b > 0),
        channels: p.channels().filter(|c| *c > 0),
        size,
    }
}

/// The usual name of a file format (names of formats are not translated).
pub fn format_name(t: FileType) -> Option<&'static str> {
    Some(match t {
        FileType::Aac => "AAC",
        FileType::Aiff => "AIFF",
        FileType::Ape => "Monkey’s Audio",
        FileType::Flac => "FLAC",
        FileType::Mpeg => "MP3",
        FileType::Mp4 => "MPEG-4 Audio",
        FileType::Mpc => "Musepack",
        FileType::Opus => "Opus",
        FileType::Vorbis => "Ogg Vorbis",
        FileType::Speex => "Speex",
        FileType::Wav => "WAV",
        FileType::WavPack => "WavPack",
        _ => return None,
    })
}

/// "320 kbit/s".
pub fn bitrate_text(kbps: u32) -> String {
    // Translators: an audio bit rate.
    trf!("{n} kbit/s", n = kbps)
}

/// "44.1 kHz", with the locale's decimal separator.
pub fn sample_rate_text(hz: u32, decimal: char) -> String {
    let khz = format!("{:.1}", hz as f64 / 1000.0);
    let khz = khz
        .strip_suffix(".0")
        .unwrap_or(&khz)
        .replace('.', &decimal.to_string());
    // Translators: an audio sample rate.
    trf!("{rate} kHz", rate = khz)
}

/// "Mono", "Stereo" or "6 channels".
pub fn channels_text(n: u8) -> String {
    match n {
        1 => tr("Mono").to_string(),
        2 => tr("Stereo").to_string(),
        n => trnf!("{n} channel", "{n} channels", n as u32),
    }
}

/// "16-bit".
pub fn bit_depth_text(bits: u8) -> String {
    // Translators: the bit depth of audio samples.
    trf!("{n}-bit", n = bits)
}

/// A group of the dialog: its title and (label, value) rows.
pub type Group = (&'static str, Vec<(&'static str, String)>);

/// What the dialog shows, without empty rows. `date` formats a time
/// stamp (seconds since the epoch) for the locale.
pub fn detail_groups(
    t: &Track,
    stat: Option<&Stat>,
    tech: &Technical,
    date: impl Fn(i64) -> String,
    decimal: char,
) -> Vec<Group> {
    let mut song = vec![(n_("Title"), t.title.clone())];
    let opt = |v: &Option<String>| v.clone().filter(|s| !s.trim().is_empty());
    for (label, value) in [
        (n_("Artist"), opt(&t.artist)),
        (n_("Album Artist"), opt(&t.album_artist)),
        (n_("Album"), opt(&t.album)),
        (n_("Genre"), opt(&t.genre)),
        (n_("Year"), t.year.map(|y| y.to_string())),
        (n_("Track"), t.track.map(|n| n.to_string())),
        (n_("Disc"), t.disc.map(|n| n.to_string())),
    ] {
        if let Some(v) = value {
            song.push((label, v));
        }
    }
    song.push((n_("Length"), format_duration(t.duration_ms)));
    let mut file = Vec::new();
    for (label, value) in [
        (n_("Format"), tech.format.map(str::to_string)),
        (n_("Bit Rate"), tech.bitrate_kbps.map(bitrate_text)),
        (
            n_("Sample Rate"),
            tech.sample_rate.map(|r| sample_rate_text(r, decimal)),
        ),
        (n_("Bit Depth"), tech.bit_depth.map(bit_depth_text)),
        (n_("Channels"), tech.channels.map(channels_text)),
        (
            n_("Size"),
            tech.size.map(|s| glib::format_size(s).to_string()),
        ),
    ] {
        if let Some(v) = value {
            file.push((label, v));
        }
    }
    file.push((n_("File Location"), t.path.display().to_string()));
    let plays = stat.map_or(0, |s| s.plays);
    let mut listening = vec![(n_("Plays"), plays.to_string())];
    if let Some(last) = stat.and_then(|s| s.last_played).filter(|l| *l > 0) {
        listening.push((n_("Last Played"), date(last)));
    }
    if t.added > 0 {
        listening.push((n_("Date Added"), date(t.added)));
    }
    vec![
        (n_("Song"), song),
        (n_("File"), file),
        (n_("Listening"), listening),
    ]
}

impl App {
    /// The Song Details dialog for track `idx`.
    pub fn show_song_details(self: &Rc<Self>, idx: usize) {
        let Some(t) = self.lib().tracks.get(idx).cloned() else {
            return;
        };
        let path = t.path.clone();
        let w = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let tech = gio::spawn_blocking(move || read_technical(&path))
                .await
                .unwrap_or_default();
            if let Some(app) = w.upgrade() {
                app.present_details(&t, &tech);
            }
        });
    }

    fn present_details(self: &Rc<Self>, t: &Track, tech: &Technical) {
        let date = |secs: i64| {
            glib::DateTime::from_unix_local(secs)
                .and_then(|d| d.format("%x"))
                .map(|s| s.to_string())
                .unwrap_or_default()
        };
        let (decimal, _) = helix_app_common::i18n::number_separators();
        let groups = detail_groups(t, self.stats.borrow().get(&t.path), tech, date, decimal);
        let page = adw::PreferencesPage::new();
        for (title, rows) in groups {
            let group = adw::PreferencesGroup::builder().title(tr(title)).build();
            for (label, value) in rows {
                let row = adw::ActionRow::builder()
                    .title(tr(label))
                    .subtitle(glib::markup_escape_text(&value))
                    .subtitle_selectable(true)
                    .css_classes(["property"])
                    .build();
                if label == "File Location" {
                    let open = gtk::Button::builder()
                        .icon_name("folder-open-symbolic")
                        .tooltip_text(tr("Show in Files"))
                        .valign(gtk::Align::Center)
                        .css_classes(["flat"])
                        .build();
                    helix_app_common::set_accessible_label(&open, tr("Show in Files"));
                    let (w, p) = (Rc::downgrade(self), t.path.clone());
                    open.connect_clicked(move |_| {
                        if let Some(app) = w.upgrade() {
                            app.show_in_files(&p);
                        }
                    });
                    row.add_suffix(&open);
                }
                group.add(&row);
            }
            page.add(&group);
        }
        let tv = adw::ToolbarView::new();
        tv.add_top_bar(&adw::HeaderBar::new());
        tv.set_content(Some(&page));
        let dialog = adw::Dialog::builder()
            .title(tr("Song Details"))
            .content_width(440)
            .content_height(600)
            .child(&tv)
            .build();
        dialog.present(Some(&self.window));
    }

    /// Opens the folder of `path` in the file manager with the file
    /// selected.
    pub fn show_in_files(self: &Rc<Self>, path: &Path) {
        let launcher = gtk::FileLauncher::new(Some(&gio::File::for_path(path)));
        let w = Rc::downgrade(self);
        let path: PathBuf = path.to_path_buf();
        launcher.open_containing_folder(Some(&self.window), gio::Cancellable::NONE, move |res| {
            let (Some(app), Err(e)) = (w.upgrade(), res) else {
                return;
            };
            if !e.matches(gtk::DialogError::Dismissed) && !e.matches(gtk::DialogError::Cancelled) {
                eprintln!("helix-music: cannot show {}: {e}", path.display());
                app.toasts.add_toast(toast(&trf!(
                    "Cannot open the folder: {e}",
                    e = helix_app_common::errors::glib_detail(&e)
                )));
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::track;

    #[test]
    fn technical_texts() {
        assert_eq!(bitrate_text(320), "320 kbit/s");
        assert_eq!(sample_rate_text(44_100, '.'), "44.1 kHz");
        assert_eq!(sample_rate_text(48_000, ','), "48 kHz");
        assert_eq!(sample_rate_text(22_050, ','), "22,1 kHz");
        assert_eq!(channels_text(1), "Mono");
        assert_eq!(channels_text(2), "Stereo");
        assert_eq!(channels_text(6), "6 channels");
        assert_eq!(bit_depth_text(24), "24-bit");
        assert_eq!(format_name(FileType::Flac), Some("FLAC"));
    }

    #[test]
    fn groups_skip_missing_values() {
        let mut t = track("/m/a.flac", "Song", Some("Artist"), None);
        t.year = Some(1999);
        t.duration_ms = 61_000;
        t.added = 100;
        let tech = Technical {
            format: Some("FLAC"),
            sample_rate: Some(96_000),
            ..Default::default()
        };
        let stat = Stat {
            plays: 3,
            last_played: Some(200),
            ..Default::default()
        };
        let g = detail_groups(&t, Some(&stat), &tech, |s| format!("@{s}"), '.');
        let labels = |i: usize| -> Vec<&str> { g[i].1.iter().map(|(l, _)| *l).collect() };
        assert_eq!(labels(0), ["Title", "Artist", "Year", "Length"]);
        assert_eq!(g[0].1[3].1, "1:01");
        assert_eq!(labels(1), ["Format", "Sample Rate", "File Location"]);
        assert_eq!(g[1].1[1].1, "96 kHz");
        assert_eq!(g[1].1[2].1, "/m/a.flac");
        assert_eq!(
            g[2].1,
            [
                ("Plays", "3".to_string()),
                ("Last Played", "@200".to_string()),
                ("Date Added", "@100".to_string())
            ]
        );
        // Never played, no stats.
        let g = detail_groups(&t, None, &Technical::default(), |_| String::new(), '.');
        assert_eq!(g[2].1.len(), 2);
    }
}
