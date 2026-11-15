//! WebKit sandbox pre-flight check and the friendly failure page.
//!
//! WebKitGTK sandboxes web content with bubblewrap, which needs
//! unprivileged user namespaces. When the kernel or AppArmor forbids them
//! (`kernel.apparmor_restrict_unprivileged_userns = 1` on Ubuntu 24.04+),
//! WebKit aborts the whole UI process the moment the first web process is
//! launched. So before creating any WebView, the app re-executes itself with
//! [`PROBE_ARG`]; that child (running under the same AppArmor profile as the
//! app would) tries to create a user namespace the way bubblewrap does.
//! If it fails, the app shows [`failure_window`] instead of browsing.
//! The sandbox itself is never disabled or weakened.

use adw::prelude::*;
use gtk::glib;
use helix_app_common::i18n::tr;
use helix_app_common::trf;
use std::path::Path;
use std::process::{Command, Stdio};

/// Hidden argument: run the user-namespace probe and exit.
pub const PROBE_ARG: &str = "--helix-probe-userns";

const RESTRICT_SYSCTL: &str = "/proc/sys/kernel/apparmor_restrict_unprivileged_userns";
const CLONE_SYSCTL: &str = "/proc/sys/kernel/unprivileged_userns_clone";
const MAX_USERNS: &str = "/proc/sys/user/max_user_namespaces";

/// Why the sandbox cannot start.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cause {
    /// AppArmor restricts unprivileged user namespaces and no profile
    /// grants `userns` to this binary.
    AppArmor,
    /// User namespaces are disabled by sysctl.
    Disabled,
    /// The probe failed for another reason.
    Unknown,
}

/// Runs the probe when invoked with [`PROBE_ARG`]; returns its exit code.
/// Must be called first thing in `main`, while the process is still
/// single-threaded (a requirement of `unshare(CLONE_NEWUSER)`).
pub fn probe_if_requested() -> Option<i32> {
    (std::env::args().nth(1).as_deref() == Some(PROBE_ARG)).then(probe_userns)
}

fn probe_userns() -> i32 {
    // SAFETY: plain syscalls in a fresh single-threaded process.
    let (uid, gid) = unsafe { (libc::getuid(), libc::getgid()) };
    if unsafe { libc::unshare(libc::CLONE_NEWUSER) } != 0 {
        return 1;
    }
    let _ = std::fs::write("/proc/self/setgroups", "deny");
    if std::fs::write("/proc/self/uid_map", format!("{uid} {uid} 1")).is_err() {
        return 2;
    }
    if std::fs::write("/proc/self/gid_map", format!("{gid} {gid} 1")).is_err() {
        return 3;
    }
    0
}

fn read_flag(path: &str) -> Option<u64> {
    std::fs::read_to_string(path).ok()?.trim().parse().ok()
}

/// Decides the cause from the probe result and the relevant sysctls.
pub fn diagnose(
    probe_ok: bool,
    apparmor_restrict: Option<u64>,
    userns_clone: Option<u64>,
    max_userns: Option<u64>,
) -> Option<Cause> {
    if probe_ok {
        return None;
    }
    Some(if userns_clone == Some(0) || max_userns == Some(0) {
        Cause::Disabled
    } else if apparmor_restrict == Some(1) {
        Cause::AppArmor
    } else {
        Cause::Unknown
    })
}

/// Checks whether WebKit's sandbox can start. `None` means it can (or the
/// check does not apply, e.g. inside Flatpak where WebKit uses the Flatpak
/// sandbox instead).
pub fn check() -> Option<Cause> {
    if Path::new("/.flatpak-info").exists() {
        return None;
    }
    let exe = std::env::current_exe().ok()?;
    let probe_ok = Command::new(exe)
        .arg(PROBE_ARG)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        // If we cannot even run the probe, let WebKit try.
        .unwrap_or(true);
    diagnose(
        probe_ok,
        read_flag(RESTRICT_SYSCTL),
        read_flag(CLONE_SYSCTL),
        read_flag(MAX_USERNS),
    )
}

/// Standard locations the packaged profile attaches to.
pub fn default_attachments(binary: &str) -> Vec<String> {
    vec![
        format!("/usr/bin/{binary}"),
        format!("/usr/local/bin/{binary}"),
    ]
}

fn safe_profile_path(p: &str) -> bool {
    p.starts_with('/')
        && !p.chars().any(|c| {
            c.is_control()
                || c.is_whitespace()
                || matches!(
                    c,
                    '"' | '{' | '}' | '[' | ']' | '*' | '?' | ',' | '^' | '\\' | '#'
                )
        })
}

