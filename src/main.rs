use dynon_usb_updater::application::DynonApplication;
use gtk::glib;
use gtk::prelude::*;

const APP_ID: &str = "io.github.yfilali.DynonUSBUpdater";

fn main() -> glib::ExitCode {
    adw::init().expect("libadwaita failed to initialise");
    gtk::Window::set_default_icon_name(APP_ID);
    // The app and symbolic icons are installed to the hicolor theme by
    // meson (data/meson.build); for an uninstalled `cargo run`, add the
    // source tree itself as a fallback icon search path so both still
    // resolve without needing `meson install` first.
    if let Some(display) = gtk::gdk::Display::default() {
        let theme = gtk::IconTheme::for_display(&display);
        let source_icons = concat!(env!("CARGO_MANIFEST_DIR"), "/data/icons");
        theme.add_search_path(source_icons);
    }

    let app = DynonApplication::new();
    app.connect_startup(|app| {
        app.install_app_actions();
        app.setup_accels();
    });
    app.run()
}
