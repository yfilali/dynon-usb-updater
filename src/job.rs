//! The update engine: copy, verify, erase, extract — off the main thread,
//! cancellable, with every failure isolated to the drive it happened on.

use crate::drive::{measure_plates, Drive, TargetKind};
use crate::scan::{self, Archive, Cycle};
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::Arc;
use std::time::{Duration, Instant};

const CHUNK: usize = 1 << 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Info,
    Success,
    Warning,
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DriveState {
    Waiting,
    CopyingDatabases,
    CheckingCopies,
    ErasingPlates,
    ExtractingPlates,
    Finishing,
    Done,
    Failed,
    Skipped,
    Stopped,
}

#[derive(Debug, Clone)]
pub enum Event {
    /// Headline step and its detail line, for the running page.
    Step {
        step: String,
        detail: String,
    },
    /// Bytes written so far, against the planned total.
    Progress {
        done: u64,
        total: u64,
    },
    DriveState {
        drive: String,
        state: DriveState,
    },
    Log {
        severity: Severity,
        message: String,
    },
    /// The first plate has been deleted: stopping now leaves the drive broken.
    PointOfNoReturn,
    Finished(Vec<DriveOutcome>),
}

#[derive(Debug, Clone)]
pub struct DriveOutcome {
    pub name: String,
    pub path: PathBuf,
    pub kind: TargetKind,
    pub result: Outcome,
    pub elapsed: Duration,
    pub plates_written: usize,
    pub cycle: Option<Cycle>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Updated,
    /// A plain-language sentence, ready to show.
    Failed(String),
    /// Never started, because the run was stopped first.
    Skipped,
    /// Stopped after the plates folder was already erased.
    Interrupted,
}

pub struct Plan {
    pub drives: Vec<Drive>,
    pub aviation: Option<PathBuf>,
    pub obstacle: Option<PathBuf>,
    pub archive: Option<Archive>,
    pub strip_wrapper: bool,
    pub verify: bool,
    pub replace_old: bool,
    pub cycle: Option<Cycle>,
}

impl Plan {
    /// Bytes written to a single drive, which is what the space check needs.
    pub fn bytes_per_drive(&self) -> u64 {
        let dbs: u64 = [&self.aviation, &self.obstacle]
            .iter()
            .filter_map(|p| p.as_ref())
            .filter_map(|p| fs::metadata(p).ok())
            .map(|m| m.len())
            .sum();
        dbs + self.archive.as_ref().map(|a| a.total_bytes).unwrap_or(0)
    }

    fn total_work(&self) -> u64 {
        let per = self.bytes_per_drive();
        let verify_extra = if self.verify {
            [&self.aviation, &self.obstacle]
                .iter()
                .filter_map(|p| p.as_ref())
                .filter_map(|p| fs::metadata(p).ok())
                .map(|m| m.len())
                .sum::<u64>()
        } else {
            0
        };
        ((per + verify_extra) * self.drives.len() as u64).max(1)
    }
}

/// Cancellation shared with the UI. `past_no_return` tells the window whether
/// stopping still costs the user anything.
#[derive(Clone, Default)]
pub struct Cancel {
    flag: Arc<AtomicBool>,
    past_no_return: Arc<AtomicBool>,
}

impl Cancel {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn request(&self) {
        self.flag.store(true, Ordering::SeqCst);
    }
    pub fn is_cancelled(&self) -> bool {
        self.flag.load(Ordering::SeqCst)
    }
    pub fn past_point_of_no_return(&self) -> bool {
        self.past_no_return.load(Ordering::SeqCst)
    }
    fn mark_no_return(&self) {
        self.past_no_return.store(true, Ordering::SeqCst);
    }
}

struct Ctx {
    tx: Sender<Event>,
    cancel: Cancel,
    done: u64,
    total: u64,
}

impl Ctx {
    fn send(&self, event: Event) {
        let _ = self.tx.send(event);
    }
    fn log(&self, severity: Severity, message: impl Into<String>) {
        self.send(Event::Log {
            severity,
            message: message.into(),
        });
    }
    fn step(&self, step: impl Into<String>, detail: impl Into<String>) {
        self.send(Event::Step {
            step: step.into(),
            detail: detail.into(),
        });
    }
    fn bump(&mut self, bytes: u64) {
        self.done += bytes;
        self.send(Event::Progress {
            done: self.done,
            total: self.total,
        });
    }
    fn cancelled(&self) -> bool {
        self.cancel.is_cancelled()
    }
}

