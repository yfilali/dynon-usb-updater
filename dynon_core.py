#!/usr/bin/env python3
"""
Core logic for the Dynon USB updater: drive discovery, .dup selection, plates
extraction and the copy/verify job.  No GUI dependencies — the GTK front end in
dynon_usb_updater.py drives all of this.
"""

from __future__ import annotations

import os
import re
import shutil
import string
import sys
import threading
import zipfile
import hashlib
import queue
from dataclasses import dataclass, field
from datetime import datetime
from pathlib import Path, PurePosixPath


CHUNK = 1 << 20  # 1 MiB
CHART_SUBPATH = ("ChartData", "Plates")

# --------------------------------------------------------------------------
# Drive discovery
# --------------------------------------------------------------------------

PSEUDO_FS = {
    "proc", "sysfs", "devtmpfs", "devpts", "tmpfs", "cgroup", "cgroup2",
    "securityfs", "pstore", "efivarfs", "bpf", "debugfs", "tracefs",
    "configfs", "fusectl", "mqueue", "hugetlbfs", "autofs", "binfmt_misc",
    "squashfs", "overlay", "ramfs", "nsfs", "rpc_pipefs", "selinuxfs",
}
REMOVABLE_FS = {"vfat", "msdos", "exfat", "ntfs", "ntfs3", "fuseblk", "udf"}


@dataclass
class Drive:
    path: Path
    label: str
    fstype: str
    total: int = 0
    free: int = 0

    @property
    def key(self) -> str:
        return str(self.path)


def _octal_unescape(s: str) -> str:
    return re.sub(r"\\([0-7]{3})", lambda m: chr(int(m.group(1), 8)), s)


def _disk_sysfs_dir(device: str) -> Path | None:
    """/dev/sdb1 -> /sys/.../block/sdb (the whole-disk kobject)."""
    name = os.path.basename(os.path.realpath(device))
    part = Path("/sys/class/block") / name
    if not part.exists():
        return None
    real = Path(os.path.realpath(part))
    parent = real.parent
    # A partition lives inside its disk's directory; a whole disk lives in .../block
    return parent if (parent / "removable").exists() else real


def _linux_drives(include_all: bool) -> list[Drive]:
    drives: list[Drive] = []
    try:
        lines = Path("/proc/mounts").read_text().splitlines()
    except OSError:
        return drives

    for line in lines:
        parts = line.split()
        if len(parts) < 4:
            continue
        device, mount, fstype, opts = (
            _octal_unescape(parts[0]),
            _octal_unescape(parts[1]),
            parts[2],
            parts[3],
        )
        if fstype in PSEUDO_FS or not device.startswith("/dev/"):
            continue
        if "ro" in opts.split(",") and not include_all:
            continue
        if mount == "/" and not include_all:
            continue

        removable = False
        sysdir = _disk_sysfs_dir(device)
        if sysdir is not None:
            try:
                removable = (sysdir / "removable").read_text().strip() == "1"
            except OSError:
                pass
            if "/usb" in str(sysdir).lower():
                removable = True

        if not include_all and not (removable and fstype in REMOVABLE_FS):
            continue

        drives.append(Drive(Path(mount), os.path.basename(mount) or mount, fstype))
    return drives


def _windows_drives(include_all: bool) -> list[Drive]:
    import ctypes

    k32 = ctypes.windll.kernel32
    drives: list[Drive] = []
    mask = k32.GetLogicalDrives()
    for i, letter in enumerate(string.ascii_uppercase):
        if not mask >> i & 1:
            continue
        root = f"{letter}:\\"
        dtype = k32.GetDriveTypeW(ctypes.c_wchar_p(root))
        # 2 = DRIVE_REMOVABLE, 3 = DRIVE_FIXED
        if dtype != 2 and not (include_all and dtype in (3, 4)):
            continue
        name_buf = ctypes.create_unicode_buffer(261)
        fs_buf = ctypes.create_unicode_buffer(261)
        try:
            k32.GetVolumeInformationW(
                ctypes.c_wchar_p(root), name_buf, 261, None, None, None, fs_buf, 261
            )
        except OSError:
            pass
        label = name_buf.value or letter
        drives.append(Drive(Path(root), f"{label} ({letter}:)", fs_buf.value or "?"))
    return drives


def _macos_drives(include_all: bool) -> list[Drive]:
    drives: list[Drive] = []
    vol = Path("/Volumes")
    if not vol.is_dir():
        return drives
    for entry in sorted(vol.iterdir()):
        try:
            if not entry.is_dir() or entry.is_symlink():
                continue
        except OSError:
            continue
        drives.append(Drive(entry, entry.name, "?"))
    return drives


