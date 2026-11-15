//! Translation of user-visible text (gettext).
//!
//! The whole suite shares one gettext domain, [`DOMAIN`] (`helix-apps`):
//! the apps live in one repository, are released together and share many
//! strings ("About", "Keyboard Shortcuts", "Cancel", …), so translators
//! work on a single catalogue (`po/helix-apps.pot`) and every string is
//! translated once. Catalogues are installed as
//! `/usr/share/locale/<lang>/LC_MESSAGES/helix-apps.mo`.
//!
//! Every app calls [`init`] first thing in `main`. Then:
//!
//! - [`tr`]`("Text")` translates a literal and returns a `&'static str`, so
//!   it can stand wherever the literal stood (`Some(tr("…"))`, builders,
//!   `match` arms returning `&str`).
//! - [`trn`]`("One page", "{n} pages", n)` picks the plural form for `n`.
//! - [`trc`]`("context", "Text")` disambiguates short strings that need
//!   different translations in different places.
//! - [`trf!`](crate::trf)`("Open {name}", name = file)` translates a message
//!   with placeholders, so translators can move them around.
//! - [`n_`]`("Text")` marks a literal in a `const`/`static` table for
//!   extraction without translating it; the code that shows it calls
//!   [`tr`] (or [`tr_dyn`]) on it.
//!
//! `tools/i18n/update-pot.sh` extracts these calls (and the desktop files
//! and metainfo) into the template; see `TRANSLATING.md`.
//!
//! Keep messages whole sentences, with placeholders instead of pieces
//! glued together, and keep mnemonics (`_`) inside the message.

use std::collections::HashMap;
use std::fmt::Display;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};

/// The gettext domain of every Helix app.
pub const DOMAIN: &str = "helix-apps";

/// Where packages install the compiled catalogues. Overridden at build
/// time by `HELIX_LOCALEDIR` (for other prefixes) and at run time by the
/// `HELIX_LOCALEDIR` environment variable (for tests and uninstalled
/// builds).
pub const LOCALEDIR: &str = match option_env!("HELIX_LOCALEDIR") {
    Some(dir) => dir,
    None => "/usr/share/locale",
};

static READY: AtomicBool = AtomicBool::new(false);

/// The locale directory in effect: `$HELIX_LOCALEDIR` or [`LOCALEDIR`].
pub fn localedir() -> std::path::PathBuf {
    std::env::var_os("HELIX_LOCALEDIR")
        .filter(|d| !d.is_empty())
        .map(Into::into)
        .unwrap_or_else(|| LOCALEDIR.into())
}

/// Sets the process locale from the environment and binds [`DOMAIN`].
/// Call it at the start of `main`, before any thread is started and
/// before the first [`tr`]. Calling it again does nothing.
pub fn init() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(bind);
}

fn bind() {
    // SAFETY: called at the start of `main`, before other threads exist,
    // as setlocale(3) requires.
    unsafe {
        gettextrs::setlocale(gettextrs::LocaleCategory::LcAll, "");
    }
    let _ = gettextrs::bindtextdomain(DOMAIN, localedir());
    let _ = gettextrs::bind_textdomain_codeset(DOMAIN, "UTF-8");
    // The default domain, so GtkBuilder files and `gettext` calls in
    // dependencies that use it also find Helix's strings.
    let _ = gettextrs::textdomain(DOMAIN);
    READY.store(true, Ordering::SeqCst);
}

/// Like [`init`], but re-reads the locale environment every time and
/// forgets cached translations. Only for tests that switch locales.
#[doc(hidden)]
pub fn init_for_tests() {
    bind();
    cache().lock().unwrap_or_else(|e| e.into_inner()).clear();
}

fn cache() -> &'static Mutex<HashMap<&'static str, &'static str>> {
    static CACHE: OnceLock<Mutex<HashMap<&'static str, &'static str>>> = OnceLock::new();
    CACHE.get_or_init(Default::default)
}

