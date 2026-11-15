//! Suite-wide checks on the apps' CSS, so ad-hoc typography does not come
//! back: text sizes come from Adwaita's typography classes (`caption`,
//! `heading`, `title-1`…, `numeric` for figures); the only custom sizes are
//! display text (clock faces, the big temperature, glyph previews…), in
//! `em` so they follow Large Text, and never smaller than `caption`.

use std::path::{Path, PathBuf};

/// Files whose CSS is not GTK's: web pages shown in WebKit, and the Text
/// Editor's user-chosen editor font.
const NOT_GTK_CSS: &[&str] = &[
    "apps/web/src/reader.rs",
    "apps/web/src/overview.rs",
    "apps/help/src/html.rs",
    "apps/text-editor/src/settings.rs",
];

/// Caption is 82 % of the body text; nothing may be smaller.
const MIN_EM: f64 = 0.82;

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// Every app source file, with its path relative to the workspace.
fn sources() -> Vec<(String, String)> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut files = Vec::new();
    rust_files(&root.join("apps"), &mut files);
    files
        .into_iter()
        .map(|p| {
            let rel = p
                .strip_prefix(&root)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            (rel, std::fs::read_to_string(&p).unwrap())
        })
        .filter(|(rel, _)| !NOT_GTK_CSS.contains(&rel.as_str()))
        .collect()
}

/// The code before the first test module (tests may contain anything).
fn non_test(text: &str) -> &str {
    text.split("#[cfg(test)]").next().unwrap_or(text)
}

/// Every app's main window asks for the suite's minimum size,
/// 360×294. (Whether its content fits at that size is checked at run
/// time: `HELIX_A11Y_CHECK=size` shrinks each main window and reports
/// what does not fit.)
#[test]
fn main_windows_ask_for_the_suite_minimum_size() {
    let mut windows = 0;
    for (rel, text) in sources() {
        // Sandbox error windows are not main windows.
        if rel.ends_with("sandbox.rs") {
            continue;
        }
        let code = non_test(&text);
        let mut rest = code;
        while let Some(i) = rest.find("ApplicationWindow::builder()") {
            rest = &rest[i + 1..];
            windows += 1;
            // The builder chain and what follows it until the window is
            // first used.
            let end = rest.find(".build();").map_or(rest.len(), |e| e + 9);
            let after = &rest[end..];
            let cut = after
                .char_indices()
                .nth(800)
                .map_or(after.len(), |(i, _)| i);
            let after = &after[..cut];
            let chain = &rest[..end];
            let width = [
                "width_request(360)",
                "width_request(helix_app_common::MIN_WIDTH)",
            ];
            let height = [
                "height_request(294)",
                "height_request(helix_app_common::MIN_HEIGHT)",
            ];
            let has = |options: &[&str]| {
                options
                    .iter()
                    .any(|o| chain.contains(o) || after.contains(o))
            };
            assert!(
                has(&width) && has(&height),
                "{rel}: a main window does not ask for 360×294"
            );
        }
    }
    assert!(windows >= 25, "only {windows} main windows found");
}

#[test]
fn no_tabular_figure_rules() {
    for (rel, text) in sources() {
        assert!(
            !non_test(&text).contains("tnum"),
            "{rel}: use the numeric style class instead of font-feature-settings: \"tnum\""
        );
    }
}

#[test]
fn font_sizes_are_relative_and_not_below_caption() {
    let mut checked = 0;
    for (rel, text) in sources() {
        for (n, line) in non_test(&text).lines().enumerate() {
            let mut rest = line;
            while let Some(i) = rest.find("font-size:") {
                rest = &rest[i + "font-size:".len()..];
                let value: String = rest
                    .trim_start()
                    .chars()
                    .take_while(|c| !matches!(c, ';' | '}' | '"' | ' '))
                    .collect();
                checked += 1;
                let em = if let Some(v) = value.strip_suffix("em") {
                    v.parse::<f64>().ok()
                } else if let Some(v) = value.strip_suffix('%') {
                    v.parse::<f64>().ok().map(|p| p / 100.0)
                } else {
                    None
                };
                match em {
                    Some(em) => assert!(
                        em >= MIN_EM,
                        "{rel}:{}: font-size {value} is smaller than caption",
                        n + 1
                    ),
                    None => panic!(
                        "{rel}:{}: font-size {value}: use a typography class, or em for display text",
                        n + 1
                    ),
                }
            }
        }
    }
    assert!(checked > 0, "no font sizes found; is the scan broken?");
}
