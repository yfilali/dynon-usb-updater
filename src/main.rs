use dynon_usb_updater::application::DynonApplication;
use gtk::gio;
use gtk::glib;
use gtk::prelude::*;

const APP_ID: &str = "io.github.yfilali.DynonUSBUpdater";
const RESOURCE_BYTES: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/resources.gresource"));

fn main() -> glib::ExitCode {
    register_resources();

    adw::init().expect("libadwaita failed to initialise");
    gtk::Window::set_default_icon_name(APP_ID);
    if let Some(display) = gtk::gdk::Display::default() {
        gtk::IconTheme::for_display(&display)
            .add_resource_path("/io/github/yfilali/DynonUSBUpdater/icons");
    }

    let app = DynonApplication::new();
    app.connect_startup(|app| {
        app.install_app_actions();
        app.setup_accels();
    });
    app.run()
}

/// Embeds and registers the compiled GResource bundle (icons today; the
/// window template is compiled in directly by `gtk::CompositeTemplate`, so
/// it doesn't need to live in the bundle to work).
fn register_resources() {
    let bytes = glib::Bytes::from_static(RESOURCE_BYTES);
    match gio::Resource::from_data(&bytes) {
        Ok(resource) => gio::resources_register(&resource),
        Err(err) => eprintln!("warning: could not load bundled resources: {err}"),
    }
}
