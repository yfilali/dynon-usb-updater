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
use std::cell::RefCell;

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
    /// (and, once Phase 3's checker exists, its periodic checking) survives
    /// every window closing. Safe to call any number of times — `held`
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

    /// Show a `GNotification` whose default action raises the window —
    /// used by the periodic checker (Phase 3) for "a new cycle is
    /// available" and "a download finished".
    pub fn notify(&self, id: &str, title: &str, body: &str) {
        let notification = gio::Notification::new(title);
        notification.set_body(Some(body));
        notification.set_default_action("app.raise-window");
        self.send_notification(Some(id), &notification);
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
