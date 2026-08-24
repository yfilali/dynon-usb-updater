//! The window: prepare, running and result pages, and the state machine that
//! moves between them.

use crate::drive::{self, Drive, EmptyReason, TargetKind};
use crate::job::{self, DriveOutcome, DriveState, Event, Outcome, Severity};
use crate::scan::{self, Archive, Cycle, DupFile, DupKind};
use adw::prelude::*;
use adw::subclass::prelude::*;
use gtk::glib::{self, clone};
use gtk::{gio, CompositeTemplate};
use std::cell::RefCell;
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver};
use std::time::{Duration, Instant};

const APP_ID: &str = "io.github.yfilali.DynonUSBUpdater";
/// Deliberately pessimistic: under-promising beats over-promising.
const ESTIMATED_BYTES_PER_SECOND: f64 = 12.0 * 1024.0 * 1024.0;

#[derive(Default)]
pub(crate) struct Sources {
    folder: Option<PathBuf>,
    dups: Vec<DupFile>,
    aviation: Option<PathBuf>,
    obstacle: Option<PathBuf>,
    skip_aviation: bool,
    skip_obstacle: bool,
    archive: Option<Archive>,
    archive_error: Option<String>,
    /// The file the failed read attempt was for, so the banner can name it.
    archive_error_path: Option<PathBuf>,
    /// Filename of an archive currently being read off the main thread.
    archive_loading: Option<String>,
    strip_wrapper: bool,
}

impl Sources {
    fn cycle(&self) -> Option<Cycle> {
        self.db_cycle()
            .or_else(|| self.archive.as_ref().and_then(|a| a.cycle))
    }

    /// Cycle from the databases alone, ignoring the archive — used to detect
    /// E17 (archive cycle disagrees with database cycle).
    fn db_cycle(&self) -> Option<Cycle> {
        let from = |p: &Option<PathBuf>| {
            p.as_ref()
                .and_then(|p| p.file_name())
                .and_then(|n| scan::parse_cycle(&n.to_string_lossy()))
        };
        from(&self.aviation).or_else(|| from(&self.obstacle))
    }

    fn anything_to_copy(&self) -> bool {
        self.aviation.is_some() || self.obstacle.is_some() || self.archive.is_some()
    }
}

pub(crate) struct Card {
    drive: Drive,
    toggle: gtk::ToggleButton,
    cycle_label: gtk::Label,
    verdict_label: gtk::Label,
    level: gtk::LevelBar,
}

/// Widgets for one drive's row on the running page.
struct RunRow {
    row: adw::ActionRow,
    state_label: gtk::Label,
    spinner: adw::Spinner,
    bar: gtk::ProgressBar,
    icon: gtk::Image,
}

/// A running job's live state, kept so the UI can compute ETA and guard exits.
pub(crate) struct Run {
    rx: Receiver<Event>,
    cancel: job::Cancel,
    samples: Vec<(Instant, u64)>,
    last_eta: Option<Duration>,
    rows: Vec<(String, RunRow)>,
    inhibit: u32,
    current_state: DriveState,
    last_announce: Instant,
    last_phase: String,
}

/// What a banner's button, if any, should do.
#[derive(Clone)]
enum BannerAction {
    ChooseFolder,
    ChooseArchive,
    Deselect(String),
    Details(String),
    ChooseAgain(String),
}

mod imp {
    use super::*;

    #[derive(CompositeTemplate, Default)]
    #[template(file = "ui/window.ui")]
    pub struct DynonWindow {
        #[template_child] pub toasts: TemplateChild<adw::ToastOverlay>,
        #[template_child] pub window_title: TemplateChild<adw::WindowTitle>,
        #[template_child] pub menu_button: TemplateChild<gtk::MenuButton>,
        #[template_child] pub stack: TemplateChild<adw::ViewStack>,
        #[template_child] pub banner: TemplateChild<adw::Banner>,
        #[template_child] pub source_path: TemplateChild<gtk::Label>,
        #[template_child] pub change_folder: TemplateChild<gtk::Button>,
        #[template_child] pub aviation_row: TemplateChild<adw::ActionRow>,
        #[template_child] pub aviation_cycle: TemplateChild<gtk::Label>,
        #[template_child] pub aviation_menu: TemplateChild<gtk::MenuButton>,
        #[template_child] pub obstacle_row: TemplateChild<adw::ActionRow>,
        #[template_child] pub obstacle_cycle: TemplateChild<gtk::Label>,
        #[template_child] pub obstacle_menu: TemplateChild<gtk::MenuButton>,
        #[template_child] pub plates_row: TemplateChild<adw::ActionRow>,
        #[template_child] pub plates_cycle: TemplateChild<gtk::Label>,
        #[template_child] pub plates_menu: TemplateChild<gtk::MenuButton>,
        #[template_child] pub plates_spinner: TemplateChild<adw::Spinner>,
        #[template_child] pub choose_folder: TemplateChild<gtk::Button>,
        #[template_child] pub rescan: TemplateChild<gtk::Button>,
        #[template_child] pub drives_stack: TemplateChild<gtk::Stack>,
        #[template_child] pub drive_flow: TemplateChild<gtk::FlowBox>,
        #[template_child] pub drives_empty: TemplateChild<adw::StatusPage>,
        #[template_child] pub drives_empty_actions: TemplateChild<gtk::Box>,
        #[template_child] pub update_button: TemplateChild<gtk::Button>,
        #[template_child] pub reason_label: TemplateChild<gtk::Label>,
        #[template_child] pub percent_label: TemplateChild<gtk::Label>,
        #[template_child] pub progress: TemplateChild<gtk::ProgressBar>,
        #[template_child] pub step_label: TemplateChild<gtk::Label>,
        #[template_child] pub detail_label: TemplateChild<gtk::Label>,
        #[template_child] pub eta_label: TemplateChild<gtk::Label>,
        #[template_child] pub running_drives: TemplateChild<adw::PreferencesGroup>,
        #[template_child] pub details_row: TemplateChild<adw::ExpanderRow>,
        #[template_child] pub cancel_button: TemplateChild<gtk::Button>,
        #[template_child] pub result_page: TemplateChild<adw::StatusPage>,
        #[template_child] pub result_drives: TemplateChild<adw::PreferencesGroup>,
        #[template_child] pub result_actions: TemplateChild<gtk::Box>,
        #[template_child] pub result_details_row: TemplateChild<adw::ExpanderRow>,

        pub(crate) sources: RefCell<Sources>,
        pub(crate) cards: RefCell<Vec<Card>>,
        pub(crate) run: RefCell<Option<Run>>,
        pub settings: RefCell<Option<gio::Settings>>,
        pub log_list: RefCell<Option<gtk::ListBox>>,
        pub log_entries: RefCell<Vec<(Severity, String, String)>>,
        pub log_header: RefCell<Option<String>>,
        pub banner_handler: RefCell<Option<glib::SignalHandlerId>>,
        pub outcomes: RefCell<Vec<DriveOutcome>>,
        #[allow(deprecated)]
        pub help_overlay: RefCell<Option<gtk::ShortcutsWindow>>,
        pub result_details_scroller: RefCell<Option<gtk::Widget>>,
        pub scan_generation: std::cell::Cell<u64>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for DynonWindow {
        const NAME: &'static str = "DynonWindow";
        type Type = super::DynonWindow;
        type ParentType = adw::ApplicationWindow;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }
        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for DynonWindow {
        fn constructed(&self) {
            self.parent_constructed();
            self.obj().setup();
        }
    }
    impl WidgetImpl for DynonWindow {}
    impl WindowImpl for DynonWindow {
        fn close_request(&self) -> glib::Propagation {
            self.obj().guarded_close()
        }
    }
    impl ApplicationWindowImpl for DynonWindow {}
    impl AdwApplicationWindowImpl for DynonWindow {}
}

glib::wrapper! {
    pub struct DynonWindow(ObjectSubclass<imp::DynonWindow>)
        @extends adw::ApplicationWindow, gtk::ApplicationWindow, gtk::Window, gtk::Widget,
        @implements gio::ActionGroup, gio::ActionMap, gtk::Accessible, gtk::Buildable,
                    gtk::ConstraintTarget, gtk::Native, gtk::Root, gtk::ShortcutManager;
}

impl DynonWindow {
    pub fn new(app: &adw::Application) -> Self {
        glib::Object::builder().property("application", app).build()
    }

    // -- setup -------------------------------------------------------------
    fn setup(&self) {
        let imp = self.imp();

        if let Some(settings) = load_settings() {
            let width = settings.int("window-width").max(360);
            let height = settings.int("window-height").max(480);
            self.set_default_size(width, height);
            if settings.boolean("window-maximized") {
                self.maximize();
            }
            imp.sources.borrow_mut().strip_wrapper = false;
            imp.settings.replace(Some(settings));
        }

        imp.menu_button.set_menu_model(Some(&main_menu()));
        self.install_actions();
        self.build_log_list();
        self.build_shortcuts_window();

        imp.change_folder.connect_clicked(clone!(
            #[weak(rename_to = win)] self,
            move |_| win.choose_source_folder()
        ));
        imp.choose_folder.connect_clicked(clone!(
            #[weak(rename_to = win)] self,
            move |_| win.choose_drive_folder()
        ));
        imp.rescan.connect_clicked(clone!(
            #[weak(rename_to = win)] self,
            move |_| win.refresh_drives()
        ));
        imp.update_button.connect_clicked(clone!(
            #[weak(rename_to = win)] self,
            move |_| win.confirm_update()
        ));
        imp.cancel_button.connect_clicked(clone!(
            #[weak(rename_to = win)] self,
            move |_| win.request_stop()
        ));

        imp.aviation_menu.set_menu_model(Some(&database_menu("aviation")));
        imp.obstacle_menu.set_menu_model(Some(&database_menu("obstacle")));
        imp.plates_menu.set_menu_model(Some(&plates_menu(false)));

        imp.update_button
            .update_relation(&[gtk::accessible::Relation::DescribedBy(&[imp.reason_label.upcast_ref()])]);

        let folder = self
            .settings_string("source-folder")
            .map(PathBuf::from)
            .filter(|p| p.is_dir())
            .or_else(default_download_dir);
        if let Some(folder) = folder {
            self.load_folder(&folder);
        }
        if let Some(archive) = self.settings_string("plates-archive").map(PathBuf::from) {
            if archive.is_file() {
                self.load_archive(&archive);
            }
        }

        // Live drive detection.
        let monitor = gio::VolumeMonitor::get();
        for signal in ["mount-added", "mount-removed", "mount-changed"] {
            monitor.connect_local(signal, false, clone!(
                #[weak(rename_to = win)] self,
                #[upgrade_or] None,
                move |_| { win.refresh_drives(); None }
            ));
        }
        self.refresh_drives();
    }

    fn install_actions(&self) {
        let entries = [
            ("choose-folder", DynonWindow::choose_source_folder as fn(&DynonWindow)),
            ("choose-archive", DynonWindow::choose_archive),
            ("choose-drive-folder", DynonWindow::choose_drive_folder),
            ("rescan", DynonWindow::refresh_drives),
            ("update", DynonWindow::confirm_update),
            ("select-all", DynonWindow::select_all_ready),
            ("deselect-all", DynonWindow::deselect_all),
            ("show-log", DynonWindow::show_log_dialog),
            ("preferences", DynonWindow::show_preferences),
            ("about", DynonWindow::show_about),
            ("cancel-run", DynonWindow::request_stop),
            ("copy-log", DynonWindow::copy_log_to_clipboard),
            ("main-menu", DynonWindow::popup_main_menu),
        ];
        for (name, handler) in entries {
            let action = gio::SimpleAction::new(name, None);
            action.connect_activate(clone!(
                #[weak(rename_to = win)] self,
                move |_, _| handler(&win)
            ));
            self.add_action(&action);
        }
        for (name, kind) in [("skip-aviation", DupKind::Aviation), ("skip-obstacle", DupKind::Obstacle)] {
            let action = gio::SimpleAction::new(name, None);
            action.connect_activate(clone!(
                #[weak(rename_to = win)] self,
                move |_, _| win.skip_database(kind)
            ));
            self.add_action(&action);
        }
        let clear = gio::SimpleAction::new("clear-archive", None);
        clear.connect_activate(clone!(
            #[weak(rename_to = win)] self,
            move |_, _| win.clear_archive()
        ));
        self.add_action(&clear);

        let preview = gio::SimpleAction::new("preview-archive", None);
        preview.connect_activate(clone!(
            #[weak(rename_to = win)] self,
            move |_, _| win.show_archive_preview()
        ));
        self.add_action(&preview);

        let show_in_files = gio::SimpleAction::new("show-in-files", Some(glib::VariantTy::STRING));
        show_in_files.connect_activate(clone!(
            #[weak(rename_to = win)] self,
            move |_, param| {
                if let Some(kind) = param.and_then(|v| v.str().map(str::to_string)) {
                    win.show_source_in_files(&kind);
                }
            }
        ));
        self.add_action(&show_in_files);

        let toggle_drive = gio::SimpleAction::new("toggle-drive", Some(glib::VariantTy::STRING));
        toggle_drive.connect_activate(clone!(
            #[weak(rename_to = win)] self,
            move |_, param| {
                if let Some(key) = param.and_then(|v| v.str().map(str::to_string)) {
                    if let Some(toggle) = win.imp().cards.borrow().iter().find(|c| c.drive.key() == key).map(|c| c.toggle.clone()) {
                        toggle.set_active(!toggle.is_active());
                    }
                }
            }
        ));
        self.add_action(&toggle_drive);

        let show_drive_in_files = gio::SimpleAction::new("show-drive-in-files", Some(glib::VariantTy::STRING));
        show_drive_in_files.connect_activate(clone!(
            #[weak(rename_to = win)] self,
            move |_, param| {
                if let Some(key) = param.and_then(|v| v.str().map(str::to_string)) {
                    win.show_drive_in_files(&key);
                }
            }
        ));
        self.add_action(&show_drive_in_files);

        let choose_again = gio::SimpleAction::new("choose-drive-again", Some(glib::VariantTy::STRING));
        choose_again.connect_activate(clone!(
            #[weak(rename_to = win)] self,
            move |_, param| {
                if let Some(path) = param.and_then(|v| v.str().map(str::to_string)) {
                    win.choose_folder_again(&path);
                }
            }
        ));
        self.add_action(&choose_again);

        let remove_target = gio::SimpleAction::new("remove-drive-target", Some(glib::VariantTy::STRING));
        remove_target.connect_activate(clone!(
            #[weak(rename_to = win)] self,
            move |_, param| {
                if let Some(path) = param.and_then(|v| v.str().map(str::to_string)) {
                    win.remove_drive_target(&path);
                }
            }
        ));
        self.add_action(&remove_target);
    }