struct Stopped;

type Step<T> = Result<T, StepError>;

enum StepError {
    Stopped,
    Failed(String),
}

impl From<Stopped> for StepError {
    fn from(_: Stopped) -> Self {
        StepError::Stopped
    }
}

/// Run the plan. Blocks; call it on a worker thread.
pub fn run(plan: Plan, tx: Sender<Event>, cancel: Cancel) {
    let total = plan.total_work();
    let mut ctx = Ctx {
        tx,
        cancel,
        done: 0,
        total,
    };
    let mut outcomes = Vec::new();

    ctx.step(
        "Preparing…",
        format!("Checking space on {} drives", plan.drives.len()),
    );

    for drive in &plan.drives {
        if ctx.cancelled() {
            outcomes.push(DriveOutcome {
                name: drive.name.clone(),
                path: drive.path.clone(),
                kind: drive.kind,
                result: Outcome::Skipped,
                elapsed: Duration::ZERO,
                plates_written: 0,
                cycle: None,
            });
            ctx.send(Event::DriveState {
                drive: drive.name.clone(),
                state: DriveState::Skipped,
            });
            continue;
        }

        let started = Instant::now();
        let mut plates_written = 0usize;
        let mut erased = false;
        let result = update_drive(&plan, drive, &mut ctx, &mut plates_written, &mut erased);

        let (outcome, state) = match result {
            Ok(()) => {
                ctx.log(
                    Severity::Success,
                    format!(
                        "{}: {} plates written, Cycle {} installed",
                        drive.name,
                        plates_written,
                        plan.cycle.map(|c| c.label()).unwrap_or_default()
                    ),
                );
                (Outcome::Updated, DriveState::Done)
            }
            Err(StepError::Stopped) if erased => {
                ctx.log(
                    Severity::Error,
                    format!("{}: stopped — plates folder is incomplete", drive.name),
                );
                (Outcome::Interrupted, DriveState::Stopped)
            }
            Err(StepError::Stopped) => {
                ctx.log(
                    Severity::Warning,
                    format!("{}: stopped before anything was changed", drive.name),
                );
                (Outcome::Skipped, DriveState::Stopped)
            }
            Err(StepError::Failed(reason)) => {
                ctx.log(Severity::Error, format!("{}: {reason}", drive.name));
                (Outcome::Failed(reason), DriveState::Failed)
            }
        };

        ctx.send(Event::DriveState {
            drive: drive.name.clone(),
            state,
        });
        outcomes.push(DriveOutcome {
            name: drive.name.clone(),
            path: drive.path.clone(),
            kind: drive.kind,
            result: outcome,
            elapsed: started.elapsed(),
            plates_written,
            cycle: plan.cycle,
        });
    }

    ctx.send(Event::Finished(outcomes));
}

