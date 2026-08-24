use dynon_usb_updater::{drive, job, scan};
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::mpsc;

fn fixture(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("dynon-job-{name}"));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn make_zip(path: &Path, names: &[&str]) {
    let mut zip = zip::ZipWriter::new(File::create(path).unwrap());
    let opts: zip::write::FileOptions<()> = zip::write::FileOptions::default();
    for name in names {
        zip.start_file(*name, opts).unwrap();
        zip.write_all(&vec![b'p'; 4096]).unwrap();
    }
    zip.finish().unwrap();
}

struct Env {
    plan_drive: drive::Drive,
    aviation: PathBuf,
    obstacle: PathBuf,
    archive: scan::Archive,
    root: PathBuf,
}

fn setup(name: &str) -> Env {
    let base = fixture(name);
    let src = base.join("downloads");
    fs::create_dir_all(&src).unwrap();
    let aviation = src.join("airmate_av_data_us_2608_013712.dup");
    let obstacle = src.join("airmate_obstacle_data_us_2608_013712.dup");
    File::create(&aviation)
        .unwrap()
        .write_all(&vec![7u8; 300_000])
        .unwrap();
    File::create(&obstacle)
        .unwrap()
        .write_all(&vec![9u8; 120_000])
        .unwrap();

    let zip_path = src.join("US-Plates-2608.zip");
    make_zip(
        &zip_path,
        &[
            "ChartData/Plates/US/a.png",
            "ChartData/Plates/US/b.png",
            "ChartData/.DS_Store",
        ],
    );

    // A fake drive carrying last cycle's data, plus a file we must not touch.
    let root = base.join("FAKEUSB");
    fs::create_dir_all(root.join("ChartData/Plates/US")).unwrap();
    File::create(root.join("ChartData/Plates/US/old.png"))
        .unwrap()
        .write_all(b"old")
        .unwrap();
    File::create(root.join("airmate_av_data_us_2607_013712.dup"))
        .unwrap()
        .write_all(b"old")
        .unwrap();
    File::create(root.join("CHARTS-013712.key"))
        .unwrap()
        .write_all(b"key")
        .unwrap();
    File::create(root.join("pilot-notes.txt"))
        .unwrap()
        .write_all(b"keep me")
        .unwrap();

    Env {
        plan_drive: drive::folder_target(&root),
        aviation,
        obstacle,
        archive: scan::read_archive(&zip_path).unwrap(),
        root,
    }
}

fn plan(env: &Env) -> job::Plan {
    job::Plan {
        drives: vec![env.plan_drive.clone()],
        aviation: Some(env.aviation.clone()),
        obstacle: Some(env.obstacle.clone()),
        package: None,
        archive: Some(env.archive.clone()),
        strip_wrapper: false,
        verify: true,
        replace_old: true,
        cycle: scan::parse_cycle("US-Plates-2608.zip"),
        full_rebuild: false,
    }
}

#[test]
fn a_full_run_replaces_data_and_leaves_everything_else_alone() {
    let env = setup("full");
    let (tx, rx) = mpsc::channel();
    job::run(plan(&env), tx, job::Cancel::new());

    let mut outcomes = Vec::new();
    let mut saw_no_return = false;
    for event in rx {
        match event {
            job::Event::PointOfNoReturn => saw_no_return = true,
            job::Event::Finished(o) => outcomes = o,
            _ => {}
        }
    }
    assert_eq!(outcomes.len(), 1);
    assert_eq!(outcomes[0].result, job::Outcome::Updated);
    assert_eq!(outcomes[0].plates_written, 2);
    assert!(
        saw_no_return,
        "erasing plates must announce the point of no return"
    );

    // New databases present, previous cycle gone, unrelated file untouched.
    assert!(env.root.join("airmate_av_data_us_2608_013712.dup").exists());
    assert!(!env.root.join("airmate_av_data_us_2607_013712.dup").exists());
    assert_eq!(
        fs::read_to_string(env.root.join("pilot-notes.txt")).unwrap(),
        "keep me"
    );
    assert!(env.root.join("CHARTS-013712.key").exists());

    // Plates replaced, not merged: the stale file is gone.
    assert!(env.root.join("ChartData/Plates/US/a.png").exists());
    assert!(!env.root.join("ChartData/Plates/US/old.png").exists());
    // No .part files survive a successful run.
    assert!(fs::read_dir(&env.root)
        .unwrap()
        .flatten()
        .all(|e| !e.file_name().to_string_lossy().ends_with(".part")));
}

