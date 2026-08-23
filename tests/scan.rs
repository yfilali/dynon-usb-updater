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
    assert_eq!(scan::classify("airmate_av_data_us_2608_1.dup"), DupKind::Aviation);
    assert_eq!(scan::classify("airmate_obstacle_data_us_2608_1.dup"), DupKind::Obstacle);
    assert_eq!(scan::classify("terrain.dup"), DupKind::Other);

    let dir = tmpdir("rank");
    for name in [
        "airmate_av_data_us_2607_013712.dup",
        "airmate_av_data_us_2608_013712.dup",
        "airmate_obstacle_data_us_2608_013712.dup",
    ] {
        File::create(dir.join(name)).unwrap().write_all(b"x").unwrap();
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
        &["ChartData/Plates/US/a.png", "ChartData/Plates/US/b.png", "ChartData/.DS_Store"],
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
    write_zip(&path, &["ChartData/Plates/US/a.png", "ChartData/Plates/index.db"]);
    let archive = scan::read_archive(&path).unwrap();
    assert_eq!(archive.members.len(), 2);
    assert!(archive.members.iter().any(|m| m.dest.to_str() == Some("US/a.png")));
    assert!(archive.members.iter().any(|m| m.dest.to_str() == Some("index.db")));
    assert_eq!(archive.wrapper, None, "no single wrapper when a file sits beside it");
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
    assert_eq!(archive.members[0].dest.components().next().unwrap().as_os_str(), "US");
    assert_eq!(archive.cycle.unwrap().label(), "2608");
}
