//! Properties, password and print helpers.

use crate::render::DocInfo;
use adw::prelude::*;
use gtk::{gio, glib};
use std::cell::RefCell;
use std::rc::Rc;

/// Paper names for common sizes (mm, portrait).
const PAPERS: &[(&str, f64, f64)] = &[
    ("A3", 297.0, 420.0),
    ("A4", 210.0, 297.0),
    ("A5", 148.0, 210.0),
    ("US Letter", 215.9, 279.4),
    ("US Legal", 215.9, 355.6),
];

/// Formats a page size in points as millimetres plus a paper name.
pub fn page_size_text(w_pt: f64, h_pt: f64) -> String {
    let (w, h) = (w_pt * 25.4 / 72.0, h_pt * 25.4 / 72.0);
    let name = PAPERS.iter().find_map(|(n, pw, ph)| {
        let fits = |a: f64, b: f64| (a - pw).abs() < 2.0 && (b - ph).abs() < 2.0;
        if fits(w, h) {
            Some(format!("{n}, Portrait"))
        } else if fits(h, w) {
            Some(format!("{n}, Landscape"))
        } else {
            None
        }
    });
    let base = format!("{:.0} × {:.0} mm", w, h);
    match name {
        Some(n) => format!("{base} ({n})"),
        None => base,
    }
}

/// Permissions that are *not* granted, as human-readable names.
pub fn restrictions(bits: u32) -> Vec<&'static str> {
    let p = poppler::Permissions::from_bits_truncate(bits);
    let mut out = Vec::new();
    if !p.contains(poppler::Permissions::OK_TO_PRINT) {
        out.push("printing");
    }
    if !p.contains(poppler::Permissions::OK_TO_COPY) {
        out.push("copying");
    }
    if !p.contains(poppler::Permissions::OK_TO_MODIFY) {
        out.push("modifying");
    }
    if !p.contains(poppler::Permissions::OK_TO_ADD_NOTES) {
        out.push("annotating");
    }
    out
}

fn date_text(t: Option<i64>) -> Option<String> {
    let dt = glib::DateTime::from_unix_local(t?).ok()?;
    dt.format("%c").ok().map(|s| s.to_string())
}

pub fn show_properties(parent: &impl IsA<gtk::Widget>, info: &DocInfo) {
    let file = gio::File::for_uri(&info.uri);
    let size = file
        .query_info(
            gio::FILE_ATTRIBUTE_STANDARD_SIZE,
            gio::FileQueryInfoFlags::NONE,
            None::<&gio::Cancellable>,
        )
        .ok()
        .map(|i| glib::format_size(i.size() as u64).to_string());
    let location = file
        .path()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| info.uri.clone());
    let security = {
        let mut s = if info.encrypted {
            "Password protected".to_string()
        } else {
            "None".to_string()
        };
        let r = restrictions(info.permissions);
        if !r.is_empty() {
            s.push_str(&format!("; not permitted: {}", r.join(", ")));
        }
        s
    };
    let rows: Vec<(&str, Option<String>)> = vec![
        ("Title", info.title.clone()),
        ("Subject", info.subject.clone()),
        ("Author", info.author.clone()),
        ("Keywords", info.keywords.clone()),
        ("Producer", info.producer.clone()),
        ("Creator", info.creator.clone()),
        ("Created", date_text(info.created)),
        ("Modified", date_text(info.modified)),
        ("Pages", Some(info.sizes.len().to_string())),
        (
            "Page Size",
            info.sizes.first().map(|(w, h)| page_size_text(*w, *h)),
        ),
        ("File Size", size),
        ("PDF Version", info.pdf_version.clone()),
        ("Location", Some(location)),
        ("Security", Some(security)),
        (
            "Fast Web View",
            Some(if info.linearized { "Yes" } else { "No" }.into()),
        ),
    ];
    let group = adw::PreferencesGroup::new();
    for (title, value) in rows {
        let row = adw::ActionRow::builder()
            .title(title)
            .subtitle(glib::markup_escape_text(value.as_deref().unwrap_or("—")))
            .subtitle_selectable(true)
            .build();
        row.add_css_class("property");
        group.add(&row);
    }
    let page = adw::PreferencesPage::new();
    page.add(&group);
    let tv = adw::ToolbarView::new();
    tv.add_top_bar(&adw::HeaderBar::new());
    tv.set_content(Some(&page));
    let dialog = adw::Dialog::builder()
        .title("Properties")
        .content_width(420)
        .content_height(640)
        .child(&tv)
        .build();
    dialog.present(Some(parent));
}

