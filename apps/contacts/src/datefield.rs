//! A date editor for birthdays and anniversaries: day and month
//! drop-downs, an optional year and a calendar popover, so no one has to
//! type ISO dates. Invalid combinations ("31 February") mark the field.

use adw::prelude::*;
use gtk::glib;
use helix_app_common::a11y::set_input_error;
use helix_app_common::i18n::tr;
use helix_app_common::trf;
use helix_eds::vcard;
use std::rc::Rc;

/// Placeholder entry of the day and month drop-downs: "not set".
const UNSET: &str = "—";

fn is_leap(year: i32) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

/// Days in `month` (1–12); without a year February allows the 29th.
fn days_in_month(month: u32, year: Option<i32>) -> u32 {
    match month {
        4 | 6 | 9 | 11 => 30,
        2 if year.is_none_or(is_leap) => 29,
        2 => 28,
        _ => 31,
    }
}

/// A date as entered: (year, month, day).
pub type Ymd = (Option<i32>, u32, u32);

/// Checks a date entered as `day` and `month` (0 = not chosen) and the
/// `year` text (empty = no year). `Ok(None)` means no date.
pub fn compose(day: u32, month: u32, year: &str) -> Result<Option<Ymd>, String> {
    let year = year.trim();
    if day == 0 && month == 0 {
        return if year.is_empty() {
            Ok(None)
        } else {
            Err(tr("Choose a day and month").into())
        };
    }
    if day == 0 || month == 0 {
        return Err(tr("Choose both a day and a month").into());
    }
    let year = if year.is_empty() {
        None
    } else {
        match year.parse::<i32>() {
            Ok(y) if (1..=9999).contains(&y) => Some(y),
            _ => return Err(tr("Enter the year as a number, like 1990").into()),
        }
    };
    let max = days_in_month(month, year);
    if day > max {
        let name = helix_app_common::i18n::month_name(month);
        return Err(match year {
            Some(y) if month == 2 => trf!("February {y} has only {max} days", y = y, max = max),
            _ => trf!("{name} has only {max} days", name = name, max = max),
        });
    }
    Ok(Some((year, month, day)))
}

/// The vCard value for a date: `1990-05-17`, or `--05-17` without a year.
pub fn to_vcard((year, month, day): Ymd) -> String {
    match year {
        Some(y) => vcard::format_date(y, month, day),
        None => format!("--{month:02}-{day:02}"),
    }
}

pub struct DateField {
    /// What the date is: "Birthday", "Anniversary".
    title: &'static str,
    pub row: gtk::ListBoxRow,
    day: gtk::DropDown,
    month: gtk::DropDown,
    year: gtk::Entry,
    error: gtk::Label,
    original: String,
}

impl DateField {
    pub fn new(title: &'static str, value: &str) -> Rc<DateField> {
        let heading = gtk::Label::builder()
            .label(title)
            .xalign(0.0)
            .css_classes(["dim-label", "caption-heading"])
            .build();
        let mut days = vec![UNSET.to_string()];
        days.extend((1..=31).map(|d| d.to_string()));
        let days: Vec<&str> = days.iter().map(String::as_str).collect();
        let day = gtk::DropDown::from_strings(&days);
        helix_app_common::set_accessible_label(&day, &trf!("{title} Day", title = title));
        let mut months = vec![UNSET];
        months.extend((1..=12).map(helix_app_common::i18n::month_name));
        let month = gtk::DropDown::from_strings(&months);
        helix_app_common::set_accessible_label(&month, &trf!("{title} Month", title = title));
        let year = gtk::Entry::builder()
            .placeholder_text(tr("Year"))
            .width_chars(4)
            .max_width_chars(5)
            .max_length(4)
            .input_purpose(gtk::InputPurpose::Digits)
            .tooltip_text(tr("Year (optional)"))
            .build();
        helix_app_common::set_accessible_label(&year, &trf!("{title} Year", title = title));
        year.update_property(&[gtk::accessible::Property::Description(tr("Optional"))]);

        let calendar = gtk::Calendar::new();
        let pop = gtk::Popover::builder().child(&calendar).build();
        let pick = gtk::MenuButton::builder()
            .icon_name("x-office-calendar-symbolic")
            .tooltip_text(tr("Pick from Calendar"))
            .css_classes(["flat"])
            .valign(gtk::Align::Center)
            .popover(&pop)
            .build();
        helix_app_common::set_accessible_label(
            &pick,
            &trf!("Pick {title} from Calendar", title = title),
        );

        let controls = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        controls.append(&day);
        controls.append(&month);
        controls.append(&year);
        controls.append(&pick);
        let error = gtk::Label::builder()
            .xalign(0.0)
            .wrap(true)
            .visible(false)
            .css_classes(["error", "caption"])
            .build();
        let content = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(6)
            .margin_top(8)
            .margin_bottom(8)
            .margin_start(12)
            .margin_end(12)
            .build();
        content.append(&heading);
        content.append(&controls);
        content.append(&error);
        let row = gtk::ListBoxRow::builder()
            .child(&content)
            .activatable(false)
            .focusable(false)
            .build();
        row.update_property(&[gtk::accessible::Property::Label(title)]);

        let f = Rc::new(DateField {
            title,
            row,
            day,
            month,
            year,
            error,
            original: value.to_string(),
        });
        if let Some(ymd) = vcard::parse_date(value) {
            f.set(ymd);
        }
        if let Some((y, m, d)) = vcard::parse_date(value) {
            if let Ok(dt) =
                glib::DateTime::from_local(y.unwrap_or(2000), m as i32, d as i32, 0, 0, 0.0)
            {
                calendar.select_day(&dt);
            }
        }
        let weak = Rc::downgrade(&f);
        calendar.connect_day_selected(move |c| {
            if let Some(f) = weak.upgrade() {
                let d = c.date();
                f.set((Some(d.year()), d.month() as u32, d.day_of_month() as u32));
            }
            pop.popdown();
        });
        for dd in [&f.day, &f.month] {
            let weak = Rc::downgrade(&f);
            dd.connect_selected_notify(move |_| {
                if let Some(f) = weak.upgrade() {
                    f.validate();
                }
            });
        }
        let weak = Rc::downgrade(&f);
        f.year.connect_changed(move |_| {
            if let Some(f) = weak.upgrade() {
                f.validate();
            }
        });
        f
    }