#[test]
fn cancelling_before_the_erase_changes_nothing() {
    let env = setup("cancel-early");
    let cancel = job::Cancel::new();
    cancel.request(); // already cancelled: the run must not touch the drive
    let (tx, rx) = mpsc::channel();
    job::run(plan(&env), tx, cancel);

    let outcomes: Vec<_> = rx
        .iter()
        .filter_map(|e| match e {
            job::Event::Finished(o) => Some(o),
            _ => None,
        })
        .next()
        .unwrap();
    assert_eq!(outcomes[0].result, job::Outcome::Skipped);
    assert!(
        env.root.join("ChartData/Plates/US/old.png").exists(),
        "nothing may be erased"
    );
    assert!(env.root.join("airmate_av_data_us_2607_013712.dup").exists());
}

#[test]
fn a_databases_only_run_leaves_plates_untouched() {
    let env = setup("dbs-only");
    let mut p = plan(&env);
    p.archive = None;
    let (tx, rx) = mpsc::channel();
    job::run(p, tx, job::Cancel::new());

    let saw_no_return = rx.iter().any(|e| matches!(e, job::Event::PointOfNoReturn));
    assert!(
        !saw_no_return,
        "a databases-only run never reaches the point of no return"
    );
    assert!(env.root.join("ChartData/Plates/US/old.png").exists());
    assert!(env.root.join("airmate_av_data_us_2608_013712.dup").exists());
}

#[test]
fn a_drive_that_cannot_fit_fails_before_erasing() {
    let env = setup("no-space");
    let mut p = plan(&env);
    // Claim the drive is nearly full; the check must fire before the erase.
    p.drives[0].free = 1;
    p.drives[0].reclaimable = Some((0, 0));
    let (tx, rx) = mpsc::channel();
    job::run(p, tx, job::Cancel::new());

    let mut outcomes = Vec::new();
    let mut saw_no_return = false;
    for event in rx {
        match event {
            job::Event::PointOfNoReturn => saw_no_return = true,
            job::Event::Finished(o) => outcomes = o,
            _ => {}
        }
    }
    match &outcomes[0].result {
        job::Outcome::Failed(reason) => assert!(reason.contains("space"), "got: {reason}"),
        other => panic!("expected a space failure, got {other:?}"),
    }
    assert!(!saw_no_return);
    assert!(
        env.root.join("ChartData/Plates/US/old.png").exists(),
        "nothing erased on a failed pre-flight"
    );
}

#[test]
fn one_failing_drive_does_not_stop_the_others() {
    let env = setup("isolation");
    let mut p = plan(&env);
    let mut broken = p.drives[0].clone();
    broken.name = "BROKEN".into();
    broken.path = PathBuf::from("/nonexistent/never");
    broken.writable = false;
    p.drives.insert(0, broken);

    let (tx, rx) = mpsc::channel();
    job::run(p, tx, job::Cancel::new());
    let outcomes: Vec<_> = rx
        .iter()
        .filter_map(|e| match e {
            job::Event::Finished(o) => Some(o),
            _ => None,
        })
        .next()
        .unwrap();
    assert!(matches!(outcomes[0].result, job::Outcome::Failed(_)));
    assert_eq!(outcomes[1].result, job::Outcome::Updated);
}

#[test]
fn the_sync_rewrites_only_what_changed_and_deletes_what_is_gone() {
    let env = setup("sync");
    let plates = env.root.join("ChartData/Plates");

    // Pre-seed the drive so each case is represented:
    //  a.png identical to the archive, b.png present but different,
    //  old.png absent from the archive entirely.
    fs::create_dir_all(plates.join("US")).unwrap();
    {
        let mut zip = zip::ZipArchive::new(File::open(&env.archive.path).unwrap()).unwrap();
        for member in &env.archive.members {
            if member.dest.to_str() == Some("US/a.png") {
                let mut entry = zip.by_index(member.index).unwrap();
                let mut bytes = Vec::new();
                std::io::copy(&mut entry, &mut bytes).unwrap();
                fs::write(plates.join("US/a.png"), &bytes).unwrap();
            }
        }
    }
    fs::write(plates.join("US/b.png"), b"stale contents").unwrap();
    fs::write(plates.join("US/old.png"), b"no longer in the archive").unwrap();

    let identical_before = fs::metadata(plates.join("US/a.png"))
        .unwrap()
        .modified()
        .unwrap();
    std::thread::sleep(std::time::Duration::from_millis(1100)); // filesystem mtime resolution

    let (tx, rx) = mpsc::channel();
    job::run(plan(&env), tx, job::Cancel::new());
    let mut log = Vec::new();
    let mut outcomes = Vec::new();
    for event in rx {
        match event {
            job::Event::Log { message, .. } => log.push(message),
            job::Event::Finished(o) => outcomes = o,
            _ => {}
        }
    }
    assert_eq!(outcomes[0].result, job::Outcome::Updated);

    // The identical file must not have been rewritten.
    let identical_after = fs::metadata(plates.join("US/a.png"))
        .unwrap()
        .modified()
        .unwrap();
    assert_eq!(
        identical_before, identical_after,
        "an unchanged plate was rewritten"
    );

    // The differing file must have been replaced with the archive's version.
    assert_ne!(
        fs::read(plates.join("US/b.png")).unwrap(),
        b"stale contents"
    );
    // The plate missing from the archive must be gone.
    assert!(
        !plates.join("US/old.png").exists(),
        "a plate not in the archive survived"
    );

    let summary = log
        .iter()
        .find(|m| m.contains("unchanged"))
        .expect("no sync summary logged");
    assert!(summary.contains("1 plates unchanged"), "got: {summary}");
}

