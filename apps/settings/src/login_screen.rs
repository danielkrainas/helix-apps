//! "Use This Layout for the Login Screen" (Displays): the monitor layout
//! shown now, from `org.gnome.Mutter.DisplayConfig`, handed to
//! helix-desktop's `helix-login-monitors` through `pkexec`, which puts it
//! into `/etc/xdg/helix/monitors.toml` for the login screen
//! (helixdesktop/spec `components/greeter.md`, "The monitor"; the format
//! is in `contracts/dbus/org.gnome.Mutter.DisplayConfig.md`, "Remembered
//! configurations"). Blocking; worker threads only.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::bus::get;
use crate::displays::State;

/// Where helix-desktop installs the helper.
pub const HELPER: &str = "/usr/libexec/helix/helix-login-monitors";
const PKEXEC: &str = "/usr/bin/pkexec";

/// How the helper is run: `pkexec helper`, the layout on its standard
/// input.
#[derive(Debug, Clone)]
pub struct Helper {
    pkexec: PathBuf,
    helper: PathBuf,
}

/// What came of it.
#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    Done,
    /// The authentication was dismissed (pkexec's 126): nothing to say.
    Cancelled,
    Failed(String),
}

impl Helper {
    /// The installed helper, if it and pkexec are there.
    pub fn system() -> Option<Self> {
        Self::at(Path::new(PKEXEC), Path::new(HELPER))
    }

    /// `helper` run through `pkexec`, if both are files.
    pub fn at(pkexec: &Path, helper: &Path) -> Option<Self> {
        (pkexec.is_file() && helper.is_file()).then(|| Self {
            pkexec: pkexec.to_owned(),
            helper: helper.to_owned(),
        })
    }

    /// Run it with `layout` (from [`layout`]).
    pub fn run(&self, layout: &str) -> Outcome {
        let child = Command::new(&self.pkexec)
            .arg(&self.helper)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn();
        let mut child = match child {
            Ok(child) => child,
            Err(e) => return Outcome::Failed(e.to_string()),
        };
        if let Some(mut stdin) = child.stdin.take() {
            // It may exit without reading (an error is reported below).
            let _ = stdin.write_all(layout.as_bytes());
        }
        let output = match child.wait_with_output() {
            Ok(output) => output,
            Err(e) => return Outcome::Failed(e.to_string()),
        };
        match output.status.code() {
            Some(0) => Outcome::Done,
            Some(126) => Outcome::Cancelled,
            code => {
                let text = String::from_utf8_lossy(&output.stderr);
                let last = text
                    .lines()
                    .map(str::trim)
                    .rev()
                    .find(|l| !l.is_empty())
                    .map(|l| l.trim_start_matches("helix-login-monitors: ").to_owned());
                Outcome::Failed(last.unwrap_or_else(|| match code {
                    Some(c) => format!("exit status {c}"),
                    None => "killed".to_owned(),
                }))
            }
        }
    }
}

