//! GTK user interface.

use adw::prelude::*;
use gtk::{gio, glib};
use helix_app_common::Prefs;
use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::rc::Rc;

use crate::engine::{to_signed, AngleUnit, Context, Mode, Value};
use crate::financial;
use crate::format::{self, NumberFormat, Options};

pub const APP_ID: &str = "org.helix.Calculator";
const SCHEMA: &str = "org.gnome.calculator";
const MAX_HISTORY: usize = 100;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ButtonMode {
    Basic,
    Advanced,
    Financial,
    Programming,
}

impl ButtonMode {
    const ALL: [ButtonMode; 4] = [
        ButtonMode::Basic,
        ButtonMode::Advanced,
        ButtonMode::Financial,
        ButtonMode::Programming,
    ];
    fn nick(self) -> &'static str {
        match self {
            ButtonMode::Basic => "basic",
            ButtonMode::Advanced => "advanced",
            ButtonMode::Financial => "financial",
            ButtonMode::Programming => "programming",
        }
    }
    fn label(self) -> &'static str {
        match self {
            ButtonMode::Basic => "Basic",
            ButtonMode::Advanced => "Advanced",
            ButtonMode::Financial => "Financial",
            ButtonMode::Programming => "Programming",
        }
    }
    fn from_nick(s: &str) -> Self {
        Self::ALL
            .into_iter()
            .find(|m| m.nick() == s)
            .unwrap_or(ButtonMode::Basic)
    }
}

#[derive(Serialize, Deserialize, Clone)]
struct HistoryEntry {
    expression: String,
    result: String,
}

struct State {
    ctx: Context,
    prefs: Prefs,
    mode: ButtonMode,
    options: Options,
    history: Vec<HistoryEntry>,
    just_evaluated: bool,
}

impl State {
    fn sync_ctx_mode(&mut self) {
        self.ctx.mode = match self.mode {
            ButtonMode::Programming => Mode::Integer {
                bits: self.options.word_size,
                base: self.options.base,
            },
            _ => Mode::Real,
        };
        if self.mode != ButtonMode::Programming {
            self.options.base = 10;
        } else {
            self.options.base = self.prefs.get::<i32>("base", 10) as u32;
        }
    }

    fn load_options(prefs: &Prefs) -> Options {
        let word = prefs.get::<i32>("word-size", 64) as u32;
        Options {
            format: NumberFormat::from_nick(&prefs.get_enum_nick("number-format", "automatic")),
            accuracy: prefs.get::<i32>("accuracy", 9).clamp(0, 15) as usize,
            thousands: prefs.get("show-thousands", false),
            show_zeroes: prefs.get("show-zeroes", false),
            base: 10,
            word_size: if [8, 16, 32, 64].contains(&word) {
                word
            } else {
                64
            },
        }
    }
}

fn history_path() -> std::path::PathBuf {
    helix_app_common::data_dir("helix-calculator").join("history.json")
}

