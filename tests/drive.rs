use dynon_usb_updater::drive;

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
