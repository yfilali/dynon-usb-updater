//! Source scanning: AIRAC cycle parsing, database discovery, archive inspection.
//!
//! Dynon names its files by cycle — `airmate_av_data_us_2608_013712.dup` is
//! cycle 2608 for chart entitlement 013712 — so a cycle, not a date, is the
//! primary version key.

use anyhow::{bail, Context, Result};
use regex::Regex;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::SystemTime;

/// An AIRAC-style cycle: two-digit year plus a cycle number 01–13.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Cycle {
    pub year: u16,
    pub number: u8,
}

impl Cycle {
    pub fn label(&self) -> String {
        format!("{:02}{:02}", self.year % 100, self.number)
    }
}

impl std::fmt::Display for Cycle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.label())
    }
}

fn cycle_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    // The digit-boundary guards stop `2608` matching inside a longer run such
    // as the entitlement group `013712`.
    RE.get_or_init(|| Regex::new(r"(?:^|[^0-9])([0-9]{2})(0[1-9]|1[0-3])(?:[^0-9]|$)").unwrap())
}

fn entitlement_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"_([0-9]{5,})\.dup$").unwrap())
}

fn key_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^CHARTS-([0-9]+)\.key$").unwrap())
}

pub fn parse_cycle(name: &str) -> Option<Cycle> {
    let caps = cycle_re().captures(name)?;
    Some(Cycle {
        year: 2000 + caps[1].parse::<u16>().ok()?,
        number: caps[2].parse::<u8>().ok()?,
    })
}

/// The chart entitlement id trailing a database filename.
pub fn parse_entitlement(name: &str) -> Option<String> {
    Some(entitlement_re().captures(name)?[1].to_string())
}

/// The entitlement id carried by a `CHARTS-nnnnnn.key` file name.
pub fn parse_key_entitlement(name: &str) -> Option<String> {
    Some(key_re().captures(name)?[1].to_string())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DupKind {
    Aviation,
    Obstacle,
    Other,
}

pub fn classify(name: &str) -> DupKind {
    let lower = name.to_ascii_lowercase();
    // Providers differ: Dynon ships `ob_data_FAA_2604.dup`, Airmate ships
    // `airmate_obstacle_data_us_2608_...dup`. Matching only "obst" misses
    // Dynon's entirely.
    if lower.contains("obst") || lower.contains("ob_data") || lower.starts_with("ob_") {
        DupKind::Obstacle
    } else if lower.contains("av_data")
        || lower.contains("avdata")
        || lower.contains("av-data")
        || lower.contains("aviation")
        || lower.contains("navdata")
    {
        DupKind::Aviation
    } else {
        DupKind::Other
    }
}

#[derive(Debug, Clone)]
pub struct DupFile {
    pub path: PathBuf,
    pub kind: DupKind,
    pub cycle: Option<Cycle>,
    pub entitlement: Option<String>,
    pub size: u64,
    pub modified: SystemTime,
}

impl DupFile {
    pub fn name(&self) -> String {
        self.path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    }

    /// Ranking key: a parsed cycle beats no cycle; ties fall back to mtime.
    fn rank(&self) -> (u8, u32, SystemTime) {
        match self.cycle {
            Some(c) => (1, (c.year as u32) * 100 + c.number as u32, self.modified),
            None => (0, 0, self.modified),
        }
    }
}

/// Find `.dup` files up to `max_depth` levels below `dir`, newest first.
pub fn scan_dup_files(dir: &Path, max_depth: usize) -> Vec<DupFile> {
    let mut found = Vec::new();
    collect_dups(dir, max_depth, &mut found);
    found.sort_by_key(|b| std::cmp::Reverse(b.rank()));
    found
}

fn collect_dups(dir: &Path, depth_left: usize, out: &mut Vec<DupFile>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(meta) = entry.metadata() else { continue };
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        if meta.is_dir() {
            if depth_left > 1 {
                collect_dups(&path, depth_left - 1, out);
            }
        } else if name.to_ascii_lowercase().ends_with(".dup") {
            out.push(DupFile {
                kind: classify(&name),
                cycle: parse_cycle(&name),
                entitlement: parse_entitlement(&name),
                size: meta.len(),
                modified: meta.modified().unwrap_or(SystemTime::UNIX_EPOCH),
                path,
            });
        }
    }
}

pub fn newest(files: &[DupFile], kind: DupKind) -> Option<&DupFile> {
    files
        .iter()
        .filter(|f| f.kind == kind)
        .max_by_key(|f| f.rank())
}

// ---------------------------------------------------------------------------
// Plates archive
// ---------------------------------------------------------------------------

const JUNK_NAMES: [&str; 3] = [".ds_store", "thumbs.db", "desktop.ini"];

