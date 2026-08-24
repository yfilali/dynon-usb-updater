use dynon_usb_updater::scan::{self, DupKind};
use std::fs::{self, File};
use std::io::Write;

fn tmpdir(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("dynon-test-{name}"));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn cycle_parsing_ignores_digits_inside_longer_runs() {
    // The entitlement group 013712 must not yield cycle 3712.
    let c = scan::parse_cycle("airmate_av_data_us_2608_013712.dup").unwrap();
    assert_eq!(c.label(), "2608");
    assert_eq!(scan::parse_cycle("no_cycle_here.dup"), None);
    assert_eq!(scan::parse_cycle("us_2614_x.dup"), None); // 14 is not a cycle
}

#[test]
fn entitlement_ids_are_extracted_and_matched() {
    assert_eq!(
        scan::parse_entitlement("airmate_av_data_us_2608_013712.dup").as_deref(),
        Some("013712")
    );
    assert_eq!(
        scan::parse_key_entitlement("CHARTS-013712.key").as_deref(),
        Some("013712")
    );
    assert_eq!(scan::parse_key_entitlement("notes.txt"), None);
}

#[test]
fn classification_and_ranking() {
    assert_eq!(
        scan::classify("airmate_av_data_us_2608_1.dup"),
        DupKind::Aviation
    );
    assert_eq!(
        scan::classify("airmate_obstacle_data_us_2608_1.dup"),
        DupKind::Obstacle
    );
    assert_eq!(scan::classify("terrain.dup"), DupKind::Other);
    // Both providers' obstacle naming must be recognised.
    assert_eq!(scan::classify("ob_data_FAA_2604.dup"), DupKind::Obstacle);
    assert_eq!(scan::classify("av_data_FAA_2608.dup"), DupKind::Aviation);

    let dir = tmpdir("rank");
    for name in [
        "airmate_av_data_us_2607_013712.dup",
        "airmate_av_data_us_2608_013712.dup",
        "airmate_obstacle_data_us_2608_013712.dup",
    ] {
        File::create(dir.join(name))
            .unwrap()
            .write_all(b"x")
            .unwrap();
    }
    let files = scan::scan_dup_files(&dir, 3);
    assert_eq!(files.len(), 3);
    let av = scan::newest(&files, DupKind::Aviation).unwrap();
    assert_eq!(av.name(), "airmate_av_data_us_2608_013712.dup");
    assert_eq!(av.cycle.unwrap().label(), "2608");
}

fn write_zip(path: &std::path::Path, names: &[&str]) {
    let mut zip = zip::ZipWriter::new(File::create(path).unwrap());
    let opts: zip::write::FileOptions<()> = zip::write::FileOptions::default();
    for name in names {
        zip.start_file(*name, opts).unwrap();
        zip.write_all(b"payload").unwrap();
    }
    zip.finish().unwrap();
}

#[test]
fn archive_strips_chartdata_and_plates_but_keeps_data_folders() {
    let dir = tmpdir("zip-strip");
    let path = dir.join("US-Plates-2608.zip");
    write_zip(
        &path,
        &[
            "ChartData/Plates/US/a.png",
            "ChartData/Plates/US/b.png",
            "ChartData/.DS_Store",
        ],
    );
    let archive = scan::read_archive(&path).unwrap();
    assert_eq!(archive.members.len(), 2, "junk must not be counted");
    assert_eq!(archive.junk_skipped, 1);
    // The data folder US/ survives; only the wrappers are removed.
    assert_eq!(archive.members[0].dest.to_str().unwrap(), "US/a.png");
    assert_eq!(archive.wrapper.as_deref(), Some("US"));
    assert_eq!(archive.cycle.unwrap().label(), "2608");
}

#[test]
fn a_file_beside_the_data_folder_stops_the_strip() {
    // The real archive has one file directly in Plates/ next to US/.
    let dir = tmpdir("zip-mixed");
    let path = dir.join("plates.zip");
    write_zip(
        &path,
        &["ChartData/Plates/US/a.png", "ChartData/Plates/index.db"],
    );
    let archive = scan::read_archive(&path).unwrap();
    assert_eq!(archive.members.len(), 2);
    assert!(archive
        .members
        .iter()
        .any(|m| m.dest.to_str() == Some("US/a.png")));
    assert!(archive
        .members
        .iter()
        .any(|m| m.dest.to_str() == Some("index.db")));
    assert_eq!(
        archive.wrapper, None,
        "no single wrapper when a file sits beside it"
    );
}

#[test]
fn traversal_and_absolute_members_are_rejected() {
    let dir = tmpdir("zip-evil");
    let path = dir.join("evil.zip");
    write_zip(&path, &["../../etc/passwd"]);
    let err = scan::read_archive(&path).unwrap_err().to_string();
    assert!(err.contains("unsafe"), "got: {err}");
}

#[test]
fn an_archive_with_only_junk_is_an_error() {
    let dir = tmpdir("zip-junk");
    let path = dir.join("junk.zip");
    write_zip(&path, &["__MACOSX/._x", "Thumbs.db"]);
    assert!(scan::read_archive(&path).is_err());
}

