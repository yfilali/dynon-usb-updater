//! Target discovery: mounted removable drives, folder targets, and the
//! evidence needed to tell "nothing plugged in" from "not allowed to look".

use crate::scan::{self, Cycle, DupKind};
use gtk::gio;
use gtk::prelude::*;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetKind {
    /// A volume the system mounted for us.
    Mounted,
    /// A folder the user picked, which also covers the portal fallback.
    Folder,
}

#[derive(Debug, Clone)]
pub struct Drive {
    pub path: PathBuf,
    pub name: String,
    pub kind: TargetKind,
    pub uuid: Option<String>,
    pub total: u64,
    pub free: u64,
    pub writable: bool,
    /// SkyView recognition score; recognised at >= 2.
    pub score: u8,
    pub installed_cycle: Option<Cycle>,
    /// Entitlement id from `CHARTS-nnnnnn.key`.
    pub entitlement: Option<String>,
    /// Bytes and file count under ChartData/Plates, filled in asynchronously.
    pub reclaimable: Option<(u64, usize)>,
    pub reachable: bool,
}

impl Drive {
    pub fn recognised(&self) -> bool {
        self.score >= 2
    }

    pub fn plates_dir(&self) -> PathBuf {
        self.path.join("ChartData").join("Plates")
    }

    /// Bytes this update needs, against what the drive can offer.
    pub fn fits(&self, needed: u64) -> bool {
        let reclaim = self.reclaimable.map(|(b, _)| b).unwrap_or(0);
        needed <= self.free.saturating_add(reclaim)
    }

    pub fn key(&self) -> String {
        match &self.uuid {
            Some(u) => format!("{u}|{}", self.name),
            None => format!("|{}", self.name),
        }
    }
}

/// Score a drive root against the signals a real SkyView stick carries.
pub fn recognise(root: &Path, label: &str) -> (u8, Option<Cycle>, Option<String>) {
    let mut score = 0u8;
    let mut cycle: Option<Cycle> = None;
    let mut entitlement: Option<String> = None;
    let mut has_dup = false;
    let mut has_key = false;
    let mut has_duc = false;
    let mut has_support_dir = false;

    if root.join("ChartData").is_dir() {
        score += 2;
    }
    if let Ok(entries) = fs::read_dir(root) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let lower = name.to_ascii_lowercase();
            let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
            if is_dir {
                if matches!(
                    name.as_str(),
                    "FACTORY" | "User Settings" | "settings_archive"
                ) {
                    has_support_dir = true;
                }
                continue;
            }
            if lower.ends_with(".dup") {
                has_dup = true;
                if scan::classify(&name) == DupKind::Aviation {
                    let c = scan::parse_cycle(&name);
                    if c > cycle {
                        cycle = c;
                    }
                }
            } else if lower.ends_with(".duc") {
                has_duc = true;
            } else if let Some(id) = scan::parse_key_entitlement(&name) {
                has_key = true;
                entitlement = Some(id);
            }
        }
    }
    if has_dup {
        score += 2;
    }
    if has_key {
        score += 2;
    }
    if has_support_dir {
        score += 1;
    }
    if has_duc {
        score += 1;
    }
    let l = label.to_ascii_lowercase();
    if l.contains("dynon") || l.contains("skyview") {
        score += 1;
    }
    (score, cycle, entitlement)
}

fn capacity(path: &Path) -> (u64, u64, bool) {
    let file = gio::File::for_path(path);
    let (mut total, mut free) = (0, 0);
    if let Ok(info) =
        file.query_filesystem_info("filesystem::size,filesystem::free", gio::Cancellable::NONE)
    {
        total = info.attribute_uint64("filesystem::size");
        free = info.attribute_uint64("filesystem::free");
    }
    let writable = file
        .query_info(
            "access::can-write",
            gio::FileQueryInfoFlags::NONE,
            gio::Cancellable::NONE,
        )
        .map(|i| i.boolean("access::can-write"))
        .unwrap_or(false);
    (total, free, writable)
}

fn drive_from_root(path: &Path, name: String, kind: TargetKind, uuid: Option<String>) -> Drive {
    let (total, free, writable) = capacity(path);
    let (score, installed_cycle, entitlement) = recognise(path, &name);
    Drive {
        path: path.to_path_buf(),
        name,
        kind,
        uuid,
        total,
        free,
        writable,
        score,
        installed_cycle,
        entitlement,
        reclaimable: None,
        reachable: true,
    }
}