fn is_junk(parts: &[String]) -> bool {
    if parts.iter().any(|p| p == "__MACOSX") {
        return true;
    }
    match parts.last() {
        Some(name) => {
            JUNK_NAMES.contains(&name.to_ascii_lowercase().as_str()) || name.starts_with("._")
        }
        None => true,
    }
}

#[derive(Debug, Clone)]
pub struct Member {
    /// Index into the zip's central directory.
    pub index: usize,
    /// Destination path relative to `ChartData/Plates` on the drive.
    pub dest: PathBuf,
    pub size: u64,
    /// CRC-32 recorded in the archive, used to tell an unchanged plate from a
    /// revised one without rewriting it.
    pub crc32: u32,
}

#[derive(Debug, Clone)]
pub struct Archive {
    pub path: PathBuf,
    pub members: Vec<Member>,
    pub total_bytes: u64,
    pub cycle: Option<Cycle>,
    /// A single folder wrapping every entry, once `ChartData`/`Plates` are gone.
    pub wrapper: Option<String>,
    /// Entries skipped as archive litter.
    pub junk_skipped: usize,
}

impl Archive {
    pub fn name(&self) -> String {
        self.path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    }

    /// Members with the wrapping folder removed, when the user asks for that.
    pub fn members_stripped(&self, strip_wrapper: bool) -> Vec<Member> {
        if !strip_wrapper || self.wrapper.is_none() {
            return self.members.clone();
        }
        self.members
            .iter()
            .map(|m| {
                let mut parts = m.dest.components();
                parts.next();
                Member {
                    index: m.index,
                    dest: parts.as_path().to_path_buf(),
                    size: m.size,
                    crc32: m.crc32,
                }
            })
            .collect()
    }
}

/// Read an archive's central directory and work out where each file belongs.
///
/// Rejects `..`/absolute members outright, drops archive litter, and strips a
/// wrapping `ChartData/` and/or `Plates/` so entries land under the drive's
/// `ChartData/Plates`.
pub fn read_archive(path: &Path) -> Result<Archive> {
    let file = fs::File::open(path).with_context(|| format!("opening {}", path.display()))?;
    let mut zip = zip::ZipArchive::new(file).context("this file is not a readable zip archive")?;

    let mut raw: Vec<(usize, Vec<String>, u64, u32)> = Vec::new();
    let mut junk_skipped = 0usize;

    for index in 0..zip.len() {
        let entry = zip.by_index_raw(index)?;
        if entry.is_dir() {
            continue;
        }
        let name = entry.name().replace('\\', "/");
        if name.starts_with('/') {
            bail!("contains unsafe file paths");
        }
        let mut parts: Vec<String> = Vec::new();
        for part in name.split('/') {
            match part {
                "" | "." => continue,
                ".." => bail!("contains unsafe file paths"),
                other => parts.push(other.to_string()),
            }
        }
        if parts.is_empty() {
            continue;
        }
        if is_junk(&parts) {
            junk_skipped += 1;
            continue;
        }
        raw.push((index, parts, entry.size(), entry.crc32()));
    }

    if raw.is_empty() {
        bail!("this archive contains no plate files");
    }

    // Strip a wrapping ChartData/ and/or Plates/, one level at a time. A file
    // sitting at the current top level ends the strip: it would be orphaned.
    loop {
        let mut heads = raw
            .iter()
            .filter(|(_, p, _, _)| p.len() > 1)
            .map(|(_, p, _, _)| p[0].as_str());
        let Some(first) = heads.next() else { break };
        let head = first.to_string();
        if !heads.all(|h| h == head) {
            break;
        }
        let lower = head.to_ascii_lowercase();
        if lower != "chartdata" && lower != "plates" {
            break;
        }
        if raw.iter().any(|(_, p, _, _)| p.len() == 1) {
            break;
        }
        for (_, parts, _, _) in raw.iter_mut() {
            parts.remove(0);
        }
    }

    // Whatever single folder remains wrapping everything is reported, not
    // removed: flattening a real data folder (US/) is worse than a wrapper.
    let wrapper = {
        let mut heads = raw
            .iter()
            .filter(|(_, p, _, _)| p.len() > 1)
            .map(|(_, p, _, _)| p[0].as_str());
        match heads.next() {
            Some(first) => {
                let head = first.to_string();
                if heads.all(|h| h == head) && !raw.iter().any(|(_, p, _, _)| p.len() == 1) {
                    Some(head)
                } else {
                    None
                }
            }
            None => None,
        }
    };

    let total_bytes = raw.iter().map(|(_, _, s, _)| *s).sum();
    let members = raw
        .into_iter()
        .map(|(index, parts, size, crc32)| Member {
            index,
            dest: parts.iter().collect::<PathBuf>(),
            size,
            crc32,
        })
        .collect();

    let cycle = path
        .file_name()
        .and_then(|n| parse_cycle(&n.to_string_lossy()));

    Ok(Archive {
        path: path.to_path_buf(),
        members,
        total_bytes,
        cycle,
        wrapper,
        junk_skipped,
    })
}