/// Runs only on the machine that has the real cycle files.
#[test]
fn real_archive_matches_the_measured_numbers() {
    let home = std::env::var("HOME").unwrap_or_default();
    let path = std::path::Path::new(&home).join("Downloads/US-Plates-2608.zip");
    if !path.exists() {
        eprintln!("skipping: {} not present", path.display());
        return;
    }
    let archive = scan::read_archive(&path).unwrap();
    assert_eq!(archive.members.len(), 23_104);
    assert_eq!(archive.junk_skipped, 1);
    assert_eq!(
        archive.members[0]
            .dest
            .components()
            .next()
            .unwrap()
            .as_os_str(),
        "US"
    );
    assert_eq!(archive.cycle.unwrap().label(), "2608");
}

#[test]
fn plate_archives_are_ranked_and_decoys_ignored() {
    let dir = tmpdir("archives");
    for name in [
        "US-Plates-2608.zip",
        "US-Plates-2607.zip",
        "324-Jaunell-Road.zip",
        "notes.zip",
    ] {
        write_zip(&dir.join(name), &["x.png"]);
    }
    let cycle = scan::parse_cycle("airmate_av_data_us_2608_013712.dup");
    let ranked = scan::rank_plate_archives(&dir, cycle);
    let names: Vec<String> = ranked
        .iter()
        .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        names.first().map(String::as_str),
        Some("US-Plates-2608.zip")
    );
    assert!(names.contains(&"US-Plates-2607.zip".to_string()));
    // An unrelated zip in the same folder must never be offered.
    assert!(
        !names.iter().any(|n| n.contains("Jaunell")),
        "decoy ranked: {names:?}"
    );
    assert!(!names.iter().any(|n| n == "notes.zip"));
}

#[test]
fn the_real_downloads_folder_picks_the_right_archive() {
    let home = std::env::var("HOME").unwrap_or_default();
    let downloads = std::path::Path::new(&home).join("Downloads");
    if !downloads.join("US-Plates-2608.zip").exists() {
        eprintln!("skipping: real cycle files not present");
        return;
    }
    let dups = scan::scan_dup_files(&downloads, 1);
    let cycle = scan::newest(&dups, DupKind::Aviation).and_then(|d| d.cycle);
    let ranked = scan::rank_plate_archives(&downloads, cycle);
    assert_eq!(
        ranked
            .first()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned()),
        Some("US-Plates-2608.zip".to_string())
    );
}

fn write_duc(path: &std::path::Path, names: &[&str]) {
    let mut zip = zip::ZipWriter::new(File::create(path).unwrap());
    let opts: zip::write::FileOptions<()> = zip::write::FileOptions::default();
    for name in names {
        zip.start_file(*name, opts).unwrap();
        zip.write_all(b"payload").unwrap();
    }
    zip.finish().unwrap();
}

#[test]
fn a_database_package_reports_both_cycles_separately() {
    let dir = tmpdir("duc");
    let path = dir.join("FAA_av2608_ob2604.duc");
    write_duc(&path, &["av_data_FAA_2608.dup", "ob_data_FAA_2604.dup"]);

    let package = scan::read_package(&path).expect("should be recognised as databases");
    assert_eq!(package.aviation.unwrap().label(), "2608");
    assert_eq!(package.obstacle.unwrap().label(), "2604");
    // The two cycles differ in Dynon's own file, so one number will not do.
    assert_eq!(package.version(), "Aviation 2608 · Obstacles 2604");
}

#[test]
fn a_firmware_package_is_not_mistaken_for_databases() {
    let dir = tmpdir("duc-firmware");
    let path = dir.join("SkyView_16.4.4.duc");
    write_duc(&path, &["firmware.bin", "release_notes.txt"]);
    assert!(
        scan::read_package(&path).is_none(),
        ".duc is Dynon's generic container; only database packages may be offered"
    );
}

#[test]
fn packages_are_ranked_newest_first() {
    let dir = tmpdir("duc-rank");
    write_duc(
        &dir.join("FAA_av2607_ob2604.duc"),
        &["av_data_FAA_2607.dup", "ob_data_FAA_2604.dup"],
    );
    write_duc(
        &dir.join("FAA_av2608_ob2604.duc"),
        &["av_data_FAA_2608.dup", "ob_data_FAA_2604.dup"],
    );
    write_duc(&dir.join("SkyView_16.4.4.duc"), &["firmware.bin"]);

    let found = scan::scan_packages(&dir);
    assert_eq!(found.len(), 2, "the firmware package must be excluded");
    assert_eq!(found[0].aviation.unwrap().label(), "2608");
    assert_eq!(found[1].aviation.unwrap().label(), "2607");
}

/// Runs against a real Dynon package when one is available:
/// DYNON_TEST_DUC=/path/to/FAA_av2608_ob2604.duc cargo test
#[test]
fn the_real_dynon_package_parses() {
    let Ok(path) = std::env::var("DYNON_TEST_DUC") else {
        eprintln!("skipping: set DYNON_TEST_DUC to a real .duc to run this");
        return;
    };
    let package = scan::read_package(std::path::Path::new(&path)).expect("real package");
    assert!(package.aviation.is_some() && package.obstacle.is_some());
    eprintln!("{} -> {}", package.name(), package.version());
}
