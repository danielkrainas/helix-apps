//! One browser tab: a WebView plus its per-page bars.

use adw::prelude::*;
use gtk::{gdk, gio};
use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::rc::{Rc, Weak};
use webkit6::prelude::*;

use crate::app::App;
use crate::permissions::{self, Decision, Kind};
use crate::urlinput;

pub const START_URI: &str = "helix-about:overview";

/// Clones a captured handle into a closure (a small `glib::clone!` stand-in).
macro_rules! with_tab {
    ($w:ident, $body:expr) => {{
        let $w = $w.clone();
        $body
    }};
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Security {
    /// Internal or local page.
    Local,
    Secure,
    /// Unencrypted connection.
    Insecure,
    /// TLS with certificate problems.
    Broken,
}

/// What changed, so windows only refresh what they need.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Change {
    Uri,
    Title,
    Progress,
    Loading,
    History,
    Zoom,
}

struct PendingPermission {
    request: webkit6::PermissionRequest,
    kinds: Vec<Kind>,
    origin: String,
}

pub struct Tab {
    pub this: Weak<Tab>,
    pub app: Weak<App>,
    pub private: bool,
    pub view: webkit6::WebView,
    /// Child widget of the AdwTabPage.
    pub root: gtk::Box,
    pub security: Cell<Security>,
    /// URI to load the first time the tab is shown (lazy session restore).
    pub pending_uri: RefCell<Option<(String, String)>>,
    perm_revealer: gtk::Revealer,
    perm_label: gtk::Label,
    perm_queue: RefCell<VecDeque<PendingPermission>>,
    tls_revealer: gtk::Revealer,
    tls_label: gtk::Label,
    tls_pending: RefCell<Option<(gio::TlsCertificate, String, String)>>,
    link_label: gtk::Label,
}

/// Display form of a URI for the address bar and tab titles.
pub fn display_uri(uri: &str) -> String {
    if uri.starts_with("helix-about:") || uri == "about:blank" {
        String::new()
    } else {
        uri.to_string()
    }
}

fn bar(css: &str) -> (gtk::Revealer, gtk::Box, gtk::Label) {
    let label = gtk::Label::builder()
        .hexpand(true)
        .xalign(0.0)
        .wrap(true)
        .build();
    let row = gtk::Box::builder()
        .spacing(6)
        .margin_start(12)
        .margin_end(6)
        .margin_top(6)
        .margin_bottom(6)
        .build();
    row.append(&label);
    let frame = gtk::Box::new(gtk::Orientation::Vertical, 0);
    frame.add_css_class(css);
    frame.append(&row);
    let revealer = gtk::Revealer::builder()
        .child(&frame)
        .reveal_child(false)
        .build();
    (revealer, row, label)
}

impl Tab {
    pub fn new(app: &Rc<App>, view: webkit6::WebView, private: bool) -> Rc<Tab> {
        view.set_vexpand(true);
        view.set_hexpand(true);
        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);

        let (perm_revealer, perm_row, perm_label) = bar("permission-bar");
        let deny = gtk::Button::with_label("Deny");
        let allow = gtk::Button::with_label("Allow");
        allow.add_css_class("suggested-action");
        perm_row.append(&deny);
        perm_row.append(&allow);

        let (tls_revealer, tls_row, tls_label) = bar("tls-bar");
        let accept = gtk::Button::with_label("Accept Risk and Continue");
        accept.add_css_class("destructive-action");
        tls_row.append(&accept);

        let link_label = gtk::Label::builder()
            .halign(gtk::Align::Start)
            .valign(gtk::Align::End)
            .ellipsize(gtk::pango::EllipsizeMode::Middle)
            .max_width_chars(80)
            .visible(false)
            .can_target(false)
            .build();
        link_label.add_css_class("link-preview");

        let overlay = gtk::Overlay::new();
        overlay.set_child(Some(&view));
        overlay.add_overlay(&link_label);
        overlay.set_vexpand(true);

        root.append(&tls_revealer);
        root.append(&perm_revealer);
        root.append(&overlay);

        let tab = Rc::new_cyclic(|this| Tab {
            this: this.clone(),
            app: Rc::downgrade(app),
            private,
            view: view.clone(),
            root,
            security: Cell::new(Security::Local),
            pending_uri: RefCell::new(None),
            perm_revealer,
            perm_label,
            perm_queue: RefCell::new(VecDeque::new()),
            tls_revealer,
            tls_label,
            tls_pending: RefCell::new(None),
            link_label,
        });