/// Candidate plates archives in a folder, best first.
///
/// A cycle's download folder usually holds unrelated zips too, so this scores
/// rather than guesses: the name saying "plates" is the strongest signal, a
/// cycle matching the databases next, and sheer size last (a real plates
/// archive is gigabytes). Anything scoring zero is not offered at all.
pub fn rank_plate_archives(folder: &Path, target: Option<Cycle>) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(folder) else {
        return Vec::new();
    };
    let mut scored: Vec<(i32, u64, PathBuf)> = Vec::new();

    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.to_ascii_lowercase().ends_with(".zip") || name.starts_with('.') {
            continue;
        }
        let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
        let lower = name.to_ascii_lowercase();
        let mut score = 0;
        if lower.contains("plate") || lower.contains("chart") {
            score += 4;
        }
        match (parse_cycle(&name), target) {
            (Some(found), Some(want)) if found == want => score += 3,
            (Some(_), _) => score += 1,
            _ => {}
        }
        if size >= 100 * 1024 * 1024 {
            score += 1;
        }
        if score > 0 {
            scored.push((score, size, path));
        }
    }

    scored.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.cmp(&a.1)));
    scored.into_iter().map(|(_, _, path)| path).collect()
}

// ---------------------------------------------------------------------------
// Database packages (.duc)
// ---------------------------------------------------------------------------

/// A Dynon `.duc` package holding the aviation and obstacle databases, as
/// published on dynonavionics.com / dynoncertified.com.
///
/// `.duc` is Dynon's generic container — firmware updates use it too — so a
/// package is only treated as databases when it actually carries them. The
/// filename alone is not trusted.
#[derive(Debug, Clone)]
pub struct Package {
    pub path: PathBuf,
    pub aviation: Option<Cycle>,
    pub obstacle: Option<Cycle>,
    pub size: u64,
    pub modified: SystemTime,
}

impl Package {
    pub fn name(&self) -> String {
        self.path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    }

    /// How this package is described in the interface. The two cycles usually
    /// differ — Dynon's own file pairs aviation 2608 with obstacle 2604 — so
    /// collapsing them to one number would be wrong.
    pub fn version(&self) -> String {
        match (self.aviation, self.obstacle) {
            (Some(av), Some(ob)) if av == ob => format!("Cycle {av}"),
            (Some(av), Some(ob)) => format!("Aviation {av} · Obstacles {ob}"),
            (Some(av), None) => format!("Aviation {av}"),
            (None, Some(ob)) => format!("Obstacles {ob}"),
            (None, None) => "Unknown cycle".into(),
        }
    }
}

/// Read a `.duc` and report the databases it contains, or `None` when it holds
/// something else (a firmware package, say).
pub fn read_package(path: &Path) -> Option<Package> {
    let meta = fs::metadata(path).ok()?;
    let file = fs::File::open(path).ok()?;
    let mut zip = zip::ZipArchive::new(file).ok()?;

    let mut aviation = None;
    let mut obstacle = None;
    for index in 0..zip.len() {
        let Ok(entry) = zip.by_index_raw(index) else {
            continue;
        };
        let name = entry
            .name()
            .rsplit('/')
            .next()
            .unwrap_or_default()
            .to_string();
        match classify(&name) {
            DupKind::Aviation => aviation = aviation.max(parse_cycle(&name)),
            DupKind::Obstacle => obstacle = obstacle.max(parse_cycle(&name)),
            DupKind::Other => {}
        }
    }
    if aviation.is_none() && obstacle.is_none() {
        return None;
    }
    Some(Package {
        path: path.to_path_buf(),
        aviation,
        obstacle,
        size: meta.len(),
        modified: meta.modified().unwrap_or(SystemTime::UNIX_EPOCH),
    })
}

/// Database packages in a folder, newest aviation cycle first.
pub fn scan_packages(dir: &Path) -> Vec<Package> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut found: Vec<Package> = entries
        .flatten()
        .filter(|e| {
            e.file_name()
                .to_string_lossy()
                .to_ascii_lowercase()
                .ends_with(".duc")
        })
        .filter_map(|e| read_package(&e.path()))
        .collect();
    found.sort_by(|a, b| {
        b.aviation
            .cmp(&a.aviation)
            .then(b.obstacle.cmp(&a.obstacle))
            .then(b.modified.cmp(&a.modified))
    });
    found
}
