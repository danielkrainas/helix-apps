//! The details and settings of one saved network connection (Wi-Fi,
//! wired or VPN), shared by the Wi-Fi and Network panels.

use adw::prelude::*;
use std::rc::Rc;

use super::widgets::{self, InfoRow};
use super::{spawn_worker, Ctx};
use crate::bus;
use crate::netconf::{self, Field, IpEdit, Method};
use crate::nm::{self, Saved};
use helix_app_common::i18n::{n_, tr};
use helix_app_common::trf;

/// What the editor is opened for.
pub struct Editor {
    pub saved: Saved,
    /// Read-only facts (title, value) shown first.
    pub info: Vec<(String, String)>,
    /// More groups the caller adds (sharing a Wi-Fi network…).
    pub extra: Vec<adw::PreferencesGroup>,
    /// The device to activate it on again after a change, when it is
    /// active.
    pub reactivate: Option<String>,
    /// The name can be changed (wired profiles, VPNs).
    pub rename: bool,
    /// "Forget Network…" / "Remove Profile…".
    pub remove: &'static str,
}

const METHODS: [&str; 4] = [
    n_("Automatic"),
    n_("Manual"),
    n_("Link-Local Only"),
    n_("Disabled"),
];

/// The rows of one address family.
struct IpRows {
    v6: bool,
    method: adw::ComboRow,
    addresses: adw::EntryRow,
    gateway: adw::EntryRow,
    auto_dns: adw::SwitchRow,
    dns: adw::EntryRow,
    routes: adw::EntryRow,
    never_default: adw::SwitchRow,
    /// The method was "shared" (a hotspot): left as it is.
    shared: bool,
}

impl IpRows {
    fn new(group: &adw::PreferencesGroup, e: &IpEdit, v6: bool) -> Rc<Self> {
        let model = gtk::StringList::new(&[]);
        for m in METHODS {
            model.append(helix_app_common::i18n::tr_dyn(m).as_str());
        }
        let family = if v6 { "IPv6" } else { "IPv4" };
        let method = adw::ComboRow::builder()
            .use_markup(false)
            .title(tr("Method"))
            .model(&model)
            .build();
        helix_app_common::set_accessible_label(&method, &trf!("{family} Method", family = family));
        let entry = |title: &str, text: &str, name: String| {
            let e = adw::EntryRow::builder()
                .use_markup(false)
                .title(title)
                .text(text)
                .build();
            helix_app_common::set_accessible_label(&e, &name);
            helix_app_common::clear_input_error_on_edit(&e, None);
            e
        };
        let addresses = entry(
            tr("Addresses"),
            &e.addresses,
            trf!("{family} Addresses", family = family),
        );
        let gateway = entry(
            tr("Gateway"),
            &e.gateway,
            trf!("{family} Gateway", family = family),
        );
        let auto_dns = adw::SwitchRow::builder()
            .use_markup(false)
            .title(tr("Automatic DNS"))
            .active(e.auto_dns)
            .build();
        helix_app_common::set_accessible_label(
            &auto_dns,
            &trf!("Automatic {family} DNS", family = family),
        );
        let dns = entry(
            tr("DNS Servers"),
            &e.dns,
            trf!("{family} DNS Servers", family = family),
        );
        let routes = entry(
            tr("Routes"),
            &e.routes,
            trf!("{family} Routes", family = family),
        );
        routes.set_tooltip_text(Some(tr(
            "Destination/prefix and gateway, for example 10.8.0.0/16 192.168.1.254; separate routes with commas",
        )));
        let never_default = adw::SwitchRow::builder()
            .use_markup(false)
            .title(tr("Use Only for Resources on This Network"))
            .active(e.never_default)
            .build();
        for w in [
            method.clone().upcast::<gtk::Widget>(),
            addresses.clone().upcast(),
            gateway.clone().upcast(),
            auto_dns.clone().upcast(),
            dns.clone().upcast(),
            routes.clone().upcast(),
            never_default.clone().upcast(),
        ] {
            group.add(&w);
        }
        let shared = e.method == Method::Shared;
        let rows = Rc::new(IpRows {
            v6,
            method,
            addresses,
            gateway,
            auto_dns,
            dns,
            routes,
            never_default,
            shared,
        });
        let index = Method::CHOICES
            .iter()
            .position(|m| *m == e.method)
            .unwrap_or(0);
        rows.method.set_selected(index as u32);
        let weak = Rc::downgrade(&rows);
        rows.method.connect_selected_notify(move |_| {
            if let Some(r) = weak.upgrade() {
                r.sync();
            }
        });
        rows.sync();
        rows
    }