/// Removable volumes the system has mounted for us.
///
/// For screenshots and manual testing (`screenshots/capture.sh`), setting
/// `DYNON_TEST_DRIVE_ROOTS` to a `:`-separated list of directories swaps in
/// those fixture directories as the drive list instead of the real
/// `GVolumeMonitor` mounts — so a run driving the UI never touches, and
/// never even sees, real hardware. Unset (the default, including under
/// `cargo test`), this is a no-op.
pub fn enumerate() -> Vec<Drive> {
    if let Ok(roots) = std::env::var("DYNON_TEST_DRIVE_ROOTS") {
        return roots
            .split(':')
            .filter(|s| !s.is_empty())
            .map(|s| {
                let path = PathBuf::from(s);
                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                drive_from_root(&path, name, TargetKind::Mounted, None)
            })
            .collect();
    }

    let mut drives = Vec::new();
    for mount in gio::VolumeMonitor::get().mounts() {
        if mount.is_shadowed() {
            continue;
        }
        let Some(root) = mount.root().path() else {
            continue;
        };
        let removable = mount.can_eject()
            || mount
                .volume()
                .and_then(|v| v.drive())
                .map(|d| d.is_removable() || d.can_eject())
                .unwrap_or(false);
        if !removable {
            continue;
        }
        let uuid = mount
            .uuid()
            .map(|u| u.to_string())
            .or_else(|| mount.volume().and_then(|v| v.uuid()).map(|u| u.to_string()));
        let name = mount.name().to_string();
        drives.push(drive_from_root(&root, name, TargetKind::Mounted, uuid));
    }
    drives.sort_by(|a, b| a.path.cmp(&b.path));
    drives
}

/// A folder the user chose, scanned exactly like a mounted drive.
pub fn folder_target(path: &Path) -> Drive {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string());
    let mut drive = drive_from_root(path, name, TargetKind::Folder, None);
    drive.reachable = path.is_dir();
    drive
}

/// Bytes and file count under a drive's plates folder.
pub fn measure_plates(dir: &Path) -> (u64, usize) {
    let (mut bytes, mut files) = (0u64, 0usize);
    let mut stack = vec![dir.to_path_buf()];
    while let Some(current) = stack.pop() {
        let Ok(entries) = fs::read_dir(&current) else {
            continue;
        };
        for entry in entries.flatten() {
            match entry.file_type() {
                Ok(t) if t.is_dir() => stack.push(entry.path()),
                Ok(t) if t.is_file() => {
                    files += 1;
                    bytes += entry.metadata().map(|m| m.len()).unwrap_or(0);
                }
                _ => {}
            }
        }
    }
    (bytes, files)
}

// ---------------------------------------------------------------------------
// Why is the drive list empty?
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmptyReason {
    /// Nothing is plugged in.
    NothingConnected,
    /// Sandboxed and not allowed to see the mounts.
    SandboxBlocked,
    /// Hardware is attached but nothing mounted it.
    NotMounted,
}

pub struct Sandbox {
    pub sandboxed: bool,
    pub grants_media: bool,
    pub media_roots_visible: bool,
    pub hardware_present: bool,
}

impl Sandbox {
    pub fn detect() -> Self {
        let info = fs::read_to_string("/.flatpak-info").ok();
        let sandboxed = info.is_some();
        let grants_media = info
            .as_deref()
            .map(|text| {
                text.lines()
                    .find(|l| l.starts_with("filesystems="))
                    .map(|l| {
                        l.split('=')
                            .nth(1)
                            .unwrap_or("")
                            .split(';')
                            .any(|f| f == "/run/media" || f == "host" || f == "host-os")
                    })
                    .unwrap_or(false)
            })
            .unwrap_or(true);

        let user = std::env::var("USER").unwrap_or_default();
        let media_roots_visible = [
            format!("/run/media/{user}"),
            "/run/media".into(),
            format!("/media/{user}"),
            "/media".into(),
            "/mnt".into(),
        ]
        .iter()
        .any(|p| fs::read_dir(p).is_ok());

        Self {
            sandboxed,
            grants_media,
            media_roots_visible,
            hardware_present: removable_hardware_present(),
        }
    }

    pub fn classify(&self) -> EmptyReason {
        if self.sandboxed
            && (!self.grants_media || !self.media_roots_visible || self.hardware_present)
        {
            EmptyReason::SandboxBlocked
        } else if self.hardware_present {
            EmptyReason::NotMounted
        } else {
            EmptyReason::NothingConnected
        }
    }
}

/// `/sys` is mounted read-only even inside the sandbox, so it can prove a USB
/// mass-storage device is attached when its mount point is invisible.
fn removable_hardware_present() -> bool {
    let Ok(entries) = fs::read_dir("/sys/class/block") else {
        return false;
    };
    for entry in entries.flatten() {
        let base = entry.path();
        let removable = fs::read_to_string(base.join("removable"))
            .map(|s| s.trim() == "1")
            .unwrap_or(false);
        let via_usb = fs::canonicalize(base.join("device"))
            .map(|p| p.to_string_lossy().contains("/usb"))
            .unwrap_or(false);
        if !(removable || via_usb) {
            continue;
        }
        // Require a partition with a non-zero size, so empty card readers
        // do not masquerade as an attached drive.
        let name = entry.file_name().to_string_lossy().into_owned();
        if let Ok(children) = fs::read_dir(&base) {
            for child in children.flatten() {
                if !child.file_name().to_string_lossy().starts_with(&name) {
                    continue;
                }
                if fs::read_to_string(child.path().join("size"))
                    .map(|s| s.trim() != "0")
                    .unwrap_or(false)
                {
                    return true;
                }
            }
        }
    }
    false
}
