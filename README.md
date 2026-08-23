# Dynon USB Updater

A GTK4 / libadwaita app that prepares one or more USB thumb drives for a Dynon
SkyView data update.

For each selected drive it:

1. copies the latest **aviation data** `.dup` file to the root of the drive,
2. copies the latest **obstacle data** `.dup` file to the root of the drive,
3. **replaces** the contents of `ChartData/Plates` from a selected plates `.zip`.

## Running

```sh
./run.sh
```

`run.sh` picks a Python that has PyGObject + GTK4 + libadwaita — normally
`/usr/bin/python3`. This matters: an Anaconda or pyenv interpreter usually lacks
`gi` entirely, and the app will not start under it. To get it into the app grid,
copy `dynon-usb-updater.desktop` to `~/.local/share/applications/`.

Requirements: `python-gobject`, `gtk4`, `libadwaita` (all already present on a
standard GNOME install).

## Files

| File | Purpose |
| --- | --- |
| `dynon_usb_updater.py` | The GTK4 / libadwaita interface |
| `dynon_core.py` | Drive discovery, `.dup` selection, zip handling, copy/verify — no GUI |
| `run.sh` | Interpreter picker / launcher |
| `dynon-usb-updater.desktop` | Desktop entry |

## How it works

**Update files.** On launch the app reads your **Downloads** folder (or whatever
you used last) and scans three levels deep for `*.dup`, sorting them into
*aviation* and *obstacle* by filename and pre-selecting the newest of each. Dynon
names its files by AIRAC-style cycle — `airmate_av_data_us_2608_013712.dup` is
cycle 2608 — so version comparison uses that cycle number where present, a date in
the filename if there is one, and the file's timestamp otherwise. The digit
boundaries in the cycle pattern stop it matching inside a longer number such as
`013712`. Both drop-downs list every `.dup` found, so you can override the guess.

**Drives.** Removable drives come from `Gio.VolumeMonitor`, so they appear and
disappear as you plug them in, with their real volume names and a capacity bar.
The **+** button in the Target Drives header adds any folder as a target, for
staging or for a reader that does not report itself as ejectable.

**Plates.** A wrapping `ChartData/` and/or `Plates/` folder inside the zip is
stripped automatically, so `ChartData/Plates/US/D120531.png` in the archive lands
at `ChartData/Plates/US/D120531.png` on the drive. Archive litter (`.DS_Store`,
`Thumbs.db`, `desktop.ini`, `._*`, `__MACOSX`) is skipped — it would otherwise
clutter the drive *and* defeat the prefix detection, which is exactly what a stray
`ChartData/.DS_Store` does. If everything in the archive sits under one further
folder, the *Strip Top-Level Folder* switch is offered but left **off**: flattening
a real data folder is much worse than leaving a wrapper in place. Path traversal
(`../`) entries are rejected outright.

## Safety

- The confirmation dialog names every drive and action and states that
  `ChartData/Plates` will be erased.
- Free space is checked per drive before anything is written; space freed by
  clearing the old plates counts toward it.
- `.dup` files are written to a `.part` file, `fsync`ed, then renamed into place,
  so an interrupted copy cannot leave a half-written file under the real name.
- *Verify After Copying* (on by default) re-reads each copy from the stick and
  compares SHA-256 against the source.
- *Replace Older Databases* (on by default) deletes stale aviation/obstacle `.dup`
  files from the drive root so SkyView sees exactly one of each. Nothing else is
  touched — only `ChartData/Plates` is cleared, and only when you select an archive.
- A failure on one drive is logged and the other drives still run.
- Cancelling during extraction is honoured, but the old plates are already gone by
  then, so the log says plainly how many files were written and that the folder is
  incomplete.

Work runs on a background thread; progress, the log (under **Details**) and Cancel
stay live. Eject the drives normally before unplugging.