def list_drives(include_all: bool = False) -> list[Drive]:
    if sys.platform.startswith("win"):
        drives = _windows_drives(include_all)
    elif sys.platform == "darwin":
        drives = _macos_drives(include_all)
    else:
        drives = _linux_drives(include_all)

    out = []
    for d in drives:
        try:
            usage = shutil.disk_usage(d.path)
        except OSError:
            continue
        d.total, d.free = usage.total, usage.free
        out.append(d)
    out.sort(key=lambda d: str(d.path))
    return out


# --------------------------------------------------------------------------
# .dup discovery
# --------------------------------------------------------------------------

OBSTACLE_RE = re.compile(r"obst", re.I)
AVDATA_RE = re.compile(r"av[_\- ]?data|aviation|nav[_\- ]?data", re.I)
CYCLE_RE = re.compile(r"(?<![0-9])([0-9]{2})(0[1-9]|1[0-3])(?![0-9])")
DATE_RES = [
    re.compile(r"(?P<y>20\d{2})[-_.]?(?P<m>0[1-9]|1[0-2])[-_.]?(?P<d>0[1-9]|[12]\d|3[01])"),
    re.compile(r"(?P<d>0[1-9]|[12]\d|3[01])[-_.](?P<m>0[1-9]|1[0-2])[-_.](?P<y>20\d{2})"),
]


@dataclass
class DupFile:
    path: Path
    kind: str  # "avdata" | "obstacle" | "other"
    date: datetime | None
    mtime: float
    size: int
    cycle: tuple[int, int] | None = None   # (year, cycle), e.g. (2026, 8)

    @property
    def sort_key(self):
        """Prefer a real date, then an AIRAC-style cycle, then the file's age."""
        if self.date:
            return (2, self.date.timestamp(), self.mtime)
        if self.cycle:
            return (1, self.cycle[0] * 100 + self.cycle[1], self.mtime)
        return (0, 0, self.mtime)

    @property
    def version(self) -> str:
        if self.date:
            return self.date.strftime("%-d %B %Y")
        if self.cycle:
            return f"Cycle {self.cycle[0] % 100:02d}{self.cycle[1]:02d}"
        return datetime.fromtimestamp(self.mtime).strftime("%-d %B %Y")

    def display(self, root: Path) -> str:
        try:
            rel = self.path.relative_to(root)
        except ValueError:
            rel = self.path
        return f"{rel}   [{self.version}, {human(self.size)}]"


def parse_date(name: str) -> datetime | None:
    for rx in DATE_RES:
        m = rx.search(name)
        if m:
            try:
                return datetime(int(m["y"]), int(m["m"]), int(m["d"]))
            except ValueError:
                continue
    return None


def parse_cycle(name: str) -> tuple[int, int] | None:
    """'airmate_av_data_us_2608_013712.dup' -> (2026, 8).  The digit-boundary
    lookarounds keep it from matching inside a longer number such as 013712."""
    match = CYCLE_RE.search(name)
    if not match:
        return None
    return 2000 + int(match.group(1)), int(match.group(2))


def classify(name: str) -> str:
    if OBSTACLE_RE.search(name):
        return "obstacle"
    if AVDATA_RE.search(name):
        return "avdata"
    return "other"


def scan_dup_files(folder: Path, max_depth: int = 3) -> list[DupFile]:
    found: list[DupFile] = []
    root_depth = len(folder.parts)
    for dirpath, dirnames, filenames in os.walk(folder):
        depth = len(Path(dirpath).parts) - root_depth
        if depth >= max_depth:
            dirnames[:] = []
        dirnames[:] = [d for d in dirnames if not d.startswith(".")]
        for fn in filenames:
            if not fn.lower().endswith(".dup"):
                continue
            p = Path(dirpath) / fn
            try:
                st = p.stat()
            except OSError:
                continue
            found.append(DupFile(p, classify(fn), parse_date(fn), st.st_mtime,
                                 st.st_size, parse_cycle(fn)))
    found.sort(key=lambda f: f.sort_key, reverse=True)
    return found


def newest(files: list[DupFile], kind: str) -> DupFile | None:
    matches = [f for f in files if f.kind == kind]
    return max(matches, key=lambda f: f.sort_key) if matches else None


# --------------------------------------------------------------------------
# Helpers
# --------------------------------------------------------------------------

def human(n: float) -> str:
    for unit in ("B", "KB", "MB", "GB", "TB"):
        if abs(n) < 1024 or unit == "TB":
            return f"{n:.0f} {unit}" if unit == "B" else f"{n:.1f} {unit}"
        n /= 1024
    return f"{n:.1f} TB"


class Cancelled(Exception):
    pass


def sha256(path: Path, cancel: threading.Event, progress=None) -> str:
    h = hashlib.sha256()
    with open(path, "rb") as fh:
        while True:
            if cancel.is_set():
                raise Cancelled()
            block = fh.read(CHUNK)
            if not block:
                break
            h.update(block)
            if progress:
                progress(len(block))
    return h.hexdigest()