fn update_drive(
    plan: &Plan,
    drive: &Drive,
    ctx: &mut Ctx,
    plates_written: &mut usize,
    erased: &mut bool,
) -> Step<()> {
    ctx.send(Event::DriveState {
        drive: drive.name.clone(),
        state: DriveState::CopyingDatabases,
    });

    // Allocated once and reused for every file this drive touches — a plates
    // archive can hold 23,000+ tiny members, and a fresh `vec![0u8; CHUNK]`
    // per member (a 1 MiB allocation each time) was measured to make
    // extraction pathologically slow under sandboxed/containerized syscall
    // interception, where each mmap/munmap round-trip carries real overhead.
    let mut buffer = vec![0u8; CHUNK];

    let needed = plan.bytes_per_drive();
    let reclaim = if plan.archive.is_some() {
        measure_plates(&drive.plates_dir()).0
    } else {
        0
    };
    let (_, free, writable) = (drive.total, drive.free, drive.writable);
    if !writable {
        return Err(StepError::Failed(
            "the drive stopped accepting writes".into(),
        ));
    }
    if needed > free.saturating_add(reclaim) {
        return Err(StepError::Failed(format!(
            "ran out of space — needs {}, {} available",
            size(needed),
            size(free + reclaim)
        )));
    }
    ctx.log(
        Severity::Info,
        format!(
            "{}: {} free, {} needed, {} reclaimable — fits",
            drive.name,
            size(free),
            size(needed),
            size(reclaim)
        ),
    );

    // 1. databases
    for (kind, source) in [
        (scan::DupKind::Aviation, &plan.aviation),
        (scan::DupKind::Obstacle, &plan.obstacle),
    ] {
        let Some(source) = source else { continue };
        let file_name = source
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        let label = if kind == scan::DupKind::Aviation {
            "aviation"
        } else {
            "obstacle"
        };

        if plan.replace_old {
            remove_old_databases(drive, kind, &file_name, ctx);
        }

        ctx.step(
            format!("Copying {label} database to {}", drive.name),
            file_name.clone(),
        );
        let dest = drive.path.join(&file_name);
        copy_verified(source, &dest, ctx, &mut buffer).map_err(|e| match e {
            StepError::Stopped => StepError::Stopped,
            StepError::Failed(m) => StepError::Failed(m),
        })?;
        ctx.log(
            Severity::Success,
            format!("{}: copied {file_name}", drive.name),
        );

        if plan.verify {
            ctx.send(Event::DriveState {
                drive: drive.name.clone(),
                state: DriveState::CheckingCopies,
            });
            ctx.step(
                format!("Checking the copy on {}", drive.name),
                "Comparing checksums",
            );
            let want = digest(source, ctx, &mut buffer)?;
            let got = digest(&dest, ctx, &mut buffer)?;
            ctx.bump(fs::metadata(source).map(|m| m.len()).unwrap_or(0));
            if want != got {
                let _ = fs::remove_file(&dest);
                return Err(StepError::Failed(format!(
                    "the copy of {file_name} did not match the original. Nothing was left half-written"
                )));
            }
            ctx.log(
                Severity::Success,
                format!("{}: checksum matches", drive.name),
            );
        }
    }

    // 2. plates
    if let Some(archive) = &plan.archive {
        let members = archive.members_stripped(plan.strip_wrapper);
        let plates_dir = drive.plates_dir();

        ctx.send(Event::DriveState {
            drive: drive.name.clone(),
            state: DriveState::ErasingPlates,
        });
        ctx.step(
            format!("Erasing old plates on {}", drive.name),
            String::new(),
        );
        let (_, existing) = measure_plates(&plates_dir);
        if plates_dir.exists() {
            ctx.cancel.mark_no_return();
            *erased = true;
            ctx.send(Event::PointOfNoReturn);
            clear_dir(&plates_dir).map_err(|e| {
                StepError::Failed(format!("could not clear the plates folder: {e}"))
            })?;
            ctx.log(
                Severity::Warning,
                format!(
                    "{}: erased {existing} files from ChartData/Plates",
                    drive.name
                ),
            );
        }
        fs::create_dir_all(&plates_dir)
            .map_err(|e| StepError::Failed(format!("could not create the plates folder: {e}")))?;

        ctx.send(Event::DriveState {
            drive: drive.name.clone(),
            state: DriveState::ExtractingPlates,
        });
        let file = File::open(&archive.path).map_err(|_| {
            StepError::Failed("the plates archive was changed while the update was running".into())
        })?;
        let mut zip = zip::ZipArchive::new(file).map_err(|_| {
            StepError::Failed("the plates archive was changed while the update was running".into())
        })?;

        let total = members.len();
        let mut unreadable = 0usize;
        for (n, member) in members.iter().enumerate() {
            if ctx.cancelled() {
                return Err(StepError::Stopped);
            }
            if n % 64 == 0 {
                ctx.step(
                    format!("Extracting plates to {}", drive.name),
                    format!("{} of {} files", group(n as u64), group(total as u64)),
                );
            }
            let target = plates_dir.join(&member.dest);
            if let Some(parent) = target.parent() {
                let _ = fs::create_dir_all(parent);
            }
            match extract_one(&mut zip, member.index, &target, ctx, &mut buffer) {
                Ok(()) => *plates_written += 1,
                Err(StepError::Stopped) => return Err(StepError::Stopped),
                Err(StepError::Failed(_)) => {
                    unreadable += 1;
                    // Tolerate scattered bad members; give up if the archive is
                    // substantially unreadable.
                    if unreadable * 100 > total {
                        return Err(StepError::Failed(format!(
                            "{unreadable} plates could not be read from the archive"
                        )));
                    }
                }
            }
        }
        if unreadable > 0 {
            ctx.log(
                Severity::Warning,
                format!(
                    "{}: {unreadable} plates could not be read from the archive",
                    drive.name
                ),
            );
        }
    }

    ctx.send(Event::DriveState {
        drive: drive.name.clone(),
        state: DriveState::Finishing,
    });
    ctx.step(
        format!("Finishing writes to {}", drive.name),
        "Do not unplug the drive",
    );
    sync_dir(&drive.path);
    Ok(())
}

