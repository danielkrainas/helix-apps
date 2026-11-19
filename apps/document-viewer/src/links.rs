//! Link targets, outline extraction and the little unsafe glue needed for
//! them.
//!
//! `poppler-rs` 0.24 leaves `PopplerAction` (a C union) and the fields of
//! `PopplerDest` / `PopplerLinkMapping` unwrapped, so this module reads them
//! through the re-exported `poppler::ffi` structs. Only plain fields of
//! structures owned by poppler are read, and every action obtained with
//! ownership is freed with `poppler_action_free`. Everything here runs on the
//! render thread, which owns the document.

use crate::layout::Rect;
use crate::search::flip_rect;
use gtk::glib;
use gtk::glib::translate::{ToGlibPtr, ToGlibPtrMut};
use helix_app_common::i18n::tr;
use helix_app_common::trf;
use poppler::ffi;
use std::ffi::CStr;

/// Named actions a link can trigger.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Named {
    NextPage,
    PrevPage,
    FirstPage,
    LastPage,
    GoBack,
    GoForward,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Target {
    /// 0-based page; `top` / `left` in top-left page points when known.
    Page {
        page: usize,
        top: Option<f64>,
        left: Option<f64>,
    },
    Uri(String),
    /// Another file (relative to the document) and optional 0-based page.
    Remote {
        file: String,
        page: Option<usize>,
    },
    Named(Named),
}

