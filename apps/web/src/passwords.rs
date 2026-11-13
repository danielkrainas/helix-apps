//! Password manager glue: the form-capture script injected into pages, the
//! messages it sends, and the decision whether to offer saving.
//!
//! The script runs in the isolated script world [`WORLD`], so page scripts
//! cannot see its message handler or call its fill function. Logins are kept
//! in the Secret Service (see `secrets.rs`); nothing is written to disk by
//! Helix itself.

use crate::secrets::Login;
use serde::Deserialize;

/// Isolated JavaScript world for Helix's own page scripts.
pub const WORLD: &str = "helix";
/// Script message handler name (`window.webkit.messageHandlers.<name>`).
pub const HANDLER: &str = "helixPasswords";

/// Injected at document end into top frames, in [`WORLD`].
pub const FORM_SCRIPT: &str = r#"(function () {
  if (window.__helixPasswords) return;
  window.__helixPasswords = true;
  var TEXTY = ['text', 'email', 'tel', 'url', ''];
  function visible(el) { return !!(el.offsetWidth || el.offsetHeight || el.getClientRects().length); }
  function fieldsOf(root) {
    var inputs = Array.prototype.slice.call(root.querySelectorAll('input'));
    var passes = inputs.filter(function (el) { return (el.type || '').toLowerCase() === 'password'; });
    if (passes.length === 0) return null;
    var pass = passes[0], user = null;
    var idx = inputs.indexOf(pass);
    for (var j = idx - 1; j >= 0; j--) {
      var t = (inputs[j].getAttribute('type') || '').toLowerCase();
      if (TEXTY.indexOf(t) >= 0 && visible(inputs[j])) { user = inputs[j]; break; }
    }
    return { user: user, pass: pass, passes: passes };
  }
  function report(form) {
    var f = fieldsOf(form || document);
    if (!f || !f.pass.value) return;
    // Sign-up and change-password forms have several password fields.
    var filled = f.passes.filter(function (p) { return p.value; });
    if (filled.length > 1) return;
    var action = (form && form.action) ? String(form.action) : String(location.href);
    window.webkit.messageHandlers.helixPasswords.postMessage(JSON.stringify({
      action: action,
      username: f.user ? f.user.value : '',
      password: f.pass.value
    }));
  }
  document.addEventListener('submit', function (e) {
    if (e.target && e.target.tagName === 'FORM') report(e.target);
  }, true);
  function setValue(el, v) {
    el.value = v;
    el.dispatchEvent(new Event('input', { bubbles: true }));
    el.dispatchEvent(new Event('change', { bubbles: true }));
  }
  window.__helixFill = function (username, password) {
    var roots = Array.prototype.slice.call(document.forms);
    if (roots.length === 0) roots = [document];
    var done = 0;
    roots.forEach(function (root) {
      var f = fieldsOf(root);
      if (!f || f.passes.length > 1 || f.pass.value) return;
      if (f.user && username && !f.user.value) setValue(f.user, username);
      setValue(f.pass, password);
      done++;
    });
    return done;
  };
})();"#;

/// A login form submission reported by [`FORM_SCRIPT`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Submission {
    pub origin: String,
    pub target_origin: String,
    pub username: String,
    pub password: String,
}

#[derive(Deserialize)]
struct Message {
    #[serde(default)]
    action: String,
    #[serde(default)]
    username: String,
    #[serde(default)]
    password: String,
}