/// Asks for a document password. `on_unlock` gets the password.
pub fn ask_password(
    parent: &impl IsA<gtk::Widget>,
    name: &str,
    wrong: bool,
    on_unlock: impl Fn(String) + 'static,
) {
    let body = if wrong {
        format!("Incorrect password. “{name}” is locked; enter its password to open it.")
    } else {
        format!("“{name}” is password protected. Enter its password to open it.")
    };
    let dialog = adw::AlertDialog::new(Some("Password Required"), Some(&body));
    let entry = gtk::PasswordEntry::builder()
        .show_peek_icon(true)
        .activates_default(true)
        .build();
    helix_app_common::set_accessible_label(&entry, "Password");
    entry.update_property(&[gtk::accessible::Property::Description(&body)]);
    dialog.set_extra_child(Some(&entry));
    dialog.add_responses(&[("cancel", "_Cancel"), ("unlock", "_Unlock")]);
    dialog.set_response_appearance("unlock", adw::ResponseAppearance::Suggested);
    dialog.set_default_response(Some("unlock"));
    dialog.set_close_response("cancel");
    dialog.connect_response(
        None,
        glib::clone!(
            #[weak]
            entry,
            move |_, resp| {
                if resp == "unlock" {
                    on_unlock(entry.text().to_string());
                }
            }
        ),
    );
    dialog.present(Some(parent));
    entry.grab_focus();
}

/// Prints the document with `gtk::PrintOperation`. A separate poppler
/// document is opened on the main thread for the duration of the job (the
/// render thread's instance must not be shared).
pub fn print(
    parent: &impl IsA<gtk::Window>,
    uri: &str,
    password: Option<String>,
    title: &str,
    current: usize,
    settings: Rc<RefCell<Option<gtk::PrintSettings>>>,
    on_error: impl Fn(String) + 'static,
) {
    let doc = match poppler::Document::from_gfile(
        &gio::File::for_uri(uri),
        password.as_deref(),
        None::<&gio::Cancellable>,
    ) {
        Ok(d) => d,
        Err(e) => return on_error(e.message().to_string()),
    };
    let op = gtk::PrintOperation::new();
    op.set_n_pages(doc.n_pages());
    op.set_current_page(current as i32);
    op.set_job_name(title);
    op.set_embed_page_setup(true);
    if let Some(s) = settings.borrow().as_ref() {
        op.set_print_settings(Some(s));
    }
    op.connect_draw_page(move |_, ctx, page_nr| {
        let Some(page) = doc.page(page_nr) else {
            return;
        };
        let cr = ctx.cairo_context();
        let (pw, ph) = page.size();
        let (cw, ch) = (ctx.width(), ctx.height());
        // Device units per point.
        let (dx, dy) = (ctx.dpi_x() / 72.0, ctx.dpi_y() / 72.0);
        let paper_landscape = cw / dx > ch / dy;
        let page_landscape = pw > ph;
        let rotate = paper_landscape != page_landscape;
        let (rw, rh) = if rotate { (ph, pw) } else { (pw, ph) };
        let s = ((cw / dx) / rw).min((ch / dy) / rh);
        cr.translate((cw - rw * s * dx) / 2.0, (ch - rh * s * dy) / 2.0);
        cr.scale(s * dx, s * dy);
        if rotate {
            cr.translate(ph, 0.0);
            cr.rotate(std::f64::consts::FRAC_PI_2);
        }
        page.render_for_printing(&cr);
    });
    let r = op.run(gtk::PrintOperationAction::PrintDialog, Some(parent));
    match r {
        Ok(gtk::PrintOperationResult::Apply) => *settings.borrow_mut() = op.print_settings(),
        Ok(_) => {}
        Err(e) => on_error(e.message().to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paper_names() {
        assert_eq!(page_size_text(595.0, 842.0), "210 × 297 mm (A4, Portrait)");
        assert_eq!(
            page_size_text(792.0, 612.0),
            "279 × 216 mm (US Letter, Landscape)"
        );
        assert_eq!(page_size_text(100.0, 100.0), "35 × 35 mm");
    }

    #[test]
    fn permission_text() {
        assert!(restrictions(poppler::Permissions::FULL.bits()).is_empty());
        let r = restrictions(poppler::Permissions::OK_TO_PRINT.bits());
        assert_eq!(r, vec!["copying", "modifying", "annotating"]);
    }
}