def copy_file(src: Path, dst: Path, cancel: threading.Event, progress=None) -> None:
    dst.parent.mkdir(parents=True, exist_ok=True)
    tmp = dst.with_name(dst.name + ".part")
    try:
        with open(src, "rb") as fin, open(tmp, "wb") as fout:
            while True:
                if cancel.is_set():
                    raise Cancelled()
                block = fin.read(CHUNK)
                if not block:
                    break
                fout.write(block)
                if progress:
                    progress(len(block))
            fout.flush()
            os.fsync(fout.fileno())
        if dst.exists():
            dst.unlink()
        os.replace(tmp, dst)
    finally:
        if tmp.exists():
            try:
                tmp.unlink()
            except OSError:
                pass


JUNK_NAMES = {".ds_store", "thumbs.db", "desktop.ini"}


def is_junk(parts) -> bool:
    """Archive litter that must not reach the drive — and, more importantly,
    must not confuse the common-prefix detection below."""
    if any(p == "__MACOSX" for p in parts):
        return True
    name = parts[-1]
    return name.lower() in JUNK_NAMES or name.startswith("._")


def safe_members(zf: zipfile.ZipFile) -> list[tuple[zipfile.ZipInfo, PurePosixPath]]:
    """Return (member, sanitized relative path) pairs, rejecting unsafe entries."""
    out = []
    for info in zf.infolist():
        if info.is_dir():
            continue
        raw = info.filename.replace("\\", "/")
        parts = [p for p in PurePosixPath(raw).parts if p not in ("", ".")]
        if any(p == ".." for p in parts) or raw.startswith("/") or ":" in parts[0:1]:
            raise ValueError(f"unsafe path in zip: {info.filename!r}")
        if not parts:
            continue
        if is_junk(parts):
            continue
        out.append((info, PurePosixPath(*parts)))
    return out


def strip_common_prefix(members, strip_single: bool = False):
    """Drop a wrapping folder such as 'Plates/', 'ChartData/Plates/', or (when
    strip_single is set) a single unnamed wrapper such as a cycle folder."""
    if not members:
        return members
    paths = [p for _, p in members]
    stripped = 0
    while True:
        firsts = {p.parts[0] for p in paths if len(p.parts) > 1}
        if len(firsts) != 1 or any(len(p.parts) == 1 for p in paths):
            break
        head = firsts.pop()
        if head.lower() not in ("plates", "chartdata"):
            break
        paths = [PurePosixPath(*p.parts[1:]) for p in paths]
        stripped += 1
    if strip_single:
        firsts = {p.parts[0] for p in paths if len(p.parts) > 1}
        if len(firsts) == 1 and not any(len(p.parts) == 1 for p in paths):
            paths = [PurePosixPath(*p.parts[1:]) for p in paths]
            stripped += 1
    if not stripped:
        return members
    return [(info, path) for (info, _), path in zip(members, paths)]


def dir_size(path: Path) -> int:
    total = 0
    for dirpath, _, filenames in os.walk(path):
        for fn in filenames:
            try:
                total += (Path(dirpath) / fn).stat().st_size
            except OSError:
                pass
    return total


# --------------------------------------------------------------------------
# The update job
# --------------------------------------------------------------------------

@dataclass
class Job:
    drives: list[Drive]
    avdata: Path | None
    obstacle: Path | None
    plates_zip: Path | None
    remove_old_dup: bool = True
    verify: bool = True
    strip_wrapper: bool = False