    fn settings_string(&self, key: &str) -> Option<String> {
        let settings = self.imp().settings.borrow();
        let value = settings.as_ref()?.string(key).to_string();
        (!value.is_empty()).then_some(value)
    }

    fn save<T: Into<glib::Variant>>(&self, key: &str, value: T) {
        if let Some(settings) = self.imp().settings.borrow().as_ref() {
            let _ = settings.set_value(key, &value.into());
        }
    }

    fn preference(&self, key: &str, fallback: bool) -> bool {
        self.imp()
            .settings
            .borrow()
            .as_ref()
            .map(|s| s.boolean(key))
            .unwrap_or(fallback)
    }

    // -- sources -----------------------------------------------------------
    fn choose_source_folder(&self) {
        let dialog = gtk::FileDialog::builder()
            .title("Select the folder with this cycle's files")
            .modal(true)
            .build();
        if let Some(current) = self.imp().sources.borrow().folder.clone() {
            dialog.set_initial_folder(Some(&gio::File::for_path(current)));
        }
        dialog.select_folder(Some(self), gio::Cancellable::NONE, clone!(
            #[weak(rename_to = win)] self,
            move |result| {
                if let Ok(file) = result {
                    if let Some(path) = file.path() {
                        win.load_folder(&path);
                    }
                }
            }
        ));
    }

    fn load_folder(&self, folder: &std::path::Path) {
        let imp = self.imp();
        let dups = scan::scan_dup_files(folder, 3);
        {
            let mut sources = imp.sources.borrow_mut();
            sources.aviation = scan::newest(&dups, DupKind::Aviation).map(|d| d.path.clone());
            sources.obstacle = scan::newest(&dups, DupKind::Obstacle).map(|d| d.path.clone());
            sources.skip_aviation = false;
            sources.skip_obstacle = false;
            sources.dups = dups;
            sources.folder = Some(folder.to_path_buf());
        }
        self.save("source-folder", folder.to_string_lossy().to_string());
        imp.source_path.set_text(&abbreviate(folder));
        imp.source_path.set_tooltip_text(Some(&folder.to_string_lossy()));
        self.refresh_sources();
    }

    fn choose_archive(&self) {
        let filters = gio::ListStore::new::<gtk::FileFilter>();
        let zips = gtk::FileFilter::new();
        zips.set_name(Some("Zip archives"));
        zips.add_pattern("*.zip");
        filters.append(&zips);
        let all = gtk::FileFilter::new();
        all.set_name(Some("All files"));
        all.add_pattern("*");
        filters.append(&all);

        let dialog = gtk::FileDialog::builder()
            .title("Select the plates archive")
            .modal(true)
            .filters(&filters)
            .default_filter(&zips)
            .build();
        if let Some(folder) = self.imp().sources.borrow().folder.clone() {
            dialog.set_initial_folder(Some(&gio::File::for_path(folder)));
        }
        dialog.open(Some(self), gio::Cancellable::NONE, clone!(
            #[weak(rename_to = win)] self,
            move |result| {
                if let Ok(file) = result {
                    if let Some(path) = file.path() {
                        win.load_archive(&path);
                    }
                }
            }
        ));
    }

    /// Reading a 23,000-entry central directory belongs off the main loop.
    fn load_archive(&self, path: &std::path::Path) {
        let imp = self.imp();
        imp.plates_spinner.set_visible(true);
        imp.plates_cycle.set_text("");
        let file_name = path.file_name().unwrap_or_default().to_string_lossy().into_owned();
        imp.plates_row.set_subtitle(&format!("Reading {file_name}…"));
        imp.sources.borrow_mut().archive_loading = Some(file_name);

        let (tx, rx) = channel();
        let target = path.to_path_buf();
        std::thread::spawn(move || {
            let _ = tx.send(scan::read_archive(&target).map_err(|e| e.to_string()));
        });

        let path = path.to_path_buf();
        glib::timeout_add_local(Duration::from_millis(60), clone!(
            #[weak(rename_to = win)] self,
            #[upgrade_or] glib::ControlFlow::Break,
            move || {
                match rx.try_recv() {
                    Ok(result) => {
                        let imp = win.imp();
                        imp.plates_spinner.set_visible(false);
                        {
                            let mut sources = imp.sources.borrow_mut();
                            sources.archive_loading = None;
                            match result {
                                Ok(archive) => {
                                    sources.strip_wrapper = false;
                                    sources.archive = Some(archive);
                                    sources.archive_error = None;
                                    sources.archive_error_path = None;
                                }
                                Err(message) => {
                                    sources.archive = None;
                                    sources.archive_error = Some(message);
                                    sources.archive_error_path = Some(path.clone());
                                }
                            }
                        }
                        win.save("plates-archive", path.to_string_lossy().to_string());
                        win.refresh_sources();
                        glib::ControlFlow::Break
                    }
                    Err(std::sync::mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
                    Err(_) => glib::ControlFlow::Break,
                }
            }
        ));
    }

    fn clear_archive(&self) {
        {
            let mut sources = self.imp().sources.borrow_mut();
            sources.archive = None;
            sources.archive_error = None;
        }
        self.save("plates-archive", String::new());
        self.toast("Plates will not be replaced");
        self.refresh_sources();
    }

    fn skip_database(&self, kind: DupKind) {
        {
            let mut sources = self.imp().sources.borrow_mut();
            match kind {
                DupKind::Aviation => {
                    sources.skip_aviation = true;
                    sources.aviation = None;
                }
                _ => {
                    sources.skip_obstacle = true;
                    sources.obstacle = None;
                }
            }
        }
        self.refresh_sources();
    }

    fn refresh_sources(&self) {
        let imp = self.imp();
        let sources = imp.sources.borrow();
        let folder_name = sources
            .folder
            .as_ref()
            .map(|f| abbreviate(f))
            .unwrap_or_else(|| "no folder".into());

        for (kind, row, badge, chosen, skipped) in [
            (DupKind::Aviation, &imp.aviation_row, &imp.aviation_cycle, &sources.aviation, sources.skip_aviation),
            (DupKind::Obstacle, &imp.obstacle_row, &imp.obstacle_cycle, &sources.obstacle, sources.skip_obstacle),
        ] {
            let label = if kind == DupKind::Aviation { "aviation" } else { "obstacle" };
            badge.remove_css_class("dimmed");
            badge.add_css_class("accent");
            match chosen {
                Some(path) => {
                    let dup = sources.dups.iter().find(|d| &d.path == path);
                    let cycle = dup.and_then(|d| d.cycle);
                    badge.set_text(&cycle.map(|c| format!("Cycle {c}")).unwrap_or_else(|| "Found".into()));
                    row.set_subtitle(&format!(
                        "{} · {}",
                        path.file_name().unwrap_or_default().to_string_lossy(),
                        job::size(dup.map(|d| d.size).unwrap_or(0))
                    ));
                }
                None if skipped => {
                    badge.remove_css_class("accent");
                    badge.add_css_class("dimmed");
                    badge.set_text("Not copying");
                    row.set_subtitle("This database will be left as it is on the drive");
                }
                None => {
                    badge.remove_css_class("accent");
                    badge.add_css_class("dimmed");
                    badge.set_text("Not found");
                    row.set_subtitle(&format!("No {label} database in {folder_name}"));
                }
            }
        }

        imp.plates_cycle.remove_css_class("dimmed");
        imp.plates_cycle.remove_css_class("error");
        imp.plates_cycle.add_css_class("accent");
        imp.plates_menu.set_menu_model(Some(&plates_menu(sources.archive.is_some())));
        match (&sources.archive, &sources.archive_error) {
            (Some(archive), _) => {
                imp.plates_cycle
                    .set_text(&archive.cycle.map(|c| format!("Cycle {c}")).unwrap_or_default());
                let mut subtitle = format!(
                    "{} · {} files · {}",
                    archive.name(),
                    job::group(archive.members.len() as u64),
                    job::size(archive.total_bytes)
                );
                if let Some(wrapper) = &archive.wrapper {
                    subtitle.push_str(&format!(" · in a folder named {wrapper}"));
                }
                imp.plates_row.set_subtitle(&subtitle);
            }
            (None, Some(_)) => {
                imp.plates_cycle.remove_css_class("accent");
                imp.plates_cycle.add_css_class("error");
                imp.plates_cycle.set_text("Can't read");
                imp.plates_row.set_subtitle("This file is not a readable plates archive");
            }
            (None, None) => {
                imp.plates_cycle.remove_css_class("accent");
                imp.plates_cycle.add_css_class("dimmed");
                imp.plates_cycle.set_text("Not replacing");
                imp.plates_row
                    .set_subtitle("Plates already on the drives will be left as they are");
            }
        }
        drop(sources);
        self.refresh_cards();
        self.update_ready();
    }
}

fn abbreviate(path: &std::path::Path) -> String {
    let text = path.to_string_lossy().to_string();
    match glib::home_dir().to_str() {
        Some(home) if text.starts_with(home) => text.replacen(home, "~", 1),
        _ => text,
    }
}

fn default_download_dir() -> Option<PathBuf> {
    glib::user_special_dir(glib::UserDirectory::Downloads)
        .filter(|p| p.is_dir())
        .or_else(|| Some(glib::home_dir()))
}

fn load_settings() -> Option<gio::Settings> {
    let source = gio::SettingsSchemaSource::default()?;
    source.lookup(APP_ID, true)?;
    Some(gio::Settings::new(APP_ID))
}

fn main_menu() -> gio::Menu {
    let menu = gio::Menu::new();
    let drives = gio::Menu::new();
    drives.append(Some("Choose Drive Folder…"), Some("win.choose-drive-folder"));
    drives.append(Some("Rescan Drives"), Some("win.rescan"));
    menu.append_section(None, &drives);
    let general = gio::Menu::new();
    general.append(Some("Activity Log"), Some("win.show-log"));
    general.append(Some("Preferences"), Some("win.preferences"));
    general.append(Some("Keyboard Shortcuts"), Some("win.show-help-overlay"));
    general.append(Some("About Dynon USB Updater"), Some("win.about"));
    menu.append_section(None, &general);
    menu
}

fn database_menu(kind: &str) -> gio::Menu {
    let menu = gio::Menu::new();
    menu.append(Some("Choose a Different File…"), Some("win.choose-folder"));
    menu.append(Some("Do Not Copy"), Some(&format!("win.skip-{kind}")));
    let show = gio::Menu::new();
    let item = gio::MenuItem::new(Some("Show in Files"), None);
    item.set_action_and_target_value(Some("win.show-in-files"), Some(&kind.to_variant()));
    show.append_item(&item);
    menu.append_section(None, &show);
    menu
}

fn plates_menu(has_archive: bool) -> gio::Menu {
    let menu = gio::Menu::new();
    if has_archive {
        menu.append(Some("Choose a Different Archive…"), Some("win.choose-archive"));
        menu.append(Some("Preview Contents…"), Some("win.preview-archive"));
        menu.append(Some("Do Not Replace Plates"), Some("win.clear-archive"));
        let show = gio::Menu::new();
        let item = gio::MenuItem::new(Some("Show in Files"), None);
        item.set_action_and_target_value(Some("win.show-in-files"), Some(&"plates".to_variant()));
        show.append_item(&item);
        menu.append_section(None, &show);
    } else {
        menu.append(Some("Choose an Archive…"), Some("win.choose-archive"));
    }
    menu
}

// ---------------------------------------------------------------------------
// Drives
// ---------------------------------------------------------------------------

impl DynonWindow {
    fn choose_drive_folder(&self) {
        let dialog = gtk::FileDialog::builder().title("Select your drive").modal(true).build();
        let user = std::env::var("USER").unwrap_or_default();
        let start = PathBuf::from(format!("/run/media/{user}"));
        dialog.set_initial_folder(Some(&gio::File::for_path(if start.is_dir() {
            start
        } else {
            glib::home_dir()
        })));
        dialog.select_folder(Some(self), gio::Cancellable::NONE, clone!(
            #[weak(rename_to = win)] self,
            move |result| {
                if let Ok(file) = result {
                    if let Some(path) = file.path() {
                        let mut targets = win.manual_targets();
                        let text = path.to_string_lossy().to_string();
                        if !targets.contains(&text) {
                            targets.push(text);
                            win.save_manual_targets(&targets);
                        }
                        let name = path.file_name().unwrap_or_default().to_string_lossy().to_string();
                        win.toast(&format!("{name} added"));
                        win.refresh_drives();
                    }
                }
            }
        ));
    }

