//! The `AdwApplication` subclass: single-instance, holds the accelerator
//! map from docs/UX-SPEC.md §6.4, and owns the one `DynonWindow`.
//!
//! It also owns background execution: when a check interval is set, closing
//! every window must not strand an invisible process, but it also must not
//! kill the periodic checker — so this holds the application open (visible
//! to GNOME as a Background App) and provides the one deliberate way out,
//! the `quit` action.

use crate::window::DynonWindow;
use adw::prelude::*;
use adw::subclass::prelude::*;
use gtk::gio;
use gtk::glib;
use std::cell::{Cell, RefCell};

const APP_ID: &str = "io.github.yfilali.DynonUSBUpdater";

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct DynonApplication {
        /// An extra `GApplication` use-count that outlives every closed
        /// window, held for as long as `check-interval` is not `manual`.
        /// `gio`'s hold/release is RAII (`ApplicationHoldGuard`): `Some`
        /// means currently held, and dropping it (setting back to `None`)
        /// releases it — there is no separate `release()` call to balance.
        pub held: RefCell<Option<gio::ApplicationHoldGuard>>,
        /// Whether the checker's heartbeat timers have already been
        /// registered. `activate()` fires on every launch and every
        /// "raise the window" — the timers must be set up exactly once per
        /// process, not once per activation.
        pub checker_scheduled: Cell<bool>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for DynonApplication {
        const NAME: &'static str = "DynonApplication";
        type Type = super::DynonApplication;
        type ParentType = adw::Application;
    }

    impl ObjectImpl for DynonApplication {}
    impl ApplicationImpl for DynonApplication {
        /// Called on every launch, including a second one — `Gio.Application`
        /// with the default (unique-bus) flags routes it here and simply
        /// presents the existing window, satisfying the single-instance
        /// requirement in §6.6 without any extra IPC of our own. It is also
        /// what a notification's "raise the window" action, and a relaunch
        /// after the window was closed while running in the background, both
        /// go through.
        fn activate(&self) {
            self.parent_activate();
            let app = self.obj();
            let window = app
                .active_window()
                .and_downcast::<DynonWindow>()
                .unwrap_or_else(|| DynonWindow::new(app.upcast_ref::<adw::Application>()));
            window.present();
            app.sync_background_hold();
            app.install_checker_schedule();
        }
    }
    impl GtkApplicationImpl for DynonApplication {}
    impl AdwApplicationImpl for DynonApplication {}
}

glib::wrapper! {
    pub struct DynonApplication(ObjectSubclass<imp::DynonApplication>)
        @extends adw::Application, gtk::Application, gio::Application,
        @implements gio::ActionMap, gio::ActionGroup;
}

impl Default for DynonApplication {
    fn default() -> Self {
        Self::new()
    }
}

impl DynonApplication {
    pub fn new() -> Self {
        glib::Object::builder()
            .property("application-id", APP_ID)
            // Explicitly the default (unique-bus) flags — never NON_UNIQUE:
            // two instances writing the same drive concurrently must stay
            // impossible (§6.6).
            .property("flags", gio::ApplicationFlags::empty())
            .build()
    }

    /// Every accelerator in §6.4 that isn't already implied by a widget
    /// default (e.g. `Escape` closing a focused popover).
    pub fn setup_accels(&self) {
        let pairs: &[(&str, &[&str])] = &[
            ("win.choose-folder", &["<Control>o"]),
            ("win.choose-archive", &["<Control><Shift>o"]),
            ("win.choose-drive-folder", &["<Control><Shift>d"]),
            ("win.rescan", &["<Control>r", "F5"]),
            ("win.select-all", &["<Control>a"]),
            ("win.deselect-all", &["<Control><Shift>a"]),
            ("win.update", &["<Control>Return", "<Control>KP_Enter"]),
            ("win.cancel-run", &["Escape"]),
            ("win.show-log", &["<Control>l"]),
            ("win.copy-log", &["<Control><Shift>c"]),
            ("win.preferences", &["<Control>comma"]),
            ("win.show-help-overlay", &["<Control>question"]),
            ("win.main-menu", &["F10"]),
            ("window.close", &["<Control>w"]),
            ("app.quit", &["<Control>q"]),
        ];
        for (action, accels) in pairs {
            self.set_accels_for_action(action, accels);
        }
    }

    pub fn install_app_actions(&self) {
        let quit = gio::SimpleAction::new("quit", None);
        quit.connect_activate(glib::clone!(
            #[weak(rename_to = app)]
            self,
            move |_, _| app.quit_guarded()
        ));
        self.add_action(&quit);

        // What a notification's default action, and GNOME's Background Apps
        // entry, use to bring the window back.
        let raise = gio::SimpleAction::new("raise-window", None);
        raise.connect_activate(glib::clone!(
            #[weak(rename_to = app)]
            self,
            move |_, _| app.activate()
        ));
        self.add_action(&raise);
    }