fn load_history() -> Vec<HistoryEntry> {
    std::fs::read(history_path())
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

fn save_history(h: &[HistoryEntry]) {
    if let Ok(json) = serde_json::to_vec_pretty(h) {
        let _ = std::fs::write(history_path(), json);
    }
}

/// What a button does.
#[derive(Clone)]
enum Act {
    Insert(&'static str),
    Eval,
    Clear,
    Backspace,
    Financial(usize),
}

struct Key {
    label: &'static str,
    act: Act,
    tooltip: Option<&'static str>,
    class: Option<&'static str>,
}

fn k(label: &'static str, insert: &'static str) -> Key {
    Key {
        label,
        act: Act::Insert(insert),
        tooltip: None,
        class: None,
    }
}

fn kt(label: &'static str, insert: &'static str, tooltip: &'static str) -> Key {
    Key {
        label,
        act: Act::Insert(insert),
        tooltip: Some(tooltip),
        class: None,
    }
}

fn basic_keys(programming: bool) -> Vec<Vec<Key>> {
    let op = |label, ins, tip| Key {
        label,
        act: Act::Insert(ins),
        tooltip: Some(tip),
        class: Some("calc-op"),
    };
    vec![
        vec![
            k("7", "7"),
            k("8", "8"),
            k("9", "9"),
            op("÷", "÷", "Divide [/]"),
            Key {
                label: "⌫",
                act: Act::Backspace,
                tooltip: Some("Backspace"),
                class: None,
            },
            Key {
                label: "C",
                act: Act::Clear,
                tooltip: Some("Clear [Escape]"),
                class: Some("destructive-action"),
            },
        ],
        vec![
            k("4", "4"),
            k("5", "5"),
            k("6", "6"),
            op("×", "×", "Multiply [*]"),
            k("(", "("),
            k(")", ")"),
        ],
        vec![
            k("1", "1"),
            k("2", "2"),
            k("3", "3"),
            op("−", "−", "Subtract [-]"),
            kt("x²", "²", "Square [Ctrl+2]"),
            kt("√", "√", "Square root [Ctrl+R]"),
        ],
        vec![
            k("0", "0"),
            if programming {
                kt("mod", " mod ", "Modulus divide")
            } else {
                kt(".", ".", "Decimal point")
            },
            if programming {
                kt("x!", "!", "Factorial")
            } else {
                kt("%", "%", "Percentage [%]")
            },
            op("+", "+", "Add [+]"),
            Key {
                label: "=",
                act: Act::Eval,
                tooltip: Some("Calculate result [Enter]"),
                class: Some("suggested-action"),
            },
        ],
    ]
}

fn advanced_keys() -> Vec<Vec<Key>> {
    vec![
        vec![
            kt("sin", "sin ", "Sine"),
            kt("cos", "cos ", "Cosine"),
            kt("tan", "tan ", "Tangent"),
            kt("xʸ", "^", "Exponent [^]"),
        ],
        vec![
            kt("sin⁻¹", "sin⁻¹ ", "Inverse sine"),
            kt("cos⁻¹", "cos⁻¹ ", "Inverse cosine"),
            kt("tan⁻¹", "tan⁻¹ ", "Inverse tangent"),
            kt("x⁻¹", "⁻¹", "Inverse"),
        ],
        vec![
            kt("ln", "ln ", "Natural logarithm"),
            kt("log", "log ", "Logarithm"),
            kt("eˣ", "e^", "Exponential"),
            kt("x!", "!", "Factorial [!]"),
        ],
        vec![
            kt("π", "π", "Pi [Ctrl+P]"),
            kt("e", "e", "Euler's number"),
            kt("|x|", "|", "Absolute value [|]"),
            kt("mod", " mod ", "Modulus divide"),
        ],
        vec![
            kt("ans", "ans", "Last answer"),
            kt("×10ʸ", "×10^", "Scientific exponent"),
            kt("∛", "∛", "Cube root"),
            kt("rand", "rand", "Random number"),
        ],
    ]
}

fn programming_keys() -> Vec<Vec<Key>> {
    vec![
        vec![
            k("A", "A"),
            k("B", "B"),
            kt("and", " and ", "Boolean AND"),
            kt("<<", "<<", "Shift left"),
        ],
        vec![
            k("C", "C"),
            k("D", "D"),
            kt("or", " or ", "Boolean OR"),
            kt(">>", ">>", "Shift right"),
        ],
        vec![
            k("E", "E"),
            k("F", "F"),
            kt("xor", " xor ", "Boolean exclusive OR"),
            kt("not", "not ", "Boolean NOT"),
        ],
        vec![
            kt("ones", "ones ", "One's complement"),
            kt("twos", "twos ", "Two's complement"),
            kt("xʸ", "^", "Exponent"),
            kt("ans", "ans", "Last answer"),
        ],
    ]
}

fn financial_keys() -> Vec<Vec<Key>> {
    let f = |i: usize| Key {
        label: financial::FUNCTIONS[i].name,
        act: Act::Financial(i),
        tooltip: Some(financial::FUNCTIONS[i].title),
        class: None,
    };
    vec![
        vec![f(0), f(1), f(2)],
        vec![f(3), f(4), f(5)],
        vec![f(6), f(7), f(8)],
        vec![f(9), kt("%", "%", "Percentage"), kt("xʸ", "^", "Exponent")],
    ]
}

#[derive(Clone)]
struct Ui {
    window: adw::ApplicationWindow,
    entry: gtk::Entry,
    error: gtk::Label,
    history: gtk::ListBox,
    history_scroll: gtk::ScrolledWindow,
    keypad: gtk::Stack,
    prog_bar: gtk::Box,
    bits: gtk::Grid,
    digit_buttons: Rc<RefCell<Vec<(gtk::Button, u32)>>>,
    state: Rc<RefCell<State>>,
}

pub fn run(initial_expr: Option<String>) -> glib::ExitCode {
    let app = adw::Application::builder()
        .application_id(APP_ID)
        .flags(gio::ApplicationFlags::NON_UNIQUE)
        .build();
    helix_app_common::install_standard_actions(
        &app,
        "Calculator",
        "Perform arithmetic, scientific, financial and programming calculations",
    );
    let initial = RefCell::new(initial_expr);
    app.connect_startup(|_| load_css());
    app.connect_activate(move |app| {
        let ui = build(app);
        if let Some(e) = initial.borrow_mut().take() {
            ui.entry.set_text(&e);
            ui.entry.set_position(-1);
        }
        ui.window.present();
    });
    app.run_with_args(&[std::env::args().next().unwrap_or_default()])
}

fn load_css() {
    let css = gtk::CssProvider::new();
    css.load_from_string(
        ".calc-entry { font-size: 22pt; min-height: 56px; }
         .calc-keypad button { min-width: 52px; min-height: 40px; font-size: 13pt; }
         .calc-op { font-weight: bold; }
         .calc-history-result { font-weight: bold; }
         .calc-bit { min-width: 14px; padding: 0 1px; font-family: monospace; font-size: 9pt; }
         .calc-bit-index { font-size: 7pt; }",
    );
    if let Some(display) = gtk::gdk::Display::default() {
        gtk::style_context_add_provider_for_display(
            &display,
            &css,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    }
}

fn build(app: &adw::Application) -> Ui {
    let prefs = Prefs::new(SCHEMA);
    let options = State::load_options(&prefs);
    let mode = ButtonMode::from_nick(&prefs.get_enum_nick("button-mode", "basic"));
    let ctx = Context {
        angle: AngleUnit::from_nick(&prefs.get_enum_nick("angle-units", "degrees")),
        ..Context::default()
    };
    let mut state = State {
        ctx,
        prefs: prefs.clone(),
        mode,
        options,
        history: load_history(),
        just_evaluated: false,
    };
    state.sync_ctx_mode();
    let state = Rc::new(RefCell::new(state));

    let (w, h) = prefs.get::<(i32, i32)>("window-size", (0, 0));
    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("Calculator")
        .default_width(if w > 200 { w } else { 420 })
        .default_height(if h > 200 { h } else { 560 })
        .build();
    if prefs.get("window-maximized", false) {
        window.maximize();
    }

    // Header bar.
    let header = adw::HeaderBar::new();
    let modes = gtk::DropDown::from_strings(&ButtonMode::ALL.map(|m| m.label()));
    modes.set_selected(ButtonMode::ALL.iter().position(|m| *m == mode).unwrap_or(0) as u32);
    modes.set_tooltip_text(Some("Calculator mode"));
    header.set_title_widget(Some(&modes));
    let menu = gio::Menu::new();
    let section = gio::Menu::new();
    section.append(Some("_Clear History"), Some("win.clear-history"));
    menu.append_section(None, &section);
    let section = gio::Menu::new();
    section.append(Some("_Preferences"), Some("win.preferences"));
    section.append(Some("_Keyboard Shortcuts"), Some("win.shortcuts"));
    section.append(Some("_About Calculator"), Some("app.about"));
    menu.append_section(None, &section);
    let menu_button = gtk::MenuButton::builder()
        .icon_name("open-menu-symbolic")
        .menu_model(&menu)
        .primary(true)
        .tooltip_text("Main Menu")
        .build();
    header.pack_end(&menu_button);

    // History.
    let history = gtk::ListBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .css_classes(["navigation-sidebar"])
        .build();
    let history_scroll = gtk::ScrolledWindow::builder()
        .child(&history)
        .vexpand(true)
        .hscrollbar_policy(gtk::PolicyType::Never)
        .min_content_height(80)
        .build();

    // Entry.
    let entry = gtk::Entry::builder()
        .css_classes(["calc-entry"])
        .xalign(1.0)
        .build();
    entry.set_input_hints(gtk::InputHints::NO_EMOJI);
    let error = gtk::Label::builder()
        .xalign(1.0)
        .css_classes(["error", "caption"])
        .wrap(true)
        .visible(false)
        .build();

    // Programming bar: base + word size + bit panel.
    let prog_bar = gtk::Box::new(gtk::Orientation::Vertical, 6);
    let prog_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let base_dd = gtk::DropDown::from_strings(&["Binary", "Octal", "Decimal", "Hexadecimal"]);
    let bases = [2u32, 8, 10, 16];
    base_dd.set_selected(
        bases
            .iter()
            .position(|b| *b == state.borrow().prefs.get::<i32>("base", 10) as u32)
            .unwrap_or(2) as u32,
    );
    base_dd.set_tooltip_text(Some("Number base"));
    let word_dd = gtk::DropDown::from_strings(&["8-bit", "16-bit", "32-bit", "64-bit"]);
    let words = [8u32, 16, 32, 64];
    word_dd.set_selected(
        words
            .iter()
            .position(|b| *b == options.word_size)
            .unwrap_or(3) as u32,
    );
    word_dd.set_tooltip_text(Some("Word size"));
    prog_row.append(&base_dd);
    prog_row.append(&word_dd);
    let bits = gtk::Grid::builder()
        .column_spacing(0)
        .row_spacing(0)
        .halign(gtk::Align::Center)
        .build();
    prog_bar.append(&prog_row);
    prog_bar.append(&bits);

    let keypad = gtk::Stack::builder()
        .transition_type(gtk::StackTransitionType::Crossfade)
        .build();

    let content = gtk::Box::new(gtk::Orientation::Vertical, 6);
    content.set_margin_start(12);
    content.set_margin_end(12);
    content.set_margin_bottom(12);
    content.append(&history_scroll);
    content.append(&entry);
    content.append(&error);
    content.append(&prog_bar);
    content.append(&keypad);

    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&header);
    toolbar.set_content(Some(&content));
    window.set_content(Some(&toolbar));

    let ui = Ui {
        window: window.clone(),
        entry: entry.clone(),
        error,
        history,
        history_scroll,
        keypad: keypad.clone(),
        prog_bar,
        bits,
        digit_buttons: Rc::default(),
        state: state.clone(),
    };

    for m in ButtonMode::ALL {
        keypad.add_named(&ui.build_keypad(m), Some(m.nick()));
    }
    ui.apply_mode();
    ui.rebuild_history();

    // Signals.
    {
        let ui = ui.clone();
        modes.connect_selected_notify(move |dd| {
            ui.set_mode(ButtonMode::ALL[dd.selected() as usize])
        });
    }
    {
        let ui = ui.clone();
        base_dd.connect_selected_notify(move |dd| {
            let base = bases[dd.selected() as usize];
            ui.state.borrow().prefs.set("base", base as i32);
            ui.convert_entry_base(base);
        });
    }
    {
        let ui = ui.clone();
        word_dd.connect_selected_notify(move |dd| {
            let bits = words[dd.selected() as usize];
            {
                let mut s = ui.state.borrow_mut();
                s.prefs.set("word-size", bits as i32);
                s.options.word_size = bits;
                s.sync_ctx_mode();
            }
            ui.rebuild_bits();
        });
    }
    {
        let ui = ui.clone();
        entry.connect_activate(move |_| ui.evaluate());
    }
    {
        let ui = ui.clone();
        entry.connect_changed(move |_| {
            ui.error.set_visible(false);
            ui.entry.remove_css_class("error");
            ui.refresh_bits();
        });
    }
    {
        // After a result, typing an operator continues from it instead of
        // replacing the selected result.
        let ui = ui.clone();
        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        keys.connect_key_pressed(move |_, key, _, mods| {
            if key == gtk::gdk::Key::Escape {
                ui.entry.set_text("");
                return glib::Propagation::Stop;
            }
            if mods
                .intersects(gtk::gdk::ModifierType::CONTROL_MASK | gtk::gdk::ModifierType::ALT_MASK)
            {
                return glib::Propagation::Proceed;
            }
            if let Some(c) = key.to_unicode() {
                if ui.state.borrow().just_evaluated && "+-*/^×÷−!%²³".contains(c) {
                    ui.entry.set_position(-1);
                }
                ui.state.borrow_mut().just_evaluated = false;
            }
            glib::Propagation::Proceed
        });
        entry.add_controller(keys);
    }

    // Window actions.
    let mode_action = gio::SimpleAction::new("mode", Some(glib::VariantTy::STRING));
    {
        let modes = modes.clone();
        mode_action.connect_activate(move |_, p| {
            if let Some(nick) = p.and_then(|p| p.get::<String>()) {
                let m = ButtonMode::from_nick(&nick);
                modes
                    .set_selected(ButtonMode::ALL.iter().position(|x| *x == m).unwrap_or(0) as u32);
            }
        });
    }
    window.add_action(&mode_action);
    let clear = gio::SimpleAction::new("clear-history", None);
    {
        let ui = ui.clone();
        clear.connect_activate(move |_, _| {
            ui.state.borrow_mut().history.clear();
            save_history(&[]);
            ui.rebuild_history();
        });
    }
    window.add_action(&clear);
    let prefs_action = gio::SimpleAction::new("preferences", None);
    {
        let ui = ui.clone();
        prefs_action.connect_activate(move |_, _| ui.show_preferences());
    }
    window.add_action(&prefs_action);
    let shortcuts = gio::SimpleAction::new("shortcuts", None);
    {
        let win = window.clone();
        shortcuts.connect_activate(move |_, _| show_shortcuts(&win));
    }
    window.add_action(&shortcuts);
    for (nick, accel) in [
        ("basic", "<Control>b"),
        ("advanced", "<Control>a"),
        ("financial", "<Control>f"),
        ("programming", "<Control>p"),
    ] {
        app.set_accels_for_action(&format!("win.mode::{nick}"), &[accel]);
    }
    app.set_accels_for_action("win.preferences", &["<Control>comma"]);
    app.set_accels_for_action("app.about", &["F1"]);

    {
        let state = state.clone();
        window.connect_close_request(move |w| {
            let s = state.borrow();
            if !w.is_maximized() {
                s.prefs
                    .set("window-size", (w.default_width(), w.default_height()));
            }
            s.prefs.set("window-maximized", w.is_maximized());
            glib::Propagation::Proceed
        });
    }

    entry.grab_focus();
    ui
}

fn show_shortcuts(win: &adw::ApplicationWindow) {
    let rows = [
        ("Calculate", "Enter"),
        ("Clear", "Escape"),
        ("Basic mode", "Ctrl+B"),
        ("Advanced mode", "Ctrl+A"),
        ("Financial mode", "Ctrl+F"),
        ("Programming mode", "Ctrl+P"),
        ("Preferences", "Ctrl+,"),
        ("About", "F1"),
        ("Quit", "Ctrl+Q"),
    ];
    let group = adw::PreferencesGroup::new();
    for (title, keys) in rows {
        let row = adw::ActionRow::builder().title(title).build();
        row.add_suffix(
            &gtk::Label::builder()
                .label(keys)
                .css_classes(["dim-label"])
                .build(),
        );
        group.add(&row);
    }
    let page = adw::PreferencesPage::new();
    page.add(&group);
    let dialog = adw::PreferencesDialog::builder()
        .title("Keyboard Shortcuts")
        .build();
    dialog.add(&page);
    dialog.present(Some(win));
}

impl Ui {
    fn build_keypad(&self, mode: ButtonMode) -> gtk::Widget {
        let outer = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        outer.add_css_class("calc-keypad");
        outer.set_homogeneous(false);
        let extra = match mode {
            ButtonMode::Basic => None,
            ButtonMode::Advanced => Some(advanced_keys()),
            ButtonMode::Financial => Some(financial_keys()),
            ButtonMode::Programming => Some(programming_keys()),
        };
        if let Some(rows) = extra {
            outer.append(&self.grid(rows, mode));
        }
        outer.append(&self.grid(basic_keys(mode == ButtonMode::Programming), mode));
        outer.into()
    }

    fn grid(&self, rows: Vec<Vec<Key>>, mode: ButtonMode) -> gtk::Grid {
        let grid = gtk::Grid::builder()
            .row_spacing(6)
            .column_spacing(6)
            .row_homogeneous(true)
            .column_homogeneous(true)
            .hexpand(true)
            .vexpand(true)
            .build();
        for (r, row) in rows.into_iter().enumerate() {
            for (c, key) in row.into_iter().enumerate() {
                let b = gtk::Button::with_label(key.label);
                b.set_tooltip_text(key.tooltip);
                if let Some(class) = key.class {
                    b.add_css_class(class);
                }
                b.set_focus_on_click(false);
                if mode == ButtonMode::Programming {
                    if let Act::Insert(s) = key.act {
                        if s.len() == 1 && s.chars().all(|c| c.is_ascii_hexdigit()) {
                            let d = s.chars().next().unwrap().to_digit(16).unwrap();
                            self.digit_buttons.borrow_mut().push((b.clone(), d));
                        }
                    }
                }
                let (cols, rows_span) = if matches!(key.act, Act::Eval) {
                    (2, 1)
                } else {
                    (1, 1)
                };
                let ui = self.clone();
                let act = key.act.clone();
                b.connect_clicked(move |_| ui.activate(&act));
                grid.attach(&b, c as i32, r as i32, cols, rows_span);
            }
        }
        grid
    }

    fn activate(&self, act: &Act) {
        match act {
            Act::Insert(text) => self.insert(text),
            Act::Eval => self.evaluate(),
            Act::Clear => {
                self.entry.set_text("");
                self.state.borrow_mut().just_evaluated = false;
            }
            Act::Backspace => {
                self.state.borrow_mut().just_evaluated = false;
                if let Some((a, b)) = self.entry.selection_bounds() {
                    self.entry.delete_text(a, b);
                } else {
                    let p = self.entry.position();
                    if p > 0 {
                        self.entry.delete_text(p - 1, p);
                    }
                }
            }
            Act::Financial(i) => self.financial_dialog(*i),
        }
        self.entry.grab_focus_without_selecting();
    }

    fn insert(&self, text: &str) {
        let continues =
            text.starts_with([' ', '+', '−', '×', '÷', '^', '!', '%', '²', '⁻', '<', '>']);
        let just = std::mem::take(&mut self.state.borrow_mut().just_evaluated);
        if just && !continues {
            self.entry.set_text("");
        }
        if let Some((a, b)) = self.entry.selection_bounds() {
            if !(just && continues) {
                self.entry.delete_text(a, b);
                self.entry.set_position(a);
            }
        }
        let mut pos = if just && continues {
            self.entry.text().chars().count() as i32
        } else {
            self.entry.position()
        };
        self.entry.insert_text(text, &mut pos);
        self.entry.set_position(pos);
    }

    fn evaluate(&self) {
        let text = self.entry.text().to_string();
        if text.trim().is_empty() {
            return;
        }
        let result = {
            let mut s = self.state.borrow_mut();
            s.ctx.evaluate(&text).map(|v| format::format(v, &s.options))
        };
        match result {
            Ok(formatted) => {
                {
                    let mut s = self.state.borrow_mut();
                    s.history.push(HistoryEntry {
                        expression: text,
                        result: formatted.clone(),
                    });
                    let excess = s.history.len().saturating_sub(MAX_HISTORY);
                    s.history.drain(..excess);
                    save_history(&s.history);
                }
                self.append_history_row(self.state.borrow().history.last().unwrap());
                self.entry.set_text(&formatted);
                self.entry.select_region(0, -1);
                self.state.borrow_mut().just_evaluated = true;
            }
            Err(e) => {
                self.error.set_label(&e.to_string());
                self.error.set_visible(true);
                self.entry.add_css_class("error");
                if let Some(p) = e.position() {
                    self.entry.set_position(p as i32);
                }
            }
        }
    }

    fn rebuild_history(&self) {
        self.history.remove_all();
        let entries = self.state.borrow().history.clone();
        for e in &entries {
            self.append_history_row(e);
        }
    }

    fn append_history_row(&self, e: &HistoryEntry) {
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        let expr = gtk::Button::builder()
            .label(&e.expression)
            .css_classes(["flat"])
            .hexpand(true)
            .halign(gtk::Align::End)
            .tooltip_text("Insert expression")
            .build();
        let eq = gtk::Label::new(Some("="));
        let res = gtk::Button::builder()
            .label(&e.result)
            .css_classes(["flat", "calc-history-result"])
            .tooltip_text("Insert result")
            .build();
        for (b, text) in [(&expr, e.expression.clone()), (&res, e.result.clone())] {
            let ui = self.clone();
            b.connect_clicked(move |_| {
                ui.state.borrow_mut().just_evaluated = false;
                let mut pos = ui.entry.position();
                ui.entry.insert_text(&text, &mut pos);
                ui.entry.set_position(pos);
                ui.entry.grab_focus_without_selecting();
            });
        }
        row.append(&expr);
        row.append(&eq);
        row.append(&res);
        self.history.append(&row);
        // Scroll to the newest entry once laid out.
        let adj = self.history_scroll.vadjustment();
        glib::idle_add_local_once(move || adj.set_value(adj.upper()));
    }

    fn set_mode(&self, mode: ButtonMode) {
        {
            let mut s = self.state.borrow_mut();
            if s.mode == mode {
                return;
            }
            s.mode = mode;
            s.prefs.set_enum_nick("button-mode", mode.nick());
            s.sync_ctx_mode();
            // Results carried across modes keep their numeric value.
            let bits = s.options.word_size;
            s.ctx.ans = match (s.ctx.ans, mode) {
                (Value::Real(x), ButtonMode::Programming) => {
                    Value::Int(x.trunc() as i64 as u64 & crate::engine::mask(bits))
                }
                (Value::Int(v), m) if m != ButtonMode::Programming => {
                    Value::Real(to_signed(v, bits) as f64)
                }
                (v, _) => v,
            };
        }
        self.entry.set_text("");
        self.apply_mode();
    }

    fn apply_mode(&self) {
        let mode = self.state.borrow().mode;
        self.keypad.set_visible_child_name(mode.nick());
        self.prog_bar.set_visible(mode == ButtonMode::Programming);
        self.update_digit_sensitivity();
        self.rebuild_bits();
    }

    fn update_digit_sensitivity(&self) {
        let base = self.state.borrow().options.base;
        for (b, d) in self.digit_buttons.borrow().iter() {
            b.set_sensitive(*d < base);
        }
    }

    fn convert_entry_base(&self, base: u32) {
        // Re-express the current value (if the entry holds one) in the new base.
        let text = self.entry.text().to_string();
        let value = {
            let mut s = self.state.borrow_mut();
            let v = if text.trim().is_empty() {
                None
            } else {
                s.ctx.clone().evaluate(&text).ok()
            };
            s.options.base = base;
            s.sync_ctx_mode();
            v
        };
        if let Some(v) = value {
            let s = self.state.borrow();
            self.entry.set_text(&format::format(
                v,
                &Options {
                    thousands: false,
                    ..s.options
                },
            ));
        }
        self.update_digit_sensitivity();
    }

    fn current_int(&self) -> Option<u64> {
        let s = self.state.borrow();
        let text = self.entry.text();
        if text.trim().is_empty() {
            return Some(0);
        }
        match s.ctx.clone().evaluate(&text) {
            Ok(Value::Int(v)) => Some(v),
            _ => None,
        }
    }

    fn rebuild_bits(&self) {
        while let Some(child) = self.bits.first_child() {
            self.bits.remove(&child);
        }
        let bits = self.state.borrow().options.word_size;
        if self.state.borrow().mode != ButtonMode::Programming {
            return;
        }
        // 16 bits per row, MSB first; one-column gap between nibbles.
        let per_row = 16u32.min(bits);
        for bit in (0..bits).rev() {
            let row = ((bits - 1 - bit) / per_row) as i32 * 2;
            let pos = per_row - 1 - (bit % per_row);
            let col = (pos + pos / 4) as i32;
            let b = gtk::ToggleButton::builder()
                .label("0")
                .css_classes(["flat", "calc-bit"])
                .tooltip_text(format!("Bit {bit}"))
                .build();
            let ui = self.clone();
            b.connect_clicked(move |_| ui.toggle_bit(bit));
            self.bits.attach(&b, col, row, 1, 1);
            if bit % 4 == 0 {
                let idx = gtk::Label::builder()
                    .label(bit.to_string())
                    .css_classes(["dim-label", "calc-bit-index"])
                    .build();
                self.bits.attach(&idx, col, row + 1, 1, 1);
            }
        }
        self.refresh_bits();
    }

    fn refresh_bits(&self) {
        if self.state.borrow().mode != ButtonMode::Programming {
            return;
        }
        let value = self.current_int();
        let mut child = self.bits.first_child();
        while let Some(w) = child {
            if let Some(b) = w.downcast_ref::<gtk::ToggleButton>() {
                let bit: u32 = b
                    .tooltip_text()
                    .and_then(|t| t.strip_prefix("Bit ").and_then(|n| n.parse().ok()))
                    .unwrap_or(0);
                let on = value.is_some_and(|v| v >> bit & 1 == 1);
                b.set_label(if on { "1" } else { "0" });
                b.set_sensitive(value.is_some());
            }
            child = w.next_sibling();
        }
    }

    fn toggle_bit(&self, bit: u32) {
        let Some(v) = self.current_int() else { return };
        let v = v ^ (1u64 << bit);
        let text = {
            let s = self.state.borrow();
            format::format(
                Value::Int(v & crate::engine::mask(s.options.word_size)),
                &Options {
                    thousands: false,
                    ..s.options
                },
            )
        };
        self.entry.set_text(&text);
        self.entry.set_position(-1);
    }

    fn financial_dialog(&self, i: usize) {
        let f = &financial::FUNCTIONS[i];
        let dialog = adw::AlertDialog::builder().heading(f.title).build();
        let group = adw::PreferencesGroup::new();
        let rows: Vec<adw::EntryRow> = f
            .params
            .iter()
            .map(|p| {
                let row = adw::EntryRow::builder().title(*p).build();
                group.add(&row);
                row
            })
            .collect();
        dialog.set_extra_child(Some(&group));
        dialog.add_responses(&[("cancel", "_Cancel"), ("calculate", "C_alculate")]);
        dialog.set_response_appearance("calculate", adw::ResponseAppearance::Suggested);
        dialog.set_default_response(Some("calculate"));
        dialog.set_close_response("cancel");
        let ui = self.clone();
        dialog.connect_response(None, move |_, resp| {
            if resp != "calculate" {
                return;
            }
            let args: Vec<String> = rows
                .iter()
                .map(|r| {
                    let t = r.text().trim().to_string();
                    if t.is_empty() {
                        "0".into()
                    } else {
                        t
                    }
                })
                .collect();
            ui.entry
                .set_text(&format!("{}({})", f.name, args.join("; ")));
            ui.evaluate();
        });
        dialog.present(Some(&self.window));
    }

    fn show_preferences(&self) {
        let dialog = adw::PreferencesDialog::builder()
            .title("Preferences")
            .build();
        let page = adw::PreferencesPage::new();
        let group = adw::PreferencesGroup::builder().title("Results").build();
        let opts = self.state.borrow().options;
        let angle = self.state.borrow().ctx.angle;

        let format_row = adw::ComboRow::builder()
            .title("Number _Format")
            .use_underline(true)
            .model(&gtk::StringList::new(&NumberFormat::ALL.map(|f| f.label())))
            .build();
        format_row.set_selected(
            NumberFormat::ALL
                .iter()
                .position(|f| *f == opts.format)
                .unwrap_or(0) as u32,
        );
        let accuracy = adw::SpinRow::with_range(0.0, 15.0, 1.0);
        accuracy.set_title("_Decimal Places");
        accuracy.set_use_underline(true);
        accuracy.set_value(opts.accuracy as f64);
        let thousands = adw::SwitchRow::builder()
            .title("_Thousands Separators")
            .use_underline(true)
            .active(opts.thousands)
            .build();
        let zeroes = adw::SwitchRow::builder()
            .title("_Trailing Zeroes")
            .use_underline(true)
            .active(opts.show_zeroes)
            .build();
        group.add(&format_row);
        group.add(&accuracy);
        group.add(&thousands);
        group.add(&zeroes);

        let group2 = adw::PreferencesGroup::builder()
            .title("Calculation")
            .build();
        let angles = [AngleUnit::Degrees, AngleUnit::Radians, AngleUnit::Gradians];
        let angle_row = adw::ComboRow::builder()
            .title("_Angle Units")
            .use_underline(true)
            .model(&gtk::StringList::new(&["Degrees", "Radians", "Gradians"]))
            .build();
        angle_row.set_selected(angles.iter().position(|a| *a == angle).unwrap_or(0) as u32);
        group2.add(&angle_row);

        page.add(&group);
        page.add(&group2);
        dialog.add(&page);

        let ui = self.clone();
        let update = move |f: &dyn Fn(&mut State)| {
            f(&mut ui.state.borrow_mut());
        };
        let u = update.clone();
        format_row.connect_selected_notify(move |r| {
            let f = NumberFormat::ALL[r.selected() as usize];
            u(&|s| {
                s.options.format = f;
                s.prefs.set_enum_nick("number-format", f.nick());
            });
        });
        let u = update.clone();
        accuracy.connect_value_notify(move |r| {
            let v = r.value() as usize;
            u(&|s| {
                s.options.accuracy = v;
                s.prefs.set("accuracy", v as i32);
            });
        });
        let u = update.clone();
        thousands.connect_active_notify(move |r| {
            let v = r.is_active();
            u(&|s| {
                s.options.thousands = v;
                s.prefs.set("show-thousands", v);
            });
        });
        let u = update.clone();
        zeroes.connect_active_notify(move |r| {
            let v = r.is_active();
            u(&|s| {
                s.options.show_zeroes = v;
                s.prefs.set("show-zeroes", v);
            });
        });
        let u = update;
        angle_row.connect_selected_notify(move |r| {
            let a = angles[r.selected() as usize];
            u(&|s| {
                s.ctx.angle = a;
                s.prefs.set_enum_nick("angle-units", a.nick());
            });
        });
        dialog.present(Some(&self.window));
    }
}