    fn manual_targets(&self) -> Vec<String> {
        self.imp()
            .settings
            .borrow()
            .as_ref()
            .map(|s| s.strv("manual-targets").iter().map(|s| s.to_string()).collect())
            .unwrap_or_default()
    }

    fn save_manual_targets(&self, targets: &[String]) {
        if let Some(settings) = self.imp().settings.borrow().as_ref() {
            let refs: Vec<&str> = targets.iter().map(|s| s.as_str()).collect();
            let _ = settings.set_strv("manual-targets", refs);
        }
    }

    fn remembered_selection(&self) -> Vec<String> {
        self.imp()
            .settings
            .borrow()
            .as_ref()
            .map(|s| s.strv("selected-drives").iter().map(|s| s.to_string()).collect())
            .unwrap_or_default()
    }

    pub fn refresh_drives(&self) {
        if self.imp().run.borrow().is_some() {
            return;
        }
        let previously_checked: Vec<String> = self
            .imp()
            .cards
            .borrow()
            .iter()
            .filter(|c| c.toggle.is_active())
            .map(|c| c.drive.key())
            .collect();

        let mut drives = drive::enumerate();
        for path in self.manual_targets() {
            let path = PathBuf::from(path);
            if drives.iter().any(|d| d.path == path) {
                continue;
            }
            drives.push(drive::folder_target(&path));
        }

        // Measuring an existing plates folder walks tens of thousands of files
        // on real hardware (27,490+ on a FAT stick is not unusual) — doing it
        // synchronously here has been observed to peg the main thread for
        // many seconds with real drives attached. Cards render immediately
        // with `reclaimable` still unknown (§6.3: "until it lands, the card
        // shows Checking…"), and a background thread fills each in.
        self.rebuild_cards(drives, &previously_checked);
        self.update_ready();
        self.spawn_reclaimable_scan();
    }

    /// Fills in each reachable drive's `reclaimable` off the main thread, one
    /// message per drive, repainting just that card as results land. Stale
    /// results from a superseded scan (a rescan started before this one
    /// finished) are dropped via the generation counter.
    fn spawn_reclaimable_scan(&self) {
        let imp = self.imp();
        let generation = imp.scan_generation.get() + 1;
        imp.scan_generation.set(generation);

        let targets: Vec<(String, PathBuf)> = imp
            .cards
            .borrow()
            .iter()
            .filter(|c| c.drive.reachable)
            .map(|c| (c.drive.key(), c.drive.plates_dir()))
            .collect();
        if targets.is_empty() {
            return;
        }

        let (tx, rx) = channel();
        std::thread::spawn(move || {
            for (key, dir) in targets {
                let measured = drive::measure_plates(&dir);
                if tx.send((key, measured)).is_err() {
                    break;
                }
            }
        });

        glib::timeout_add_local(Duration::from_millis(150), clone!(
            #[weak(rename_to = win)] self,
            #[upgrade_or] glib::ControlFlow::Break,
            move || {
                if win.imp().scan_generation.get() != generation {
                    return glib::ControlFlow::Break;
                }
                let mut any = false;
                let mut exhausted = false;
                loop {
                    match rx.try_recv() {
                        Ok((key, measured)) => {
                            any = true;
                            let needed = win.bytes_needed();
                            let target_cycle = win.imp().sources.borrow().cycle();
                            let mut cards = win.imp().cards.borrow_mut();
                            if let Some(card) = cards.iter_mut().find(|c| c.drive.key() == key) {
                                card.drive.reclaimable = Some(measured);
                                win.paint_card(card, needed, target_cycle);
                            }
                        }
                        Err(std::sync::mpsc::TryRecvError::Empty) => break,
                        Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                            exhausted = true;
                            break;
                        }
                    }
                }
                if any {
                    win.update_ready();
                }
                if exhausted {
                    glib::ControlFlow::Break
                } else {
                    glib::ControlFlow::Continue
                }
            }
        ));
    }

    fn rebuild_cards(&self, drives: Vec<Drive>, previously_checked: &[String]) {
        let imp = self.imp();
        while let Some(child) = imp.drive_flow.first_child() {
            imp.drive_flow.remove(&child);
        }
        imp.cards.borrow_mut().clear();

        if drives.is_empty() {
            self.show_empty_drives();
            return;
        }
        imp.drives_stack.set_visible_child_name("cards");

        let remembered = self.remembered_selection();
        let needed = self.bytes_needed();
        let target_cycle = imp.sources.borrow().cycle();

        for drive in drives {
            let card = self.build_card(&drive, needed, target_cycle);
            let key = drive.key();
            let ready = drive.recognised() && drive.writable && drive.fits(needed) && drive.reachable;
            let up_to_date = target_cycle.is_some() && drive.installed_cycle == target_cycle;
            let checked = if previously_checked.is_empty() {
                // Safe default: only a recognised, writable, roomy drive that is
                // not already current starts selected. An unrelated stick never does.
                ready && (!up_to_date || remembered.contains(&key))
            } else {
                previously_checked.contains(&key)
            };
            card.toggle.set_active(checked);
            imp.drive_flow.append(&card.toggle);
            imp.cards.borrow_mut().push(card);
        }
    }

    fn build_card(&self, drive: &Drive, needed: u64, target_cycle: Option<Cycle>) -> Card {
        let toggle = gtk::ToggleButton::builder()
            .width_request(236)
            .height_request(132)
            .css_classes(["card"])
            .build();

        let content = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(6)
            .margin_top(12).margin_bottom(12).margin_start(12).margin_end(12)
            .build();

        let top = gtk::Box::builder().spacing(6).build();
        let tick = gtk::Image::from_icon_name("object-select-symbolic");
        tick.add_css_class("success");
        tick.set_visible(false);
        tick.update_state(&[gtk::accessible::State::Hidden(true)]);
        let spacer = gtk::Label::new(None);
        spacer.set_hexpand(true);
        let icon = gtk::Image::from_icon_name(match drive.kind {
            TargetKind::Folder => "folder-symbolic",
            TargetKind::Mounted => "drive-harddisk-usb-symbolic",
        });
        icon.set_pixel_size(24);
        icon.add_css_class("dimmed");
        icon.update_state(&[gtk::accessible::State::Hidden(true)]);
        top.append(&tick);
        top.append(&spacer);
        top.append(&icon);

        let name = gtk::Label::builder()
            .label(&drive.name)
            .xalign(0.0)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .css_classes(["title-4"])
            .build();

        let cycle_label = gtk::Label::builder().xalign(0.0).css_classes(["caption"]).build();
        let verdict_label = gtk::Label::builder()
            .xalign(0.0)
            .wrap(true)
            .css_classes(["caption"])
            .build();
        let level = gtk::LevelBar::builder().height_request(6).hexpand(true).build();
        level.add_offset_value("high", 0.90);
        level.add_offset_value("full", 1.0);

        content.append(&top);
        content.append(&name);
        content.append(&cycle_label);
        content.append(&level);
        content.append(&verdict_label);
        toggle.set_child(Some(&content));
        toggle.set_tooltip_text(Some(&drive.path.to_string_lossy()));

        toggle.connect_toggled(clone!(
            #[weak(rename_to = win)] self,
            #[weak] tick,
            move |t| {
                tick.set_visible(t.is_active());
                win.update_ready();
            }
        ));
        tick.set_visible(toggle.is_active());

        // Context menu: right-click, and the Menu key / Shift+F10 (§4.7, §6.4).
        let key = drive.key();
        let right_click = gtk::GestureClick::new();
        right_click.set_button(3);
        right_click.connect_pressed(clone!(
            #[weak(rename_to = win)] self,
            #[weak] toggle,
            #[strong] key,
            move |_, _, x, y| win.show_card_menu(&toggle, &key, x, y)
        ));
        toggle.add_controller(right_click);

        let key_controller = gtk::EventControllerKey::new();
        key_controller.connect_key_pressed(clone!(
            #[weak(rename_to = win)] self,
            #[weak] toggle,
            #[strong] key,
            #[upgrade_or] glib::Propagation::Proceed,
            move |_, keyval, _, state| {
                let is_menu = keyval == gtk::gdk::Key::Menu;
                let is_shift_f10 = keyval == gtk::gdk::Key::F10 && state.contains(gtk::gdk::ModifierType::SHIFT_MASK);
                if is_menu || is_shift_f10 {
                    win.show_card_menu(&toggle, &key, 0.0, 0.0);
                    glib::Propagation::Stop
                } else {
                    glib::Propagation::Proceed
                }
            }
        ));
        toggle.add_controller(key_controller);

        let card = Card { drive: drive.clone(), toggle, cycle_label, verdict_label, level };
        self.paint_card(&card, needed, target_cycle);
        card
    }

    /// Everything the card says about a drive, in one place.
    fn paint_card(&self, card: &Card, needed: u64, target_cycle: Option<Cycle>) {
        let drive = &card.drive;
        let cycle = &card.cycle_label;
        let verdict = &card.verdict_label;
        for label in [cycle, verdict] {
            for class in ["dimmed", "warning", "error", "success"] {
                label.remove_css_class(class);
            }
        }

        let reclaim = drive.reclaimable.map(|(b, _)| b).unwrap_or(0);
        let available = drive.free.saturating_add(reclaim);
        let projected = if drive.total > 0 {
            ((drive.total.saturating_sub(available)).saturating_add(needed)) as f64 / drive.total as f64
        } else {
            0.0
        };
        card.level.set_value(projected.clamp(0.0, 1.0));
        card.level.update_property(&[
            gtk::accessible::Property::Label(&format!("Space on {} after this update", drive.name)),
            gtk::accessible::Property::ValueText(&format!(
                "{} of {} used",
                job::size(drive.total.saturating_sub(available).saturating_add(needed)),
                job::size(drive.total)
            )),
        ]);

        if !drive.reachable {
            cycle.set_text("Not connected");
            cycle.add_css_class("dimmed");
            verdict.set_text("Choose this folder again to use it");
            verdict.add_css_class("dimmed");
        } else if drive.kind == TargetKind::Folder {
            cycle.set_text("Folder on this computer");
            cycle.add_css_class("dimmed");
            verdict.set_text(&format!("{} free · fits", job::size(drive.free)));
            verdict.add_css_class("dimmed");
        } else if !drive.recognised() {
            cycle.set_text("Not a SkyView drive");
            cycle.add_css_class("warning");
            verdict.set_text(&format!("{} free · {} total", job::size(drive.free), job::size(drive.total)));
            verdict.add_css_class("dimmed");
        } else {
            match (drive.installed_cycle, target_cycle) {
                (Some(installed), Some(target)) if installed == target => {
                    cycle.set_text(&format!("Cycle {target} installed"));
                    cycle.add_css_class("success");
                    verdict.set_text("Already up to date");
                    verdict.add_css_class("dimmed");
                }
                (Some(installed), Some(target)) => {
                    cycle.set_text(&format!("Cycle {installed} → {target}"));
                }
                (Some(installed), None) => cycle.set_text(&format!("Cycle {installed}")),
                (None, _) => {
                    cycle.set_text("No databases installed");
                    cycle.add_css_class("dimmed");
                }
            }
            if !drive.writable {
                verdict.set_text("Read-only — can't be written to");
                verdict.add_css_class("error");
            } else if !drive.fits(needed) {
                verdict.set_text(&format!(
                    "Needs {}, {} available",
                    job::size(needed),
                    job::size(available)
                ));
                verdict.add_css_class("error");
            } else if verdict.text().is_empty() {
                verdict.set_text(&format!("{} free · fits", job::size(drive.free)));
                verdict.add_css_class("dimmed");
            }
        }

        if self.entitlement_mismatch(drive).is_some() {
            verdict.set_text(&format!(
                "Registered to SkyView {}",
                drive.entitlement.clone().unwrap_or_default()
            ));
            verdict.remove_css_class("dimmed");
            verdict.add_css_class("warning");
        }

        card.toggle.update_property(&[gtk::accessible::Property::Label(&format!(
            "{}, {}, {}",
            drive.name,
            cycle.text(),
            verdict.text()
        ))]);
    }

    /// Some(drive_serial) when the drive's chart key names a different SkyView.
    fn entitlement_mismatch(&self, drive: &Drive) -> Option<String> {
        let sources = self.imp().sources.borrow();
        let source_id = sources
            .aviation
            .as_ref()
            .or(sources.obstacle.as_ref())
            .and_then(|p| p.file_name())
            .and_then(|n| scan::parse_entitlement(&n.to_string_lossy()))?;
        let drive_id = drive.entitlement.clone()?;
        (drive_id != source_id).then_some(drive_id)
    }

    fn show_empty_drives(&self) {
        let imp = self.imp();
        imp.drives_stack.set_visible_child_name("empty");
        while let Some(child) = imp.drives_empty_actions.first_child() {
            imp.drives_empty_actions.remove(&child);
        }

        let sandbox = drive::Sandbox::detect();
        let (icon, title, description, show_help) = match sandbox.classify() {
            EmptyReason::SandboxBlocked => (
                "dialog-warning-symbolic",
                "Can't See Your Drives",
                "A USB drive is connected, but this app has not been given permission to read it. \
                 Choose the drive's folder to continue, or grant permission once and it will appear on its own.",
                true,
            ),
            EmptyReason::NotMounted => (
                "drive-removable-media-symbolic",
                "Drive Not Ready",
                "A USB drive is connected but has not been mounted yet. Open it once in Files, or choose its folder here.",
                false,
            ),
            EmptyReason::NothingConnected => (
                "drive-removable-media-symbolic",
                "No Drives Connected",
                "Plug in the USB drive you use with your SkyView. It will appear here automatically.",
                false,
            ),
        };
        imp.drives_empty.set_icon_name(Some(icon));
        imp.drives_empty.set_title(title);
        imp.drives_empty.set_description(Some(description));

        let choose = gtk::Button::builder()
            .label("Choose Folder…")
            .css_classes(if show_help { vec!["pill", "suggested-action"] } else { vec!["pill"] })
            .build();
        choose.connect_clicked(clone!(
            #[weak(rename_to = win)] self,
            move |_| win.choose_drive_folder()
        ));
        imp.drives_empty_actions.append(&choose);

        if show_help {
            let help = gtk::Button::builder().label("How to Fix This…").css_classes(["pill"]).build();
            help.connect_clicked(clone!(
                #[weak(rename_to = win)] self,
                move |_| win.show_sandbox_help()
            ));
            imp.drives_empty_actions.append(&help);
        }
    }

