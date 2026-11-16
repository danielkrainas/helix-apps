//! HTML page shell.

pub use crate::xmlutil::{escape, escape_attr};
use helix_app_common::i18n::tr;

pub const CSS: &str = include_str!("style.css");

/// Wraps rendered body markup in a complete HTML document in language
/// `lang` (a BCP 47 tag; `None` when it is not known).
pub fn page(lang: Option<&str>, title: &str, body: &str) -> String {
    let lang = lang
        .map(|l| format!(" lang=\"{}\"", escape_attr(l)))
        .unwrap_or_default();
    format!(
        "<!DOCTYPE html>\n<html{lang}><head><meta charset=\"utf-8\">\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
         <meta name=\"color-scheme\" content=\"light dark\">\
         <title>{}</title><style>{CSS}</style></head>\
         <body><main>{body}</main></body></html>",
        escape(title)
    )
}

/// One entry in a trail (breadcrumb).
pub fn trail(items: &[(String, String)]) -> String {
    if items.is_empty() {
        return String::new();
    }
    let parts: Vec<String> = items
        .iter()
        .map(|(uri, title)| format!("<a href=\"{}\">{}</a>", escape_attr(uri), escape(title)))
        .collect();
    format!(
        "<nav class=\"trail\">{}<span class=\"sep\">\u{203A}</span></nav>",
        parts.join("<span class=\"sep\">\u{203A}</span>")
    )
}

/// A page of Help's own, in the language of its interface.
pub fn ui_page(title: &str, body: &str) -> String {
    page(Some(crate::locale::ui_lang()), title, body)
}

pub fn error_page(title: &str, message: &str) -> String {
    ui_page(
        title,
        &format!(
            "<h1>{}</h1><p class=\"error\">{}</p><p><a href=\"help:\">{}</a></p>",
            escape(title),
            escape(message),
            escape(tr("All help documents"))
        ),
    )
}

/// A list of links with optional descriptions.
pub struct LinkItem {
    pub uri: String,
    pub title: String,
    pub desc: Option<String>,
}

pub fn link_list(class: &str, heading: Option<&str>, items: &[LinkItem]) -> String {
    if items.is_empty() {
        return String::new();
    }
    let mut s = format!("<div class=\"links {class}\">");
    if let Some(h) = heading {
        s.push_str(&format!("<h2>{}</h2>", escape(h)));
    }
    s.push_str("<ul>");
    for it in items {
        s.push_str(&format!(
            "<li><a href=\"{}\">{}</a>",
            escape_attr(&it.uri),
            escape(&it.title)
        ));
        if let Some(d) = it.desc.as_deref().filter(|d| !d.is_empty()) {
            s.push_str(&format!("<span class=\"link-desc\">{}</span>", escape(d)));
        }
        s.push_str("</li>");
    }
    s.push_str("</ul></div>");
    s
}

pub fn guess_mime(path: &str) -> &'static str {
    let ext = path
        .rsplit_once('.')
        .map(|(_, e)| e.to_ascii_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "svg" => "image/svg+xml",
        "webp" => "image/webp",
        "ogv" => "video/ogg",
        "webm" => "video/webm",
        "mp4" => "video/mp4",
        "ogg" | "oga" => "audio/ogg",
        "mp3" => "audio/mpeg",
        "css" => "text/css",
        "js" => "text/javascript",
        "html" | "htm" => "text/html",
        "txt" => "text/plain",
        _ => "application/octet-stream",
    }
}