    /// Quitting mid-run must go through the same guard as closing the
    /// window (§10 acceptance 25) rather than tearing the process down. Once
    /// it is actually safe to close — no run in progress — this goes further
    /// than an ordinary window close and terminates the process outright,
    /// releasing any background hold: `quit` is the one deliberate way out
    /// of background execution, so closing the window alone must never be
    /// mistaken for it.
    fn quit_guarded(&self) {
        let Some(window) = self.active_window().and_downcast::<DynonWindow>() else {
            self.force_quit();
            return;
        };
        if window.has_active_run() {
            // Reuse the same guard a window-close or Cancel would hit; the
            // background hold (if any) keeps the process alive meanwhile, so
            // nothing is stranded by not force-quitting here.
            window.close();
            return;
        }
        window.close();
        self.force_quit();
    }

    /// Actually end the process: drop the background hold first so the
    /// use-count accounting stays balanced, then quit outright.
    fn force_quit(&self) {
        self.imp().held.replace(None);
        self.quit();
    }

    /// Reflects `check-interval` into `GApplication::hold()`, so the process
    /// (and its periodic checking) survives every window closing. Safe to
    /// call any number of times — `held`
    /// tracks whether an extra use-count is currently outstanding so
    /// hold/release stay balanced. On the transition into holding, also
    /// requests the XDG Background portal once (see `request_background`);
    /// it is not re-requested on every call here, only on that transition
    /// and whenever `request_background` is called directly (e.g. after the
    /// `autostart` setting changes in Preferences).
    pub fn sync_background_hold(&self) {
        let should_hold = background_wanted();
        let imp = self.imp();
        let currently_held = imp.held.borrow().is_some();
        if should_hold && !currently_held {
            imp.held.replace(Some(self.hold()));
            self.request_background();
        } else if !should_hold && currently_held {
            imp.held.replace(None);
        }
    }

    /// Ask the XDG Background portal for permission to run in the
    /// background and, per the `autostart` setting, to be launched at
    /// login. Best-effort and never fatal: a non-Flatpak install, or a
    /// portal denial, just means `GApplication::hold()` alone keeps the
    /// process running without a Background Apps entry or a login autostart
    /// entry. Safe to call repeatedly (e.g. whenever `autostart` changes).
    pub fn request_background(&self) {
        let autostart = app_settings()
            .map(|s| s.boolean("autostart"))
            .unwrap_or(true);
        glib::spawn_future_local(async move {
            if let Err(err) = crate::background::request(autostart).await {
                glib::g_debug!(
                    "dynon-usb-updater",
                    "background portal request declined or unavailable: {err}"
                );
            }
        });
    }

    /// Show a `GNotification` whose default action raises the window — used
    /// by the periodic checker for "a new cycle is available" and "a
    /// download finished".
    pub fn notify(&self, id: &str, title: &str, body: &str) {
        let notification = gio::Notification::new(title);
        notification.set_body(Some(body));
        notification.set_default_action("app.raise-window");
        self.send_notification(Some(id), &notification);
    }

    /// One heartbeat shortly after startup, then hourly — coarse on purpose,
    /// since the coarsest interval a user can choose (`weekly`) tolerates an
    /// hour of slack easily, and it means daily/weekly never need their own
    /// separate timers. `maybe_run_checker` itself decides, every time,
    /// whether an actual check is due.
    pub fn install_checker_schedule(&self) {
        if self.imp().checker_scheduled.replace(true) {
            return; // already running from an earlier activate()
        }
        glib::timeout_add_seconds_local_once(
            30,
            glib::clone!(
                #[weak(rename_to = app)]
                self,
                move || app.maybe_run_checker()
            ),
        );
        glib::timeout_add_seconds_local(
            3600,
            glib::clone!(
                #[weak(rename_to = app)]
                self,
                #[upgrade_or]
                glib::ControlFlow::Break,
                move || {
                    app.maybe_run_checker();
                    glib::ControlFlow::Continue
                }
            ),
        );
    }