    fn refresh_cards(&self) {
        let needed = self.bytes_needed();
        let target_cycle = self.imp().sources.borrow().cycle();
        for card in self.imp().cards.borrow().iter() {
            self.paint_card(card, needed, target_cycle);
        }
    }

    fn selected(&self) -> Vec<Drive> {
        self.imp()
            .cards
            .borrow()
            .iter()
            .filter(|c| c.toggle.is_active())
            .map(|c| c.drive.clone())
            .collect()
    }

    fn select_all_ready(&self) {
        let needed = self.bytes_needed();
        for card in self.imp().cards.borrow().iter() {
            let ready = card.drive.recognised() && card.drive.writable && card.drive.fits(needed);
            if ready {
                card.toggle.set_active(true);
            }
        }
    }

    fn deselect_all(&self) {
        for card in self.imp().cards.borrow().iter() {
            card.toggle.set_active(false);
        }
    }

    fn bytes_needed(&self) -> u64 {
        let sources = self.imp().sources.borrow();
        let dbs: u64 = [&sources.aviation, &sources.obstacle]
            .iter()
            .filter_map(|p| p.as_ref())
            .filter_map(|p| std::fs::metadata(p).ok())
            .map(|m| m.len())
            .sum();
        dbs + sources.archive.as_ref().map(|a| a.total_bytes).unwrap_or(0)
    }
}

// ---------------------------------------------------------------------------
// Readiness, banner, confirmation
// ---------------------------------------------------------------------------

impl DynonWindow {
    fn update_ready(&self) {
        let imp = self.imp();
        let selected = self.selected();
        let needed = self.bytes_needed();

        let (folder_missing, folder_label, dups_empty, nothing_to_copy, loading) = {
            let sources = imp.sources.borrow();
            let folder_missing = sources.folder.as_ref().map(|f| !f.is_dir()).unwrap_or(true);
            let folder_label = sources
                .folder
                .as_ref()
                .map(|f| abbreviate(f))
                .unwrap_or_else(|| "the folder".into());
            (
                folder_missing,
                folder_label,
                sources.dups.is_empty(),
                !sources.anything_to_copy(),
                sources.archive_loading.clone(),
            )
        };

        let no_drives_at_all = imp.cards.borrow().is_empty();
        let bad_selected = selected.iter().find(|d| d.reachable && !d.writable);
        let full_selected = selected
            .iter()
            .find(|d| d.reachable && d.writable && !d.fits(needed));

        let reason: Option<String> = if folder_missing {
            Some("Choose a folder that contains this cycle's files".into())
        } else if dups_empty {
            Some(format!("No aviation or obstacle database found in {folder_label}"))
        } else if nothing_to_copy {
            Some("Choose at least one database or a plates archive to copy".into())
        } else if let Some(name) = loading {
            Some(format!("Still reading {name}…"))
        } else if no_drives_at_all {
            Some("Plug in a USB drive, or choose a drive folder, to continue".into())
        } else if selected.is_empty() {
            Some("Select a drive to continue".into())
        } else if let Some(d) = bad_selected {
            Some(format!("{} can't be written to — deselect it to continue", d.name))
        } else {
            full_selected.map(|d| format!("{} doesn't have room for this update — deselect it to continue", d.name))
        };

        let label = match selected.len() {
            0 => "Update Drives".to_string(),
            1 => {
                let name = &selected[0].name;
                if name.chars().count() <= 16 {
                    format!("Update {name}")
                } else {
                    "Update 1 Drive".into()
                }
            }
            n => format!("Update {n} Drives"),
        };
        imp.update_button.set_label(&label);

        match &reason {
            Some(text) => {
                imp.update_button.set_sensitive(false);
                imp.reason_label.set_text(text);
                imp.reason_label.remove_css_class("dimmed");
                imp.reason_label.add_css_class("warning");
            }
            None => {
                imp.update_button.set_sensitive(true);
                let drive_list = drive_list_text(&selected);
                let duration = duration_text(estimate_seconds(needed));
                let mut summary = format!("Writes {} to {drive_list} · about {duration}", job::size(needed));
                if self.preference("verify-copies", true) {
                    summary.push_str(" · copies verified");
                }
                if self.preference("replace-old-databases", true)
                    && selected.iter().any(|d| d.installed_cycle.is_some())
                {
                    summary.push_str(" · old databases replaced");
                }
                imp.reason_label.set_text(&summary);
                imp.reason_label.remove_css_class("warning");
                imp.reason_label.add_css_class("dimmed");
                if !self.preference("first-run-done", false) {
                    self.save("first-run-done", true);
                }
            }
        }

        self.update_banner();
    }