/// AppArmor profile text granting `userns` to `binary` at `paths`.
/// Unusual paths (globbing characters) are skipped.
pub fn profile_text(binary: &str, paths: &[String]) -> String {
    let paths: Vec<&String> = paths.iter().filter(|p| safe_profile_path(p)).collect();
    // An attachment must start with '/', so alternatives go after it.
    let attach = match paths.as_slice() {
        [one] => one.to_string(),
        many => format!(
            "/{{{}}}",
            many.iter()
                .map(|s| s.trim_start_matches('/'))
                .collect::<Vec<_>>()
                .join(",")
        ),
    };
    format!(
        "# AppArmor profile for {binary}.\n\
         #\n\
         # WebKitGTK sandboxes web content with bubblewrap, which needs\n\
         # unprivileged user namespaces. Systems that restrict them\n\
         # (kernel.apparmor_restrict_unprivileged_userns = 1) must grant\n\
         # `userns` to the application. WebKit's helper processes\n\
         # (WebKitWebProcess, WebKitNetworkProcess) and bwrap are started by\n\
         # {binary} and inherit this profile. The profile confines nothing\n\
         # else; it only names the application and allows user namespaces.\n\
         \n\
         abi <abi/4.0>,\n\
         include <tunables/global>\n\
         \n\
         profile {binary} {attach} flags=(unconfined) {{\n\
         \x20 userns,\n\
         \n\
         \x20 # Site-specific additions and overrides. See local/README for details.\n\
         \x20 include if exists <local/{binary}>\n\
         }}\n"
    )
}

/// Shell commands that install and load `profile_path`.
pub fn fix_commands(binary: &str, profile_installed: bool) -> String {
    let target = format!("/etc/apparmor.d/{binary}");
    if profile_installed {
        format!("sudo apparmor_parser -r {target}")
    } else {
        format!(
            "sudo tee {target} < {binary}.apparmor >/dev/null\nsudo apparmor_parser -r {target}"
        )
    }
}

/// Body text for the status page.
pub fn explanation(app_name: &str, cause: Cause, exe: &str, profile_installed: bool) -> String {
    let lead = trf!("{app_name} shows help pages only inside WebKit’s sandbox, which needs unprivileged user namespaces.", app_name = app_name);
    let rest = match cause {
        Cause::AppArmor if profile_installed => trf!(
            "This system’s AppArmor policy restricts them (kernel.apparmor_restrict_unprivileged_userns = 1). \
             An AppArmor profile for {app_name} is installed but does not apply to this program ({exe}): \
             load it, or add this path to it, then start {app_name} again.",
            app_name = app_name,
            exe = exe
        ),
        Cause::AppArmor => trf!(
            "This system’s AppArmor policy restricts them (kernel.apparmor_restrict_unprivileged_userns = 1), \
             and no AppArmor profile grants them to {app_name}. Save the profile below as a file, install it \
             with the commands shown, then start {app_name} again.",
            app_name = app_name
        ),
        Cause::Disabled => trf!(
            "They are disabled on this system (kernel.unprivileged_userns_clone or \
             user.max_user_namespaces is 0). Ask your administrator to enable them, then start {app_name} again.",
            app_name = app_name
        ),
        Cause::Unknown => trf!(
            "Creating one failed on this system, so help pages cannot be shown safely. \
             Check your security policy (AppArmor, SELinux, seccomp), then start {app_name} again.",
            app_name = app_name
        ),
    };
    // Translators: two sentences of the sandbox explanation; swap them if your language needs it.
    trf!("{lead} {rest}", lead = lead, rest = rest)
}

fn mono_box(text: &str) -> gtk::Widget {
    let label = gtk::Label::builder()
        .label(text)
        .selectable(true)
        .wrap(true)
        .wrap_mode(gtk::pango::WrapMode::WordChar)
        .xalign(0.0)
        .margin_start(12)
        .margin_end(12)
        .margin_top(8)
        .margin_bottom(8)
        .build();
    label.add_css_class("monospace");
    let frame = gtk::Frame::new(None);
    frame.add_css_class("view");
    frame.set_child(Some(&label));
    frame.upcast()
}

fn copy_button(label: &str, text: String) -> gtk::Button {
    let b = gtk::Button::with_label(label);
    b.add_css_class("pill");
    b.connect_clicked(move |b| {
        b.clipboard().set_text(&text);
        b.set_label(tr("Copied"));
    });
    b
}