fn remove_old_databases(drive: &Drive, kind: scan::DupKind, keep: &str, ctx: &Ctx) {
    let Ok(entries) = fs::read_dir(&drive.path) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.to_ascii_lowercase().ends_with(".dup") || name == keep {
            continue;
        }
        if scan::classify(&name) != kind {
            continue;
        }
        if fs::remove_file(entry.path()).is_ok() {
            ctx.log(Severity::Info, format!("{}: removed {name}", drive.name));
        }
    }
}

/// Write through a `.part` file and rename, so an interrupted copy never
/// leaves a half-written file under the real name.
fn copy_verified(source: &Path, dest: &Path, ctx: &mut Ctx, buffer: &mut [u8]) -> Step<()> {
    let part = dest.with_extension("part");
    let mut input = File::open(source)
        .map_err(|e| StepError::Failed(format!("could not read {}: {e}", source.display())))?;
    let mut output = File::create(&part)
        .map_err(|e| StepError::Failed(format!("could not write to the drive: {e}")))?;
    loop {
        if ctx.cancelled() {
            let _ = fs::remove_file(&part);
            return Err(StepError::Stopped);
        }
        let read = input
            .read(buffer)
            .map_err(|e| StepError::Failed(format!("read error: {e}")))?;
        if read == 0 {
            break;
        }
        output
            .write_all(&buffer[..read])
            .map_err(|e| StepError::Failed(format!("the drive reported a write error: {e}")))?;
        ctx.bump(read as u64);
    }
    output.flush().ok();
    output
        .sync_all()
        .map_err(|e| StepError::Failed(format!("the drive reported a write error: {e}")))?;
    drop(output);
    fs::rename(&part, dest)
        .map_err(|e| StepError::Failed(format!("could not finish writing: {e}")))?;
    Ok(())
}

fn extract_one(
    zip: &mut zip::ZipArchive<File>,
    index: usize,
    target: &Path,
    ctx: &mut Ctx,
    buffer: &mut [u8],
) -> Step<()> {
    let mut entry = zip
        .by_index(index)
        .map_err(|e| StepError::Failed(format!("unreadable archive member: {e}")))?;
    let mut output = File::create(target)
        .map_err(|e| StepError::Failed(format!("the drive reported a write error: {e}")))?;
    loop {
        if ctx.cancelled() {
            return Err(StepError::Stopped);
        }
        let read = entry
            .read(buffer)
            .map_err(|e| StepError::Failed(format!("unreadable archive member: {e}")))?;
        if read == 0 {
            break;
        }
        output
            .write_all(&buffer[..read])
            .map_err(|e| StepError::Failed(format!("the drive reported a write error: {e}")))?;
        ctx.bump(read as u64);
    }
    Ok(())
}

fn digest(path: &Path, ctx: &Ctx, buffer: &mut [u8]) -> Step<[u8; 32]> {
    let mut file = File::open(path)
        .map_err(|e| StepError::Failed(format!("could not re-read {}: {e}", path.display())))?;
    let mut hasher = Sha256::new();
    loop {
        if ctx.cancelled() {
            return Err(StepError::Stopped);
        }
        let read = file
            .read(buffer)
            .map_err(|e| StepError::Failed(format!("read error: {e}")))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hasher.finalize().into())
}

fn clear_dir(dir: &Path) -> std::io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if entry.file_type()?.is_dir() {
            fs::remove_dir_all(&path)?;
        } else {
            fs::remove_file(&path)?;
        }
    }
    Ok(())
}

/// Flush directory metadata, which matters on FAT.
fn sync_dir(dir: &Path) {
    if let Ok(handle) = File::open(dir) {
        let _ = handle.sync_all();
    }
}

pub fn size(bytes: u64) -> String {
    gtk::glib::format_size(bytes).to_string()
}

pub fn group(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}
