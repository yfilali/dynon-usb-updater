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
    File::create(&aviation).unwrap().write_all(&vec![7u8; 300_000]).unwrap();
    File::create(&obstacle).unwrap().write_all(&vec![9u8; 120_000]).unwrap();

    let zip_path = src.join("US-Plates-2608.zip");
    make_zip(&zip_path, &["ChartData/Plates/US/a.png", "ChartData/Plates/US/b.png", "ChartData/.DS_Store"]);

    // A fake drive carrying last cycle's data, plus a file we must not touch.
    let root = base.join("FAKEUSB");
    fs::create_dir_all(root.join("ChartData/Plates/US")).unwrap();
    File::create(root.join("ChartData/Plates/US/old.png")).unwrap().write_all(b"old").unwrap();
    File::create(root.join("airmate_av_data_us_2607_013712.dup")).unwrap().write_all(b"old").unwrap();
    File::create(root.join("CHARTS-013712.key")).unwrap().write_all(b"key").unwrap();
    File::create(root.join("pilot-notes.txt")).unwrap().write_all(b"keep me").unwrap();

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
        archive: Some(env.archive.clone()),
        strip_wrapper: false,
        verify: true,
        replace_old: true,
        cycle: scan::parse_cycle("US-Plates-2608.zip"),
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
    assert!(saw_no_return, "erasing plates must announce the point of no return");

    // New databases present, previous cycle gone, unrelated file untouched.
    assert!(env.root.join("airmate_av_data_us_2608_013712.dup").exists());
    assert!(!env.root.join("airmate_av_data_us_2607_013712.dup").exists());
    assert_eq!(fs::read_to_string(env.root.join("pilot-notes.txt")).unwrap(), "keep me");
    assert!(env.root.join("CHARTS-013712.key").exists());

    // Plates replaced, not merged: the stale file is gone.
    assert!(env.root.join("ChartData/Plates/US/a.png").exists());
    assert!(!env.root.join("ChartData/Plates/US/old.png").exists());
    // No .part files survive a successful run.
    assert!(fs::read_dir(&env.root).unwrap().flatten().all(|e| !e.file_name().to_string_lossy().ends_with(".part")));
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
    assert!(env.root.join("ChartData/Plates/US/old.png").exists(), "nothing may be erased");
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
    assert!(!saw_no_return, "a databases-only run never reaches the point of no return");
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
    assert!(env.root.join("ChartData/Plates/US/old.png").exists(), "nothing erased on a failed pre-flight");
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