        let weak = Rc::downgrade(&tab);
        deny.connect_clicked(with_tab!(weak, move |_| {
            if let Some(t) = weak.upgrade() {
                t.answer_permission(false)
            }
        }));
        allow.connect_clicked(with_tab!(weak, move |_| {
            if let Some(t) = weak.upgrade() {
                t.answer_permission(true)
            }
        }));
        accept.connect_clicked(with_tab!(weak, move |_| {
            if let Some(t) = weak.upgrade() {
                t.accept_tls()
            }
        }));
        tab.connect_view();
        tab
    }

    fn rc(&self) -> Option<Rc<Tab>> {
        self.this.upgrade()
    }

    fn changed(&self, what: Change) {
        if let (Some(app), Some(tab)) = (self.app.upgrade(), self.rc()) {
            app.tab_changed(&tab, what);
        }
    }

    pub fn uri(&self) -> String {
        if let Some((uri, _)) = self.pending_uri.borrow().as_ref() {
            return uri.clone();
        }
        self.view.uri().map(|s| s.to_string()).unwrap_or_default()
    }

    pub fn title(&self) -> String {
        if let Some((uri, title)) = self.pending_uri.borrow().as_ref() {
            return if title.is_empty() {
                display_uri(uri)
            } else {
                title.clone()
            };
        }
        let title = self.view.title().map(|s| s.to_string()).unwrap_or_default();
        if !title.is_empty() {
            return title;
        }
        let uri = display_uri(&self.uri());
        if uri.is_empty() {
            "New Tab".into()
        } else {
            uri
        }
    }

    pub fn is_start_page(&self) -> bool {
        let uri = self.uri();
        uri.is_empty() || uri.starts_with("helix-about:") || uri == "about:blank"
    }

    pub fn load(&self, uri: &str) {
        self.pending_uri.replace(None);
        if uri == "about:overview" || uri == "about:newtab" || uri == "about:home" {
            self.view.load_uri(START_URI);
        } else {
            self.view.load_uri(uri);
        }
    }

    /// Defers loading until the tab is first shown.
    pub fn set_lazy(&self, uri: &str, title: &str) {
        self.pending_uri
            .replace(Some((uri.to_string(), title.to_string())));
    }

    /// Loads a deferred URI, if any.
    pub fn realize_lazy(&self) {
        let pending = self.pending_uri.take();
        if let Some((uri, _)) = pending {
            self.load(&uri);
        }
    }

    fn update_security(&self) {
        let uri = self.uri().to_ascii_lowercase();
        let sec = if uri.starts_with("https://") {
            match self.view.tls_info() {
                Some((_, flags)) if flags.is_empty() => Security::Secure,
                Some(_) => Security::Broken,
                None => Security::Insecure,
            }
        } else if uri.starts_with("http://") {
            Security::Insecure
        } else {
            Security::Local
        };
        self.security.set(sec);
    }

    fn connect_view(self: &Rc<Self>) {
        let view = &self.view;
        let weak = Rc::downgrade(self);
        let w = move || weak.upgrade();

        view.connect_uri_notify(with_tab!(w, move |_| {
            if let Some(t) = w() {
                t.changed(Change::Uri)
            }
        }));
        view.connect_title_notify(with_tab!(w, move |v| {
            let Some(t) = w() else { return };
            if let (Some(app), Some(uri), Some(title)) = (t.app.upgrade(), v.uri(), v.title()) {
                if !t.private {
                    app.update_history_title(&uri, &title);
                }
            }
            t.changed(Change::Title)
        }));
        view.connect_estimated_load_progress_notify(with_tab!(w, move |_| {
            if let Some(t) = w() {
                t.changed(Change::Progress)
            }
        }));
        view.connect_is_loading_notify(with_tab!(w, move |_| {
            if let Some(t) = w() {
                t.changed(Change::Loading)
            }
        }));
        view.connect_favicon_notify(with_tab!(w, move |_| {
            if let Some(t) = w() {
                t.changed(Change::Title)
            }
        }));
        view.connect_zoom_level_notify(with_tab!(w, move |_| {
            if let Some(t) = w() {
                t.changed(Change::Zoom)
            }
        }));
        view.connect_is_playing_audio_notify(with_tab!(w, move |_| {
            if let Some(t) = w() {
                t.changed(Change::Title)
            }
        }));
        view.connect_is_muted_notify(with_tab!(w, move |_| {
            if let Some(t) = w() {
                t.changed(Change::Title)
            }
        }));

        view.connect_load_changed(with_tab!(w, move |v, event| {
            let Some(t) = w() else { return };
            match event {
                webkit6::LoadEvent::Started => {
                    t.deny_all_permissions();
                    t.tls_revealer.set_reveal_child(false);
                }
                webkit6::LoadEvent::Committed => {
                    t.update_security();
                    if let (Some(app), Some(uri)) = (t.app.upgrade(), v.uri()) {
                        if !t.private {
                            let title = v.title().map(|s| s.to_string()).unwrap_or_default();
                            app.record_visit(&uri, &title);
                        }
                    }
                    t.changed(Change::Uri);
                }
                webkit6::LoadEvent::Finished => {
                    t.changed(Change::History);
                    if let Some(app) = t.app.upgrade() {
                        app.schedule_session_save();
                    }
                }
                _ => {}
            }
            t.changed(Change::History);
        }));

        view.connect_load_failed_with_tls_errors(with_tab!(w, move |_, uri, cert, _flags| {
            let Some(t) = w() else { return false };
            let host = urlinput::host_of(uri).unwrap_or_else(|| uri.to_string());
            let host_only = host.split(':').next().unwrap_or(&host).to_string();
            t.tls_label.set_text(&format!(
                "The identity of {host} could not be verified. Its security certificate is not trusted."
            ));
            t.tls_pending.replace(Some((cert.clone(), host_only, uri.to_string())));
            t.tls_revealer.set_reveal_child(true);
            t.security.set(Security::Broken);
            t.changed(Change::Uri);
            false
        }));

        view.connect_mouse_target_changed(with_tab!(w, move |_, hit, _| {
            let Some(t) = w() else { return };
            match hit.link_uri().filter(|_| hit.context_is_link()) {
                Some(uri) => {
                    t.link_label.set_text(&uri);
                    t.link_label.set_visible(true);
                }
                None => t.link_label.set_visible(false),
            }
        }));

        view.connect_decide_policy(with_tab!(w, move |_, decision, kind| {
            let Some(t) = w() else { return false };
            t.decide_policy(decision, kind)
        }));

        view.connect_create(with_tab!(w, move |v, _action| {
            match w().and_then(|t| t.app.upgrade().map(|a| (a, t))) {
                Some((app, t)) => app.create_related_tab(&t).upcast(),
                // Should not happen; give WebKit a detached view.
                None => webkit6::WebView::builder().related_view(v).build().upcast(),
            }
        }));
        view.connect_close(with_tab!(w, move |_| {
            if let Some(t) = w() {
                if let Some(app) = t.app.upgrade() {
                    app.close_tab(&t)
                }
            }
        }));

        view.connect_permission_request(with_tab!(w, move |_, req| {
            let Some(t) = w() else { return false };
            t.handle_permission(req);
            true
        }));
        view.connect_query_permission_state(with_tab!(w, move |_, query| {
            let Some(t) = w() else { return false };
            let (Some(app), Some(name)) = (t.app.upgrade(), query.name()) else {
                return false;
            };
            let Some(kind) = Kind::from_api_name(&name) else {
                return false;
            };
            let origin = query
                .security_origin()
                .map(|o| origin_string(&o))
                .unwrap_or_default();
            let state = match app.permissions.borrow().get(&origin, kind) {
                Decision::Allow => webkit6::PermissionState::Granted,
                Decision::Deny => webkit6::PermissionState::Denied,
                Decision::Ask => webkit6::PermissionState::Prompt,
            };
            query.finish(state);
            true
        }));

        view.connect_script_dialog(with_tab!(w, move |_, dialog| {
            let Some(t) = w() else { return false };
            t.show_script_dialog(dialog);
            true
        }));

        view.connect_enter_fullscreen(with_tab!(w, move |_| {
            if let (Some(t), Some(app)) = (w(), w().and_then(|t| t.app.upgrade())) {
                app.page_fullscreen(&t, true);
            }
            false
        }));
        view.connect_leave_fullscreen(with_tab!(w, move |_| {
            if let (Some(t), Some(app)) = (w(), w().and_then(|t| t.app.upgrade())) {
                app.page_fullscreen(&t, false);
            }
            false
        }));

        if let Some(fc) = view.find_controller() {
            fc.connect_counted_matches(with_tab!(w, move |_, n| {
                if let (Some(t), Some(app)) = (w(), w().and_then(|t| t.app.upgrade())) {
                    app.find_result(&t, Some(n));
                }
            }));
            fc.connect_found_text(with_tab!(w, move |_, n| {
                if let (Some(t), Some(app)) = (w(), w().and_then(|t| t.app.upgrade())) {
                    app.find_result(&t, Some(n));
                }
            }));
            fc.connect_failed_to_find_text(with_tab!(w, move |_| {
                if let (Some(t), Some(app)) = (w(), w().and_then(|t| t.app.upgrade())) {
                    app.find_result(&t, Some(0));
                }
            }));
        }
    }

    fn decide_policy(
        &self,
        decision: &webkit6::PolicyDecision,
        kind: webkit6::PolicyDecisionType,
    ) -> bool {
        match kind {
            webkit6::PolicyDecisionType::NavigationAction => {
                let Some(nav) = decision.downcast_ref::<webkit6::NavigationPolicyDecision>() else {
                    return false;
                };
                let Some(mut action) = nav.navigation_action() else {
                    return false;
                };
                if action.navigation_type() != webkit6::NavigationType::LinkClicked {
                    return false;
                }
                let button = action.mouse_button();
                let mods = gdk::ModifierType::from_bits_truncate(action.modifiers());
                let ctrl = mods.contains(gdk::ModifierType::CONTROL_MASK);
                if button != 2 && !ctrl {
                    return false;
                }
                let Some(uri) = action.request().and_then(|r| r.uri()) else {
                    return false;
                };
                let foreground = mods.contains(gdk::ModifierType::SHIFT_MASK);
                decision.ignore();
                if let (Some(app), Some(t)) = (self.app.upgrade(), self.rc()) {
                    app.open_in_new_tab(&t, &uri, foreground);
                }
                true
            }
            webkit6::PolicyDecisionType::Response => {
                let Some(resp) = decision.downcast_ref::<webkit6::ResponsePolicyDecision>() else {
                    return false;
                };
                if resp.is_main_frame_main_resource() && !resp.is_mime_type_supported() {
                    decision.download();
                    return true;
                }
                false
            }
            _ => false,
        }
    }

    // ---- permissions ------------------------------------------------------

    fn handle_permission(&self, req: &webkit6::PermissionRequest) {
        let kinds: Vec<Kind> = if req.is::<webkit6::GeolocationPermissionRequest>() {
            vec![Kind::Geolocation]
        } else if req.is::<webkit6::NotificationPermissionRequest>() {
            vec![Kind::Notifications]
        } else if req.is::<webkit6::ClipboardPermissionRequest>() {
            vec![Kind::Clipboard]
        } else if let Some(m) = req.downcast_ref::<webkit6::UserMediaPermissionRequest>() {
            let mut k = Vec::new();
            if m.is_for_audio_device() {
                k.push(Kind::Microphone);
            }
            if m.is_for_video_device() {
                k.push(Kind::Camera);
            }
            k
        } else if req.is::<webkit6::PointerLockPermissionRequest>() {
            req.allow();
            return;
        } else {
            req.deny();
            return;
        };
        let Some(app) = self.app.upgrade() else {
            req.deny();
            return;
        };
        let origin = urlinput::origin_of(&self.uri()).unwrap_or_default();
        if kinds.is_empty() || origin.is_empty() {
            req.deny();
            return;
        }
        match app.permissions.borrow().get_all(&origin, &kinds) {
            Decision::Allow => {
                req.allow();
                return;
            }
            Decision::Deny => {
                req.deny();
                return;
            }
            Decision::Ask => {}
        }
        self.perm_queue.borrow_mut().push_back(PendingPermission {
            request: req.clone(),
            kinds,
            origin,
        });
        self.show_next_permission();
    }

    fn show_next_permission(&self) {
        let queue = self.perm_queue.borrow();
        match queue.front() {
            Some(p) => {
                let host = p
                    .origin
                    .split_once("://")
                    .map(|(_, h)| h)
                    .unwrap_or(&p.origin);
                self.perm_label
                    .set_text(&permissions::request_text(host, &p.kinds));
                self.perm_revealer.set_reveal_child(true);
            }
            None => self.perm_revealer.set_reveal_child(false),
        }
    }

    fn answer_permission(&self, allow: bool) {
        let Some(p) = self.perm_queue.borrow_mut().pop_front() else {
            return;
        };
        if allow {
            p.request.allow();
        } else {
            p.request.deny();
        }
        if let Some(app) = self.app.upgrade() {
            if !self.private {
                for k in &p.kinds {
                    app.permissions.borrow_mut().set(&p.origin, *k, allow);
                }
                app.save_permissions();
            }
        }
        self.show_next_permission();
    }

    /// Denies every queued request (navigation away or tab closing).
    pub fn deny_all_permissions(&self) {
        let drained: Vec<PendingPermission> = self.perm_queue.borrow_mut().drain(..).collect();
        for p in drained {
            p.request.deny();
        }
        self.perm_revealer.set_reveal_child(false);
    }

    fn accept_tls(&self) {
        let Some((cert, host, uri)) = self.tls_pending.take() else {
            return;
        };
        if let Some(session) = self.view.network_session() {
            session.allow_tls_certificate_for_host(&cert, &host);
        }
        self.tls_revealer.set_reveal_child(false);
        self.view.load_uri(&uri);
    }

    // ---- script dialogs ---------------------------------------------------

    fn show_script_dialog(&self, dialog: &webkit6::ScriptDialog) {
        let message = dialog.message().map(|s| s.to_string()).unwrap_or_default();
        let host = urlinput::host_of(&self.uri()).unwrap_or_else(|| "This page".into());
        let ad = adw::AlertDialog::new(Some(&format!("{host} says")), Some(&message));
        let kind = dialog.dialog_type();
        let entry = gtk::Entry::new();
        match kind {
            webkit6::ScriptDialogType::Alert => {
                ad.add_response("ok", "_OK");
            }
            webkit6::ScriptDialogType::Confirm => {
                ad.add_responses(&[("cancel", "_Cancel"), ("ok", "_OK")]);
                ad.set_response_appearance("ok", adw::ResponseAppearance::Suggested);
            }
            webkit6::ScriptDialogType::Prompt => {
                entry.set_text(&dialog.prompt_get_default_text().unwrap_or_default());
                entry.set_activates_default(true);
                ad.set_extra_child(Some(&entry));
                ad.add_responses(&[("cancel", "_Cancel"), ("ok", "_OK")]);
                ad.set_response_appearance("ok", adw::ResponseAppearance::Suggested);
            }
            webkit6::ScriptDialogType::BeforeUnloadConfirm => {
                ad.set_heading(Some("Leave Page?"));
                ad.set_body("Changes you made may not be saved.");
                ad.add_responses(&[("cancel", "_Stay"), ("ok", "_Leave")]);
                ad.set_response_appearance("ok", adw::ResponseAppearance::Destructive);
            }
            _ => ad.add_response("ok", "_OK"),
        }
        ad.set_default_response(Some("ok"));
        ad.set_close_response("cancel");
        let dialog = dialog.clone();
        ad.connect_response(None, move |_, response| {
            let ok = response == "ok";
            match kind {
                webkit6::ScriptDialogType::Confirm
                | webkit6::ScriptDialogType::BeforeUnloadConfirm => {
                    dialog.confirm_set_confirmed(ok)
                }
                webkit6::ScriptDialogType::Prompt if ok => dialog.prompt_set_text(&entry.text()),
                _ => {}
            }
            dialog.close();
        });
        ad.present(Some(&self.root));
    }
}

/// `scheme://host[:port]` for a WebKit security origin.
pub fn origin_string(o: &webkit6::SecurityOrigin) -> String {
    let proto = o.protocol().map(|s| s.to_string()).unwrap_or_default();
    let host = o.host().map(|s| s.to_string()).unwrap_or_default();
    match o.port() {
        0 => format!("{proto}://{host}"),
        p => format!("{proto}://{host}:{p}"),
    }
}