    fn set(&self, (year, month, day): Ymd) {
        self.day.set_selected(day.min(31));
        self.month.set_selected(month.min(12));
        self.year
            .set_text(&year.map(|y| y.to_string()).unwrap_or_default());
        self.validate();
    }

    fn entered(&self) -> Result<Option<Ymd>, String> {
        compose(
            self.day.selected(),
            self.month.selected(),
            &self.year.text(),
        )
    }

    /// Shows or clears the inline error for the current input.
    fn validate(&self) {
        let err = self.entered().err();
        self.error.set_label(err.as_deref().unwrap_or(""));
        self.error.set_visible(err.is_some());
        // Mark the control that needs fixing.
        let target: &gtk::Widget = match err.as_deref() {
            Some(e) if e.starts_with("Enter the year") => self.year.upcast_ref(),
            Some(e) if e.contains(" has only ") => self.day.upcast_ref(),
            _ if self.day.selected() == 0 => self.day.upcast_ref(),
            _ => self.month.upcast_ref(),
        };
        for w in [
            self.day.upcast_ref::<gtk::Widget>(),
            self.month.upcast_ref(),
            self.year.upcast_ref(),
        ] {
            if err.is_some() && w == target {
                w.add_css_class("error");
                set_input_error(w, Some(&self.error), err.as_deref());
            } else {
                w.remove_css_class("error");
                set_input_error(w, None, None);
            }
        }
    }

    /// The vCard value, or a message naming the field when it is invalid.
    /// An unchanged date keeps its original text (time, format).
    pub fn value(&self) -> Result<String, String> {
        match self.entered() {
            Ok(None) => Ok(String::new()),
            Ok(Some(ymd)) if vcard::parse_date(&self.original) == Some(ymd) => {
                Ok(self.original.clone())
            }
            Ok(Some(ymd)) => Ok(to_vcard(ymd)),
            Err(e) => Err(format!("{}: {e}", self.title)),
        }
    }

    /// Moves focus to the field (after a failed save).
    pub fn focus(&self) {
        if self.year.has_css_class("error") {
            self.year.grab_focus();
        } else if self.month.has_css_class("error") {
            self.month.grab_focus();
        } else {
            self.day.grab_focus();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_compose_without_typing_iso() {
        assert_eq!(compose(0, 0, ""), Ok(None));
        assert_eq!(compose(17, 5, "1990"), Ok(Some((Some(1990), 5, 17))));
        assert_eq!(compose(17, 5, " "), Ok(Some((None, 5, 17))));
        assert_eq!(to_vcard((Some(1990), 5, 17)), "1990-05-17");
        assert_eq!(to_vcard((None, 5, 7)), "--05-07");
        // Round trip through the vCard parser.
        assert_eq!(vcard::parse_date("--05-07"), Some((None, 5, 7)));
    }

    #[test]
    fn invalid_dates_are_explained() {
        assert!(compose(17, 0, "").is_err());
        assert!(compose(0, 0, "1990").is_err());
        assert!(compose(1, 1, "19x0").is_err());
        assert_eq!(compose(31, 4, "").unwrap_err(), "April has only 30 days");
        assert_eq!(
            compose(29, 2, "2023").unwrap_err(),
            "February 2023 has only 28 days"
        );
        assert_eq!(compose(29, 2, "2024"), Ok(Some((Some(2024), 2, 29))));
        // Without a year, 29 February is allowed.
        assert_eq!(compose(29, 2, ""), Ok(Some((None, 2, 29))));
    }
}
