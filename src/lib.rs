/// The application id, shared by the window, settings schema, tray item
/// and desktop entry.
pub const APP_ID: &str = "io.github.yfilali.DynonUSBUpdater";

pub mod application;
pub mod background;
pub mod checker;
pub mod drive;
pub mod job;
pub mod scan;
pub mod tray;
pub mod window;