/// A TOML basic string.
fn quoted(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if c.is_control() => out.push_str(&format!("\\u{:04X}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Virtual monitors of screen casts are not part of the set of monitors.
fn is_virtual(spec: &crate::displays::Spec) -> bool {
    spec.0.starts_with("Virtual-") && spec.1 == "Helix" && spec.2 == "Virtual"
}

/// The layout shown now, as one remembered configuration for the
/// monitors connected now; `None` when there is nothing to describe.
pub fn layout(state: &State) -> Option<String> {
    let (_, monitors, logical, _) = state;
    let real: Vec<_> = monitors.iter().filter(|m| !is_virtual(&m.0)).collect();
    let mut set: Vec<String> = real
        .iter()
        .map(|(s, _, _)| format!("{}|{}|{}|{}", s.0, s.1, s.2, s.3))
        .collect();
    set.sort();
    if set.is_empty() {
        return None;
    }
    let mut out = format!("[[configuration]]\nmonitors = {}\n", quoted(&set.join(";")));
    let mut any = false;
    for (x, y, scale, transform, primary, specs, _) in logical {
        let shown: Vec<_> = specs.iter().filter(|s| !is_virtual(s)).collect();
        if shown.is_empty() {
            continue;
        }
        any = true;
        out.push_str(&format!(
            "\n[[configuration.logical]]\nx = {x}\ny = {y}\nscale = {scale:?}\ntransform = {transform}\nprimary = {primary}\n"
        ));
        for spec in shown {
            let (_, modes, props) = real.iter().find(|m| m.0 .0 == spec.0)?;
            let (_, width, height, refresh, ..) = modes
                .iter()
                .find(|m| get::<bool>(&m.6, "is-current").unwrap_or(false))?;
            let color_mode: u32 = get(props, "color-mode").unwrap_or(0);
            let underscan: bool = get(props, "is-underscanning").unwrap_or(false);
            out.push_str(&format!(
                "\n[[configuration.logical.monitors]]\nconnector = {}\nwidth = {width}\nheight = {height}\nrefresh_mhz = {}\ncolor_mode = {color_mode}\nunderscan = {underscan}\n",
                quoted(&spec.0),
                (refresh * 1000.0).round() as i64,
            ));
        }
    }
    any.then_some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bus::testbus;
    use crate::displays;
    use std::os::unix::fs::PermissionsExt;

    /// A scratch directory with a fake `pkexec` (runs its arguments, or
    /// exits `pkexec_status`) and a fake helper (copies its input to
    /// `got`, prints `message` and exits `status`).
    fn fakes(
        name: &str,
        pkexec_status: Option<i32>,
        status: i32,
        message: &str,
    ) -> (PathBuf, Helper) {
        let dir = std::env::temp_dir().join(format!(
            "helix-settings-login-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let script = |file: &str, body: String| {
            let path = dir.join(file);
            std::fs::write(&path, format!("#!/bin/sh\n{body}")).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
            path
        };
        let pkexec = script(
            "pkexec",
            match pkexec_status {
                Some(s) => format!("exit {s}\n"),
                None => format!(
                    "echo \"$@\" > '{}'\nexec \"$@\"\n",
                    dir.join("args").display()
                ),
            },
        );
        let helper = script(
            "helix-login-monitors",
            format!(
                "cat > '{}'\necho 'helix-login-monitors: {message}' >&2\nexit {status}\n",
                dir.join("got").display()
            ),
        );
        let missing = dir.join("missing");
        assert!(Helper::at(&pkexec, &missing).is_none(), "no helper: hidden");
        assert!(Helper::at(&missing, &helper).is_none(), "no pkexec: hidden");
        (dir, Helper::at(&pkexec, &helper).unwrap())
    }

    /// The fake DisplayConfig's state, as the panel reads it.
    fn state() -> Option<State> {
        let bus = testbus::private_bus()?;
        let service = testbus::connect(&bus);
        displays::tests::serve(&service, &Default::default());
        let app = testbus::connect(&bus);
        Some(displays::current_state(&app).unwrap())
    }

    const LAYOUT: &str = "[[configuration]]
monitors = \"HDMI-1|HLX|TV|0;eDP-1|HLX|Panel|0\"

[[configuration.logical]]
x = 0
y = 0
scale = 1.25
transform = 0
primary = true

[[configuration.logical.monitors]]
connector = \"eDP-1\"
width = 2560
height = 1600
refresh_mhz = 60000
color_mode = 0
underscan = false

[[configuration.logical]]
x = 2048
y = 0
scale = 1.0
transform = 0
primary = false

[[configuration.logical.monitors]]
connector = \"HDMI-1\"
width = 1920
height = 1080
refresh_mhz = 60000
color_mode = 0
underscan = false
";

    #[test]
    fn layout_from_display_config() {
        let Some(state) = state() else { return };
        assert_eq!(layout(&state).as_deref(), Some(LAYOUT));

        // Virtual monitors are left out; nothing left, no layout.
        let mut cast = state.clone();
        let spec = (
            "Virtual-1".to_owned(),
            "Helix".to_owned(),
            "Virtual".to_owned(),
            "0".to_owned(),
        );
        cast.1
            .push((spec.clone(), state.1[1].1.clone(), Default::default()));
        cast.2
            .push((2048, 1080, 1.0, 0, false, vec![spec], Default::default()));
        assert_eq!(layout(&cast).as_deref(), Some(LAYOUT));
        let mut none = state;
        none.1.clear();
        none.2.clear();
        assert_eq!(layout(&none), None);
        assert_eq!(quoted("a\"b\\c\n"), "\"a\\\"b\\\\c\\u000A\"");
    }

    #[test]
    fn runs_the_helper_through_pkexec() {
        let (dir, helper) = fakes("ok", None, 0, "updated");
        assert_eq!(helper.run(LAYOUT), Outcome::Done);
        assert_eq!(std::fs::read_to_string(dir.join("got")).unwrap(), LAYOUT);
        let args = std::fs::read_to_string(dir.join("args")).unwrap();
        assert_eq!(
            args.trim(),
            dir.join("helix-login-monitors").display().to_string(),
            "pkexec HELPER, nothing else"
        );
        let _ = std::fs::remove_dir_all(&dir);

        let (dir, helper) = fakes("bad", None, 1, "invalid configuration: two primaries");
        assert_eq!(
            helper.run(LAYOUT),
            Outcome::Failed("invalid configuration: two primaries".into())
        );
        let _ = std::fs::remove_dir_all(&dir);

        let (dir, helper) = fakes("dismissed", Some(126), 0, "");
        assert_eq!(helper.run(LAYOUT), Outcome::Cancelled);
        let _ = std::fs::remove_dir_all(&dir);

        let (dir, helper) = fakes("refused", Some(127), 0, "");
        assert_eq!(
            helper.run(LAYOUT),
            Outcome::Failed("exit status 127".into())
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
