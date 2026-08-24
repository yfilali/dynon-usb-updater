//! The `AdwApplication` subclass: single-instance, holds the accelerator
//! map from docs/UX-SPEC.md §6.4, and owns the one `DynonWindow`.

use crate::window::DynonWindow;
use adw::prelude::*;
use adw::subclass::prelude::*;
use gtk::gio;
use gtk::glib;

const APP_ID: &str = "io.github.yfilali.DynonUSBUpdater";

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct DynonApplication;

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
        /// requirement in §6.6 without any extra IPC of our own.
        fn activate(&self) {
            self.parent_activate();
            let app = self.obj();
            let window = app
                .active_window()
                .and_downcast::<DynonWindow>()
                .unwrap_or_else(|| DynonWindow::new(app.upcast_ref::<adw::Application>()));
            window.present();
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
    }

    /// Quitting mid-run must go through the same guard as closing the
    /// window (§10 acceptance 25) rather than tearing the process down.
    fn quit_guarded(&self) {
        if let Some(window) = self.active_window() {
            window.close();
        } else {
            self.quit();
        }
    }
}