#[test]
fn a_databases_only_run_never_reaches_the_point_of_no_return() {
    let env = setup("no-return");
    let mut p = plan(&env);
    p.archive = None;
    let (tx, rx) = mpsc::channel();
    let cancel = job::Cancel::new();
    job::run(p, tx, cancel.clone());
    assert!(!rx.iter().any(|e| matches!(e, job::Event::PointOfNoReturn)));
    assert!(!cancel.past_point_of_no_return());
}

#[test]
fn drives_are_updated_concurrently_not_one_after_another() {
    let env = setup("parallel");
    let mut p = plan(&env);
    // Three copies of the same fixture drive, each a separate directory.
    for n in 0..2 {
        let extra = env.root.parent().unwrap().join(format!("EXTRA{n}"));
        fs::create_dir_all(extra.join("ChartData/Plates")).unwrap();
        let mut drive = drive::folder_target(&extra);
        drive.name = format!("EXTRA{n}");
        p.drives.push(drive);
    }
    let names: Vec<String> = p.drives.iter().map(|d| d.name.clone()).collect();

    let (tx, rx) = mpsc::channel();
    job::run(p, tx, job::Cancel::new());

    // Interleaved per-drive events prove the drives ran at the same time
    // rather than strictly in sequence.
    let mut order = Vec::new();
    let mut outcomes = Vec::new();
    for event in rx {
        match event {
            job::Event::DriveState { drive, .. } => {
                if order.last() != Some(&drive) {
                    order.push(drive);
                }
            }
            job::Event::Finished(o) => outcomes = o,
            _ => {}
        }
    }
    assert_eq!(outcomes.len(), 3, "every drive must report an outcome");
    assert!(outcomes.iter().all(|o| o.result == job::Outcome::Updated));
    // Outcomes come back in the order the drives were listed, whatever order
    // the threads finished in.
    assert_eq!(
        outcomes.iter().map(|o| o.name.clone()).collect::<Vec<_>>(),
        names
    );
    assert!(
        order.len() > outcomes.len(),
        "drive states never interleaved, so the drives ran sequentially: {order:?}"
    );
}

fn write_duc(path: &Path, names: &[&str]) {
    let mut zip = zip::ZipWriter::new(File::create(path).unwrap());
    let opts: zip::write::FileOptions<()> = zip::write::FileOptions::default();
    for name in names {
        zip.start_file(*name, opts).unwrap();
        zip.write_all(&vec![b'd'; 2048]).unwrap();
    }
    zip.finish().unwrap();
}

#[test]
fn a_duc_package_is_copied_whole_and_verified() {
    let base = fixture("duc-copy");
    let src = base.join("downloads");
    fs::create_dir_all(&src).unwrap();
    let package = src.join("FAA_av2608_ob2604.duc");
    write_duc(&package, &["av_data_FAA_2608.dup", "ob_data_FAA_2604.dup"]);

    let root = base.join("FAKEUSB");
    fs::create_dir_all(&root).unwrap();

    let plan = job::Plan {
        drives: vec![drive::folder_target(&root)],
        aviation: None,
        obstacle: None,
        package: Some(package.clone()),
        archive: None,
        strip_wrapper: false,
        verify: true,
        replace_old: true,
        cycle: scan::read_package(&package).unwrap().aviation,
        full_rebuild: false,
    };

    let (tx, rx) = mpsc::channel();
    job::run(plan, tx, job::Cancel::new());
    let outcomes: Vec<_> = rx
        .iter()
        .filter_map(|e| match e {
            job::Event::Finished(o) => Some(o),
            _ => None,
        })
        .next()
        .unwrap();
    assert_eq!(outcomes[0].result, job::Outcome::Updated);

    // The package landed on the drive as-is — not unpacked.
    let dest = root.join("FAA_av2608_ob2604.duc");
    assert!(dest.is_file());
    assert_eq!(fs::read(&dest).unwrap(), fs::read(&package).unwrap());
    assert!(
        scan::read_package(&dest).is_some(),
        "the copy must still parse as the same database package"
    );
    // No .part file survives a successful copy.
    assert!(fs::read_dir(&root)
        .unwrap()
        .flatten()
        .all(|e| !e.file_name().to_string_lossy().ends_with(".part")));
}

