//! A fake IBus input-method daemon for tests: `org.freedesktop.IBus` on
//! a private bus ([`helix_app_common::test_bus::TestBus`]), never the
//! host's IBus.
//!
//! GTK's IBus module (`GTK_IM_MODULE=ibus`, from the `ibus-gtk4` package)
//! connects to the bus named by `IBUS_ADDRESS`, creates an input context
//! per text widget and tells it about focus, the caret's rectangle (where
//! an input method puts its candidate window), the content type and the
//! surrounding text. This fake records all of that ([`Event`]) and sends
//! what an input method sends while someone types Chinese, Japanese or
//! Korean: preedit text ([`FakeIbus::preedit`]) and committed text
//! ([`FakeIbus::commit`]), to the focused context.
//!
//! Key presses are not consumed (`ProcessKeyEvent` answers "not
//! handled"), so the tests drive composition through the signals, as an
//! input method does when a candidate is picked. Not for use in the apps.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use helix_app_common::test_bus::TestBus;
use zbus::blocking::Connection;
use zbus::interface;
use zbus::zvariant::{OwnedObjectPath, Value};

const NAME: &str = "org.freedesktop.IBus";
const PATH: &str = "/org/freedesktop/IBus";
const CONTEXT_IFACE: &str = "org.freedesktop.IBus.InputContext";

/// What a client told the fake, in order.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    Created {
        context: u32,
        client: String,
    },
    FocusIn(u32),
    FocusOut(u32),
    Reset(u32),
    /// The caret's rectangle: x, y, width, height (window coordinates, or
    /// relative to the surface for `SetCursorLocationRelative`).
    Cursor {
        context: u32,
        rect: (i32, i32, i32, i32),
        relative: bool,
    },
    ContentType {
        context: u32,
        purpose: u32,
        hints: u32,
    },
    Capabilities {
        context: u32,
        caps: u32,
    },
    Surrounding {
        context: u32,
        text: String,
        cursor: u32,
        anchor: u32,
    },
    Key {
        context: u32,
        keyval: u32,
        state: u32,
    },
}

#[derive(Default)]
struct State {
    events: Vec<Event>,
    next: u32,
    focused: Option<u32>,
}

type Shared = Arc<Mutex<State>>;

struct Ibus {
    state: Shared,
}

#[interface(name = "org.freedesktop.IBus")]
impl Ibus {
    async fn create_input_context(
        &self,
        client: String,
        #[zbus(object_server)] server: &zbus::ObjectServer,
    ) -> zbus::fdo::Result<OwnedObjectPath> {
        let id = {
            let mut st = self.state.lock().unwrap();
            st.next += 1;
            let id = st.next;
            st.events.push(Event::Created {
                context: id,
                client,
            });
            id
        };
        let path = format!("{PATH}/InputContext_{id}");
        server
            .at(
                path.as_str(),
                Context {
                    id,
                    state: self.state.clone(),
                    content_type: (0, 0),
                    commit_preedit: false,
                },
            )
            .await?;
        Ok(OwnedObjectPath::try_from(path).unwrap())
    }
}

struct Context {
    id: u32,
    state: Shared,
    content_type: (u32, u32),
    commit_preedit: bool,
}

impl Context {
    fn log(&self, e: Event) {
        self.state.lock().unwrap().events.push(e);
    }
}

#[interface(name = "org.freedesktop.IBus.InputContext")]
impl Context {
    fn process_key_event(&self, keyval: u32, _keycode: u32, state: u32) -> bool {
        self.log(Event::Key {
            context: self.id,
            keyval,
            state,
        });
        false
    }
    fn set_cursor_location(&self, x: i32, y: i32, w: i32, h: i32) {
        self.log(Event::Cursor {
            context: self.id,
            rect: (x, y, w, h),
            relative: false,
        });
    }
    fn set_cursor_location_relative(&self, x: i32, y: i32, w: i32, h: i32) {
        self.log(Event::Cursor {
            context: self.id,
            rect: (x, y, w, h),
            relative: true,
        });
    }
    fn focus_in(&self) {
        self.state.lock().unwrap().focused = Some(self.id);
        self.log(Event::FocusIn(self.id));
    }
    fn focus_out(&self) {
        let mut st = self.state.lock().unwrap();
        if st.focused == Some(self.id) {
            st.focused = None;
        }
        st.events.push(Event::FocusOut(self.id));
    }
    fn reset(&self) {
        self.log(Event::Reset(self.id));
    }
    fn set_capabilities(&self, caps: u32) {
        self.log(Event::Capabilities {
            context: self.id,
            caps,
        });
    }
    fn set_surrounding_text(&self, text: Value<'_>, cursor: u32, anchor: u32) {
        let text = ibus_text_string(&text).unwrap_or_default();
        self.log(Event::Surrounding {
            context: self.id,
            text,
            cursor,
            anchor,
        });
    }
    fn destroy(&self) {}

