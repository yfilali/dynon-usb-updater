//! System tray presence via StatusNotifierItem.
//!
//! StatusNotifierItem is the freedesktop/KDE standard for tray icons. KDE
//! Plasma, XFCE, Cinnamon, Budgie and LXQt host it natively; GNOME does so
//! through an extension. Where no host is running the item simply never
//! appears, and the application falls back to staying alive as a background
//! app, so this is offered everywhere rather than gated on any one desktop.

use gtk::gio;
use gtk::glib;
use gtk::prelude::*;
use std::sync::mpsc::{channel, Receiver, Sender};

/// What the user picked from the tray menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayCommand {
    Show,
    CheckNow,
    Quit,
}

/// Whether a StatusNotifier host is running right now.
///
/// This decides *behaviour* (whether closing the window can leave an icon
/// behind), never whether the feature exists. Checked at runtime because a
/// host can appear or vanish with a session's configuration.
pub fn host_available() -> bool {
    let Ok(bus) = gio::bus_get_sync(gio::BusType::Session, gio::Cancellable::NONE) else {
        return false;
    };
    bus.call_sync(
        Some("org.freedesktop.DBus"),
        "/org/freedesktop/DBus",
        "org.freedesktop.DBus",
        "NameHasOwner",
        Some(&("org.kde.StatusNotifierWatcher",).to_variant()),
        Some(glib::VariantTy::new("(b)").unwrap()),
        gio::DBusCallFlags::NONE,
        1000,
        gio::Cancellable::NONE,
    )
    .ok()
    .and_then(|reply| reply.child_value(0).get::<bool>())
    .unwrap_or(false)
}

struct AppTray {
    tx: Sender<TrayCommand>,
    status: String,
}

impl ksni::Tray for AppTray {
    fn id(&self) -> String {
        crate::APP_ID.into()
    }

    fn title(&self) -> String {
        "Dynon USB Updater".into()
    }

    fn icon_name(&self) -> String {
        crate::APP_ID.into()
    }

    fn tool_tip(&self) -> ksni::ToolTip {
        ksni::ToolTip {
            title: "Dynon USB Updater".into(),
            description: self.status.clone(),
            icon_name: crate::APP_ID.into(),
            icon_pixmap: Vec::new(),
        }
    }

    /// Clicking the icon raises the window, which is what every tray host's
    /// users expect of a primary activation.
    fn activate(&mut self, _x: i32, _y: i32) {
        let _ = self.tx.send(TrayCommand::Show);
    }

    fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
        use ksni::menu::{MenuItem, StandardItem};
        vec![
            StandardItem {
                label: "Show Window".into(),
                icon_name: "window-new-symbolic".into(),
                activate: Box::new(|tray: &mut Self| {
                    let _ = tray.tx.send(TrayCommand::Show);
                }),
                ..Default::default()
            }
            .into(),
            StandardItem {
                label: "Check for Updates Now".into(),
                icon_name: "view-refresh-symbolic".into(),
                activate: Box::new(|tray: &mut Self| {
                    let _ = tray.tx.send(TrayCommand::CheckNow);
                }),
                ..Default::default()
            }
            .into(),
            MenuItem::Separator,
            StandardItem {
                label: "Quit".into(),
                icon_name: "application-exit-symbolic".into(),
                activate: Box::new(|tray: &mut Self| {
                    let _ = tray.tx.send(TrayCommand::Quit);
                }),
                ..Default::default()
            }
            .into(),
        ]
    }
}

/// A live tray item. Dropping it removes the icon.
pub struct Tray {
    handle: ksni::Handle<AppTray>,
    commands: Option<Receiver<TrayCommand>>,
}

impl Tray {
    /// Publish the item. Returns None when it could not be published (no host,
    /// or no session bus), in which case the caller keeps the background-app
    /// behaviour it would have used anyway.
    pub async fn publish(status: &str) -> Option<Self> {
        let (tx, commands) = channel();
        let tray = AppTray {
            tx,
            status: status.to_string(),
        };
        match ksni::TrayMethods::spawn(tray).await {
            Ok(handle) => Some(Self {
                handle,
                commands: Some(commands),
            }),
            Err(error) => {
                eprintln!("tray icon unavailable: {error}");
                None
            }
        }
    }

    /// Hand the command channel to the caller, which polls it on the main
    /// loop. Available once; the tray itself never reads it.
    pub fn take_commands(&mut self) -> Option<Receiver<TrayCommand>> {
        self.commands.take()
    }

    /// Refresh the tooltip, e.g. after a check finds a new cycle.
    pub fn set_status(&self, status: &str) {
        let status = status.to_string();
        let handle = self.handle.clone();
        glib::spawn_future_local(async move {
            handle
                .update(move |tray: &mut AppTray| tray.status = status.clone())
                .await;
        });
    }

    pub fn shutdown(&self) {
        let handle = self.handle.clone();
        glib::spawn_future_local(async move {
            handle.shutdown().await;
        });
    }
}