#[test]
fn replacing_older_packages_retires_only_the_same_family() {
    let base = fixture("duc-retire");
    let src = base.join("downloads");
    fs::create_dir_all(&src).unwrap();
    let package = src.join("FAA_av2608_ob2604.duc");
    write_duc(&package, &["av_data_FAA_2608.dup", "ob_data_FAA_2604.dup"]);

    let root = base.join("FAKEUSB");
    fs::create_dir_all(&root).unwrap();
    // An older database package of the same family: must be retired.
    write_duc(
        &root.join("FAA_av2607_ob2604.duc"),
        &["av_data_FAA_2607.dup", "ob_data_FAA_2604.dup"],
    );
    // A firmware package: must never be touched, even though it is a .duc.
    write_duc(&root.join("SkyView_16.4.4.duc"), &["firmware.bin"]);

    let plan = job::Plan {
        drives: vec![drive::folder_target(&root)],
        aviation: None,
        obstacle: None,
        package: Some(package.clone()),
        archive: None,
        strip_wrapper: false,
        verify: false,
        replace_old: true,
        cycle: scan::read_package(&package).unwrap().aviation,
        full_rebuild: false,
    };

    let (tx, rx) = mpsc::channel();
    job::run(plan, tx, job::Cancel::new());
    let outcomes: Vec<_> = rx
        .iter()
        .filter_map(|e| match e {
            job::Event::Finished(o) => Some(o),
            _ => None,
        })
        .next()
        .unwrap();
    assert_eq!(outcomes[0].result, job::Outcome::Updated);

    assert!(root.join("FAA_av2608_ob2604.duc").exists());
    assert!(
        !root.join("FAA_av2607_ob2604.duc").exists(),
        "the superseded database package must be retired"
    );
    assert!(
        root.join("SkyView_16.4.4.duc").exists(),
        "an unrelated .duc (firmware) must never be deleted"
    );
}

/// Not run by default: builds a 20,000-plate archive to show what the sync
/// saves on a realistic cycle. Run with `cargo test --release -- --ignored --nocapture`.
#[test]
#[ignore]
fn sync_scale_benchmark() {
    let base = fixture("scale-bench");
    let src = base.join("dl");
    fs::create_dir_all(&src).unwrap();
    let aviation = src.join("airmate_av_data_us_2608_013712.dup");
    let obstacle = src.join("airmate_obstacle_data_us_2608_013712.dup");
    File::create(&aviation)
        .unwrap()
        .write_all(&vec![7u8; 8_600_000])
        .unwrap();
    File::create(&obstacle)
        .unwrap()
        .write_all(&vec![9u8; 2_000_000])
        .unwrap();

    let zip_path = src.join("US-Plates-2608.zip");
    {
        let mut zip = zip::ZipWriter::new(File::create(&zip_path).unwrap());
        let opts: zip::write::FileOptions<()> =
            zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Stored);
        let payload = vec![0xABu8; 16 * 1024];
        for i in 0..20_000 {
            zip.start_file(format!("ChartData/Plates/US/P{i:06}.png"), opts)
                .unwrap();
            zip.write_all(&payload).unwrap();
        }
        zip.finish().unwrap();
    }
    let archive = scan::read_archive(&zip_path).unwrap();

    let root = base.join("DRIVE");
    fs::create_dir_all(root.join("ChartData/Plates")).unwrap();
    let make_plan = |archive: &scan::Archive| job::Plan {
        drives: vec![drive::folder_target(&root)],
        aviation: Some(aviation.clone()),
        obstacle: Some(obstacle.clone()),
        package: None,
        archive: Some(archive.clone()),
        strip_wrapper: false,
        verify: true,
        replace_old: true,
        cycle: scan::parse_cycle("US-Plates-2608.zip"),
        full_rebuild: false,
    };

    let run_once = |label: &str, plan: job::Plan| -> std::time::Duration {
        let started = std::time::Instant::now();
        let (tx, rx) = mpsc::channel();
        job::run(plan, tx, job::Cancel::new());
        let mut summary = String::new();
        for event in rx {
            if let job::Event::Log { message, .. } = event {
                if message.contains("unchanged") {
                    summary = message;
                }
            }
        }
        let elapsed = started.elapsed();
        println!("{label}: {:?}  {summary}", elapsed);
        elapsed
    };

    let first = run_once("first install (nothing on the drive)", make_plan(&archive));
    let second = run_once("re-run, same cycle already installed", make_plan(&archive));
    println!(
        "re-run took {:.0}% of the first install",
        second.as_secs_f64() / first.as_secs_f64() * 100.0
    );
    assert!(
        second < first,
        "a re-run must be cheaper than the first install"
    );
}