    fn method(&self) -> Method {
        if self.shared {
            return Method::Shared;
        }
        Method::CHOICES
            .get(self.method.selected() as usize)
            .copied()
            .unwrap_or(Method::Auto)
    }

    /// Shows the rows that matter for the method.
    fn sync(&self) {
        let m = self.method();
        let manual = m == Method::Manual;
        let on = !matches!(m, Method::Disabled);
        self.addresses.set_visible(manual);
        self.gateway.set_visible(manual);
        self.auto_dns.set_visible(m == Method::Auto);
        for w in [
            self.dns.clone().upcast::<gtk::Widget>(),
            self.routes.clone().upcast(),
            self.never_default.clone().upcast(),
        ] {
            w.set_visible(on && m != Method::LinkLocal);
        }
        self.method.set_sensitive(!self.shared);
    }

    fn edit(&self) -> IpEdit {
        IpEdit {
            method: self.method(),
            addresses: self.addresses.text().to_string(),
            gateway: self.gateway.text().to_string(),
            dns: self.dns.text().to_string(),
            auto_dns: self.auto_dns.is_active(),
            routes: self.routes.text().to_string(),
            never_default: self.never_default.is_active(),
        }
    }

    /// Marks the wrong field; false when there is one.
    fn check(&self) -> bool {
        let Err(field) = netconf::check(&self.edit(), self.v6) else {
            return true;
        };
        let (entry, message) = match field {
            Field::Addresses => (
                &self.addresses,
                tr("Enter addresses such as 192.168.1.10/24, separated by commas"),
            ),
            Field::Gateway => (&self.gateway, tr("Enter the gateway's address")),
            Field::Dns => (
                &self.dns,
                tr("Enter DNS server addresses separated by commas"),
            ),
            Field::Routes => (
                &self.routes,
                tr("Enter routes as destination/prefix and gateway, separated by commas"),
            ),
        };
        helix_app_common::set_input_error(entry, None, Some(message));
        entry.grab_focus();
        false
    }
}

