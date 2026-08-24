use dynon_usb_updater::drive;
use std::fs::{self, File};
use std::io::Write;

fn tmpdir(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("dynon-drive-{name}"));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
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

/// A drive updated from Dynon's own combined `.duc` package (no root `.dup`
/// files at all) must still report its installed cycle correctly.
#[test]
fn installed_cycle_is_read_from_a_duc_package_when_no_dup_files_are_present() {
    let root = tmpdir("duc-cycle");
    fs::create_dir_all(root.join("ChartData")).unwrap();
    write_duc(
        &root.join("FAA_av2608_ob2604.duc"),
        &["av_data_FAA_2608.dup", "ob_data_FAA_2604.dup"],
    );

    let (score, cycle, _entitlement) = drive::recognise(&root, "DYNON");
    assert!(
        score >= 2,
        "a ChartData folder plus a .duc should recognise"
    );
    assert_eq!(
        cycle.map(|c| c.label()),
        Some("2608".to_string()),
        "the installed cycle must come from the package, not a root .dup"
    );
}

/// A firmware `.duc` must never be mistaken for a database cycle.
#[test]
fn a_firmware_duc_does_not_produce_a_bogus_installed_cycle() {
    let root = tmpdir("duc-firmware-cycle");
    fs::create_dir_all(root.join("ChartData")).unwrap();
    write_duc(&root.join("SkyView_16.4.4.duc"), &["firmware.bin"]);

    let (_, cycle, _) = drive::recognise(&root, "DYNON");
    assert_eq!(cycle, None, "firmware packages carry no database cycle");
}

#[test]
fn recognises_real_skyview_drives_when_present() {
    let drives = drive::enumerate();
    if drives.is_empty() {
        eprintln!("skipping: no removable drives mounted");
        return;
    }
    for d in &drives {
        eprintln!(
            "{:8} score={} recognised={} cycle={:?} key={:?} free={} writable={} uuid={:?}",
            d.name,
            d.score,
            d.recognised(),
            d.installed_cycle.map(|c| c.label()),
            d.entitlement,
            d.free,
            d.writable,
            d.uuid
        );
    }
    // Both real sticks carry ChartData, root .dup files and a CHARTS key.
    if let Some(d) = drives.iter().find(|d| d.name == "DYNON") {
        // The mount can be listed while its contents are unreadable — that is
        // exactly the case under scripts/no-drives.sh, where the drives are
        // masked on purpose. Nothing to assert about a drive we cannot read.
        let readable = std::fs::read_dir(&d.path)
            .map(|mut entries| entries.next().is_some())
            .unwrap_or(false);
        if !readable {
            eprintln!("skipping: {} is mounted but not readable here", d.name);
            return;
        }
        assert!(d.recognised(), "DYNON must be recognised");
        assert_eq!(d.entitlement.as_deref(), Some("013712"));
        assert!(d.writable);
        // Deliberately not asserting a specific cycle: the point of this app is
        // that the number changes, so pinning it fails the moment the drive is
        // legitimately updated — which is exactly what happened here.
        assert!(
            d.installed_cycle.is_some(),
            "a SkyView drive carries a cycle"
        );
    }
}

#[test]
fn sandbox_detection_reports_the_host_correctly() {
    let sandbox = drive::Sandbox::detect();
    eprintln!(
        "sandboxed={} grants_media={} roots_visible={} hardware={} -> {:?}",
        sandbox.sandboxed,
        sandbox.grants_media,
        sandbox.media_roots_visible,
        sandbox.hardware_present,
        sandbox.classify()
    );
    assert!(
        !sandbox.sandboxed,
        "the test suite does not run in a sandbox"
    );
    assert!(
        sandbox.hardware_present,
        "USB sticks are attached on this machine"
    );
}