/// Translates `msgid`.
///
/// Translations are looked up once and kept for the life of the process
/// (the locale never changes while an app runs), which is what lets this
/// return a `&'static str`. Before [`init`] (unit tests, `--help` before
/// set-up) the message is returned untranslated and not cached.
pub fn tr(msgid: &'static str) -> &'static str {
    if msgid.is_empty() || !READY.load(Ordering::Relaxed) {
        return msgid;
    }
    let mut cache = cache().lock().unwrap_or_else(|e| e.into_inner());
    if let Some(t) = cache.get(msgid) {
        return t;
    }
    let t = gettextrs::dgettext(DOMAIN, msgid);
    let t: &'static str = if t == msgid {
        msgid
    } else {
        Box::leak(t.into_boxed_str())
    };
    cache.insert(msgid, t);
    t
}

/// Translates a message that is not a `'static` literal at the call site
/// but was marked for extraction elsewhere (with [`n_`] or [`tr`]).
pub fn tr_dyn(msgid: &str) -> String {
    if msgid.is_empty() || !READY.load(Ordering::Relaxed) {
        return msgid.to_string();
    }
    gettextrs::dgettext(DOMAIN, msgid)
}

/// Translates `msgid` in `context` (`msgctxt`), for short strings whose
/// translation depends on where they appear ("Open" a file vs "Open"
/// as a state).
pub fn trc(context: &'static str, msgid: &'static str) -> String {
    if !READY.load(Ordering::Relaxed) {
        return msgid.to_string();
    }
    gettextrs::pgettext(context, msgid)
}

/// Picks the translated singular or plural form for `n` and fills in
/// `{n}` (and `{}`) with `n`: `trn("{n} page", "{n} pages", count)`.
pub fn trn(singular: &'static str, plural: &'static str, n: u64) -> String {
    // gettext takes a u32; the plural form of a huge count is the plural
    // form of a huge u32.
    let k = n.min(u32::MAX as u64) as u32;
    let template = if READY.load(Ordering::Relaxed) {
        gettextrs::dngettext(DOMAIN, singular, plural, k)
    } else if n == 1 {
        singular.to_string()
    } else {
        plural.to_string()
    };
    fill(&template, &[], &[("n", &n)])
}