/// Opens the editor; `after` runs once something changed.
pub fn open(ctx: &Rc<Ctx>, editor: Editor, after: impl Fn() + 'static) {
    let after = Rc::new(after);
    let saved = editor.saved.clone();
    let page = widgets::page();
    if !editor.info.is_empty() {
        let details = widgets::group(tr("Details"));
        for (title, value) in &editor.info {
            details.add(&InfoRow::new(title, value).row);
        }
        page.add(&details);
    }
    for g in &editor.extra {
        page.add(g);
    }

    let general = widgets::group(tr("General"));
    let name = adw::EntryRow::builder()
        .use_markup(false)
        .title(tr("Name"))
        .text(saved.id())
        .build();
    name.set_visible(editor.rename);
    general.add(&name);
    let auto = adw::SwitchRow::builder()
        .use_markup(false)
        .title(tr("Connect Automatically"))
        .active(saved.autoconnect())
        .build();
    general.add(&auto);
    let metered = adw::SwitchRow::builder()
        .use_markup(false)
        .title(tr("Metered Connection"))
        .subtitle(tr("Apps and updates use less data on it"))
        .active(saved.metered())
        .build();
    general.add(&metered);
    page.add(&general);

    let has_ip = !saved.is_vpn() || saved.settings.contains_key("ipv4");
    let v4_group = widgets::group(tr("IPv4"));
    let v4 = IpRows::new(
        &v4_group,
        &netconf::read(saved.settings.get("ipv4"), false),
        false,
    );
    let v6_group = widgets::group(tr("IPv6"));
    let v6 = IpRows::new(
        &v6_group,
        &netconf::read(saved.settings.get("ipv6"), true),
        true,
    );
    if has_ip && !saved.is_hotspot() {
        page.add(&v4_group);
        page.add(&v6_group);
    }

    let remove_group = widgets::group("");
    let (remove_row, remove) = widgets::action_row(
        tr(editor.remove),
        "",
        tr("Remove…"),
        Some("destructive-action"),
    );
    remove_group.add(&remove_row);
    page.add(&remove_group);

    let view = adw::ToolbarView::new();
    let header = adw::HeaderBar::new();
    let apply = gtk::Button::builder()
        .label(tr("_Apply"))
        .use_underline(true)
        .css_classes(["suggested-action"])
        .build();
    header.pack_end(&apply);
    view.add_top_bar(&header);
    view.set_content(Some(&page));
    let title = if saved.kind() == "802-11-wireless" {
        nm::ssid_text(&saved.ssid())
    } else {
        saved.id()
    };
    let dialog = adw::Dialog::builder()
        .title(&title)
        .content_width(520)
        .content_height(640)
        .child(&view)
        .build();

    {
        let ctx = Rc::downgrade(ctx);
        let dialog = dialog.downgrade();
        let after = after.clone();
        let saved = saved.clone();
        let reactivate = editor.reactivate.clone();
        apply.connect_clicked(move |_| {
            let (Some(ctx), Some(dialog)) = (ctx.upgrade(), dialog.upgrade()) else {
                return;
            };
            let ip = has_ip && !saved.is_hotspot();
            if ip && !(v4.check() && v6.check()) {
                return;
            }
            let mut settings = saved.settings.clone();
            let conn = settings.entry("connection".into()).or_default();
            conn.insert("autoconnect".into(), nm::val(auto.is_active()));
            conn.insert(
                "metered".into(),
                nm::val(if metered.is_active() { 1i32 } else { 2i32 }),
            );
            let new_name = name.text().trim().to_string();
            if editor.rename && !new_name.is_empty() {
                conn.insert("id".into(), nm::val(new_name));
            }
            if ip {
                let g4 = netconf::write(settings.get("ipv4"), &v4.edit(), false);
                let g6 = netconf::write(settings.get("ipv6"), &v6.edit(), true);
                settings.insert("ipv4".into(), g4);
                settings.insert("ipv6".into(), g6);
            }
            let path = saved.path.clone();
            let device = reactivate.clone();
            let weak = Rc::downgrade(&ctx);
            let after = after.clone();
            dialog.close();
            spawn_worker(
                move || {
                    let c = bus::system()?;
                    let groups = nm::secret_groups(&settings);
                    nm::update(&c, &path, settings, &groups)?;
                    // Changes take effect when it connects again.
                    if let Some(d) = device {
                        nm::activate(&c, &path, &d, "/")?;
                    }
                    Ok::<_, bus::Error>(())
                },
                move |r| {
                    if let (Err(e), Some(ctx)) = (r, weak.upgrade()) {
                        if e != bus::Error::Denied {
                            ctx.toast(&trf!("Could not save the changes: {e}", e = e));
                        }
                    }
                    after();
                },
            );
        });
    }
    {
        let ctx = Rc::downgrade(ctx);
        let d = dialog.downgrade();
        remove.connect_clicked(move |_| {
            let (Some(ctx), Some(d)) = (ctx.upgrade(), d.upgrade()) else {
                return;
            };
            confirm_remove(&ctx, &d, &saved, &title, after.clone());
        });
    }
    dialog.present(Some(&ctx.window));
}

fn confirm_remove(
    ctx: &Rc<Ctx>,
    editor: &adw::Dialog,
    saved: &Saved,
    title: &str,
    after: Rc<dyn Fn()>,
) {
    let alert = adw::AlertDialog::new(
        Some(&trf!("Remove {name}?", name = title)),
        Some(tr(
            "Its settings and password are removed from this computer.",
        )),
    );
    alert.add_response("cancel", tr("_Cancel"));
    alert.add_response("remove", tr("_Remove"));
    alert.set_response_appearance("remove", adw::ResponseAppearance::Destructive);
    alert.set_default_response(Some("cancel"));
    alert.set_close_response("cancel");
    let weak = Rc::downgrade(ctx);
    let editor_weak = editor.downgrade();
    let path = saved.path.clone();
    alert.connect_response(None, move |_, response| {
        if response != "remove" {
            return;
        }
        if let Some(e) = editor_weak.upgrade() {
            e.close();
        }
        let path = path.clone();
        let weak = weak.clone();
        let after = after.clone();
        spawn_worker(
            move || bus::system().and_then(|c| nm::delete(&c, &path)),
            move |r| {
                if let (Err(e), Some(ctx)) = (r, weak.upgrade()) {
                    if e != bus::Error::Denied {
                        ctx.toast(&trf!("Could not remove it: {e}", e = e));
                    }
                }
                after();
            },
        );
    });
    alert.present(Some(editor));
}