class Runner(threading.Thread):
    """Executes a Job, reporting to a queue consumed by the GUI thread."""

    def __init__(self, job: Job, out: queue.Queue, cancel: threading.Event):
        super().__init__(daemon=True)
        self.job = job
        self.out = out
        self.cancel = cancel
        self.done_bytes = 0
        self.total_bytes = 1

    # -- messaging ---------------------------------------------------------
    def log(self, text: str, tag: str = "info"):
        self.out.put(("log", (text, tag)))

    def status(self, text: str):
        self.out.put(("status", text))

    def bump(self, n: int):
        self.done_bytes += n
        self.out.put(("progress", min(100.0, self.done_bytes * 100.0 / self.total_bytes)))

    # -- planning ----------------------------------------------------------
    def plan(self):
        job = self.job
        dup_bytes = 0
        for p in (job.avdata, job.obstacle):
            if p:
                dup_bytes += p.stat().st_size
        zip_bytes = 0
        if job.plates_zip:
            with zipfile.ZipFile(job.plates_zip) as zf:
                zip_bytes = sum(i.file_size for i in zf.infolist() if not i.is_dir())
        per_drive = dup_bytes + zip_bytes
        if job.verify:
            per_drive += dup_bytes
        self.total_bytes = max(1, per_drive * len(job.drives))
        return dup_bytes, zip_bytes

    # -- work --------------------------------------------------------------
    def run(self):
        ok, failed = [], []
        try:
            dup_bytes, zip_bytes = self.plan()
        except Exception as exc:                       # noqa: BLE001
            self.log(f"Could not read the source files: {exc}", "error")
            self.out.put(("done", ([], [d.label for d in self.job.drives])))
            return

        for drive in self.job.drives:
            if self.cancel.is_set():
                break
            self.log(f"── {drive.label}  ({drive.path})", "head")
            try:
                self.update_drive(drive, dup_bytes, zip_bytes)
                self.log(f"{drive.label}: done", "good")
                ok.append(drive.label)
            except Cancelled:
                self.log(f"{drive.label}: cancelled", "warn")
                failed.append(drive.label)
                break
            except Exception as exc:                   # noqa: BLE001
                self.log(f"{drive.label}: FAILED — {exc}", "error")
                failed.append(drive.label)

        self.out.put(("done", (ok, failed)))

    def update_drive(self, drive: Drive, dup_bytes: int, zip_bytes: int):
        job = self.job
        root = drive.path
        if not os.access(root, os.W_OK):
            raise PermissionError("drive is not writable")

        plates_dir = root.joinpath(*CHART_SUBPATH)
        reclaimed = dir_size(plates_dir) if (job.plates_zip and plates_dir.is_dir()) else 0
        needed = dup_bytes + zip_bytes
        free = shutil.disk_usage(root).free + reclaimed
        if needed > free:
            raise OSError(
                f"not enough space — needs {human(needed)}, {human(free)} available"
            )

        # 1. .dup files ----------------------------------------------------
        for kind, src in (("avdata", job.avdata), ("obstacle", job.obstacle)):
            if not src:
                continue
            if job.remove_old_dup:
                for old in sorted(root.glob("*.dup")):
                    if old.name == src.name or classify(old.name) != kind:
                        continue
                    self.log(f"   removing old {old.name}", "warn")
                    old.unlink()
            self.status(f"{drive.label}: copying {src.name}")
            self.log(f"   copying {src.name} ({human(src.stat().st_size)})")
            dst = root / src.name
            copy_file(src, dst, self.cancel, self.bump)
            if job.verify:
                self.status(f"{drive.label}: verifying {src.name}")
                if sha256(src, self.cancel) != sha256(dst, self.cancel, self.bump):
                    raise OSError(f"verification failed for {src.name}")
                self.log("   verified", "good")

        # 2. Plates --------------------------------------------------------
        if job.plates_zip:
            self.install_plates(drive, plates_dir)

        # flush directory metadata (matters on FAT sticks)
        try:
            fd = os.open(root, os.O_RDONLY)
            try:
                os.fsync(fd)
            finally:
                os.close(fd)
        except OSError:
            pass

    def install_plates(self, drive: Drive, plates_dir: Path):
        job = self.job
        with zipfile.ZipFile(job.plates_zip) as zf:
            members = strip_common_prefix(safe_members(zf), job.strip_wrapper)
            if not members:
                raise ValueError("the plates zip contains no files")

            if plates_dir.exists():
                self.status(f"{drive.label}: clearing {'/'.join(CHART_SUBPATH)}")
                self.log(f"   clearing existing {'/'.join(CHART_SUBPATH)}", "warn")
                for child in plates_dir.iterdir():
                    if child.is_dir() and not child.is_symlink():
                        shutil.rmtree(child)
                    else:
                        child.unlink()
            plates_dir.mkdir(parents=True, exist_ok=True)

            self.status(f"{drive.label}: extracting plates ({len(members)} files)")
            self.log(f"   extracting {len(members)} plate files")
            done = 0
            try:
                for info, rel in members:
                    if self.cancel.is_set():
                        raise Cancelled()
                    target = plates_dir / Path(*rel.parts)
                    target.parent.mkdir(parents=True, exist_ok=True)
                    with zf.open(info) as fin, open(target, "wb") as fout:
                        while True:
                            if self.cancel.is_set():
                                raise Cancelled()
                            block = fin.read(CHUNK)
                            if not block:
                                break
                            fout.write(block)
                            self.bump(len(block))
                    done += 1
                    if done % 500 == 0:
                        self.status(f"{drive.label}: extracting plates "
                                    f"({done} of {len(members)})")
            except Cancelled:
                # the old plates are already gone, so say plainly what is on the drive
                self.log(f"   STOPPED after {done} of {len(members)} plates — "
                         f"{'/'.join(CHART_SUBPATH)} on this drive is incomplete "
                         f"and must be re-run before use", "error")
                raise
        self.log(f"   plates installed in {plates_dir}", "good")