    /// Runs the checker if — and only if — every gate passes: a provider
    /// that supports it, a real interval (not `manual`), enough time
    /// actually elapsed since the last check, and a network that is up and
    /// not metered. Never installs anything to a drive; at most it
    /// downloads a file and shows a notification.
    pub fn maybe_run_checker(&self) {
        let Some(settings) = app_settings() else {
            return;
        };
        if settings.string("data-provider") != "dynon" {
            // Every other provider's site isn't something this app knows
            // how to parse — Preferences already says so plainly.
            return;
        }
        let interval = settings.string("check-interval");
        let interval_secs: i64 = match interval.as_str() {
            "weekly" => 7 * 24 * 3600,
            "daily" => 24 * 3600,
            _ => return, // "manual", or an unrecognised value: never guess a schedule
        };
        let last = settings.int64("last-check-time");
        let now = glib::DateTime::now_utc().map(|d| d.to_unix()).unwrap_or(0);
        if last != 0 && now.saturating_sub(last) < interval_secs {
            return;
        }

        let monitor = gio::NetworkMonitor::default();
        if !monitor.is_network_available() || monitor.is_network_metered() {
            glib::g_debug!(
                "dynon-usb-updater",
                "checker skipped: offline or on a metered connection"
            );
            return;
        }

        glib::spawn_future_local(glib::clone!(
            #[weak(rename_to = app)]
            self,
            async move { app.run_checker_once().await }
        ));
    }

    /// The actual check: one conditional GET of the provider page for the
    /// current `system-type`, and — only when a genuinely new current cycle
    /// is found — one more GET to download it. Downloading only: nothing
    /// here ever touches a drive.
    async fn run_checker_once(&self) {
        let Some(settings) = app_settings() else {
            return;
        };
        let system_type = settings.string("system-type");
        let url = crate::checker::page_url(&system_type);
        let cache_validator = settings.string("checker-etag");
        let cache_validator = (!cache_validator.is_empty()).then(|| cache_validator.to_string());

        let session = soup::Session::new();
        let fetch = crate::checker::fetch_page(&session, url, cache_validator.as_deref()).await;

        let now = glib::DateTime::now_utc().map(|d| d.to_unix()).unwrap_or(0);
        let _ = settings.set_int64("last-check-time", now);

        let page = match fetch {
            Ok(page) => page,
            Err(err) => {
                glib::g_debug!("dynon-usb-updater", "checker page fetch failed: {err}");
                return;
            }
        };
        if let Some(etag) = &page.etag {
            let _ = settings.set_string("checker-etag", etag);
        }
        let Some(body) = page.body else {
            return; // 304 Not Modified: nothing changed since last time
        };

        let listings = crate::checker::parse_dynon_page(&body);
        let today = glib::DateTime::now_local()
            .map(|d| crate::checker::SimpleDate {
                year: d.year(),
                month: d.month() as u8,
                day: d.day_of_month() as u8,
            })
            .unwrap_or(crate::checker::SimpleDate {
                year: 1970,
                month: 1,
                day: 1,
            });

        let Some(crate::checker::Selection::Current(listing)) =
            crate::checker::select(&listings, today)
        else {
            // Nothing covers today (or only an upcoming one does): surfaced
            // in the UI, never downloaded automatically.
            return;
        };

        let aviation_label = listing
            .aviation_cycle
            .map(|c| c.label())
            .unwrap_or_default();
        let obstacle_label = listing
            .obstacle_cycle
            .map(|c| c.label())
            .unwrap_or_default();
        let already_seen = settings.string("last-seen-aviation-cycle") == aviation_label
            && settings.string("last-seen-obstacle-cycle") == obstacle_label
            && !aviation_label.is_empty();
        if already_seen {
            return;
        }

        let Some(full_url) = crate::checker::resolve_url(url, &listing.href) else {
            return;
        };
        let dest_dir = {
            let folder = settings.string("download-folder");
            if folder.is_empty() {
                glib::user_special_dir(glib::UserDirectory::Downloads)
                    .unwrap_or_else(glib::home_dir)
            } else {
                std::path::PathBuf::from(folder.as_str())
            }
        };

        self.notify(
            "new-cycle",
            "New AIRAC Cycle Available",
            &format!("Aviation {aviation_label} · Obstacles {obstacle_label} is downloading."),
        );

        match crate::checker::download_package(&session, &full_url, &dest_dir).await {
            Ok(path) => {
                let _ = settings.set_string("last-seen-aviation-cycle", &aviation_label);
                let _ = settings.set_string("last-seen-obstacle-cycle", &obstacle_label);
                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                self.notify(
                    "download-complete",
                    "Download Complete",
                    &format!(
                        "{name} was saved to your downloads. Install it from the app when ready."
                    ),
                );
            }
            Err(err) => {
                glib::g_debug!("dynon-usb-updater", "checker download failed: {err}");
            }
        }
    }
}

/// Whether `check-interval` currently calls for the app to keep running in
/// the background at all (i.e. it is not `manual`).
fn background_wanted() -> bool {
    app_settings()
        .map(|s| s.string("check-interval") != "manual")
        .unwrap_or(false)
}

fn app_settings() -> Option<gio::Settings> {
    let source = gio::SettingsSchemaSource::default()?;
    source.lookup(APP_ID, true)?;
    Some(gio::Settings::new(APP_ID))
}