    fn update_banner(&self) {
        let imp = self.imp();
        let sources = imp.sources.borrow();
        let cards = imp.cards.borrow();
        let folder_label = sources.folder.as_ref().map(|f| abbreviate(f)).unwrap_or_default();
        let needed = self.bytes_needed();

        let mut picked: Option<(String, Option<(&'static str, BannerAction)>)> = None;

        // E7: unsafe archive paths.
        if let (Some(err), Some(path)) = (&sources.archive_error, &sources.archive_error_path) {
            if err.contains("unsafe file paths") {
                let name = path.file_name().unwrap_or_default().to_string_lossy();
                picked = Some((format!("{name} contains unsafe file paths and will not be used."), None));
            }
        }
        // E5 / E6: archive unreadable or empty.
        if picked.is_none() {
            if let (Some(_), Some(path)) = (&sources.archive_error, &sources.archive_error_path) {
                let name = path.file_name().unwrap_or_default().to_string_lossy().into_owned();
                picked = Some((
                    format!("{name} could not be read as a plates archive."),
                    Some(("Choose Another…", BannerAction::ChooseArchive)),
                ));
            }
        }
        // E1: source folder gone.
        if picked.is_none() {
            if let Some(folder) = &sources.folder {
                if !folder.is_dir() {
                    picked = Some((
                        format!("The folder {} is no longer available.", abbreviate(folder)),
                        Some(("Choose Folder…", BannerAction::ChooseFolder)),
                    ));
                }
            }
        }
        // E11: selected drive is read-only.
        if picked.is_none() {
            if let Some(card) = cards.iter().find(|c| c.toggle.is_active() && c.drive.reachable && !c.drive.writable) {
                picked = Some((
                    format!("{} cannot be written to.", card.drive.name),
                    Some(("Deselect", BannerAction::Deselect(card.drive.key()))),
                ));
            }
        }
        // E12: selected drive won't fit.
        if picked.is_none() {
            if let Some(card) = cards
                .iter()
                .find(|c| c.toggle.is_active() && c.drive.reachable && c.drive.writable && !c.drive.fits(needed))
            {
                picked = Some((
                    format!("{} does not have room for this update.", card.drive.name),
                    Some(("Deselect", BannerAction::Deselect(card.drive.key()))),
                ));
            }
        }
        // E13: folder target unreachable.
        if picked.is_none() {
            if let Some(card) = cards.iter().find(|c| c.drive.kind == TargetKind::Folder && !c.drive.reachable) {
                picked = Some((
                    format!("{} is not connected.", card.drive.name),
                    Some(("Choose Again…", BannerAction::ChooseAgain(card.drive.path.to_string_lossy().into_owned()))),
                ));
            }
        }
        // E2: no databases in folder.
        if picked.is_none() && sources.folder.as_ref().map(|f| f.is_dir()).unwrap_or(false) && sources.dups.is_empty() {
            picked = Some((
                format!("No databases found in {folder_label}."),
                Some(("Choose Folder…", BannerAction::ChooseFolder)),
            ));
        }
        // E15: entitlement mismatch.
        if picked.is_none() {
            if let Some(card) = cards.iter().find(|c| self.entitlement_mismatch(&c.drive).is_some()) {
                picked = Some((
                    format!("{} is registered to a different SkyView.", card.drive.name),
                    Some(("Details", BannerAction::Details(card.drive.key()))),
                ));
            }
        }
        // E17: archive cycle disagrees with database cycle.
        if picked.is_none() {
            if let (Some(archive), Some(dbcycle)) = (&sources.archive, sources.db_cycle()) {
                if let Some(acycle) = archive.cycle {
                    if acycle != dbcycle {
                        picked = Some((
                            format!("The plates archive is Cycle {acycle} but the databases are Cycle {dbcycle}."),
                            None,
                        ));
                    }
                }
            }
        }
        // E3: only one database kind found.
        if picked.is_none() {
            let has_av = sources.dups.iter().any(|d| d.kind == DupKind::Aviation);
            let has_ob = sources.dups.iter().any(|d| d.kind == DupKind::Obstacle);
            if has_av != has_ob {
                let which = if has_av { "aviation" } else { "obstacle" };
                picked = Some((
                    format!("Only the {which} database was found in {folder_label}."),
                    Some(("Choose Folder…", BannerAction::ChooseFolder)),
                ));
            }
        }
        // P10: drives present, none recognised.
        if picked.is_none() && !cards.is_empty() && cards.iter().all(|c| !c.drive.recognised()) {
            picked = Some(("No SkyView drives found. Select a drive to write to it anyway.".into(), None));
        }
        // P0: first run.
        if picked.is_none() && !self.preference("first-run-done", false) {
            picked = Some((
                "Choose this cycle's files, pick the drive you fly with, then update.".into(),
                None,
            ));
        }

        drop(cards);
        drop(sources);

        if let Some(id) = imp.banner_handler.borrow_mut().take() {
            imp.banner.disconnect(id);
        }

        match picked {
            Some((title, action)) => {
                imp.banner.set_title(&title);
                imp.banner.set_button_label(action.as_ref().map(|(l, _)| *l));
                imp.banner.set_revealed(true);
                if let Some((_, act)) = action {
                    let handler = imp.banner.connect_button_clicked(clone!(
                        #[weak(rename_to = win)] self,
                        move |_| win.run_banner_action(act.clone())
                    ));
                    imp.banner_handler.replace(Some(handler));
                }
            }
            None => {
                imp.banner.set_revealed(false);
                imp.banner.set_button_label(None);
            }
        }
    }

    fn run_banner_action(&self, action: BannerAction) {
        match action {
            BannerAction::ChooseFolder => self.choose_source_folder(),
            BannerAction::ChooseArchive => self.choose_archive(),
            BannerAction::Deselect(key) => {
                let card = self.imp().cards.borrow().iter().find(|c| c.drive.key() == key).map(|c| c.toggle.clone());
                if let Some(toggle) = card {
                    toggle.set_active(false);
                }
            }
            BannerAction::Details(key) => self.show_entitlement_details(&key),
            BannerAction::ChooseAgain(path) => self.choose_folder_again(&path),
        }
    }

    fn show_entitlement_details(&self, key: &str) {
        let Some(drive) = self.imp().cards.borrow().iter().find(|c| c.drive.key() == key).map(|c| c.drive.clone())
        else {
            return;
        };
        let Some(source_id) = ({
            let sources = self.imp().sources.borrow();
            sources
                .aviation
                .as_ref()
                .or(sources.obstacle.as_ref())
                .and_then(|p| p.file_name())
                .and_then(|n| scan::parse_entitlement(&n.to_string_lossy()))
        }) else {
            return;
        };
        let drive_serial = drive.entitlement.clone().unwrap_or_default();
        let body = format!(
            "{} carries a chart key for SkyView {drive_serial}, but this cycle's files are for {source_id}. \
             If this drive belongs to another aircraft, its charts may not open. Check before you use it.",
            drive.name
        );
        let dialog = adw::AlertDialog::builder()
            .heading("Registered to a Different SkyView")
            .body(body)
            .default_response("deselect")
            .close_response("deselect")
            .build();
        dialog.add_response("deselect", "Deselect Drive");
        dialog.add_response("use-anyway", "Use It Anyway");
        dialog.set_response_appearance("deselect", adw::ResponseAppearance::Suggested);
        let key = key.to_string();
        dialog.connect_response(None, clone!(
            #[weak(rename_to = win)] self,
            move |_, response| {
                if response == "deselect" {
                    win.run_banner_action(BannerAction::Deselect(key.clone()));
                }
            }
        ));
        dialog.present(Some(self));
    }

    fn choose_folder_again(&self, old_path: &str) {
        let dialog = gtk::FileDialog::builder().title("Select your drive").modal(true).build();
        let old = PathBuf::from(old_path);
        if let Some(parent) = old.parent().filter(|p| p.is_dir()) {
            dialog.set_initial_folder(Some(&gio::File::for_path(parent)));
        }
        let old_path = old_path.to_string();
        dialog.select_folder(Some(self), gio::Cancellable::NONE, clone!(
            #[weak(rename_to = win)] self,
            move |result| {
                if let Ok(file) = result {
                    if let Some(path) = file.path() {
                        let mut targets = win.manual_targets();
                        targets.retain(|p| p != &old_path);
                        let text = path.to_string_lossy().to_string();
                        if !targets.contains(&text) {
                            targets.push(text);
                        }
                        win.save_manual_targets(&targets);
                        win.refresh_drives();
                    }
                }
            }
        ));
    }

    fn confirm_update(&self) {
        let imp = self.imp();
        let selected = self.selected();
        if selected.is_empty() {
            return;
        }
        let needed = self.bytes_needed();
        let (has_archive, archive_name, archive_count) = {
            let sources = imp.sources.borrow();
            (
                sources.archive.is_some(),
                sources.archive.as_ref().map(|a| a.name()).unwrap_or_default(),
                sources.archive.as_ref().map(|a| a.members_stripped(sources.strip_wrapper).len()).unwrap_or(0),
            )
        };
        let drive_list = drive_list_text(&selected);
        let db_count = {
            let sources = imp.sources.borrow();
            [sources.aviation.is_some(), sources.obstacle.is_some()].iter().filter(|b| **b).count()
        };

        let heading = if has_archive {
            if selected.len() == 1 {
                format!("Replace Plates on {}?", selected[0].name)
            } else {
                format!("Replace Plates on {} Drives?", selected.len())
            }
        } else if selected.len() == 1 {
            format!("Update {}?", selected[0].name)
        } else {
            format!("Update {} Drives?", selected.len())
        };
        let body = if has_archive {
            let duration = duration_text(estimate_seconds(needed));
            format!(
                "The plates folder on {drive_list} will be erased and rebuilt from {archive_name}. \
                 This takes about {duration} and cannot be undone."
            )
        } else {
            format!("The aviation and obstacle databases on {drive_list} will be replaced. Plates are left as they are.")
        };

        let group = adw::PreferencesGroup::new();
        let mut unrecognised_names = Vec::new();
        for drive in &selected {
            let mut parts = Vec::new();
            if db_count > 0 {
                parts.push(format!("Copy {db_count} database{}", if db_count == 1 { "" } else { "s" }));
            }
            if has_archive {
                let existing = drive.reclaimable.map(|(_, n)| n).unwrap_or(0);
                parts.push(format!("erase {} plates", job::group(existing as u64)));
                parts.push(format!("write {} plates", job::group(archive_count as u64)));
            }
            let mut subtitle = parts.join(" · ");
            if !drive.recognised() {
                subtitle = format!("Not a SkyView drive · {subtitle}");
                unrecognised_names.push(drive.name.clone());
            }
            let row = adw::ActionRow::builder().title(&drive.name).subtitle(&subtitle).build();
            let icon = gtk::Image::from_icon_name(match drive.kind {
                TargetKind::Folder => "folder-symbolic",
                TargetKind::Mounted => "drive-harddisk-usb-symbolic",
            });
            row.add_prefix(&icon);
            row.set_tooltip_text(Some(&drive.path.to_string_lossy()));
            group.add(&row);
        }

        let extra = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(12).build();
        extra.append(&group);

        let checkbox = if !unrecognised_names.is_empty() {
            let label = if unrecognised_names.len() == 1 {
                format!("I understand that {} is not a SkyView drive", unrecognised_names[0])
            } else {
                format!("I understand that {} of these are not SkyView drives", unrecognised_names.len())
            };
            let check = gtk::CheckButton::builder().label(label).build();
            extra.append(&check);
            Some(check)
        } else {
            None
        };

        let dialog = adw::AlertDialog::builder()
            .heading(heading)
            .body(body)
            .extra_child(&extra)
            .default_response("cancel")
            .close_response("cancel")
            .build();
        dialog.add_response("cancel", "Cancel");
        if has_archive {
            dialog.add_response("update", "Replace Plates");
            dialog.set_response_appearance("update", adw::ResponseAppearance::Destructive);
        } else {
            dialog.add_response("update", "Update");
            dialog.set_response_appearance("update", adw::ResponseAppearance::Suggested);
        }
        if checkbox.is_some() {
            dialog.set_response_enabled("update", false);
        }
        if let Some(check) = &checkbox {
            check.connect_toggled(clone!(
                #[weak] dialog,
                move |c| dialog.set_response_enabled("update", c.is_active())
            ));
        }

        dialog.connect_response(None, clone!(
            #[weak(rename_to = win)] self,
            move |_, response| {
                if response == "update" {
                    win.begin_run(selected.clone());
                }
            }
        ));
        dialog.present(Some(self));
    }

    fn begin_run(&self, drives: Vec<Drive>) {
        let imp = self.imp();
        let sources = imp.sources.borrow();
        let plan = job::Plan {
            drives,
            aviation: sources.aviation.clone(),
            obstacle: sources.obstacle.clone(),
            archive: sources.archive.clone(),
            strip_wrapper: sources.strip_wrapper,
            verify: self.preference("verify-copies", true),
            replace_old: self.preference("replace-old-databases", true),
            cycle: sources.cycle(),
        };
        drop(sources);
        self.start_job(plan);
    }
}

// ---------------------------------------------------------------------------
// Running page
// ---------------------------------------------------------------------------

impl DynonWindow {
    fn start_job(&self, plan: job::Plan) {
        let imp = self.imp();
        let drives = plan.drives.clone();
        let (tx, rx) = channel();
        let cancel = job::Cancel::new();

        imp.log_header.replace(Some(self.log_header_text(&plan)));
        imp.log_entries.borrow_mut().clear();
        if let Some(list) = imp.log_list.borrow().as_ref() {
            while let Some(child) = list.first_child() {
                list.remove(&child);
            }
        }

        if let Some(old) = imp.run.borrow_mut().take() {
            for (_, row) in old.rows {
                imp.running_drives.remove(&row.row);
            }
            if old.inhibit != 0 {
                self.uninhibit(old.inhibit);
            }
        }

        let mut rows = Vec::new();
        for d in &drives {
            let rr = build_run_row(&d.name);
            imp.running_drives.add(&rr.row);
            rows.push((d.name.clone(), rr));
        }

        let inhibit_id = self
            .application()
            .and_downcast::<gtk::Application>()
            .map(|app| {
                app.inhibit(
                    Some(self),
                    gtk::ApplicationInhibitFlags::LOGOUT | gtk::ApplicationInhibitFlags::SUSPEND | gtk::ApplicationInhibitFlags::IDLE,
                    Some("Writing avionics data to USB drives"),
                )
            })
            .unwrap_or(0);

        *imp.run.borrow_mut() = Some(Run {
            rx,
            cancel: cancel.clone(),
            samples: Vec::new(),
            last_eta: None,
            rows,
            inhibit: inhibit_id,
            current_state: DriveState::Waiting,
            last_announce: Instant::now(),
            last_phase: String::new(),
        });

        imp.stack.set_visible_child_name("running");
        imp.percent_label.set_text("—");
        imp.progress.set_fraction(0.0);
        imp.step_label.set_text("Preparing…");
        imp.detail_label.set_text(&format!("Checking space on {} drives", drives.len()));
        imp.eta_label.set_text("Estimating time left…");
        imp.window_title.set_subtitle("0% — Estimating time left…");
        imp.cancel_button.grab_focus();

        std::thread::spawn(move || job::run(plan, tx, cancel));

        glib::timeout_add_local(Duration::from_millis(200), clone!(
            #[weak(rename_to = win)] self,
            #[upgrade_or] glib::ControlFlow::Break,
            move || win.poll_job()
        ));
    }

    /// Uninhibits exactly once, on whatever path ends the run.
    fn uninhibit(&self, cookie: u32) {
        if let Some(app) = self.application().and_downcast::<gtk::Application>() {
            app.uninhibit(cookie);
        }
    }

    fn poll_job(&self) -> glib::ControlFlow {
        let imp = self.imp();
        let events: Vec<Event> = {
            let run = imp.run.borrow();
            match run.as_ref() {
                Some(r) => {
                    let mut v = Vec::new();
                    while let Ok(e) = r.rx.try_recv() {
                        v.push(e);
                    }
                    v
                }
                None => return glib::ControlFlow::Break,
            }
        };

        let mut finished = None;
        for event in events {
            match event {
                Event::Finished(outcomes) => finished = Some(outcomes),
                other => self.handle_job_event(other),
            }
        }

        if let Some(outcomes) = finished {
            self.finish_job(outcomes);
            return glib::ControlFlow::Break;
        }
        if imp.run.borrow().is_none() {
            return glib::ControlFlow::Break;
        }
        glib::ControlFlow::Continue
    }

    fn handle_job_event(&self, event: Event) {
        let imp = self.imp();
        match event {
            Event::Step { step, detail } => {
                imp.step_label.set_text(&step);
                let showing_extraction = matches!(
                    imp.run.borrow().as_ref().map(|r| r.current_state),
                    Some(DriveState::ExtractingPlates) | Some(DriveState::ErasingPlates) | Some(DriveState::Finishing) | None
                );
                if showing_extraction || step.starts_with("Preparing") {
                    imp.detail_label.set_text(&detail);
                }
                let phase_changed = {
                    let mut run = imp.run.borrow_mut();
                    if let Some(run) = run.as_mut() {
                        let changed = run.last_phase != step;
                        run.last_phase = step.clone();
                        changed
                    } else {
                        false
                    }
                };
                if phase_changed {
                    self.announce_progress(&step);
                }
            }
            Event::Progress { done, total } => {
                let percent = if total > 0 { ((done as f64 / total as f64) * 100.0).clamp(0.0, 100.0) } else { 0.0 };
                imp.progress.set_fraction((percent / 100.0).clamp(0.0, 1.0));
                imp.percent_label.set_text(&format!("{}%", percent.round() as u32));

                let db_phase = matches!(
                    imp.run.borrow().as_ref().map(|r| r.current_state),
                    Some(DriveState::CopyingDatabases) | Some(DriveState::CheckingCopies)
                );
                if db_phase {
                    imp.detail_label.set_text(&format!("{} of {}", job::size(done), job::size(total)));
                }

                let eta = {
                    let mut run = imp.run.borrow_mut();
                    let Some(run) = run.as_mut() else { return };
                    let now = Instant::now();
                    run.samples.push((now, done));
                    run.samples.retain(|(t, _)| now.duration_since(*t) <= Duration::from_secs(30));
                    eta_for(run, done, total)
                };
                imp.eta_label.set_text(&eta);
                imp.window_title.set_subtitle(&format!("{}% — {eta}", percent.round() as u32));

                let announce = {
                    let mut run = imp.run.borrow_mut();
                    match run.as_mut() {
                        Some(run) if now_elapsed(run.last_announce) >= Duration::from_secs(15) => {
                            run.last_announce = Instant::now();
                            true
                        }
                        _ => false,
                    }
                };
                if announce {
                    let step = imp.step_label.text().to_string();
                    self.announce_progress(&format!("{step}, {} percent, {eta}", percent.round() as u32));
                }
            }
            Event::DriveState { drive, state } => {
                if let Some(run) = imp.run.borrow_mut().as_mut() {
                    run.current_state = state;
                    if let Some((_, row)) = run.rows.iter().find(|(n, _)| n == &drive) {
                        apply_drive_state(row, state, None);
                    }
                }
            }
            Event::Log { severity, message } => {
                let time = time_now();
                imp.log_entries.borrow_mut().push((severity, time.clone(), message.clone()));
                if let Some(list) = imp.log_list.borrow().as_ref() {
                    list.append(&log_row(severity, &time, &message));
                }
                if severity == Severity::Error {
                    self.announce_progress(&message);
                }
            }
            Event::PointOfNoReturn => {
                // Nothing further to do here: `job::Cancel::past_point_of_no_return`
                // is the source of truth the guard consults directly.
            }
            Event::Finished(_) => unreachable!("handled by the caller"),
        }
    }

    fn announce_progress(&self, text: &str) {
        self.imp().step_label.announce(text, gtk::AccessibleAnnouncementPriority::Medium);
    }

    fn finish_job(&self, outcomes: Vec<DriveOutcome>) {
        let imp = self.imp();
        if let Some(run) = imp.run.borrow_mut().take() {
            if run.inhibit != 0 {
                self.uninhibit(run.inhibit);
            }
        }
        imp.outcomes.replace(outcomes.clone());
        self.write_log_file();
        self.show_result(&outcomes);
    }

    /// Guarded stop: before the erase, stop immediately; after it, confirm.
    fn request_stop(&self) {
        let imp = self.imp();
        let past = imp.run.borrow().as_ref().map(|r| r.cancel.past_point_of_no_return()).unwrap_or(false);
        if !past {
            if let Some(run) = imp.run.borrow().as_ref() {
                run.cancel.request();
            }
            self.toast("Update stopped. Nothing was changed on your drives.");
            return;
        }
        let dialog = adw::AlertDialog::builder()
            .heading("Stop the Update?")
            .body(
                "This drive's plates folder has already been erased. Stopping now leaves it incomplete, \
                 and your SkyView will not find your approach plates until you run the update again.",
            )
            .default_response("keep")
            .close_response("keep")
            .build();
        dialog.add_response("keep", "Keep Updating");
        dialog.add_response("stop", "Stop Update");
        dialog.set_response_appearance("stop", adw::ResponseAppearance::Destructive);
        dialog.connect_response(None, clone!(
            #[weak(rename_to = win)] self,
            move |_, response| {
                if response == "stop" {
                    if let Some(run) = win.imp().run.borrow().as_ref() {
                        run.cancel.request();
                    }
                }
            }
        ));
        dialog.present(Some(self));
    }

    /// `WindowImpl::close_request` calls this. Blocks the close during the
    /// dangerous window (R3–R5) exactly like the Cancel button does.
    pub(super) fn guarded_close(&self) -> glib::Propagation {
        let imp = self.imp();
        let past = imp.run.borrow().as_ref().map(|r| r.cancel.past_point_of_no_return()).unwrap_or(false);
        if imp.run.borrow().is_none() {
            self.save_window_state();
            return glib::Propagation::Proceed;
        }
        if !past {
            // Stopping now costs nothing, but closing mid-run without asking
            // would still surprise the user; stop the job and let it wind down.
            if let Some(run) = imp.run.borrow().as_ref() {
                run.cancel.request();
            }
            return glib::Propagation::Stop;
        }
        self.request_stop();
        glib::Propagation::Stop
    }

    fn save_window_state(&self) {
        self.save("window-width", self.default_size().0);
        self.save("window-height", self.default_size().1);
        self.save("window-maximized", self.is_maximized());
        let selected: Vec<String> = self.selected().iter().map(|d| d.key()).collect();
        let refs: Vec<&str> = selected.iter().map(|s| s.as_str()).collect();
        if let Some(settings) = self.imp().settings.borrow().as_ref() {
            let _ = settings.set_strv("selected-drives", refs);
        }
    }
}

fn now_elapsed(since: Instant) -> Duration {
    Instant::now().saturating_duration_since(since)
}

fn eta_for(run: &mut Run, done: u64, total: u64) -> String {
    let elapsed_since_first = run
        .samples
        .first()
        .map(|(t, _)| now_elapsed(*t))
        .unwrap_or(Duration::ZERO);
    if run.samples.len() < 2 || elapsed_since_first < Duration::from_secs(20) {
        return "Estimating time left…".into();
    }
    let (t0, d0) = run.samples.first().copied().unwrap();
    let (t1, d1) = run.samples.last().copied().unwrap();
    let dt = t1.saturating_duration_since(t0).as_secs_f64().max(0.5);
    let bytes_per_sec = ((d1.saturating_sub(d0)) as f64 / dt).max(1.0);
    let remaining = total.saturating_sub(done) as f64;
    let raw = remaining / bytes_per_sec;

    let damped = match run.last_eta {
        Some(last) if raw > last.as_secs_f64() => (last.as_secs_f64() * 1.2).min(raw),
        _ => raw,
    };
    run.last_eta = Some(Duration::from_secs_f64(damped.max(0.0)));
    eta_vocabulary(damped)
}

fn eta_vocabulary(seconds: f64) -> String {
    let secs = seconds.max(0.0) as u64;
    if secs < 60 {
        "Less than a minute left".into()
    } else {
        let minutes = (secs as f64 / 60.0).round() as u64;
        if minutes <= 1 {
            "About 1 minute left".into()
        } else if minutes < 60 {
            format!("About {minutes} minutes left")
        } else {
            let hours = minutes / 60;
            let rem = minutes % 60;
            format!("About {hours} hours {rem} minutes left")
        }
    }
}

fn drive_state_text(state: DriveState) -> &'static str {
    match state {
        DriveState::Waiting => "Waiting",
        DriveState::CopyingDatabases => "Copying databases",
        DriveState::CheckingCopies => "Checking copies",
        DriveState::ErasingPlates => "Erasing plates",
        DriveState::ExtractingPlates => "Extracting plates",
        DriveState::Finishing => "Finishing",
        DriveState::Done => "Done",
        DriveState::Failed => "Failed",
        DriveState::Skipped => "Skipped",
        DriveState::Stopped => "Stopped",
    }
}

fn apply_drive_state(row: &RunRow, state: DriveState, reason: Option<&str>) {
    row.state_label.set_text(drive_state_text(state));
    for class in ["success", "error", "warning", "dimmed"] {
        row.state_label.remove_css_class(class);
    }
    let active = matches!(
        state,
        DriveState::CopyingDatabases
            | DriveState::CheckingCopies
            | DriveState::ErasingPlates
            | DriveState::ExtractingPlates
            | DriveState::Finishing
    );
    row.spinner.set_visible(active);
    row.bar.set_visible(active);
    row.icon.set_visible(matches!(state, DriveState::Done | DriveState::Failed));
    match state {
        DriveState::Done => {
            row.icon.set_icon_name(Some("emblem-ok-symbolic"));
            row.icon.remove_css_class("error");
            row.icon.add_css_class("success");
            row.state_label.add_css_class("success");
        }
        DriveState::Failed => {
            row.icon.set_icon_name(Some("dialog-error-symbolic"));
            row.icon.remove_css_class("success");
            row.icon.add_css_class("error");
            row.state_label.add_css_class("error");
            if let Some(r) = reason {
                row.row.set_subtitle(r);
            }
        }
        DriveState::Skipped | DriveState::Stopped => row.state_label.add_css_class("dimmed"),
        _ => {}
    }
    let name = row.row.title();
    row.row
        .update_property(&[gtk::accessible::Property::Label(&format!("{name}, {}", drive_state_text(state).to_lowercase()))]);
}

fn build_run_row(name: &str) -> RunRow {
    let icon = gtk::Image::new();
    icon.set_visible(false);
    icon.set_pixel_size(16);
    let spinner = adw::Spinner::new();
    spinner.set_visible(false);
    spinner.set_valign(gtk::Align::Center);
    let state_label = gtk::Label::builder().css_classes(["caption"]).valign(gtk::Align::Center).build();
    let bar = gtk::ProgressBar::builder().height_request(4).hexpand(true).visible(false).build();

    let top = gtk::Box::builder().spacing(6).valign(gtk::Align::Center).build();
    top.append(&state_label);
    top.append(&spinner);

    let suffix = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(2).valign(gtk::Align::Center).build();
    suffix.append(&top);
    suffix.append(&bar);

    let row = adw::ActionRow::builder().title(name).build();
    row.add_prefix(&icon);
    row.add_suffix(&suffix);
    row.update_property(&[gtk::accessible::Property::Label(&format!("{name}, waiting"))]);

    RunRow { row, state_label, spinner, bar, icon }
}

// ---------------------------------------------------------------------------
// Result page
// ---------------------------------------------------------------------------

impl DynonWindow {
    fn show_result(&self, outcomes: &[DriveOutcome]) {
        let imp = self.imp();
        imp.stack.set_visible_child_name("result");

        let total = outcomes.len();
        let updated: Vec<&DriveOutcome> = outcomes.iter().filter(|o| o.result == Outcome::Updated).collect();
        let interrupted: Vec<&DriveOutcome> = outcomes.iter().filter(|o| o.result == Outcome::Interrupted).collect();
        let failed: Vec<&DriveOutcome> = outcomes.iter().filter(|o| matches!(o.result, Outcome::Failed(_))).collect();
        let cycle = outcomes.iter().find_map(|o| o.cycle);

        let (icon, title, description) = if !interrupted.is_empty() {
            (
                "dialog-warning-symbolic",
                "Update Stopped".to_string(),
                format!(
                    "{}'s plates were being replaced when you stopped. Its plates folder is incomplete — \
                     run the update again before you fly with it.",
                    interrupted[0].name
                ),
            )
        } else if !updated.is_empty() && updated.len() == total {
            let cycle_text = cycle.map(|c| c.to_string()).unwrap_or_default();
            if total == 1 {
                (
                    "emblem-ok-symbolic",
                    format!("{} Updated", updated[0].name),
                    format!("Cycle {cycle_text} is installed. Eject the drive before unplugging it."),
                )
            } else {
                (
                    "emblem-ok-symbolic",
                    format!("{total} Drives Updated"),
                    format!("Cycle {cycle_text} is installed. Eject the drives before unplugging them."),
                )
            }
        } else if updated.is_empty() {
            (
                "dialog-error-symbolic",
                "No Drives Were Updated".to_string(),
                "Nothing was written. Check the reason below.".to_string(),
            )
        } else {
            let failed_names = drive_names_text(&failed.iter().map(|o| o.name.clone()).collect::<Vec<_>>());
            (
                "dialog-warning-symbolic",
                format!("{} of {total} Drives Updated", updated.len()),
                format!("{failed_names} was not updated. Check the reason below before you fly."),
            )
        };

        imp.result_page.set_icon_name(Some(icon));
        imp.result_page.set_title(&title);
        imp.result_page.set_description(Some(&description));
        imp.result_page.announce(&format!("{title}. {description}"), gtk::AccessibleAnnouncementPriority::High);

        while let Some(child) = imp.result_drives.first_child() {
            imp.result_drives.remove(&child);
        }
        while let Some(child) = imp.result_actions.first_child() {
            imp.result_actions.remove(&child);
        }

        for outcome in outcomes {
            let (icon_name, class, subtitle) = match &outcome.result {
                Outcome::Updated => {
                    let mut parts = Vec::new();
                    if let Some(c) = outcome.cycle {
                        parts.push(format!("Cycle {c}"));
                    }
                    if outcome.plates_written > 0 {
                        parts.push(format!("{} plates", job::group(outcome.plates_written as u64)));
                    }
                    parts.push(duration_precise(outcome.elapsed.as_secs()));
                    ("emblem-ok-symbolic", "success", parts.join(" · "))
                }
                Outcome::Failed(reason) => ("dialog-error-symbolic", "error", capitalize(reason)),
                Outcome::Skipped => ("dialog-warning-symbolic", "warning", "Not started".to_string()),
                Outcome::Interrupted => ("dialog-warning-symbolic", "warning", "Interrupted — plates folder is incomplete".to_string()),
            };
            let row = adw::ActionRow::builder().title(&outcome.name).subtitle(&subtitle).build();
            let prefix = gtk::Image::from_icon_name(icon_name);
            prefix.add_css_class(class);
            row.add_prefix(&prefix);
            if matches!(outcome.result, Outcome::Failed(_)) {
                let retry = gtk::Button::builder().label("Retry").css_classes(["flat"]).valign(gtk::Align::Center).build();
                let name = outcome.name.clone();
                retry.connect_clicked(clone!(
                    #[weak(rename_to = win)] self,
                    move |_| win.retry_drive(&name)
                ));
                row.add_suffix(&retry);
            }
            imp.result_drives.add(&row);
        }

        let ejectable: Vec<&DriveOutcome> = updated.iter().filter(|o| o.kind == TargetKind::Mounted).copied().collect();
        if !ejectable.is_empty() {
            let label = match ejectable.len() {
                1 => format!("Eject {}", ejectable[0].name),
                2 => "Eject Both".to_string(),
                _ => "Eject All".to_string(),
            };
            let eject = gtk::Button::builder().label(label).css_classes(["pill", "suggested-action"]).build();
            let paths: Vec<PathBuf> = ejectable.iter().map(|o| o.path.clone()).collect();
            let names: Vec<String> = ejectable.iter().map(|o| o.name.clone()).collect();
            eject.connect_clicked(clone!(
                #[weak(rename_to = win)] self,
                move |_| win.eject_drives(paths.clone(), names.clone())
            ));
            imp.result_actions.append(&eject);
        }

        let done = gtk::Button::builder().label("Done").css_classes(["pill"]).build();
        done.connect_clicked(clone!(
            #[weak(rename_to = win)] self,
            move |_| win.finish_result()
        ));
        imp.result_actions.append(&done);

        if let Some(old) = imp.result_details_scroller.borrow_mut().take() {
            imp.result_details_row.remove(&old);
        }
        let details_list = self.build_plain_log_list();
        // F0 (everything updated) starts collapsed; every other outcome starts
        // expanded, because the reason the user needs is right there.
        let all_ok = icon == "emblem-ok-symbolic" && updated.len() == total;
        imp.result_details_row.set_expanded(!all_ok);

        let scroller = gtk::ScrolledWindow::builder()
            .child(&details_list)
            .propagate_natural_height(true)
            .max_content_height(300)
            .build();
        imp.result_details_row.add_row(&scroller);
        imp.result_details_scroller.replace(Some(scroller.upcast()));

        if let Some(first) = imp.result_actions.first_child() {
            first.grab_focus();
        }
    }

