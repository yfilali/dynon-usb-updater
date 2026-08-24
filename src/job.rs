//! The update engine: copy, verify, erase, extract — off the main thread,
//! cancellable, with every failure isolated to the drive it happened on.

use crate::drive::{measure_plates, Drive, TargetKind};
use crate::scan::{self, Archive, Cycle};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::Sender;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::{Duration, Instant};

const CHUNK: usize = 1 << 20;
/// What one deletion is "worth" in progress units. Deleting a file costs a
/// directory write, not its size, so charging it as bytes would make the bar
/// leap; this keeps it moving honestly during a long delete pass.
const DELETE_UNIT: u64 = 48 * 1024;

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
    /// Comparing the plates already on the drive against the archive.
    ComparingPlates,
    /// Removing plates the new cycle no longer contains.
    ErasingPlates,
    /// Writing plates that are new or have changed.
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
    /// Per-drive detail line ("8,421 of 23,104 files"). With drives running
    /// concurrently there is no single global step, so each drive reports its
    /// own and the window composes the headline.
    DriveDetail {
        drive: String,
        detail: String,
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
    /// Rewrite every plate instead of syncing only what changed. Off by
    /// default; useful when a drive is suspected of holding corrupt files.
    pub full_rebuild: bool,
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

    /// A first estimate of the work ahead, in arbitrary units that happen to be
    /// bytes. It is deliberately pessimistic — it assumes every plate has to be
    /// rewritten — and each drive corrects its own share downwards once it has
    /// compared what is already there.
    fn estimated_work(&self) -> u64 {
        let dbs: u64 = [&self.aviation, &self.obstacle]
            .iter()
            .filter_map(|p| p.as_ref())
            .filter_map(|p| fs::metadata(p).ok())
            .map(|m| m.len())
            .sum();
        let verify = if self.verify { dbs } else { 0 };
        let archive = self.archive.as_ref().map(|a| a.total_bytes).unwrap_or(0);
        let per_drive: u64 = self
            .drives
            .iter()
            .map(|d| {
                // Comparing means reading what is on the drive already.
                let compare = if self.archive.is_some() && !self.full_rebuild {
                    d.reclaimable.map(|(bytes, _)| bytes).unwrap_or(0)
                } else {
                    0
                };
                dbs + verify + compare + archive
            })
            .sum();
        per_drive.max(1)
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

/// Progress shared by every drive thread. `total` is an estimate that firms up
/// once each drive has compared its plates, so it is adjustable rather than fixed.
#[derive(Default)]
struct Shared {
    done: AtomicU64,
    total: AtomicU64,
}

impl Shared {
    fn adjust_total(&self, delta: i64) {
        let current = self.total.load(Ordering::Relaxed) as i64;
        self.total
            .store((current + delta).max(1) as u64, Ordering::Relaxed);
    }
}

struct Ctx {
    tx: Sender<Event>,
    cancel: Cancel,
    shared: Arc<Shared>,
    drive: String,
    last_sent: Instant,
}

impl Ctx {
    fn send(&self, event: Event) {
        let _ = self.tx.send(event);
    }

    fn detail(&self, detail: impl Into<String>) {
        self.send(Event::DriveDetail {
            drive: self.drive.clone(),
            detail: detail.into(),
        });
    }

    fn state(&self, state: DriveState) {
        self.send(Event::DriveState {
            drive: self.drive.clone(),
            state,
        });
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
    fn bump(&mut self, units: u64) {
        let done = self.shared.done.fetch_add(units, Ordering::Relaxed) + units;
        if self.last_sent.elapsed() >= Duration::from_millis(100) {
            self.last_sent = Instant::now();
            self.send(Event::Progress {
                done,
                total: self.shared.total.load(Ordering::Relaxed),
            });
        }
    }

    fn flush_progress(&mut self) {
        self.last_sent = Instant::now() - Duration::from_secs(1);
        self.bump(0);
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

/// Run the plan. Blocks until every drive has finished; call it on a worker
/// thread. Drives are updated concurrently — they are independent devices, and
/// doing them one at a time doubled the wall time for no benefit.
pub fn run(plan: Plan, tx: Sender<Event>, cancel: Cancel) {
    let shared = Arc::new(Shared::default());
    shared.total.store(plan.estimated_work(), Ordering::Relaxed);

    let _ = tx.send(Event::Step {
        step: "Preparing…".into(),
        detail: format!("Checking {} drives", plan.drives.len()),
    });

    let plan = Arc::new(plan);
    let outcomes: Arc<Mutex<Vec<(usize, DriveOutcome)>>> = Arc::new(Mutex::new(Vec::new()));
    let mut handles = Vec::new();

    for (index, drive) in plan.drives.iter().cloned().enumerate() {
        let (tx, cancel, shared, plan, outcomes) = (
            tx.clone(),
            cancel.clone(),
            Arc::clone(&shared),
            Arc::clone(&plan),
            Arc::clone(&outcomes),
        );
        handles.push(std::thread::spawn(move || {
            let mut ctx = Ctx {
                tx,
                cancel,
                shared,
                drive: drive.name.clone(),
                last_sent: Instant::now() - Duration::from_secs(1),
            };
            let outcome = run_one_drive(&plan, &drive, &mut ctx);
            if let Ok(mut list) = outcomes.lock() {
                list.push((index, outcome));
            }
        }));
    }

    for handle in handles {
        let _ = handle.join();
    }

    let mut collected = outcomes.lock().map(|l| l.clone()).unwrap_or_default();
    collected.sort_by_key(|(index, _)| *index);
    let _ = tx.send(Event::Finished(
        collected.into_iter().map(|(_, outcome)| outcome).collect(),
    ));
}

fn run_one_drive(plan: &Plan, drive: &Drive, ctx: &mut Ctx) -> DriveOutcome {
    if ctx.cancelled() {
        ctx.state(DriveState::Skipped);
        return DriveOutcome {
            name: drive.name.clone(),
            path: drive.path.clone(),
            kind: drive.kind,
            result: Outcome::Skipped,
            elapsed: Duration::ZERO,
            plates_written: 0,
            cycle: None,
        };
    }

    let started = Instant::now();
    let mut plates_written = 0usize;
    let mut erased = false;
    let result = update_drive(plan, drive, ctx, &mut plates_written, &mut erased);

    let (outcome, state) = match result {
        Ok(()) => {
            ctx.log(
                Severity::Success,
                format!(
                    "{}: {} plates written, Cycle {} installed",
                    drive.name,
                    group(plates_written as u64),
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

    ctx.state(state);
    ctx.flush_progress();
    DriveOutcome {
        name: drive.name.clone(),
        path: drive.path.clone(),
        kind: drive.kind,
        result: outcome,
        elapsed: started.elapsed(),
        plates_written,
        cycle: plan.cycle,
    }
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

        ctx.detail(format!("Copying the {label} database"));
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
            ctx.detail("Comparing checksums");
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

    // 2. plates — a sync, not a wipe.
    //
    // The old approach erased ChartData/Plates and unpacked the whole archive.
    // On a real cycle that is 27,000 deletes and 23,000 writes of which the
    // overwhelming majority are byte-identical to what was already there. It
    // also meant that stopping part-way left the drive with no plates at all.
    //
    // Instead: compare what is on the drive against the archive's central
    // directory, then write only what is new or changed and delete only what
    // the new cycle no longer contains. Size is checked first (free, from
    // metadata) and CRC-32 second (the archive already stores one per entry,
    // so the only cost is reading the file that is already on the drive —
    // and reading is far cheaper than writing on these devices).
    if let Some(archive) = &plan.archive {
        let members = archive.members_stripped(plan.strip_wrapper);
        let plates_dir = drive.plates_dir();
        let mut buffer = vec![0u8; CHUNK];

        let wanted: HashMap<PathBuf, (usize, u64, u32)> = members
            .iter()
            .map(|m| (m.dest.clone(), (m.index, m.size, m.crc32)))
            .collect();

        let mut to_write: Vec<&crate::scan::Member> = Vec::new();
        let mut to_delete: Vec<PathBuf> = Vec::new();
        let mut unchanged = 0usize;

        if plan.full_rebuild {
            to_write.extend(members.iter());
            collect_files(&plates_dir, &plates_dir, &mut to_delete);
        } else {
            ctx.state(DriveState::ComparingPlates);
            ctx.detail("Checking plates already on the drive");

            let mut existing = Vec::new();
            collect_files(&plates_dir, &plates_dir, &mut existing);
            let existing_total = existing.len();
            let mut up_to_date: HashMap<PathBuf, bool> = HashMap::new();

            for (n, relative) in existing.iter().enumerate() {
                if ctx.cancelled() {
                    return Err(StepError::Stopped);
                }
                if n % 256 == 0 {
                    ctx.detail(format!(
                        "Checked {} of {} plates",
                        group(n as u64),
                        group(existing_total as u64)
                    ));
                }
                let full = plates_dir.join(relative);
                match wanted.get(relative) {
                    // The new cycle does not have this plate at all.
                    None => to_delete.push(relative.clone()),
                    Some(&(_, size, crc)) => {
                        let same_size = fs::metadata(&full)
                            .map(|m| m.len() == size)
                            .unwrap_or(false);
                        let identical =
                            same_size && file_crc32(&full, ctx, &mut buffer) == Some(crc);
                        up_to_date.insert(relative.clone(), identical);
                        if identical {
                            unchanged += 1;
                        }
                    }
                }
            }

            // Write anything the drive does not already have byte-for-byte.
            to_write.extend(
                members
                    .iter()
                    .filter(|m| !up_to_date.get(&m.dest).copied().unwrap_or(false)),
            );
        }

        // Now that the real work is known, correct this drive's share of the
        // estimate: it assumed every plate would be rewritten.
        let write_bytes: u64 = to_write.iter().map(|m| m.size).sum();
        let delete_units = to_delete.len() as u64 * DELETE_UNIT;
        ctx.shared
            .adjust_total((write_bytes + delete_units) as i64 - archive.total_bytes as i64);
        ctx.log(
            Severity::Info,
            format!(
                "{}: {} plates unchanged, {} to write, {} to remove",
                drive.name,
                group(unchanged as u64),
                group(to_write.len() as u64),
                group(to_delete.len() as u64)
            ),
        );

        if !to_delete.is_empty() {
            ctx.state(DriveState::ErasingPlates);
            let total = to_delete.len();
            for (n, relative) in to_delete.iter().enumerate() {
                if ctx.cancelled() {
                    return Err(StepError::Stopped);
                }
                if n == 0 {
                    ctx.cancel.mark_no_return();
                    *erased = true;
                    ctx.send(Event::PointOfNoReturn);
                }
                let _ = fs::remove_file(plates_dir.join(relative));
                ctx.bump(DELETE_UNIT);
                if n % 128 == 0 {
                    ctx.detail(format!(
                        "Removed {} of {} old plates",
                        group(n as u64),
                        group(total as u64)
                    ));
                }
            }
            ctx.log(
                Severity::Warning,
                format!(
                    "{}: removed {} plates the new cycle does not contain",
                    drive.name,
                    group(total as u64)
                ),
            );
        }

        if !to_write.is_empty() {
            ctx.state(DriveState::ExtractingPlates);
            *erased = true;
            let file = File::open(&archive.path).map_err(|_| {
                StepError::Failed(
                    "the plates archive was changed while the update was running".into(),
                )
            })?;
            let mut zip = zip::ZipArchive::new(file).map_err(|_| {
                StepError::Failed(
                    "the plates archive was changed while the update was running".into(),
                )
            })?;

            let total = to_write.len();
            let mut unreadable = 0usize;
            for (n, member) in to_write.iter().enumerate() {
                if ctx.cancelled() {
                    return Err(StepError::Stopped);
                }
                if n % 64 == 0 {
                    ctx.detail(format!(
                        "Wrote {} of {} plates",
                        group(n as u64),
                        group(total as u64)
                    ));
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

        prune_empty_dirs(&plates_dir);
        *plates_written += unchanged;
    }

    ctx.send(Event::DriveState {
        drive: drive.name.clone(),
        state: DriveState::Finishing,
    });
    ctx.detail("Finishing — do not unplug the drive");
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

/// Every file below `dir`, as paths relative to `root`.
fn collect_files(root: &Path, dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        match entry.file_type() {
            Ok(t) if t.is_dir() => collect_files(root, &path, out),
            Ok(t) if t.is_file() => {
                if let Ok(relative) = path.strip_prefix(root) {
                    out.push(relative.to_path_buf());
                }
            }
            _ => {}
        }
    }
}

/// CRC-32 of a file already on the drive, to compare against the archive's.
/// Counts toward progress: on a USB stick this read is real work.
fn file_crc32(path: &Path, ctx: &mut Ctx, buffer: &mut [u8]) -> Option<u32> {
    let mut file = File::open(path).ok()?;
    let mut hasher = crc32fast::Hasher::new();
    loop {
        let read = file.read(buffer).ok()?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
        ctx.bump(read as u64);
    }
    Some(hasher.finalize())
}

/// Remove directories the sync emptied, deepest first.
fn prune_empty_dirs(root: &Path) {
    let mut dirs = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                stack.push(entry.path());
                dirs.push(entry.path());
            }
        }
    }
    dirs.sort_by_key(|d| std::cmp::Reverse(d.components().count()));
    for dir in dirs {
        let _ = fs::remove_dir(&dir); // fails harmlessly when not empty
    }
}

#[allow(dead_code)]
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
