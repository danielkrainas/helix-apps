//! Discovery of external subtitle files next to a video.

use std::path::{Path, PathBuf};

/// Subtitle extensions in priority order.
pub const EXTENSIONS: &[&str] = &["srt", "vtt", "ass", "ssa", "sub"];

fn ext_rank(path: &Path) -> Option<usize> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    EXTENSIONS.iter().position(|e| *e == ext)
}

pub fn is_subtitle_file(path: &Path) -> bool {
    ext_rank(path).is_some()
}

/// Ranks a candidate file name against the video stem: `Some((0, ext))` for
/// `stem.ext`, `Some((1, ext))` for `stem.<tag>.ext` (language tags such as
/// `movie.en.srt`), `None` otherwise. Comparison is case-insensitive.
fn candidate_rank(video_stem: &str, candidate: &Path) -> Option<(usize, usize)> {
    let ext = ext_rank(candidate)?;
    let stem = candidate.file_stem()?.to_str()?.to_lowercase();
    let video_stem = video_stem.to_lowercase();
    if stem == video_stem {
        return Some((0, ext));
    }
    let tag = stem.strip_prefix(&video_stem)?.strip_prefix('.')?;
    (!tag.is_empty() && !tag.contains('.')).then_some((1, ext))
}

/// Picks the best subtitle for `video` among `candidates` (file paths in the
/// same directory).
pub fn pick(video: &Path, candidates: &[PathBuf]) -> Option<PathBuf> {
    let video_stem = video.file_stem()?.to_str()?;
    candidates
        .iter()
        .filter(|c| c.as_path() != video)
        .filter_map(|c| candidate_rank(video_stem, c).map(|r| (r, c)))
        .min_by(|(ra, a), (rb, b)| ra.cmp(rb).then_with(|| a.file_name().cmp(&b.file_name())))
        .map(|(_, c)| c.clone())
}

/// Looks in the video's directory for a matching subtitle file.
pub fn discover(video: &Path) -> Option<PathBuf> {
    let dir = video.parent()?;
    let candidates: Vec<PathBuf> = std::fs::read_dir(dir)
        .ok()?
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().map(|t| t.is_file() || t.is_symlink()).unwrap_or(false))
        .map(|e| e.path())
        .filter(|p| is_subtitle_file(p))
        .collect();
    pick(video, &candidates)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> PathBuf {
        PathBuf::from(s)
    }

    #[test]
    fn exact_beats_tagged_and_ext_priority() {
        let v = p("/m/Movie.mkv");
        let c = vec![p("/m/Movie.en.srt"), p("/m/movie.vtt"), p("/m/Movie.srt"), p("/m/Other.srt")];
        assert_eq!(pick(&v, &c), Some(p("/m/Movie.srt")));
        let c = vec![p("/m/Movie.en.srt"), p("/m/MOVIE.VTT")];
        assert_eq!(pick(&v, &c), Some(p("/m/MOVIE.VTT")));
        let c = vec![p("/m/Movie.fr.srt"), p("/m/Movie.de.srt")];
        assert_eq!(pick(&v, &c), Some(p("/m/Movie.de.srt")));
    }

    #[test]
    fn rejects_unrelated() {
        let v = p("/m/Movie.mkv");
        let c = vec![p("/m/Movie2.srt"), p("/m/Movie.en.forced.srt"), p("/m/Movie.txt"), p("/m/Movie..srt")];
        assert_eq!(pick(&v, &c), None);
    }

    #[test]
    fn discover_in_dir() {
        let dir = tempfile::tempdir().unwrap();
        let video = dir.path().join("clip.webm");
        std::fs::write(&video, b"").unwrap();
        assert_eq!(discover(&video), None);
        std::fs::write(dir.path().join("clip.en.vtt"), b"WEBVTT").unwrap();
        std::fs::write(dir.path().join("unrelated.srt"), b"").unwrap();
        assert_eq!(discover(&video), Some(dir.path().join("clip.en.vtt")));
        std::fs::write(dir.path().join("Clip.SRT"), b"").unwrap();
        assert_eq!(discover(&video), Some(dir.path().join("Clip.SRT")));
    }
}