    fn retry_drive(&self, name: &str) {
        self.refresh_drives();
        let imp = self.imp();
        for card in imp.cards.borrow().iter() {
            card.toggle.set_active(card.drive.name == name);
        }
        self.update_ready();
        self.confirm_update();
    }

    fn finish_result(&self) {
        let imp = self.imp();
        imp.stack.set_visible_child_name("prepare");
        self.refresh_drives();
        self.refresh_sources();
    }

    fn eject_drives(&self, paths: Vec<PathBuf>, names: Vec<String>) {
        for (path, name) in paths.into_iter().zip(names) {
            let file = gio::File::for_path(&path);
            let Ok(mount) = file.find_enclosing_mount(gio::Cancellable::NONE) else {
                self.toast_persistent(&format!("Could not eject {name} — eject it from Files"));
                continue;
            };
            mount.eject_with_operation(
                gio::MountUnmountFlags::NONE,
                gio::MountOperation::NONE,
                gio::Cancellable::NONE,
                clone!(
                    #[weak(rename_to = win)] self,
                    move |result| {
                        if result.is_ok() {
                            win.toast(&format!("{name} can be unplugged"));
                        } else {
                            win.toast_persistent(&format!("Could not eject {name} — eject it from Files"));
                        }
                    }
                ),
            );
        }
    }
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(first) => first.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

fn drive_names_text(names: &[String]) -> String {
    match names.len() {
        0 => String::new(),
        1 => names[0].clone(),
        2 => format!("{} and {}", names[0], names[1]),
        n => format!("{n} drives"),
    }
}

// ---------------------------------------------------------------------------
// Log: building, exporting, writing to disk
// ---------------------------------------------------------------------------

impl DynonWindow {
    fn build_log_list(&self) {
        let imp = self.imp();
        let list = gtk::ListBox::builder().selection_mode(gtk::SelectionMode::None).css_classes(["background"]).build();
        list.update_property(&[gtk::accessible::Property::Label("Activity log")]);

        let scroller = gtk::ScrolledWindow::builder()
            .child(&list)
            .min_content_height(180)
            .max_content_height(300)
            .propagate_natural_height(true)
            .build();

        let copy = gtk::Button::builder()
            .icon_name("edit-copy-symbolic")
            .tooltip_text("Copy the log to the clipboard")
            .css_classes(["flat"])
            .valign(gtk::Align::Center)
            .build();
        copy.update_property(&[gtk::accessible::Property::Label("Copy the log")]);
        copy.connect_clicked(clone!(
            #[weak(rename_to = win)] self,
            move |_| win.copy_log_to_clipboard()
        ));

        let save = gtk::Button::builder()
            .icon_name("document-save-symbolic")
            .tooltip_text("Save the log to a file")
            .css_classes(["flat"])
            .valign(gtk::Align::Center)
            .build();
        save.update_property(&[gtk::accessible::Property::Label("Save the log to a file")]);
        save.connect_clicked(clone!(
            #[weak(rename_to = win)] self,
            move |_| win.save_log_to_file()
        ));

        let suffix = gtk::Box::builder().spacing(6).build();
        suffix.append(&copy);
        suffix.append(&save);
        imp.details_row.add_suffix(&suffix);
        imp.details_row.add_row(&scroller);

        imp.log_list.replace(Some(list));
    }