/// Marks `msgid` for extraction without translating it, for `const` and
/// `static` tables; translate it with [`tr`] where it is shown.
pub const fn n_(msgid: &'static str) -> &'static str {
    msgid
}

/// Replaces placeholders in a translated `template`: `{}` takes the next
/// of `positional`, `{0}`, `{1}`… a given one, `{name}` the value named in
/// `named`; `{{` and `}}` are literal braces. Unknown placeholders are
/// kept as they are, so a mistranslated name shows up instead of
/// vanishing.
pub fn fill(template: &str, positional: &[&dyn Display], named: &[(&str, &dyn Display)]) -> String {
    let mut out = String::with_capacity(template.len() + 16);
    let mut next = 0;
    let mut rest = template;
    while let Some(i) = rest.find(['{', '}']) {
        out.push_str(&rest[..i]);
        let tail = &rest[i..];
        if tail.starts_with("{{") || tail.starts_with("}}") {
            out.push_str(&tail[..1]);
            rest = &tail[2..];
            continue;
        }
        if let Some(after) = tail.strip_prefix('}') {
            out.push('}');
            rest = after;
            continue;
        }
        let Some(end) = tail.find('}') else {
            out.push_str(tail);
            rest = "";
            break;
        };
        let key = &tail[1..end];
        let value: Option<&dyn Display> = if key.is_empty() {
            let v = positional.get(next).copied();
            next += 1;
            v
        } else if let Ok(k) = key.parse::<usize>() {
            positional.get(k).copied()
        } else {
            named.iter().find(|(name, _)| *name == key).map(|(_, v)| *v)
        };
        match value {
            Some(v) => out.push_str(&v.to_string()),
            None => out.push_str(&tail[..=end]),
        }
        rest = &tail[end + 1..];
    }
    out.push_str(rest);
    out
}

/// Translates a message with placeholders and fills them in:
///
/// ```ignore
/// trf!("Page {} of {}", page, total)
/// trf!("Open {name}", name = file_name)
/// ```
///
/// The message must be a string literal (it is extracted for
/// translation). Placeholders take no format specs; format the value
/// first (`trf!("{size} free", size = format_size(n))`).
#[macro_export]
macro_rules! trf {
    ($msg:literal $(,)?) => {
        $crate::i18n::tr($msg).to_string()
    };
    ($msg:literal, $($name:ident = $value:expr),+ $(,)?) => {
        $crate::i18n::fill(
            $crate::i18n::tr($msg),
            &[],
            &[$((stringify!($name), &$value as &dyn ::std::fmt::Display)),+],
        )
    };
    ($msg:literal, $($value:expr),+ $(,)?) => {
        $crate::i18n::fill(
            $crate::i18n::tr($msg),
            &[$(&$value as &dyn ::std::fmt::Display),+],
            &[],
        )
    };
}

/// Like [`trf!`], with singular and plural messages chosen by `n` (which
/// is also available as `{n}`):
///
/// ```ignore
/// trnf!("{n} file in {folder}", "{n} files in {folder}", count, folder = name)
/// ```
#[macro_export]
macro_rules! trnf {
    ($one:literal, $many:literal, $n:expr $(, $name:ident = $value:expr)* $(,)?) => {{
        let n: u64 = ($n) as u64;
        $crate::i18n::fill(
            &$crate::i18n::plural_template($one, $many, n),
            &[&n as &dyn ::std::fmt::Display],
            &[("n", &n as &dyn ::std::fmt::Display) $(, (stringify!($name), &$value as &dyn ::std::fmt::Display))*],
        )
    }};
}

/// The translated singular or plural message for `n`, placeholders left
/// in (see [`trnf!`]).
pub fn plural_template(singular: &'static str, plural: &'static str, n: u64) -> String {
    let k = n.min(u32::MAX as u64) as u32;
    if READY.load(Ordering::Relaxed) {
        gettextrs::dngettext(DOMAIN, singular, plural, k)
    } else if n == 1 {
        singular.to_string()
    } else {
        plural.to_string()
    }
}

/// Formats `value` with `decimals` digits after the locale's decimal
/// separator and its digit grouping (`1,234.5` in English, `1.234,5` in
/// German), through the C library's `%'.*f`.
pub fn format_number(value: f64, decimals: usize) -> String {
    let fmt = format!("%'.{decimals}f");
    format_c(&fmt, value).unwrap_or_else(|| format!("{value:.decimals$}"))
}

fn format_c(fmt: &str, value: f64) -> Option<String> {
    extern "C" {
        fn snprintf(buf: *mut std::ffi::c_char, n: usize, fmt: *const std::ffi::c_char, ...)
            -> i32;
    }
    let cfmt = std::ffi::CString::new(fmt).ok()?;
    let mut buf = vec![0u8; 64];
    // SAFETY: the buffer is writable for its length, the format has one
    // `double` conversion and `value` is passed as a double.
    let n = unsafe { snprintf(buf.as_mut_ptr().cast(), buf.len(), cfmt.as_ptr(), value) };
    if n < 0 || n as usize >= buf.len() {
        return None;
    }
    buf.truncate(n as usize);
    String::from_utf8(buf).ok()
}

/// A date part in the current locale through GLib (`%OB`, `%Ob`, `%A`,
/// `%a`), cached for the life of the process like [`tr`].
fn date_part(format: &'static str, index: u32, date: (i32, i32, i32)) -> &'static str {
    type Key = (&'static str, u32);
    static CACHE: OnceLock<Mutex<HashMap<Key, &'static str>>> = OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    let mut cache = cache.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(s) = cache.get(&(format, index)) {
        return s;
    }
    let text = glib::DateTime::from_utc(date.0, date.1, date.2, 12, 0, 0.0)
        .ok()
        .and_then(|d| d.format(format).ok())
        .map(|s| s.to_string())
        .unwrap_or_default();
    let text: &'static str = Box::leak(text.into_boxed_str());
    cache.insert((format, index), text);
    text
}

/// The month's name on its own, as in a list or a heading ("March"),
/// in the current locale; `month` is 1-12.
pub fn month_name(month: u32) -> &'static str {
    let m = month.clamp(1, 12);
    date_part("%OB", m, (2024, m as i32, 1))
}

/// The month's abbreviated name on its own ("Mar"); `month` is 1-12.
pub fn month_abbr(month: u32) -> &'static str {
    let m = month.clamp(1, 12);
    date_part("%Ob", m, (2024, m as i32, 1))
}