impl Target {
    pub fn describe(&self, labels: &[String]) -> String {
        match self {
            Target::Page { page, .. } => {
                let label = labels
                    .get(*page)
                    .cloned()
                    .unwrap_or_else(|| (page + 1).to_string());
                trf!("Go to page {label}", label = label)
            }
            Target::Uri(u) => u.clone(),
            // Translators: a link to another document, "Go to report.pdf".
            Target::Remote { file, .. } => trf!("Go to {file}", file = file),
            Target::Named(n) => match n {
                Named::NextPage => tr("Next page").into(),
                Named::PrevPage => tr("Previous page").into(),
                Named::FirstPage => tr("First page").into(),
                Named::LastPage => tr("Last page").into(),
                Named::GoBack => tr("Go back").into(),
                Named::GoForward => tr("Go forward").into(),
            },
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Link {
    /// Top-left-origin page points.
    pub rect: Rect,
    pub target: Target,
    /// The text under the link, whitespace collapsed (may be empty).
    pub text: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct OutlineItem {
    pub title: String,
    pub target: Option<Target>,
    pub open: bool,
    pub children: Vec<OutlineItem>,
}

/// Maps a PDF named action (`/N` entry) to [`Named`].
pub fn parse_named(name: &str) -> Option<Named> {
    Some(match name {
        "NextPage" => Named::NextPage,
        "PrevPage" => Named::PrevPage,
        "FirstPage" => Named::FirstPage,
        "LastPage" => Named::LastPage,
        "GoBack" => Named::GoBack,
        "GoForward" => Named::GoForward,
        _ => return None,
    })
}

unsafe fn cstr(p: *const std::os::raw::c_char) -> Option<String> {
    if p.is_null() {
        None
    } else {
        Some(CStr::from_ptr(p).to_string_lossy().into_owned())
    }
}

/// Page heights of the document, used to flip destination coordinates.
pub struct DocCtx<'a> {
    pub doc: &'a poppler::Document,
    pub heights: &'a [f64],
}

impl DocCtx<'_> {
    /// Resolves a destination (named destinations included).
    unsafe fn dest(&self, d: *const ffi::PopplerDest) -> Option<Target> {
        if d.is_null() {
            return None;
        }
        let d = &*d;
        if d.type_ == ffi::POPPLER_DEST_NAMED {
            let name = cstr(d.named_dest)?;
            let resolved = self.doc.find_dest(&name)?;
            let ptr: *const ffi::PopplerDest = resolved.to_glib_none().0;
            if (*ptr).type_ == ffi::POPPLER_DEST_NAMED {
                return None;
            }
            return self.dest(ptr);
        }
        if d.page_num < 1 {
            return None;
        }
        let page = (d.page_num - 1) as usize;
        let h = *self.heights.get(page)?;
        // `change_left` is the first bit of a C bit-field word; the binding
        // exposes the whole word, bit 1 is `change_top`.
        let flags = d.change_left;
        let has_left = flags & 1 != 0;
        let has_top = flags & 2 != 0;
        let (top, left) = match d.type_ {
            ffi::POPPLER_DEST_XYZ => (
                has_top.then(|| (h - d.top).clamp(0.0, h)),
                has_left.then_some(d.left.max(0.0)),
            ),
            ffi::POPPLER_DEST_FITH | ffi::POPPLER_DEST_FITBH => {
                (Some((h - d.top).clamp(0.0, h)), None)
            }
            ffi::POPPLER_DEST_FITR => (Some((h - d.top.max(d.bottom)).clamp(0.0, h)), Some(d.left)),
            _ => (None, None),
        };
        Some(Target::Page { page, top, left })
    }

    /// Converts an action (borrowed) to a target.
    unsafe fn action(&self, a: *const ffi::PopplerAction) -> Option<Target> {
        if a.is_null() {
            return None;
        }
        match (*a).type_ {
            ffi::POPPLER_ACTION_GOTO_DEST => self.dest((*a).goto_dest.dest),
            ffi::POPPLER_ACTION_URI => cstr((*a).uri.uri).map(Target::Uri),
            ffi::POPPLER_ACTION_NAMED => cstr((*a).named.named_dest)
                .as_deref()
                .and_then(parse_named)
                .map(Target::Named),
            ffi::POPPLER_ACTION_GOTO_REMOTE => {
                let file = cstr((*a).goto_remote.file_name)?;
                let dest = (*a).goto_remote.dest;
                let page = if dest.is_null() || (*dest).page_num < 1 {
                    None
                } else {
                    Some(((*dest).page_num - 1) as usize)
                };
                Some(Target::Remote { file, page })
            }
            _ => None,
        }
    }

    /// Links on `page` (bottom-left areas flipped to top-left).
    pub fn page_links(&self, page: &poppler::Page) -> Vec<Link> {
        let h = page.size().1;
        page.link_mapping()
            .iter()
            .filter_map(|m| unsafe {
                let raw: *const ffi::PopplerLinkMapping = m.to_glib_none().0;
                let a = &(*raw).area;
                let target = self.action((*raw).action)?;
                Some(Link {
                    rect: flip_rect(a.x1, a.y1, a.x2, a.y2, h),
                    target,
                    text: String::new(),
                })
            })
            .collect()
    }

    /// Walks the document outline.
    pub fn outline(&self) -> Vec<OutlineItem> {
        fn walk(ctx: &DocCtx, iter: &mut poppler::IndexIter, depth: usize) -> Vec<OutlineItem> {
            let mut out = Vec::new();
            loop {
                let mut item = OutlineItem::default();
                unsafe {
                    let a = ffi::poppler_index_iter_get_action(iter.to_glib_none_mut().0);
                    if !a.is_null() {
                        item.title = cstr((*a).any.title).unwrap_or_default();
                        item.target = ctx.action(a);
                        ffi::poppler_action_free(a);
                    }
                }
                item.open = iter.is_open();
                if depth < 32 {
                    if let Some(mut child) = iter.child() {
                        item.children = walk(ctx, &mut child, depth + 1);
                    }
                }
                out.push(item);
                if !iter.next() {
                    break;
                }
            }
            out
        }
        // `IndexIter::new` cannot represent the NULL returned for documents
        // without an outline, so create the iterator directly.
        let raw = unsafe { ffi::poppler_index_iter_new(self.doc.to_glib_none().0) };
        if raw.is_null() {
            return Vec::new();
        }
        let mut iter: poppler::IndexIter = unsafe { glib::translate::from_glib_full(raw) };
        walk(self, &mut iter, 0)
    }
}

/// The document a "go to another file" link opens. A relative name is
/// resolved next to the current document (`base_dir`, its folder, which
/// the user already opened from); a name with a scheme only when it is a
/// local `file:` URI, so a link cannot make GIO mount a share or fetch
/// from a server.
pub fn remote_document(base_dir: Option<&gtk::gio::File>, file: &str) -> Option<gtk::gio::File> {
    use gtk::gio::prelude::*;
    if file.contains("://") || helix_app_common::links::scheme(file).is_some() {
        return helix_app_common::links::local_path(file).map(gtk::gio::File::for_path);
    }
    match base_dir {
        Some(dir) => Some(dir.resolve_relative_path(file)),
        None if std::path::Path::new(file).is_absolute() => Some(gtk::gio::File::for_path(file)),
        None => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_documents_stay_local() {
        use gtk::gio::prelude::*;
        let dir = gtk::gio::File::for_path("/docs/manual");
        let path = |f: Option<gtk::gio::File>| f.and_then(|f| f.path());
        assert_eq!(
            path(remote_document(Some(&dir), "part2.pdf")),
            Some("/docs/manual/part2.pdf".into())
        );
        assert_eq!(
            path(remote_document(None, "file:///docs/b.pdf")),
            Some("/docs/b.pdf".into())
        );
        assert_eq!(
            path(remote_document(None, "/docs/c.pdf")),
            Some("/docs/c.pdf".into())
        );
        for bad in [
            "smb://host/share/x.pdf",
            "http://example.org/x.pdf",
            "sftp://h/x.pdf",
            "smb:host/x.pdf",
            "file://otherhost/x.pdf",
        ] {
            assert!(remote_document(Some(&dir), bad).is_none(), "{bad}");
        }
        assert!(remote_document(None, "relative.pdf").is_none());
    }

    #[test]
    fn named_actions() {
        assert_eq!(parse_named("NextPage"), Some(Named::NextPage));
        assert_eq!(parse_named("Print"), None);
        let labels = vec!["i".to_string(), "1".to_string()];
        assert_eq!(
            Target::Page {
                page: 0,
                top: None,
                left: None
            }
            .describe(&labels),
            "Go to page i"
        );
        assert_eq!(
            Target::Page {
                page: 5,
                top: None,
                left: None
            }
            .describe(&labels),
            "Go to page 6"
        );
    }
}