    /// A fresh, read-only rendering of the current log — used for the result
    /// page's Details and the standalone Activity Log dialog.
    fn build_plain_log_list(&self) -> gtk::ListBox {
        let list = gtk::ListBox::builder().selection_mode(gtk::SelectionMode::None).css_classes(["background"]).build();
        list.update_property(&[gtk::accessible::Property::Label("Activity log")]);
        for (severity, time, message) in self.imp().log_entries.borrow().iter() {
            list.append(&log_row(*severity, time, message));
        }
        list
    }

    fn log_header_text(&self, plan: &job::Plan) -> String {
        let now = glib::DateTime::now_local().or_else(|_| glib::DateTime::now_utc()).expect("system clock");
        let started = now.format("%Y-%m-%d %H:%M:%S").map(|s| s.to_string()).unwrap_or_default();
        let mut out = format!("Dynon USB Updater 1.0 — log started {started}\n");
        let sources = self.imp().sources.borrow();
        if let Some(folder) = &sources.folder {
            out.push_str(&format!("Source folder: {}\n", folder.display()));
        }
        if let Some(av) = &plan.aviation {
            let name = av.file_name().unwrap_or_default().to_string_lossy();
            let cycle = scan::parse_cycle(&name).map(|c| c.to_string()).unwrap_or_default();
            let size = std::fs::metadata(av).map(|m| job::size(m.len())).unwrap_or_default();
            out.push_str(&format!("Aviation:  {name} (Cycle {cycle}, {size})\n"));
        }
        if let Some(ob) = &plan.obstacle {
            let name = ob.file_name().unwrap_or_default().to_string_lossy();
            let cycle = scan::parse_cycle(&name).map(|c| c.to_string()).unwrap_or_default();
            let size = std::fs::metadata(ob).map(|m| job::size(m.len())).unwrap_or_default();
            out.push_str(&format!("Obstacle:  {name} (Cycle {cycle}, {size})\n"));
        }
        if let Some(archive) = &plan.archive {
            out.push_str(&format!(
                "Plates:    {} ({} files, {})\n",
                archive.name(),
                job::group(archive.members.len() as u64),
                job::size(archive.total_bytes)
            ));
        }
        let drives: Vec<String> = plan
            .drives
            .iter()
            .map(|d| format!("{} ({})", d.name, d.path.display()))
            .collect();
        out.push_str(&format!("Drives:    {}\n", drives.join(", ")));
        out.push_str(&format!(
            "Options:   verify={} replace-old={} eject={}\n",
            onoff(plan.verify),
            onoff(plan.replace_old),
            onoff(self.preference("eject-when-finished", false)),
        ));
        out
    }

    fn log_export_text(&self) -> String {
        let imp = self.imp();
        let mut out = imp.log_header.borrow().clone().unwrap_or_default();
        out.push_str("---\n");
        for (severity, time, message) in imp.log_entries.borrow().iter() {
            let tag = match severity {
                Severity::Success => "[ok]   ",
                Severity::Warning => "[warn] ",
                Severity::Error => "[error]",
                Severity::Info => "       ",
            };
            out.push_str(&format!("{time}\t{tag} {message}\n"));
        }
        out
    }

    fn write_log_file(&self) {
        let Some(dir) = log_dir() else { return };
        let _ = std::fs::create_dir_all(&dir);
        let stamp = glib::DateTime::now_local()
            .or_else(|_| glib::DateTime::now_utc())
            .expect("system clock")
            .format("%Y-%m-%d-%H%M%S")
            .map(|s| s.to_string())
            .unwrap_or_else(|_| "run".to_string());
        let path = dir.join(format!("{stamp}.log"));
        let _ = std::fs::write(&path, self.log_export_text());

        // Keep the last 20.
        if let Ok(entries) = std::fs::read_dir(&dir) {
            let mut files: Vec<_> = entries
                .flatten()
                .filter(|e| e.path().extension().map(|x| x == "log").unwrap_or(false))
                .collect();
            files.sort_by_key(|e| e.file_name());
            if files.len() > 20 {
                for old in &files[..files.len() - 20] {
                    let _ = std::fs::remove_file(old.path());
                }
            }
        }
    }

    fn copy_log_to_clipboard(&self) {
        self.clipboard().set_text(&self.log_export_text());
        self.toast("Log copied to clipboard");
    }

    fn save_log_to_file(&self) {
        let dialog = gtk::FileDialog::builder().title("Save the activity log").initial_name("dynon-usb-updater.log").build();
        let text = self.log_export_text();
        dialog.save(Some(self), gio::Cancellable::NONE, clone!(
            #[weak(rename_to = win)] self,
            move |result| {
                if let Ok(file) = result {
                    if file
                        .replace_contents(text.as_bytes(), None, false, gio::FileCreateFlags::NONE, gio::Cancellable::NONE)
                        .is_ok()
                    {
                        win.toast_action("Log saved", "Open", clone!(
                            #[weak] file,
                            #[weak] win,
                            move || {
                                gtk::FileLauncher::new(Some(&file)).launch(Some(&win), gio::Cancellable::NONE, |_| {});
                            }
                        ));
                    }
                }
            }
        ));
    }

    fn show_log_dialog(&self) {
        let list = self.build_plain_log_list();
        let scroller = gtk::ScrolledWindow::builder().child(&list).vexpand(true).build();

        let copy = gtk::Button::builder().icon_name("edit-copy-symbolic").tooltip_text("Copy the log to the clipboard").css_classes(["flat"]).build();
        copy.update_property(&[gtk::accessible::Property::Label("Copy the log")]);
        copy.connect_clicked(clone!(
            #[weak(rename_to = win)] self,
            move |_| win.copy_log_to_clipboard()
        ));
        let save = gtk::Button::builder().icon_name("document-save-symbolic").tooltip_text("Save the log to a file").css_classes(["flat"]).build();
        save.update_property(&[gtk::accessible::Property::Label("Save the log to a file")]);
        save.connect_clicked(clone!(
            #[weak(rename_to = win)] self,
            move |_| win.save_log_to_file()
        ));

        let header = adw::HeaderBar::new();
        header.pack_end(&save);
        header.pack_end(&copy);
        let toolbar = adw::ToolbarView::new();
        toolbar.add_top_bar(&header);
        toolbar.set_content(Some(&scroller));

        let dialog = adw::Dialog::builder().title("Activity Log").content_width(700).content_height(560).child(&toolbar).build();
        dialog.present(Some(self));
    }
}

fn time_now() -> String {
    glib::DateTime::now_local()
        .or_else(|_| glib::DateTime::now_utc())
        .ok()
        .and_then(|d| d.format("%H:%M:%S").ok())
        .map(|s| s.to_string())
        .unwrap_or_default()
}

fn onoff(b: bool) -> &'static str {
    if b { "on" } else { "off" }
}

fn log_dir() -> Option<PathBuf> {
    Some(glib::user_data_dir().join(APP_ID).join("logs"))
}

fn log_row(severity: Severity, time: &str, message: &str) -> gtk::Box {
    let row = gtk::Box::builder()
        .spacing(8)
        .margin_top(6)
        .margin_bottom(6)
        .margin_start(12)
        .margin_end(12)
        .build();

    let icon = gtk::Image::builder().pixel_size(16).build();
    let (icon_name, class) = match severity {
        Severity::Info => (None, ""),
        Severity::Success => (Some("emblem-ok-symbolic"), "success"),
        Severity::Warning => (Some("dialog-warning-symbolic"), "warning"),
        Severity::Error => (Some("dialog-error-symbolic"), "error"),
    };
    if let Some(name) = icon_name {
        icon.set_icon_name(Some(name));
        icon.add_css_class(class);
    }
    row.append(&icon);

    let time_label = gtk::Label::builder().label(time).css_classes(["caption", "dimmed", "numeric"]).build();
    row.append(&time_label);

    let msg = gtk::Label::builder()
        .label(message)
        .wrap(true)
        .xalign(0.0)
        .hexpand(true)
        .selectable(true)
        .css_classes(["monospace"])
        .build();
    if !class.is_empty() {
        msg.add_css_class(class);
    }
    row.append(&msg);

    let prefix = match severity {
        Severity::Success => "Success. ",
        Severity::Warning => "Warning. ",
        Severity::Error => "Error. ",
        Severity::Info => "",
    };
    row.update_property(&[gtk::accessible::Property::Label(&format!("{prefix}{message}"))]);
    row
}

// ---------------------------------------------------------------------------
// Toasts
// ---------------------------------------------------------------------------

impl DynonWindow {
    fn toast(&self, message: &str) {
        let toast = adw::Toast::builder().title(message).timeout(5).build();
        self.imp().toasts.add_toast(toast);
    }

    /// Never auto-dismisses; used only for the eject failure, which needs a
    /// deliberate acknowledgement.
    fn toast_persistent(&self, message: &str) {
        let toast = adw::Toast::builder().title(message).timeout(0).build();
        self.imp().toasts.add_toast(toast);
    }

    fn toast_action(&self, message: &str, button: &str, action: impl Fn() + 'static) {
        let toast = adw::Toast::builder().title(message).button_label(button).timeout(5).build();
        toast.connect_button_clicked(move |_| action());
        self.imp().toasts.add_toast(toast);
    }
}

// ---------------------------------------------------------------------------
// Preferences, About, sandbox help, shortcuts
// ---------------------------------------------------------------------------

impl DynonWindow {
    fn show_preferences(&self) {
        let dialog = adw::PreferencesDialog::new();
        let page = adw::PreferencesPage::new();

        let updating = adw::PreferencesGroup::builder().title("Updating").build();
        let verify = adw::SwitchRow::builder()
            .title("Verify Copies")
            .subtitle("Read each database back from the drive and compare checksums. Adds about a minute.")
            .active(self.preference("verify-copies", true))
            .build();
        verify.connect_active_notify(clone!(
            #[weak(rename_to = win)] self,
            move |row| { win.save("verify-copies", row.is_active()); win.update_ready(); }
        ));
        let replace_old = adw::SwitchRow::builder()
            .title("Replace Older Databases")
            .subtitle("Delete previous cycles from the drive so your SkyView sees only the new one.")
            .active(self.preference("replace-old-databases", true))
            .build();
        replace_old.connect_active_notify(clone!(
            #[weak(rename_to = win)] self,
            move |row| { win.save("replace-old-databases", row.is_active()); win.update_ready(); }
        ));
        let eject = adw::SwitchRow::builder()
            .title("Eject When Finished")
            .subtitle("Eject each drive automatically after it is updated successfully.")
            .active(self.preference("eject-when-finished", false))
            .build();
        eject.connect_active_notify(clone!(
            #[weak(rename_to = win)] self,
            move |row| win.save("eject-when-finished", row.is_active())
        ));
        updating.add(&verify);
        updating.add(&replace_old);
        updating.add(&eject);
        page.add(&updating);

        let logs = adw::PreferencesGroup::builder().title("Logs").build();
        let logs_row = adw::ActionRow::builder().title("Activity Logs").subtitle("Every run is saved here").build();
        let open = gtk::Button::builder().icon_name("folder-open-symbolic").css_classes(["flat"]).valign(gtk::Align::Center).build();
        open.update_property(&[gtk::accessible::Property::Label("Open the logs folder")]);
        open.connect_clicked(clone!(
            #[weak(rename_to = win)] self,
            move |_| {
                if let Some(dir) = log_dir() {
                    let _ = std::fs::create_dir_all(&dir);
                    let file = gio::File::for_path(&dir);
                    gtk::FileLauncher::new(Some(&file)).launch(Some(&win), gio::Cancellable::NONE, |_| {});
                }
            }
        ));
        logs_row.add_suffix(&open);
        logs_row.set_activatable_widget(Some(&open));
        logs.add(&logs_row);
        page.add(&logs);

        dialog.add(&page);
        dialog.present(Some(self));
    }

    fn show_about(&self) {
        let about = adw::AboutDialog::builder()
            .application_name("Dynon USB Updater")
            .application_icon(APP_ID)
            .version("1.0.0")
            .developer_name("Yacine Filali")
            .developers(vec!["Yacine Filali".to_string()])
            .copyright("© 2026 Yacine Filali")
            .license_type(gtk::License::Gpl30)
            .comments(
                "Copies each AIRAC cycle's aviation and obstacle databases and approach plates \
                 onto the USB drives you carry to the aircraft.",
            )
            .website("https://github.com/yfilali/dynon-usb-updater")
            .issue_url("https://github.com/yfilali/dynon-usb-updater/issues")
            .build();
        about.add_legal_section(
            "Trademark Notice",
            None,
            gtk::License::Custom,
            Some("Not affiliated with, endorsed by, or supported by Dynon Avionics."),
        );
        about.present(Some(self));
    }

