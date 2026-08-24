//! XDG Background portal: staying alive after the window closes (so a
//! periodic checker can keep running) and starting at login. Both are
//! entirely best-effort — this portal only exists for sandboxed apps, and a
//! denial or a missing portal implementation must never be treated as
//! fatal. The caller drives this on the GLib main context via
//! `glib::spawn_future_local`; nothing here blocks a thread.

use ashpd::desktop::background::Background;

/// Ask the portal for permission to keep running once every window is
/// closed, and — when `autostart` is true — to be launched again at login.
/// Returns whether the portal actually granted background execution, purely
/// for logging; the caller does not need to react to a `false` or an error,
/// because `GApplication::hold()` (called independently by the caller) is
/// what actually keeps the process alive on a non-sandboxed install where
/// this portal simply is not available.
pub async fn request(autostart: bool) -> ashpd::Result<Background> {
    Background::request()
        .reason("Check for new AIRAC cycles and download them")
        .auto_start(autostart)
        .dbus_activatable(false)
        .send()
        .await?
        .response()
}
