//! Locally generated start page.

use crate::bookmarks::escape_html;
use crate::history::{strip_scheme, Entry};

const STYLE: &str = "
:root { color-scheme: light dark; font-family: system-ui, sans-serif; }
body { margin: 0; padding: 48px 24px; display: flex; flex-direction: column; align-items: center; }
h1 { font-weight: 800; font-size: 1.6em; margin: 0 0 24px; opacity: .8; }
p.note { max-width: 36em; text-align: center; line-height: 1.5; opacity: .8; }
.grid { display: grid; grid-template-columns: repeat(auto-fill, minmax(200px, 1fr)); gap: 12px; width: 100%; max-width: 900px; }
a.tile { display: block; padding: 14px 16px; border-radius: 12px; text-decoration: none; color: inherit;
         background: rgba(127,127,127,.12); overflow: hidden; }
a.tile:hover { background: rgba(127,127,127,.22); }
.t { font-weight: 600; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
.u { font-size: .85em; opacity: .65; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; margin-top: 4px; }
@media (prefers-contrast: more) {
  h1, p.note, .u { opacity: 1; }
  a.tile { background: transparent; border: 1px solid currentColor; }
}
";

pub fn html(most_visited: &[Entry], private: bool) -> String {
    let mut body = String::new();
    if private {
        body.push_str(
            "<h1>Private Browsing</h1><p class=\"note\">Pages you visit in this window are not \
             saved to history, and cookies and site data are discarded when the window closes. \
             Downloads and bookmarks are kept.</p>",
        );
    } else if most_visited.is_empty() {
        body.push_str(
            "<h1>Welcome to Web</h1><p class=\"note\">Sites you visit often will appear here.</p>",
        );
    } else {
        body.push_str("<h1>Most Visited</h1><div class=\"grid\">");
        for e in most_visited {
            let title = if e.title.is_empty() {
                strip_scheme(&e.url)
            } else {
                &e.title
            };
            body.push_str(&format!(
                "<a class=\"tile\" href=\"{u}\"><div class=\"t\">{t}</div><div class=\"u\">{d}</div></a>",
                u = escape_html(&e.url),
                t = escape_html(title),
                d = escape_html(strip_scheme(&e.url).trim_end_matches('/')),
            ));
        }
        body.push_str("</div>");
    }
    format!(
        "<!DOCTYPE html><html><head><meta charset=\"utf-8\"><title>New Tab</title>\
         <style>{STYLE}</style></head><body>{body}</body></html>"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes_and_variants() {
        let e = Entry {
            url: "https://a.org/?x=<y>".into(),
            title: "<script>".into(),
            visits: 1,
            last_visit: 0,
        };
        let h = html(&[e], false);
        assert!(h.contains("&lt;script&gt;"));
        assert!(!h.contains("<script>"));
        assert!(html(&[], true).contains("Private Browsing"));
        assert!(html(&[], false).contains("Welcome"));
    }
}