/// A window explaining why web content cannot be shown and how to fix it.
pub fn failure_window(
    app: &adw::Application,
    app_name: &str,
    binary: &str,
    cause: Cause,
) -> adw::ApplicationWindow {
    let exe = std::env::current_exe()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| format!("/usr/bin/{binary}"));
    let installed = Path::new("/etc/apparmor.d").join(binary).exists();
    let page = adw::StatusPage::builder()
        .icon_name("computer-fail-symbolic")
        .title(tr("Help Content Sandbox Unavailable"))
        .description(
            glib::markup_escape_text(&explanation(app_name, cause, &exe, installed)).as_str(),
        )
        .build();

    let content = gtk::Box::new(gtk::Orientation::Vertical, 12);
    if cause == Cause::AppArmor {
        let mut paths = default_attachments(binary);
        if !paths.contains(&exe) {
            paths.push(exe.clone());
        }
        let profile = profile_text(binary, &paths);
        if !installed {
            let heading = gtk::Label::builder()
                .label(trf!("Profile ({binary}.apparmor)", binary = binary))
                .xalign(0.0)
                .build();
            heading.add_css_class("heading");
            content.append(&heading);
            let expander = gtk::Expander::builder()
                .label(tr("Show profile"))
                .child(&mono_box(&profile))
                .build();
            content.append(&expander);
        }
        let heading = gtk::Label::builder()
            .label(tr("Commands"))
            .xalign(0.0)
            .build();
        heading.add_css_class("heading");
        content.append(&heading);
        let commands = fix_commands(binary, installed);
        content.append(&mono_box(&commands));
        let buttons = gtk::Box::builder()
            .spacing(12)
            .halign(gtk::Align::Center)
            .margin_top(6)
            .build();
        if !installed {
            buttons.append(&copy_button(tr("Copy Profile"), profile));
        }
        buttons.append(&copy_button(tr("Copy Commands"), commands));
        content.append(&buttons);
    }
    let quit = gtk::Button::builder()
        .label(tr("_Quit"))
        .use_underline(true)
        .halign(gtk::Align::Center)
        .margin_top(12)
        .build();
    quit.add_css_class("pill");
    quit.add_css_class("suggested-action");
    let a = app.clone();
    quit.connect_clicked(move |_| a.quit());
    content.append(&quit);
    let clamp = adw::Clamp::builder()
        .maximum_size(560)
        .child(&content)
        .build();
    page.set_child(Some(&clamp));

    let scroller = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .child(&page)
        .build();
    let view = adw::ToolbarView::new();
    view.add_top_bar(&adw::HeaderBar::new());
    view.set_content(Some(&scroller));
    adw::ApplicationWindow::builder()
        .application(app)
        .title(app_name)
        .default_width(680)
        .default_height(640)
        .content(&view)
        .build()
}

/// A page shown inside a view whose web process terminated.
pub fn crashed_page(reason: webkit6::WebProcessTerminationReason) -> adw::StatusPage {
    let description = match reason {
        webkit6::WebProcessTerminationReason::ExceededMemoryLimit => {
            tr("The page used too much memory and was stopped.")
        }
        _ => tr("The process showing this page stopped unexpectedly."),
    };
    adw::StatusPage::builder()
        .icon_name("computer-fail-symbolic")
        .title(tr("Page Crashed"))
        .description(description)
        .build()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diagnoses() {
        assert_eq!(diagnose(true, Some(1), None, None), None);
        assert_eq!(
            diagnose(false, Some(1), None, Some(1000)),
            Some(Cause::AppArmor)
        );
        assert_eq!(
            diagnose(false, Some(1), Some(0), None),
            Some(Cause::Disabled)
        );
        assert_eq!(diagnose(false, None, None, Some(0)), Some(Cause::Disabled));
        assert_eq!(
            diagnose(false, Some(0), Some(1), Some(10)),
            Some(Cause::Unknown)
        );
        assert_eq!(diagnose(false, None, None, None), Some(Cause::Unknown));
    }

    #[test]
    fn shipped_profile_matches_generator() {
        let shipped = include_str!("../data/apparmor.d/helix-help");
        assert_eq!(
            shipped,
            profile_text("helix-help", &default_attachments("helix-help"))
        );
    }

    #[test]
    fn profile_paths() {
        let p = profile_text("helix-help", &["/opt/x/helix-help".into()]);
        assert!(p.contains("profile helix-help /opt/x/helix-help flags=(unconfined) {"));
        assert!(p.contains("  userns,"));
        let p = profile_text(
            "helix-help",
            &[
                "/usr/bin/helix-help".into(),
                "/tmp/a*b/helix-help".into(),
                "/home/u/t/helix-help".into(),
            ],
        );
        assert!(p.contains(" /{usr/bin/helix-help,home/u/t/helix-help} "));
        assert!(!p.contains("a*b"));
    }

    #[test]
    fn texts() {
        assert!(fix_commands("helix-help", true)
            .starts_with("sudo apparmor_parser -r /etc/apparmor.d/helix-help"));
        assert!(fix_commands("helix-help", false).contains("sudo tee /etc/apparmor.d/helix-help"));
        let t = explanation("Help", Cause::AppArmor, "/x", false);
        assert!(t.contains("apparmor_restrict_unprivileged_userns"));
        assert!(
            explanation("Help", Cause::AppArmor, "/x/helix-help", true).contains("/x/helix-help")
        );
        assert!(explanation("Help", Cause::Disabled, "/x", false).contains("max_user_namespaces"));
    }
}