    #[zbus(property)]
    fn content_type(&self) -> (u32, u32) {
        self.content_type
    }
    #[zbus(property)]
    fn set_content_type(&mut self, value: (u32, u32)) {
        self.content_type = value;
        self.log(Event::ContentType {
            context: self.id,
            purpose: value.0,
            hints: value.1,
        });
    }
    #[zbus(property)]
    fn client_commit_preedit(&self) -> (bool,) {
        (self.commit_preedit,)
    }
    #[zbus(property)]
    fn set_client_commit_preedit(&mut self, value: (bool,)) {
        self.commit_preedit = value.0;
    }
}

/// The text of a serialized `IBusText` ("IBusText", attachments, text,
/// attributes).
fn ibus_text_string(v: &Value<'_>) -> Option<String> {
    let v = match v {
        Value::Value(inner) => inner.as_ref(),
        other => other,
    };
    let Value::Structure(s) = v else { return None };
    match s.fields().get(2)? {
        Value::Str(t) => Some(t.to_string()),
        _ => None,
    }
}

/// An `IBusText` holding `text`, with no attributes.
fn ibus_text(text: &str) -> Value<'static> {
    let attrs = zbus::zvariant::StructureBuilder::new()
        .add_field("IBusAttrList")
        .add_field(HashMap::<String, Value<'static>>::new())
        .add_field(Vec::<Value<'static>>::new())
        .build()
        .unwrap();
    let text = zbus::zvariant::StructureBuilder::new()
        .add_field("IBusText")
        .add_field(HashMap::<String, Value<'static>>::new())
        .add_field(text.to_owned())
        .append_field(Value::Value(Box::new(Value::Structure(attrs))))
        .build()
        .unwrap();
    Value::Value(Box::new(Value::Structure(text)))
}

/// A running fake input-method daemon.
pub struct FakeIbus {
    state: Shared,
    conn: Connection,
    // Last: the bus outlives the connection.
    bus: TestBus,
}

impl FakeIbus {
    /// Starts the fake on a new private bus. `None` (with a note) without
    /// `dbus-daemon`.
    pub fn start() -> Option<FakeIbus> {
        let bus = TestBus::start()?;
        let state: Shared = Arc::default();
        let conn = zbus::blocking::connection::Builder::address(bus.address())
            .ok()?
            .name(NAME)
            .ok()?
            .serve_at(
                PATH,
                Ibus {
                    state: state.clone(),
                },
            )
            .ok()?
            .build()
            .ok()?;
        Some(FakeIbus { state, conn, bus })
    }

    /// For `IBUS_ADDRESS`.
    pub fn address(&self) -> &str {
        self.bus.address()
    }

    /// Everything clients told the fake so far.
    pub fn events(&self) -> Vec<Event> {
        self.state.lock().unwrap().events.clone()
    }

    /// The input context that has focus.
    pub fn focused(&self) -> Option<u32> {
        self.state.lock().unwrap().focused
    }

    /// The last caret rectangle `context` reported.
    pub fn cursor(&self, context: u32) -> Option<(i32, i32, i32, i32)> {
        self.events().into_iter().rev().find_map(|e| match e {
            Event::Cursor {
                context: c, rect, ..
            } if c == context => Some(rect),
            _ => None,
        })
    }

    /// Forgets the events recorded so far.
    pub fn clear(&self) {
        self.state.lock().unwrap().events.clear();
    }

    fn emit(
        &self,
        signal: &str,
        body: &(impl serde::Serialize + zbus::zvariant::DynamicType),
    ) -> bool {
        let Some(id) = self.focused() else {
            return false;
        };
        let path = format!("{PATH}/InputContext_{id}");
        self.conn
            .emit_signal(None::<&str>, path.as_str(), CONTEXT_IFACE, signal, body)
            .is_ok()
    }

    /// Shows `text` as preedit (composition in progress) in the focused
    /// field, with the cursor after `cursor` characters. False when no
    /// field has focus.
    pub fn preedit(&self, text: &str, cursor: u32) -> bool {
        // GTK's module follows the "with mode" form (mode 0: the preedit
        // is cleared, not committed, when focus moves) and show/hide.
        let visible = !text.is_empty();
        self.emit(
            "UpdatePreeditTextWithMode",
            &(ibus_text(text), cursor, visible, 0u32),
        ) && self.emit(
            if visible {
                "ShowPreeditText"
            } else {
                "HidePreeditText"
            },
            &(),
        )
    }

    /// Commits `text` to the focused field. False when no field has focus.
    pub fn commit(&self, text: &str) -> bool {
        self.emit("CommitText", &(ibus_text(text),))
    }
}

/// Waits up to five seconds for `cond`, running `step` (say, a GLib main
/// context iteration) in between.
pub fn wait_for(mut step: impl FnMut(), mut cond: impl FnMut() -> bool) -> bool {
    let end = Instant::now() + Duration::from_secs(5);
    while Instant::now() < end {
        if cond() {
            return true;
        }
        step();
        std::thread::sleep(Duration::from_millis(5));
    }
    cond()
}