/// The weekday's name ("Monday"); `day` counts from Monday = 0.
pub fn weekday_name(day: u32) -> &'static str {
    let d = day % 7;
    // 1 January 2024 was a Monday.
    date_part("%A", d, (2024, 1, 1 + d as i32))
}

/// The weekday's abbreviated name ("Mon"); `day` counts from Monday = 0.
pub fn weekday_abbr(day: u32) -> &'static str {
    let d = day % 7;
    date_part("%a", d, (2024, 1, 1 + d as i32))
}

/// Whether the interface reads right to left (GTK picks the direction
/// from the translation of the current language).
pub fn is_rtl() -> bool {
    gtk::Widget::default_direction() == gtk::TextDirection::Rtl
}

/// Text justification toward the start of a line (left in English,
/// right in Arabic or Hebrew). GTK mirrors alignment but not
/// justification, so labels that justify use this.
pub fn justify_start() -> gtk::Justification {
    if is_rtl() {
        gtk::Justification::Right
    } else {
        gtk::Justification::Left
    }
}

/// Text justification toward the end of a line; see [`justify_start`].
pub fn justify_end() -> gtk::Justification {
    if is_rtl() {
        gtk::Justification::Left
    } else {
        gtk::Justification::Right
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fills_placeholders() {
        assert_eq!(fill("Page {} of {}", &[&2, &5], &[]), "Page 2 of 5");
        assert_eq!(fill("{1} before {0}", &[&"a", &"b"], &[]), "b before a");
        assert_eq!(
            fill("Open {name}", &[], &[("name", &"x.txt")]),
            "Open x.txt"
        );
        assert_eq!(
            fill("{{literal}} {missing}", &[], &[]),
            "{literal} {missing}"
        );
        assert_eq!(fill("unbalanced {", &[], &[]), "unbalanced {");
        assert_eq!(fill("ünï {} ✓", &[&1], &[]), "ünï 1 ✓");
    }

    #[test]
    fn untranslated_before_init() {
        // Tests never call init(), so messages come back as written.
        assert_eq!(tr("Keyboard Shortcuts"), "Keyboard Shortcuts");
        assert_eq!(trn("{n} item", "{n} items", 1), "1 item");
        assert_eq!(trn("{n} item", "{n} items", 3), "3 items");
        assert_eq!(crate::trf!("Open {name}", name = "a"), "Open a");
        assert_eq!(crate::trf!("{} of {}", 1, 2), "1 of 2");
        assert_eq!(
            crate::trnf!("{n} file in {d}", "{n} files in {d}", 2, d = "x"),
            "2 files in x"
        );
        assert_eq!(n_("Marked"), "Marked");
    }

    #[test]
    fn date_names_in_the_c_locale() {
        assert_eq!(month_name(1), "January");
        assert_eq!(month_abbr(12), "Dec");
        assert_eq!(weekday_name(0), "Monday");
        assert_eq!(weekday_abbr(6), "Sun");
    }

    #[test]
    fn number_formatting_falls_back_to_c_locale() {
        // Tests run in the C locale: no grouping, '.' as separator.
        assert_eq!(format_number(1234.5, 1), "1234.5");
        assert_eq!(format_number(2.0, 0), "2");
    }
}