/// Parses a script message. `page_origin` comes from the tab (not from the
/// page) so a page cannot claim to be another site.
pub fn parse_message(json: &str, page_origin: &str) -> Option<Submission> {
    if page_origin.is_empty() || !page_origin.starts_with("http") {
        return None;
    }
    let m: Message = serde_json::from_str(json).ok()?;
    if m.password.is_empty() || m.password.len() > 4096 || m.username.len() > 1024 {
        return None;
    }
    let target_origin =
        crate::urlinput::origin_of(&m.action).unwrap_or_else(|| page_origin.to_string());
    Some(Submission {
        origin: page_origin.to_string(),
        target_origin,
        username: m.username.trim().to_string(),
        password: m.password,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Offer {
    /// Already saved with this password.
    Nothing,
    Save,
    /// The user exists with a different password.
    Update,
}

pub fn offer(existing: &[Login], s: &Submission) -> Offer {
    match existing.iter().find(|l| l.username == s.username) {
        Some(l) if l.password == s.password => Offer::Nothing,
        Some(_) => Offer::Update,
        None => Offer::Save,
    }
}

impl Submission {
    pub fn to_login(&self) -> Login {
        Login {
            origin: self.origin.clone(),
            target_origin: self.target_origin.clone(),
            username: self.username.clone(),
            password: self.password.clone(),
        }
    }
}

/// The login to fill into a page: the only one, or the first by user name.
pub fn pick_fill(logins: &[Login]) -> Option<&Login> {
    logins.iter().min_by(|a, b| a.username.cmp(&b.username))
}

/// JavaScript calling the fill function with safely encoded arguments.
pub fn fill_script(login: &Login) -> String {
    let user = serde_json::to_string(&login.username).unwrap_or_else(|_| "\"\"".into());
    let pass = serde_json::to_string(&login.password).unwrap_or_else(|_| "\"\"".into());
    format!("window.__helixFill ? window.__helixFill({user}, {pass}) : 0")
}

/// Text of the save bar.
pub fn prompt_text(s: &Submission, kind: Offer) -> String {
    let host = s.origin.split_once("://").map(|(_, h)| h).unwrap_or(&s.origin);
    let who = if s.username.is_empty() {
        String::new()
    } else {
        format!(" for “{}”", s.username)
    };
    match kind {
        Offer::Update => format!("Update the saved password{who} on {host}?"),
        _ => format!("Save password{who} on {host}?"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn login(user: &str, pw: &str) -> Login {
        Login {
            origin: "https://e.com".into(),
            target_origin: "https://e.com".into(),
            username: user.into(),
            password: pw.into(),
        }
    }

    #[test]
    fn parses_messages() {
        let s = parse_message(
            r#"{"action":"https://auth.e.com/login?x=1","username":" ada ","password":"pw"}"#,
            "https://e.com",
        )
        .unwrap();
        assert_eq!(s.origin, "https://e.com");
        assert_eq!(s.target_origin, "https://auth.e.com");
        assert_eq!(s.username, "ada");
        assert_eq!(s.password, "pw");
        // Missing/odd action falls back to the page origin.
        let s = parse_message(r#"{"username":"","password":"pw"}"#, "https://e.com").unwrap();
        assert_eq!(s.target_origin, "https://e.com");
        assert!(parse_message(r#"{"username":"a","password":""}"#, "https://e.com").is_none());
        assert!(parse_message("not json", "https://e.com").is_none());
        assert!(parse_message(r#"{"password":"pw"}"#, "file://").is_none());
        assert!(parse_message(r#"{"password":"pw"}"#, "").is_none());
    }

    #[test]
    fn offers() {
        let s = parse_message(r#"{"username":"ada","password":"pw"}"#, "https://e.com").unwrap();
        assert_eq!(offer(&[], &s), Offer::Save);
        assert_eq!(offer(&[login("bob", "pw")], &s), Offer::Save);
        assert_eq!(offer(&[login("ada", "old")], &s), Offer::Update);
        assert_eq!(offer(&[login("ada", "pw")], &s), Offer::Nothing);
        assert_eq!(s.to_login(), login("ada", "pw"));
        assert_eq!(prompt_text(&s, Offer::Save), "Save password for “ada” on e.com?");
        assert_eq!(prompt_text(&s, Offer::Update), "Update the saved password for “ada” on e.com?");
    }

    #[test]
    fn fill_scripts_are_escaped() {
        let l = login("a\"b", "p'w\\\n</script>");
        let js = fill_script(&l);
        assert_eq!(
            js,
            r#"window.__helixFill ? window.__helixFill("a\"b", "p'w\\\n</script>") : 0"#
        );
        assert_eq!(pick_fill(&[login("zed", "1"), login("amy", "2")]).unwrap().username, "amy");
        assert!(pick_fill(&[]).is_none());
    }
}
