//! Resources page: CPU, memory/swap and network history graphs (cairo).

use adw::prelude::*;
use gtk::gdk;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

use crate::format;
use crate::history::{nice_ceiling, History};
use crate::sampler::ResourceSample;

/// Distinct colours for per-CPU lines (GNOME palette hues).
const CPU_COLORS: [&str; 16] = [
    "#e01b24", "#ff7800", "#f6d32d", "#33d17a", "#26a269", "#3584e4", "#1c71d8", "#9141ac",
    "#c061cb", "#865e3c", "#ed333b", "#ffa348", "#57e389", "#62a0ea", "#dc8add", "#cdab8f",
];

pub struct Colors {
    pub mem: gdk::RGBA,
    pub swap: gdk::RGBA,
    pub net_in: gdk::RGBA,
    pub net_out: gdk::RGBA,
}

pub fn parse_color(s: &str, fallback: &str) -> gdk::RGBA {
    gdk::RGBA::parse(s)
        .or_else(|_| gdk::RGBA::parse(fallback))
        .unwrap_or(gdk::RGBA::RED)
}

fn cpu_color(i: usize) -> gdk::RGBA {
    parse_color(CPU_COLORS[i % CPU_COLORS.len()], "#3584e4")
}

#[derive(Clone, Copy, PartialEq)]
enum Scale {
    /// Values are fractions, axis 0–100 %.
    Percent,
    /// Values are bytes/second, auto-scaled axis.
    Rate,
}

struct Graph {
    area: gtk::DrawingArea,
    history: RefCell<History>,
    colors: RefCell<Vec<gdk::RGBA>>,
    scale: Scale,
    bits: Cell<bool>,
    interval_ms: Cell<u32>,
}

impl Graph {
    /// `name` is the graph's accessible name ("CPU History"); its
    /// description gives the current values (see [`Graph::describe`]).
    fn new(scale: Scale, points: usize, name: &str) -> Rc<Self> {
        let area = gtk::DrawingArea::builder()
            .content_height(160)
            .hexpand(true)
            .accessible_role(gtk::AccessibleRole::Img)
            .build();
        area.update_property(&[gtk::accessible::Property::Label(name)]);
        let g = Rc::new(Graph {
            area,
            history: RefCell::new(History::new(points)),
            colors: RefCell::new(Vec::new()),
            scale,
            bits: Cell::new(false),
            interval_ms: Cell::new(3000),
        });
        let weak = Rc::downgrade(&g);
        g.area.set_draw_func(move |area, cr, w, h| {
            if let Some(g) = weak.upgrade() {
                g.draw(area, cr, w as f64, h as f64);
            }
        });
        g
    }

    fn describe(&self, description: &str) {
        self.area
            .update_property(&[gtk::accessible::Property::Description(description)]);
    }

