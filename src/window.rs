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
struct Sources {
    folder: Option<PathBuf>,
    dups: Vec<DupFile>,
    aviation: Option<PathBuf>,
    obstacle: Option<PathBuf>,
    skip_aviation: bool,
    skip_obstacle: bool,
    archive: Option<Archive>,
    archive_error: Option<String>,
    strip_wrapper: bool,
}

impl Sources {
    fn cycle(&self) -> Option<Cycle> {
        let from = |p: &Option<PathBuf>| {
            p.as_ref()
                .and_then(|p| p.file_name())
                .and_then(|n| scan::parse_cycle(&n.to_string_lossy()))
        };
        from(&self.aviation)
            .or_else(|| from(&self.obstacle))
            .or_else(|| self.archive.as_ref().and_then(|a| a.cycle))
    }

    fn anything_to_copy(&self) -> bool {
        self.aviation.is_some() || self.obstacle.is_some() || self.archive.is_some()
    }
}

struct Card {
    drive: Drive,
    toggle: gtk::ToggleButton,
    cycle_label: gtk::Label,
    verdict_label: gtk::Label,
    level: gtk::LevelBar,
}

/// A running job's live state, kept so the UI can compute ETA and guard exits.
struct Run {
    rx: Receiver<Event>,
    cancel: job::Cancel,
    started: Instant,
    samples: Vec<(Instant, u64)>,
    last_eta: Option<Duration>,
    rows: Vec<(String, adw::ActionRow, gtk::Label)>,
    inhibit: u32,
    log: Vec<(Severity, String, String)>,
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

        pub sources: RefCell<Sources>,
        pub cards: RefCell<Vec<Card>>,
        pub run: RefCell<Option<Run>>,
        pub settings: RefCell<Option<gio::Settings>>,
        pub log_list: RefCell<Option<gtk::ListBox>>,
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
        imp.plates_row.set_subtitle(&format!(
            "Reading {}…",
            path.file_name().unwrap_or_default().to_string_lossy()
        ));

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
                            match result {
                                Ok(archive) => {
                                    sources.strip_wrapper = false;
                                    sources.archive = Some(archive);
                                    sources.archive_error = None;
                                }
                                Err(message) => {
                                    sources.archive = None;
                                    sources.archive_error = Some(message);
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
    menu
}

fn plates_menu(has_archive: bool) -> gio::Menu {
    let menu = gio::Menu::new();
    if has_archive {
        menu.append(Some("Choose a Different Archive…"), Some("win.choose-archive"));
        menu.append(Some("Do Not Replace Plates"), Some("win.clear-archive"));
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
            let _ = settings.set_strv("manual-targets", &refs);
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

        // Measuring an existing plates folder walks tens of thousands of files;
        // do it once per refresh, off the main loop, and fill the cards in when
        // it lands.
        for drive in &mut drives {
            if drive.reachable {
                drive.reclaimable = Some(drive::measure_plates(&drive.plates_dir()));
            }
        }

        self.rebuild_cards(drives, &previously_checked);
        self.update_ready();
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
                (ready && !up_to_date) || (remembered.contains(&key) && ready)
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
