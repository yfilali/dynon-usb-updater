# Dynon USB Updater

Prepare the USB drives you carry to the aircraft: this app copies each AIRAC
cycle's aviation and obstacle databases onto them and replaces their approach
plates, for one drive or several at once.

[![CI](https://github.com/yfilali/dynon-usb-updater/actions/workflows/ci.yml/badge.svg)](https://github.com/yfilali/dynon-usb-updater/actions/workflows/ci.yml)
[![License: GPL v3](https://img.shields.io/badge/License-GPLv3-blue.svg)](COPYING)

![The app ready to update two drives](screenshots/ready.png)

## What it does

Every cycle you download three things from your chart provider: an aviation
database, an obstacle database, and a plates archive. For each USB drive you
select, this app:

1. copies the newest **aviation database** `.dup` to the drive,
2. copies the newest **obstacle database** `.dup` to the drive,
3. **erases `ChartData/Plates` and rebuilds it** from the plates archive.

It works out which files are newest by their cycle number, recognises which of
your USB drives are SkyView drives, checks each one has room before it starts,
and verifies every database it writes by reading it back.

## Installing

**Flathub** (recommended)

```sh
flatpak install flathub io.github.yfilali.DynonUSBUpdater
```

**Arch / Manjaro**

```sh
yay -S dynon-usb-updater
```

**From source** — see [Building](#building) below.

## Using it

Plug in your drives and open the app. It reads your Downloads folder, picks
this cycle's files, and pre-selects the SkyView drives it recognises — usually
there is nothing to do but press the button.

![The prepare page](screenshots/ready.png)

Each drive card tells you what is on that drive now, what it will become, and
whether the update fits. A drive is selected for you only when it is a
recognised SkyView drive, is writable, has room, and is not already on the
cycle you are installing; anything else you must select yourself, deliberately.

Press **Update** and confirm, and the app shows you exactly where it is:

![The running page](screenshots/running.png)

When it finishes, it stays on the result and tells you what happened to each
drive. Nothing disappears on a timer.

![The result page](screenshots/result.png)

## What it will and will not touch on your drives

This matters more than anything else in this README, so it is explicit:

**It writes:**

- the two `.dup` database files, at the top level of the drive
- everything inside `ChartData/Plates`

**It deletes:**

- older aviation and obstacle `.dup` files, and only those, and only when
  *Replace Older Databases* is on (it is by default)
- the entire contents of `ChartData/Plates`, and only when you have selected a
  plates archive

**It never touches anything else** — your `CHARTS-*.key`, `FACTORY`, logbooks,
settings archives, manuals, flight logs, or any other file or folder on the
drive is left exactly as it was.

Other safety behaviour worth knowing:

- Every database is written to a temporary file, flushed, and only then renamed
  into place, so an interrupted copy can never leave a half-written file under
  the real name.
- *Verify Copies* (on by default) re-reads each database from the drive and
  compares SHA-256 against the source. A mismatch fails that drive and deletes
  the bad copy rather than leaving it.
- Space is checked per drive before anything is written, counting the space
  that clearing the old plates will free.
- Once the plates folder has been erased, stopping leaves it incomplete. The
  app says so plainly, asks you to confirm, and refuses to close silently while
  a run is in progress. It also prevents your computer from suspending mid-run.
- If one drive fails, the others still finish, and the result page tells you
  which failed and why.

## How it decides

**Which file is newest.** Dynon names files by AIRAC cycle —
`airmate_av_data_us_2608_013712.dup` is cycle 2608. The app ranks by that cycle,
falling back to a date in the filename and then the file's timestamp. The
trailing `013712` is the chart entitlement id, not a cycle, and is not mistaken
for one.

**Which drives are yours.** A drive is recognised as a SkyView drive by
evidence, not by its name: a `ChartData` folder, `.dup` files at the root, a
`CHARTS-*.key`, a `FACTORY` or settings folder, `.duc` packages, or a matching
volume label. It also compares the drive's chart key against the entitlement id
of the files you are installing, and warns you if the drive belongs to a
different SkyView.

**How the archive is unpacked.** A wrapping `ChartData/` and `Plates/` folder is
removed so plates land in the right place, archive litter (`.DS_Store`,
`Thumbs.db`, `__MACOSX`, …) is skipped, and any entry with an absolute path or
`..` in it causes the archive to be rejected outright.

## Troubleshooting

**My drive doesn't appear.** If you installed from Flathub, the sandbox may not
have permission to see removable media. The app detects this and tells you; the
fix is either to choose the drive's folder manually (which always works), or to
grant permission once:

```sh
flatpak override --user --filesystem=/run/media io.github.yfilali.DynonUSBUpdater
```

**It says the drive doesn't have room.** The figure counts the space that
erasing the current plates will free. If it still does not fit, the archive is
genuinely larger than the drive.

**I stopped the update part-way.** If it had started replacing plates, that
drive's plates folder is incomplete — run the update again before you fly with
it. Databases are unaffected: they are written whole or not at all.

**Where are the logs?** Every run writes one to
`~/.local/share/io.github.yfilali.DynonUSBUpdater/logs/`, and the last 20 are
kept. The **Details** section shows the current run, with buttons to copy or
save it.

## Building

Requires Rust 1.80+, GTK 4.16+, and libadwaita 1.7+.

```sh
meson setup build
meson install -C build
```

For development, plain Cargo works too — but the app needs its GSettings schema
to be findable:

```sh
cargo build
glib-compile-schemas data/
GSETTINGS_SCHEMA_DIR=data ./target/debug/dynon-usb-updater
```

Run the tests with `cargo test`. The suite covers cycle parsing, archive
handling (including path-traversal rejection), drive recognition, and full
update runs against fixture directories. Two tests check against real hardware
and real cycle files and skip themselves when those are absent.

| Path | What it is |
| --- | --- |
| `src/scan.rs` | Cycle parsing, database discovery, archive inspection |
| `src/drive.rs` | Drive discovery, SkyView recognition, sandbox detection |
| `src/job.rs` | The update engine — copy, verify, erase, extract |
| `src/window.rs`, `src/ui/` | The interface |
| `docs/UX-SPEC.md` | The design this implements, and its acceptance criteria |
| `docs/PUBLISHING.md` | Packaging and release process |
| `screenshots/capture.sh` | Regenerates every screenshot below from fixtures |

## Screenshots

Every image below is generated by `screenshots/capture.sh` against fixture
data — never against real avionics drives.

### Ready to update

The prepare page: this cycle's files at the top, the drives that will receive
them below, and a single button that says exactly what it is about to do.

![Ready to update](screenshots/ready.png)

### Updating

One percentage for the whole job, the current step, a per-drive breakdown, and
a time estimate. The window title carries the progress too, so it is readable
from the overview.

![Updating](screenshots/running.png)

### Finished

The result stays until dismissed, names every drive, and offers to eject.

![Finished](screenshots/result.png)

### No drives connected

The app distinguishes "nothing is plugged in" from "this sandbox is not allowed
to see your drives", and offers a way forward in both cases.

![No drives connected](screenshots/drives-empty.png)

## License and credits

GPL-3.0-or-later. Built by Yacine Filali.

Not affiliated with, endorsed by, or supported by Dynon Avionics. "Dynon" and
"SkyView" are trademarks of Dynon Avionics, Inc.