    fn show_sandbox_help(&self) {
        let command = format!("flatpak override --user --filesystem=/run/media {APP_ID}");
        let label = gtk::Label::builder()
            .label(&command)
            .selectable(true)
            .css_classes(["monospace"])
            .wrap(true)
            .xalign(0.0)
            .build();
        let copy = gtk::Button::builder().label("Copy Command").icon_name("edit-copy-symbolic").css_classes(["flat"]).build();
        let cmd = command.clone();
        copy.connect_clicked(clone!(
            #[weak(rename_to = win)] self,
            move |_| win.clipboard().set_text(&cmd)
        ));
        let extra = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(8).build();
        extra.append(&label);
        extra.append(&copy);

        let dialog = adw::AlertDialog::builder()
            .heading("Allow Access to USB Drives")
            .body(
                "This app is sandboxed and cannot list removable drives until it is given permission. \
                 Run this command in a terminal, then restart the app. Choosing a drive folder works without it.",
            )
            .extra_child(&extra)
            .default_response("close")
            .close_response("close")
            .build();
        dialog.add_response("close", "Close");
        dialog.present(Some(self));
    }

    #[allow(deprecated)]
    fn build_shortcuts_window(&self) {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<interface>
  <object class="GtkShortcutsWindow" id="shortcuts">
    <property name="modal">true</property>
    <child>
      <object class="GtkShortcutsSection">
        <property name="section-name">main</property>
        <child>
          <object class="GtkShortcutsGroup">
            <property name="title" translatable="yes">Files</property>
            <child>
              <object class="GtkShortcutsShortcut">
                <property name="title" translatable="yes">Choose update folder</property>
                <property name="accelerator">&lt;Control&gt;o</property>
              </object>
            </child>
            <child>
              <object class="GtkShortcutsShortcut">
                <property name="title" translatable="yes">Choose plates archive</property>
                <property name="accelerator">&lt;Control&gt;&lt;Shift&gt;o</property>
              </object>
            </child>
          </object>
        </child>
        <child>
          <object class="GtkShortcutsGroup">
            <property name="title" translatable="yes">Drives</property>
            <child>
              <object class="GtkShortcutsShortcut">
                <property name="title" translatable="yes">Choose drive folder</property>
                <property name="accelerator">&lt;Control&gt;&lt;Shift&gt;d</property>
              </object>
            </child>
            <child>
              <object class="GtkShortcutsShortcut">
                <property name="title" translatable="yes">Rescan drives</property>
                <property name="accelerator">&lt;Control&gt;r F5</property>
              </object>
            </child>
            <child>
              <object class="GtkShortcutsShortcut">
                <property name="title" translatable="yes">Select all ready drives</property>
                <property name="accelerator">&lt;Control&gt;a</property>
              </object>
            </child>
            <child>
              <object class="GtkShortcutsShortcut">
                <property name="title" translatable="yes">Deselect all drives</property>
                <property name="accelerator">&lt;Control&gt;&lt;Shift&gt;a</property>
              </object>
            </child>
            <child>
              <object class="GtkShortcutsShortcut">
                <property name="title" translatable="yes">Toggle focused drive card</property>
                <property name="accelerator">space Return</property>
              </object>
            </child>
            <child>
              <object class="GtkShortcutsShortcut">
                <property name="title" translatable="yes">Context menu on focused drive card</property>
                <property name="accelerator">Menu &lt;Shift&gt;F10</property>
              </object>
            </child>
          </object>
        </child>
        <child>
          <object class="GtkShortcutsGroup">
            <property name="title" translatable="yes">General</property>
            <child>
              <object class="GtkShortcutsShortcut">
                <property name="title" translatable="yes">Update (primary action)</property>
                <property name="accelerator">&lt;Control&gt;Return</property>
              </object>
            </child>
            <child>
              <object class="GtkShortcutsShortcut">
                <property name="title" translatable="yes">Cancel the run</property>
                <property name="accelerator">Escape</property>
              </object>
            </child>
            <child>
              <object class="GtkShortcutsShortcut">
                <property name="title" translatable="yes">Activity log</property>
                <property name="accelerator">&lt;Control&gt;l</property>
              </object>
            </child>
            <child>
              <object class="GtkShortcutsShortcut">
                <property name="title" translatable="yes">Copy log to clipboard</property>
                <property name="accelerator">&lt;Control&gt;&lt;Shift&gt;c</property>
              </object>
            </child>
            <child>
              <object class="GtkShortcutsShortcut">
                <property name="title" translatable="yes">Preferences</property>
                <property name="accelerator">&lt;Control&gt;comma</property>
              </object>
            </child>
            <child>
              <object class="GtkShortcutsShortcut">
                <property name="title" translatable="yes">Keyboard shortcuts</property>
                <property name="accelerator">&lt;Control&gt;question</property>
              </object>
            </child>
            <child>
              <object class="GtkShortcutsShortcut">
                <property name="title" translatable="yes">Main menu</property>
                <property name="accelerator">F10</property>
              </object>
            </child>
            <child>
              <object class="GtkShortcutsShortcut">
                <property name="title" translatable="yes">Close window</property>
                <property name="accelerator">&lt;Control&gt;w</property>
              </object>
            </child>
            <child>
              <object class="GtkShortcutsShortcut">
                <property name="title" translatable="yes">Quit</property>
                <property name="accelerator">&lt;Control&gt;q</property>
              </object>
            </child>
          </object>
        </child>
      </object>
    </child>
  </object>
</interface>"#;
        let builder = gtk::Builder::from_string(xml);
        if let Some(window) = builder.object::<gtk::ShortcutsWindow>("shortcuts") {
            self.set_help_overlay(Some(&window));
            self.imp().help_overlay.replace(Some(window));
        }
    }

    fn popup_main_menu(&self) {
        self.imp().menu_button.popup();
    }
}

// ---------------------------------------------------------------------------
// Small text helpers
// ---------------------------------------------------------------------------

fn drive_list_text(drives: &[Drive]) -> String {
    match drives.len() {
        0 => String::new(),
        1 => drives[0].name.clone(),
        2 => format!("{} and {}", drives[0].name, drives[1].name),
        n => format!("{n} drives"),
    }
}

fn estimate_seconds(bytes: u64) -> u64 {
    (bytes as f64 / ESTIMATED_BYTES_PER_SECOND).ceil() as u64
}

/// "about {duration}" vocabulary — used in the reason label and D1's body.
fn duration_text(seconds: u64) -> String {
    if seconds < 60 {
        format!("{} seconds", seconds.max(1))
    } else if seconds < 3600 {
        let minutes = (seconds as f64 / 60.0).round().max(1.0) as u64;
        if minutes == 1 {
            "1 minute".into()
        } else {
            format!("{minutes} minutes")
        }
    } else {
        let hours = seconds / 3600;
        let minutes = (seconds % 3600) / 60;
        format!("{hours} hour{} {minutes} minutes", if hours == 1 { "" } else { "s" })
    }
}

/// "{m} min {s} s" style used on the result page's per-drive rows.
fn duration_precise(seconds: u64) -> String {
    if seconds < 60 {
        format!("{seconds} s")
    } else if seconds < 3600 {
        format!("{} min {} s", seconds / 60, seconds % 60)
    } else {
        format!("{} h {} min", seconds / 3600, (seconds % 3600) / 60)
    }
}

// ---------------------------------------------------------------------------
// Card context menu, "Show in Files", archive preview (D6)
// ---------------------------------------------------------------------------

impl DynonWindow {
    fn show_card_menu(&self, toggle: &gtk::ToggleButton, key: &str, x: f64, y: f64) {
        let Some(drive) = self.imp().cards.borrow().iter().find(|c| c.drive.key() == key).map(|c| c.drive.clone()) else {
            return;
        };
        let menu = gio::Menu::new();
        let select_item = gio::MenuItem::new(Some(if toggle.is_active() { "Deselect" } else { "Select" }), None);
        select_item.set_action_and_target_value(Some("win.toggle-drive"), Some(&key.to_variant()));
        let core = gio::Menu::new();
        core.append_item(&select_item);
        let show_item = gio::MenuItem::new(Some("Show in Files"), None);
        show_item.set_action_and_target_value(Some("win.show-drive-in-files"), Some(&key.to_variant()));
        core.append_item(&show_item);
        menu.append_section(None, &core);

        if drive.kind == TargetKind::Folder {
            let extra = gio::Menu::new();
            let path = drive.path.to_string_lossy().to_string();
            let again = gio::MenuItem::new(Some("Choose Again…"), None);
            again.set_action_and_target_value(Some("win.choose-drive-again"), Some(&path.to_variant()));
            extra.append_item(&again);
            let remove = gio::MenuItem::new(Some("Remove"), None);
            remove.set_action_and_target_value(Some("win.remove-drive-target"), Some(&path.to_variant()));
            extra.append_item(&remove);
            menu.append_section(None, &extra);
        }

        let popover = gtk::PopoverMenu::from_model(Some(&menu));
        popover.set_parent(toggle);
        popover.set_pointing_to(Some(&gtk::gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
        popover.connect_closed(|p| p.unparent());
        popover.popup();
    }

    fn show_source_in_files(&self, kind: &str) {
        let path = {
            let sources = self.imp().sources.borrow();
            match kind {
                "aviation" => sources.aviation.clone(),
                "obstacle" => sources.obstacle.clone(),
                "plates" => sources.archive.as_ref().map(|a| a.path.clone()),
                _ => None,
            }
        };
        if let Some(path) = path {
            let file = gio::File::for_path(&path);
            gtk::FileLauncher::new(Some(&file)).open_containing_folder(Some(self), gio::Cancellable::NONE, |_| {});
        }
    }

    fn show_drive_in_files(&self, key: &str) {
        let Some(drive) = self.imp().cards.borrow().iter().find(|c| c.drive.key() == key).map(|c| c.drive.clone()) else {
            return;
        };
        let file = gio::File::for_path(&drive.path);
        gtk::FileLauncher::new(Some(&file)).launch(Some(self), gio::Cancellable::NONE, |_| {});
    }

    fn remove_drive_target(&self, path: &str) {
        let mut targets = self.manual_targets();
        let before = targets.len();
        targets.retain(|p| p != path);
        if targets.len() != before {
            self.save_manual_targets(&targets);
            let name = PathBuf::from(path).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let removed = path.to_string();
            self.toast_action(&format!("{name} removed"), "Undo", clone!(
                #[weak(rename_to = win)] self,
                move || {
                    let mut targets = win.manual_targets();
                    if !targets.contains(&removed) {
                        targets.push(removed.clone());
                    }
                    win.save_manual_targets(&targets);
                    win.refresh_drives();
                }
            ));
            self.refresh_drives();
        }
    }

    /// D6: preview of the resulting destination paths, with the wrapper switch.
    fn show_archive_preview(&self) {
        let Some(archive) = self.imp().sources.borrow().archive.clone() else { return };
        let strip_current = self.imp().sources.borrow().strip_wrapper;

        let group = adw::PreferencesGroup::builder()
            .description("Files will be written to ChartData/Plates on each drive.")
            .build();

        let wrapper_row = archive.wrapper.clone().map(|folder| {
            let row = adw::SwitchRow::builder()
                .title("Remove Extra Folder")
                .subtitle(format!(
                    "Everything in this archive sits inside “{folder}”. Remove it so plates land directly in ChartData/Plates."
                ))
                .active(strip_current)
                .build();
            group.add(&row);
            row
        });

        let list = gtk::ListBox::builder().css_classes(["boxed-list"]).selection_mode(gtk::SelectionMode::None).build();
        let footer = gtk::Label::builder().css_classes(["caption", "dimmed"]).xalign(0.0).build();

        let refill = clone!(
            #[weak] list,
            #[weak] footer,
            #[strong] archive,
            move |strip: bool| {
                while let Some(child) = list.first_child() {
                    list.remove(&child);
                }
                let members = archive.members_stripped(strip);
                for member in members.iter().take(20) {
                    let label = gtk::Label::builder()
                        .label(member.dest.to_string_lossy())
                        .xalign(0.0)
                        .css_classes(["monospace", "caption"])
                        .margin_top(4).margin_bottom(4).margin_start(8).margin_end(8)
                        .build();
                    list.append(&label);
                }
                let remaining = members.len().saturating_sub(20);
                footer.set_text(&format!(
                    "…and {} more · {} junk files skipped",
                    job::group(remaining as u64),
                    job::group(archive.junk_skipped as u64)
                ));
            }
        );
        refill(strip_current);
        if let Some(row) = &wrapper_row {
            row.connect_active_notify(clone!(
                #[weak(rename_to = win)] self,
                #[strong] refill,
                move |row| {
                    win.imp().sources.borrow_mut().strip_wrapper = row.is_active();
                    refill(row.is_active());
                }
            ));
        }

        let content = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(12).margin_top(12).margin_bottom(12).margin_start(12).margin_end(12).build();
        content.append(&group);
        content.append(&list);
        content.append(&footer);
        let scroller = gtk::ScrolledWindow::builder().child(&content).vexpand(true).build();

        let toolbar = adw::ToolbarView::new();
        toolbar.add_top_bar(&adw::HeaderBar::new());
        toolbar.set_content(Some(&scroller));

        let dialog = adw::Dialog::builder().title("Archive Contents").content_width(520).content_height(480).child(&toolbar).build();
        dialog.present(Some(self));
    }
}