    fn draw(&self, area: &gtk::DrawingArea, cr: &gtk::cairo::Context, w: f64, h: f64) {
        let fg = area.color();
        let history = self.history.borrow();
        let max = match self.scale {
            Scale::Percent => 1.0,
            Scale::Rate => nice_ceiling(history.max(), 1000.0),
        };
        let label_w = 80.0;
        let (left, top, right, bottom) = (label_w, 6.0, w - 4.0, h - 18.0);
        let gw = (right - left).max(1.0);
        let gh = (bottom - top).max(1.0);

        cr.set_line_width(1.0);
        cr.set_font_size(10.0);
        // Grid with axis labels.
        for i in 0..=4 {
            let y = (top + gh * i as f64 / 4.0).round() + 0.5;
            cr.set_source_rgba(fg.red() as f64, fg.green() as f64, fg.blue() as f64, 0.15);
            cr.move_to(left, y);
            cr.line_to(right, y);
            let _ = cr.stroke();
            let v = max * (4 - i) as f64 / 4.0;
            let text = match self.scale {
                Scale::Percent => format!("{:.0} %", v * 100.0),
                Scale::Rate => format::rate(v, self.bits.get()),
            };
            cr.set_source_rgba(fg.red() as f64, fg.green() as f64, fg.blue() as f64, 0.7);
            if let Ok(ext) = cr.text_extents(&text) {
                cr.move_to(left - 6.0 - ext.width(), y + ext.height() / 2.0);
                let _ = cr.show_text(&text);
            }
        }
        // Time labels.
        let span_s =
            history.capacity().saturating_sub(1) as f64 * self.interval_ms.get() as f64 / 1000.0;
        for (frac, text) in [
            (0.0, format!("{span_s:.0} seconds")),
            (0.5, format!("{:.0}", span_s / 2.0)),
            (1.0, "0".to_string()),
        ] {
            if let Ok(ext) = cr.text_extents(&text) {
                let x = (left + gw * frac - ext.width() * frac).clamp(left, right - ext.width());
                cr.move_to(x, h - 4.0);
                let _ = cr.show_text(&text);
            }
        }
        cr.rectangle(left + 0.5, top + 0.5, gw - 1.0, gh);
        cr.set_source_rgba(fg.red() as f64, fg.green() as f64, fg.blue() as f64, 0.3);
        let _ = cr.stroke();

        // Series, newest at the right edge.
        let cap = history.capacity();
        let step = gw / (cap - 1) as f64;
        let colors = self.colors.borrow();
        cr.set_line_width(1.5);
        cr.set_line_join(gtk::cairo::LineJoin::Round);
        for (i, series) in history.series().iter().enumerate() {
            let c = colors.get(i).copied().unwrap_or(fg);
            cr.set_source_rgba(c.red() as f64, c.green() as f64, c.blue() as f64, 1.0);
            let offset = cap - series.len();
            for (j, v) in series.iter().enumerate() {
                let x = left + (offset + j) as f64 * step;
                let y = bottom - (v / max).clamp(0.0, 1.0) * gh;
                if j == 0 {
                    cr.move_to(x, y);
                } else {
                    cr.line_to(x, y);
                }
            }
            let _ = cr.stroke();
        }
    }
}

/// A legend's colour sample; the entry's text says what it stands for.
fn swatch(color: gdk::RGBA) -> gtk::DrawingArea {
    let a = gtk::DrawingArea::builder()
        .content_width(14)
        .content_height(14)
        .valign(gtk::Align::Center)
        .accessible_role(gtk::AccessibleRole::Presentation)
        .build();
    a.set_draw_func(move |_, cr, w, h| {
        cr.set_source_rgba(
            color.red() as f64,
            color.green() as f64,
            color.blue() as f64,
            1.0,
        );
        cr.rectangle(0.0, 0.0, w as f64, h as f64);
        let _ = cr.fill();
    });
    a
}

/// A legend entry: swatch, title and value. Assistive technology reads it
/// as one text, "CPU1: 12.5 %" (see [`set_legend_value`]).
fn legend_entry(color: gdk::RGBA, title: &str) -> (gtk::Box, gtk::Label) {
    let b = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(8)
        .accessible_role(gtk::AccessibleRole::Label)
        .build();
    b.update_property(&[gtk::accessible::Property::Label(title)]);
    b.append(&swatch(color));
    let v = gtk::Box::new(gtk::Orientation::Vertical, 0);
    let t = gtk::Label::builder()
        .label(title)
        .xalign(0.0)
        .accessible_role(gtk::AccessibleRole::Presentation)
        .build();
    t.add_css_class("caption-heading");
    let value = gtk::Label::builder()
        .xalign(0.0)
        .accessible_role(gtk::AccessibleRole::Presentation)
        .build();
    value.add_css_class("caption");
    value.add_css_class("numeric");
    v.append(&t);
    v.append(&value);
    b.append(&v);
    (b, value)
}

/// Shows `value` in a legend entry and reads the entry as "Title: value".
fn set_legend_value(value_label: &gtk::Label, value: &str) {
    value_label.set_label(value);
    let entry = value_label.parent().and_then(|v| v.parent());
    let title = entry
        .as_ref()
        .and_then(|e| e.last_child())
        .and_then(|v| v.first_child())
        .and_downcast::<gtk::Label>();
    if let (Some(entry), Some(title)) = (entry, title) {
        entry.update_property(&[gtk::accessible::Property::Label(&legend_text(
            &title.label(),
            value,
        ))]);
    }
}

/// One legend entry as text: "CPU1: 12.5 %".
fn legend_text(title: &str, value: &str) -> String {
    format!("{title}: {value}")
}

/// The CPU graph's description: the average, then each CPU.
fn cpu_description(cpus: &[f64]) -> String {
    if cpus.is_empty() {
        return String::new();
    }
    let avg = cpus.iter().sum::<f64>() / cpus.len() as f64;
    let each: Vec<String> = cpus
        .iter()
        .enumerate()
        .map(|(i, v)| format!("CPU{} {:.1} %", i + 1, v * 100.0))
        .collect();
    format!("Average {:.1} %; {}", avg * 100.0, each.join(", "))
}

fn section(title: &str, graph: &gtk::DrawingArea, legend: &impl IsA<gtk::Widget>) -> gtk::Box {
    let b = gtk::Box::new(gtk::Orientation::Vertical, 8);
    let l = gtk::Label::builder().label(title).xalign(0.0).build();
    l.add_css_class("heading");
    b.append(&l);
    b.append(graph);
    b.append(legend);
    b
}

pub struct ResView {
    pub page: gtk::ScrolledWindow,
    cpu: Rc<Graph>,
    mem: Rc<Graph>,
    net: Rc<Graph>,
    cpu_legend: gtk::FlowBox,
    cpu_labels: RefCell<Vec<gtk::Label>>,
    mem_label: gtk::Label,
    swap_label: gtk::Label,
    rx_label: gtk::Label,
    tx_label: gtk::Label,
    pub iec: Cell<bool>,
    pub bits: Cell<bool>,
}

impl ResView {
    pub fn new(points: usize, colors: Colors) -> Rc<Self> {
        let cpu = Graph::new(Scale::Percent, points, "CPU History");
        let mem = Graph::new(Scale::Percent, points, "Memory and Swap History");
        let net = Graph::new(Scale::Rate, points, "Network History");
        *mem.colors.borrow_mut() = vec![colors.mem, colors.swap];
        *net.colors.borrow_mut() = vec![colors.net_in, colors.net_out];

        let cpu_legend = gtk::FlowBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .max_children_per_line(8)
            .min_children_per_line(2)
            .column_spacing(12)
            .row_spacing(6)
            .homogeneous(true)
            .build();

        let mem_legend = gtk::Box::new(gtk::Orientation::Horizontal, 24);
        let (e, mem_label) = legend_entry(colors.mem, "Memory");
        mem_legend.append(&e);
        let (e, swap_label) = legend_entry(colors.swap, "Swap");
        mem_legend.append(&e);

        let net_legend = gtk::Box::new(gtk::Orientation::Horizontal, 24);
        let (e, rx_label) = legend_entry(colors.net_in, "Receiving");
        net_legend.append(&e);
        let (e, tx_label) = legend_entry(colors.net_out, "Sending");
        net_legend.append(&e);

        let content = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(24)
            .margin_top(18)
            .margin_bottom(18)
            .margin_start(18)
            .margin_end(18)
            .build();
        content.append(&section("CPU", &cpu.area, &cpu_legend));
        content.append(&section("Memory and Swap", &mem.area, &mem_legend));
        content.append(&section("Network", &net.area, &net_legend));
        let page = gtk::ScrolledWindow::builder()
            .child(&content)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .build();

        Rc::new(ResView {
            page,
            cpu,
            mem,
            net,
            cpu_legend,
            cpu_labels: RefCell::default(),
            mem_label,
            swap_label,
            rx_label,
            tx_label,
            iec: Cell::new(false),
            bits: Cell::new(false),
        })
    }

    pub fn set_interval(&self, ms: u32) {
        for g in [&self.cpu, &self.mem, &self.net] {
            g.interval_ms.set(ms);
            g.area.queue_draw();
        }
    }

    pub fn set_bits(&self, bits: bool) {
        self.bits.set(bits);
        self.net.bits.set(bits);
        self.net.area.queue_draw();
    }

    fn ensure_cpu_legend(&self, n: usize) {
        if self.cpu_labels.borrow().len() == n {
            return;
        }
        while let Some(c) = self.cpu_legend.first_child() {
            self.cpu_legend.remove(&c);
        }
        let mut labels = Vec::new();
        let mut colors = Vec::new();
        for i in 0..n {
            let c = cpu_color(i);
            let (e, l) = legend_entry(c, &format!("CPU{}", i + 1));
            self.cpu_legend.append(&e);
            labels.push(l);
            colors.push(c);
        }
        *self.cpu.colors.borrow_mut() = colors;
        *self.cpu_labels.borrow_mut() = labels;
    }

    pub fn push(&self, r: &ResourceSample) {
        self.ensure_cpu_legend(r.cpus.len());
        for (l, v) in self.cpu_labels.borrow().iter().zip(&r.cpus) {
            set_legend_value(l, &format!("{:.1} %", v * 100.0));
        }
        self.cpu.describe(&cpu_description(&r.cpus));
        self.cpu.history.borrow_mut().push(&r.cpus);

        let frac = |used: u64, total: u64| {
            if total == 0 {
                0.0
            } else {
                used as f64 / total as f64
            }
        };
        let m = &r.mem;
        let mem_f = frac(m.mem_used(), m.mem_total);
        let swap_f = frac(m.swap_used(), m.swap_total);
        self.mem.history.borrow_mut().push(&[mem_f, swap_f]);
        let iec = self.iec.get();
        let mem_text = format!(
            "{} ({:.1} %) of {}",
            format::size(m.mem_used(), iec),
            mem_f * 100.0,
            format::size(m.mem_total, iec)
        );
        let swap_text = if m.swap_total == 0 {
            "Not available".to_string()
        } else {
            format!(
                "{} ({:.1} %) of {}",
                format::size(m.swap_used(), iec),
                swap_f * 100.0,
                format::size(m.swap_total, iec)
            )
        };
        set_legend_value(&self.mem_label, &mem_text);
        set_legend_value(&self.swap_label, &swap_text);
        self.mem.describe(&format!(
            "{}, {}",
            legend_text("Memory", &mem_text),
            legend_text("Swap", &swap_text)
        ));

        self.net.history.borrow_mut().push(&[r.rx_rate, r.tx_rate]);
        let bits = self.bits.get();
        set_legend_value(
            &self.rx_label,
            &format!(
                "{}   Total received {}",
                format::rate(r.rx_rate, bits),
                format::size(r.rx_total, false)
            ),
        );
        set_legend_value(
            &self.tx_label,
            &format!(
                "{}   Total sent {}",
                format::rate(r.tx_rate, bits),
                format::size(r.tx_total, false)
            ),
        );
        self.net.describe(&format!(
            "Receiving {}, Sending {}",
            format::rate(r.rx_rate, bits),
            format::rate(r.tx_rate, bits)
        ));

        for g in [&self.cpu, &self.mem, &self.net] {
            g.area.queue_draw();
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn graphs_and_legends_as_text() {
        assert_eq!(legend_text("CPU3", "12.5 %"), "CPU3: 12.5 %");
        assert_eq!(
            cpu_description(&[0.1, 0.3]),
            "Average 20.0 %; CPU1 10.0 %, CPU2 30.0 %"
        );
        assert_eq!(cpu_description(&[]), "");
    }
}
