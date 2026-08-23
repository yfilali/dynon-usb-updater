# Dynon USB Updater — UX Specification

**Status:** implementable spec, v1.0
**Target stack:** Rust + gtk4-rs + libadwaita, GTK 4.22 / libadwaita 1.9, `.ui` XML with `gtk::CompositeTemplate` (no Blueprint), GNOME 49 runtime (50 compatible), shipped as a Flatpak on Flathub.
**Scope:** this document is the single source of truth for product identity, screens, states, copy, interaction, accessibility, and acceptance. An engineer implements from this without making design decisions.

Ground truth this spec was written against (measured on the target machine, not assumed):

| Fact | Value |
| --- | --- |
| Source folder | `~/Downloads` |
| Aviation database | `airmate_av_data_us_2608_013712.dup` — 8,630,943 B |
| Obstacle database | `airmate_obstacle_data_us_2608_013712.dup` — 1,999,540 B |
| Plates archive | `US-Plates-2608.zip` — 7,406,700,127 B on disk, 23,105 entries |
| Archive layout | `ChartData/Plates/US/*.png`, plus `ChartData/Plates/Plates.sqlite`, plus one junk entry `ChartData/.DS_Store` |
| Usable archive entries | 23,104 (23,105 minus junk), ~7.3 GB uncompressed |
| Decoy in the same folder | `324-Jaunell-Road.zip` — an unrelated archive that must never be auto-selected |
| Drive DYNON | 29 GB, 12 GB free, VFAT, `airmate_av_data_us_2607_013712.dup`, `airmate_obstacle_data_us_2607_013712.dup`, `ChartData/Plates/{Plates.sqlite,US/}` (27,490 stale files), `CHARTS-013712.key`, `FACTORY/`, `Logbooks/`, `settings_archive/`, `User Settings/`, two `.duc` firmware packages |
| Drive DYNON2 | 29 GB, 15 GB free, VFAT, same structure, same `CHARTS-013712.key` |
| Cycle semantics | `2608` = AIRAC cycle (year 26, cycle 08). Installed on both drives today: `2607`. |
| Serial semantics | The trailing `013712` in the `.dup` filenames matches `CHARTS-013712.key` on both drives — the SkyView chart entitlement ID. |
| Display | 4K at 200% scale, GNOME/Wayland |

Two consequences the design leans on: the previous cycle leaves **27,490** files where the new cycle has **23,103** images, so a merge is not an option — a full replace of `ChartData/Plates` is required and is genuinely destructive; and the entitlement ID is machine-checkable, which gives us a "this data is for a different aircraft" warning nobody else offers.

---

## 1. Product identity

### 1.1 Names and IDs

| Field | Value |
| --- | --- |
| App ID | `io.github.yfilali.DynonUSBUpdater` (fixed) |
| Display name | **Dynon USB Updater** |
| Binary | `dynon-usb-updater` |
| Desktop file | `io.github.yfilali.DynonUSBUpdater.desktop` |
| Metainfo | `io.github.yfilali.DynonUSBUpdater.metainfo.xml` |
| Icons | `io.github.yfilali.DynonUSBUpdater.svg`, `io.github.yfilali.DynonUSBUpdater-symbolic.svg` |
| GResource prefix | `/io/github/yfilali/DynonUSBUpdater/` |
| GSettings schema | `io.github.yfilali.DynonUSBUpdater` |
| Licence | GPL-3.0-or-later (matches `docs/PUBLISHING.md`; `project_license` in the metainfo) |
| Config/state | GSettings only (no hand-rolled JSON) |
| Log files | `$XDG_DATA_HOME/io.github.yfilali.DynonUSBUpdater/logs/` |

**Trademark note (act on this before submitting to Flathub).** "Dynon" and "SkyView" are trademarks of Dynon Avionics. Nominative use in the *description* ("works with Dynon SkyView") is normal and accepted; a trademark in the *display name* is the thing Flathub reviewers occasionally push back on. The app ID is fixed and must never change after publication, but the display name is free to change at any time. Mitigations, in order: (a) ship the disclaimer sentence specified in §1.3 in both the metainfo description and the About dialog; (b) if a reviewer objects, change only `<name>` and the `.desktop` `Name=` to **Cycle Loader** — the ID, schema, and GResource paths stay exactly as they are. Do not block the release on this.

### 1.2 AppStream summary

```
Prepare USB drives for your avionics
```

(35 characters, does not repeat the app name, no trailing period — Flathub quality guidelines.)

### 1.3 AppStream long description

```
Dynon USB Updater copies each AIRAC cycle's aviation and obstacle databases and
the matching approach plates onto the USB drives you carry to the aircraft.

Point it at the folder you downloaded this cycle into. It identifies the newest
aviation and obstacle database by cycle number, reads the plates archive, and
shows you every connected drive with the cycle already installed on it, its free
space, and whether this update will fit. Drives that already look like SkyView
drives are offered first; anything else is left alone unless you deliberately
choose it.

Before writing, it tells you exactly what will be erased and what it will be
replaced with. While it runs — usually about twenty minutes for a full plates
archive — it shows the current step, the file count, and the time remaining for
every drive, and it keeps your session awake so a suspend cannot corrupt a drive
halfway through. Copies are checked by SHA-256 read-back, and every run is
written to a log you can save.

Not affiliated with, endorsed by, or supported by Dynon Avionics. Always confirm
your data is current and correct before flight.
```

Metainfo also carries: `<categories>Utility</categories>`; `<keywords>` Dynon, SkyView, AIRAC, avionics, aviation, charts, plates, obstacle, database, USB, EFIS; `<content_rating type="oars-1.1"/>` (all none); `<launchable type="desktop-id">`; `<developer id="io.github.yfilali">` with the real name; `<url type="homepage|bugtracker|vcs-browser">`; `<branding>` primary colour `#1c71d8` (light) / `#1a5fb4` (dark); `<recommends><control>pointing</control><control>keyboard</control></recommends>`; `<requires><display_length compare="ge">360</display_length></requires>`; four screenshots in this order — ready-to-update, update running, all drives updated, partial failure.

### 1.4 Icon concept

Drawn to the GNOME app-icon template: 128×128 canvas, all content inside a 112×112 safe area, no background plate, subtle inner shading only, GNOME palette colours.

**Metaphor:** an approach plate being loaded into a USB drive — the two nouns the app deals with, in one silhouette.

**Full-colour icon — `io.github.yfilali.DynonUSBUpdater.svg`:**
- **Chart sheet (back layer).** Portrait rounded rectangle, 62×80, rotated −8°, positioned upper-left, centred near (52, 50). Fill `#ffffff`, 1.5px outline `#9a9996` (Light 4). Inside it, a simplified approach plate: a 2px-inset thin border box in `#77767b`; two short horizontal rules at the top in `#c0bfbc` standing for the briefing strip; a magenta (`#c061cb`, Purple 2) 45° course line from lower-left to upper-right ending in a solid arrowhead; one small `#3584e4` circle at the line's midpoint as a fix. Magenta on white is the visual grammar of an instrument approach chart and is what makes the icon read as *aviation* rather than *file*.
- **USB drive (front layer).** Vertical, tilted +8°, occupying the lower-right, roughly 40 wide × 76 tall centred near (82, 82). Body: rounded rect, fill a top-to-bottom linear gradient `#62a0ea` (Blue 2) → `#1c71d8` (Blue 4), 1.5px outline `#1a5fb4` (Blue 5). Connector: a 26×22 rounded rect at the top of the body, fill `#deddda` (Light 3) with a `#9a9996` outline and two 3px-tall `#77767b` contact slots. A 3px `#ffffff` at 25% opacity highlight runs down the body's left edge.
- **Load cue.** A white, 60%-opacity downward chevron (not a full arrow) centred on the drive body, 18px wide, signalling "into the drive". Drop it if it muddies the 64px rendering; the overlap already implies direction.
- **Composition rule.** The drive overlaps the chart's lower-right corner by ~12px and casts a 2px soft shadow (`#000000` at 12%) onto it. Nothing touches the canvas edge.

**Symbolic icon — `io.github.yfilali.DynonUSBUpdater-symbolic.svg`:** 16×16, single `currentColor` path, no fills other than the glyph itself, designed at 16 and checked at 16 (not scaled down from 128). Content: a USB-drive outline — a 7×11 rounded rect with a 5×3 connector notch on top, 1px stroke — with a 5×5 solid downward arrow inside the body. The chart is dropped entirely; at 16px two objects become mud. This icon is used for the app in the shell's symbolic contexts, for drive cards' fallback, and nowhere else.

---

## 2. Information architecture

### 2.1 The shape of the task

Three phases, strictly ordered, one of which is long and destructive:

```
   PREPARE  ──confirm──▶  RUNNING  ──finish──▶  RESULT  ──done──▶  PREPARE
   (form)                 (monitor)             (report)
```

The window is a single `Adw.ApplicationWindow` containing `Adw.ToolbarView` → `Adw.ViewStack` with exactly three pages: `prepare`, `running`, `result`. The stack is not user-navigable; transitions are driven by the state machine in §6.5. There is no `Adw.NavigationView` — there is no "back", and offering one during a 7 GB write would be a lie.

Everything that is *configuration rather than task* (verify, replace old databases, eject when finished) lives in an `Adw.PreferencesDialog`. Everything that is *evidence rather than task* (the log) lives on the running and result pages and in a dedicated log dialog. The prepare page contains only: what will be written, where it will be written, and the button that writes it.

### 2.2 Screen and state inventory

**P — Prepare page states** (all render inside the same page; they differ in the banner slot, the drives section, and the action block)

| ID | State | Trigger |
| --- | --- | --- |
| P0 | First run | No GSettings history |
| P1 | Ready | ≥1 drive selected, all validation passes |
| P2 | No source folder | Saved folder gone/unreadable, or user cleared it |
| P3 | No databases in folder | Folder readable, zero `.dup` found |
| P4 | Only one database kind found | Aviation found, obstacle missing (or vice versa) |
| P5 | No plates archive selected | Valid state — databases-only update |
| P6 | Archive scanning | Async read of the zip central directory in flight |
| P7 | Archive unreadable / not a plates archive | Zip error, or zero usable members after filtering |
| P8 | No drives connected | Enumeration empty, no removable hardware detected, filesystem access confirmed |
| P9 | Cannot see drives (sandbox) | Enumeration empty but removable hardware detected, or `/run/media` unreadable while sandboxed |
| P10 | Drives present, none recognised | Mounts found, none pass the SkyView test |
| P11 | Drives present, none selected | Recognised drives exist but user deselected all, or all are up to date |
| P12 | Selected drive is read-only | `access(W_OK)` fails or mount is `ro` |
| P13 | Selected drive does not fit | Projected bytes > free + reclaimable |
| P14 | Selected drive is up to date | Installed cycle == target cycle |
| P15 | Entitlement mismatch | Drive's `CHARTS-nnnnnn.key` ≠ source `.dup` serial |
| P16 | Manual folder target needs reconnecting | Remembered portal folder no longer accessible |

**R — Running page states**

| ID | State |
| --- | --- |
| R0 | Planning (sizing the job, indeterminate) |
| R1 | Copying databases |
| R2 | Verifying databases |
| R3 | Erasing old plates (**point of no return**) |
| R4 | Extracting plates |
| R5 | Finishing writes (fsync/flush) |
| R6 | Stopping (cancel requested, unwinding) |

**F — Finished page states**

| ID | State |
| --- | --- |
| F0 | All drives updated |
| F1 | Some drives updated, some failed |
| F2 | No drives updated |
| F3 | Stopped by the user, with at least one drive left incomplete |

**D — Dialogs**

| ID | Dialog |
| --- | --- |
| D1 | Confirm update (`Adw.AlertDialog`) |
| D2 | Confirm stop / close during run (`Adw.AlertDialog`) |
| D3 | Preferences (`Adw.PreferencesDialog`) |
| D4 | Keyboard shortcuts (`Gtk.ShortcutsWindow`) |
| D5 | About (`Adw.AboutDialog`) |
| D6 | Archive contents preview (`Adw.Dialog`) |
| D7 | Activity log (`Adw.Dialog`) |
| D8 | Choose file / folder (`Gtk.FileDialog`, portal-backed) |
| D9 | Sandbox permission help (`Adw.AlertDialog`) |

---

## 3. Screens

Default window: **820 × 700** logical px. Minimum: **360 × 480**. Content clamped to **`Adw.Clamp` maximum-size 720, tightening-threshold 560**. Window size and maximised state persist in GSettings.

### 3.1 Prepare — P1 Ready (the target state)

```
┌──────────────────────────────────────────────────────────────────────────────┐
│  ☰                        Dynon USB Updater                        ─  □  ✕  │
├──────────────────────────────────────────────────────────────────────────────┤
│                                                                              │
│   ┌────────────────────────────────────────────────────────────────────┐    │
│   │ ⚠  DYNON2 is registered to a different SkyView.        [ Details ] │    │
│   └────────────────────────────────────────────────────────────────────┘    │
│                                                                              │
│   Update Files                                        ~/Downloads  [Change…] │
│   ┌────────────────────────────────────────────────────────────────────┐    │
│   │  Aviation Database                                Cycle 2608   [⋯] │    │
│   │  airmate_av_data_us_2608_013712.dup · 8.2 MB                       │    │
│   ├────────────────────────────────────────────────────────────────────┤    │
│   │  Obstacle Database                                Cycle 2608   [⋯] │    │
│   │  airmate_obstacle_data_us_2608_013712.dup · 1.9 MB                 │    │
│   ├────────────────────────────────────────────────────────────────────┤    │
│   │  Approach Plates                                  Cycle 2608   [⋯] │    │
│   │  US-Plates-2608.zip · 23,104 files · 7.3 GB                        │    │
│   └────────────────────────────────────────────────────────────────────┘    │
│                                                                              │
│   Drives                                        [ Choose Folder… ]  [ ↻ ]   │
│   ┌──────────────────────────────┐   ┌──────────────────────────────┐       │
│   │  ✔                       ▮   │   │                          ▮   │       │
│   │  DYNON                       │   │  DYNON2                      │       │
│   │  Cycle 2607 → 2608           │   │  Cycle 2607 → 2608           │       │
│   │  ▓▓▓▓▓▓▓▓▓▓▒▒▒▒▒░░░░░░░░░░   │   │  ▓▓▓▓▓▓▓▒▒▒▒▒▒▒▒░░░░░░░░░░   │       │
│   │  11.1 GB free · fits         │   │  ⚠ Different SkyView         │       │
│   └──────────────────────────────┘   └──────────────────────────────┘       │
│                                                                              │
│                      ╭────────────────────────────╮                          │
│                      │       Update DYNON         │                          │
│                      ╰────────────────────────────╯                          │
│         Writes 7.3 GB to DYNON · about 20 minutes · copies verified          │
│                                                                              │
└──────────────────────────────────────────────────────────────────────────────┘
```

**Visible without scrolling at 820 × 700:** header bar, banner slot (when populated), the complete Update Files group, the Drives heading, one full row of drive cards, the primary button, and the summary line beneath it. The page scrolls only when a banner is present *and* there are more than three drives, or when the window is shorter than 620 px.

**Widget breakdown, outside in:**

| Widget | Details |
| --- | --- |
| `Adw.ApplicationWindow` | `default-width 820`, `default-height 700`, `width-request 360`, `height-request 480` |
| └ `Adw.ToastOverlay` | Wraps everything; hosts all toasts |
| └ `Adw.ToolbarView` | `top-bar-style: raised-border` |
| ├ `Adw.HeaderBar` | Title widget `Adw.WindowTitle`: title `Dynon USB Updater`, subtitle **empty on the prepare page** (the subtitle is reserved for run progress; it is never used to explain validation). `pack_start`: `Gtk.MenuButton` icon `open-menu-symbolic`, `primary=true`, tooltip `Main Menu`, menu model §4.3. No other header buttons — the primary action is in the content. |
| └ `Adw.ViewStack` | `transition-type: crossfade`, 200 ms. Page `prepare` follows. |

Prepare page = `Gtk.ScrolledWindow` (`hscrollbar-policy: never`) → `Adw.Clamp` (max 720) → `Gtk.Box` vertical, `spacing 24`, margins `24 / 24 / 24 / 24` (`margin-bottom 32` so the summary line never kisses the frame).

1. **Banner slot** — `Adw.Banner`, `revealed` bound to the highest-priority active advisory (§7.3). `title` set per §4.6, `button-label` where an action exists. Exactly one banner at a time; priority order is fixed in §7.3. Banners are for *advisories the user may act on*, never for the routine "nothing selected yet" case.

2. **Update Files** — `Adw.PreferencesGroup`, `title` `Update Files`.
   `header-suffix`: `Gtk.Box` horizontal, `spacing 6`, `valign center` — a `Gtk.Label` with the abbreviated source path (`~/Downloads`), classes `.caption .dimmed`, `ellipsize start`, `max-width-chars 24`, tooltip = full path; and a `Gtk.Button` `Change…`, class `.flat`.
   Three `Adw.ActionRow`s, **no prefix icons** (the group title already establishes what these are; symbolic glyphs here carry no information and create a dead vertical stripe):
   - *Aviation Database* — `title` `Aviation Database`; `subtitle` `{filename} · {size}`, `subtitle-lines 1`, ellipsize middle; suffix `Gtk.Label` `Cycle 2608` classes `.heading .accent`, then `Gtk.MenuButton` icon `view-more-symbolic` class `.flat`, menu §4.4. `activatable-widget` = the menu button.
   - *Obstacle Database* — identical structure.
   - *Approach Plates* — identical structure; the label shows the cycle parsed from the archive name when one is present, otherwise nothing. Subtitle carries `{archive} · {n} files · {size}` with `n` group-separated per locale.
   When a kind is absent or deliberately excluded the value label becomes `Not copying` with class `.dimmed`, and the subtitle states why (§4.5).

3. **Drives** — not a `PreferencesGroup`; a hand-built section so the cards are not forced into rows:
   - Header line: `Gtk.Box` horizontal — `Gtk.Label` `Drives` class `.heading`, `hexpand start`; `Gtk.Button` `Choose Folder…` class `.flat` (see §3.3 — this is a first-class affordance, not a `+`); `Gtk.Button` icon `view-refresh-symbolic` class `.flat`, tooltip `Rescan Drives`, accessible label `Rescan drives`.
   - `Gtk.FlowBox`: `selection-mode: none` (selection is owned by the toggle in each child, so keyboard focus and selection never disagree), `homogeneous true`, `min-children-per-line 1`, `max-children-per-line 3`, `row-spacing 12`, `column-spacing 12`, `activate-on-single-click false`.
   - Each child: `Gtk.ToggleButton`, class `.card`, `width-request 236`, `height-request 132`, containing `Gtk.Box` vertical `spacing 6`, margins 12:
     - Top line `Gtk.Box` horizontal: `Gtk.Image` `object-select-symbolic` class `.success`, `visible` bound to `active` (the selection tick); `Gtk.Label` hexpand spacer; `Gtk.Image` 24px `drive-removable-media-symbolic` (or `folder-symbolic` for folder targets), class `.dimmed`.
     - `Gtk.Label` volume name, class `.title-4`, `xalign 0`, ellipsize end, single line.
     - `Gtk.Label` cycle line, class `.caption`, `xalign 0` — text and semantic class per §4.7.
     - `Gtk.LevelBar`, `mode: continuous`, `height-request 6`, `hexpand`, value = *projected* fraction used after this update, with `add_offset_value("high", 0.90)` and `("full", 1.0)` so an overrun renders in the theme's warning/error colour with no hardcoded hex.
     - `Gtk.Label` capacity/verdict line, class `.caption`, plus `.dimmed`, `.warning`, or `.error` per state.
   - **Custom drawing:** none is required. A two-segment "already used / this update adds" bar would be more expressive, and if it is built it must be a `Gtk.DrawingArea` that reads its two colours from `Adw.StyleManager`'s accent colour and `@borders` via `Gtk.StyleContext`, exposes `Gtk.AccessibleRole::ProgressBar` with `VALUE_TEXT`, and falls back to the stock `Gtk.LevelBar` above under `Gtk.Settings::gtk-high-contrast`. Ship the `LevelBar`; treat the two-segment bar as a post-1.0 enhancement. The projected-fraction `LevelBar` already answers the only question that matters ("will it fit"), because the offsets recolour it automatically when it will not.

4. **Action block** — `Gtk.Box` vertical, `spacing 8`, `halign center`, `margin-top 8`:
   - `Gtk.Button`, classes `.pill .suggested-action`, `Adw.ButtonContent` is **not** used (no icon; the label is the message). Label per §4.8. `width-request 220`.
   - `Gtk.Label` beneath, classes `.caption`, `justify center`, `wrap true`, `max-width-chars 52`. In the ready state it is `.dimmed` and describes the work; when the button is insensitive it is **`.warning`** and states the blocking reason. This label is `Gtk.AccessibleRelation::DESCRIBED_BY` on the button, so a screen reader that lands on a disabled button always hears why. **Requirement: the button is never insensitive without this label being populated.**

### 3.2 Prepare — P8 No drives connected

```
│   Drives                                        [ Choose Folder… ]  [ ↻ ]   │
│   ┌────────────────────────────────────────────────────────────────────┐    │
│   │                                                                    │    │
│   │                              ▮▮▮                                   │    │
│   │                                                                    │    │
│   │                        No Drives Connected                         │    │
│   │                                                                    │    │
│   │      Plug in the USB drive you use with your SkyView. It will      │    │
│   │              appear here automatically.                            │    │
│   │                                                                    │    │
│   │                     ┌──────────────────────┐                       │    │
│   │                     │   Choose Folder…     │                       │    │
│   │                     └──────────────────────┘                       │    │
│   └────────────────────────────────────────────────────────────────────┘    │
```

The `FlowBox` is replaced (same slot, `Gtk.Stack` with `crossfade` 150 ms) by `Adw.StatusPage`: `icon-name: drive-removable-media-symbolic`, `title`, `description`, and a child `Gtk.Button` `Choose Folder…` with classes `.pill` (no `.suggested-action` — the suggested action on this page is still Update). `vexpand false`; the status page is inset in the section, not the whole window, so the Update Files group stays visible and the user can keep working. Primary button is insensitive with the reason `Plug in a USB drive, or choose a drive folder, to continue`.

### 3.3 Prepare — P9 Cannot see drives (sandbox-degraded)

This state exists because in a Flatpak the API cannot distinguish "nothing plugged in" from "not allowed to look". The app **must** distinguish them, using evidence outside the mount table.

**Detection algorithm (run on every rescan):**

1. `sandboxed = Path::new("/.flatpak-info").exists()`.
2. If sandboxed, parse `/.flatpak-info` `[Context] filesystems=` and record whether it grants `/run/media`, `host`, or `host-os`. This is authoritative for what was *granted*.
3. `media_roots_visible` = any of `/run/media/$USER`, `/run/media`, `/media/$USER`, `/media`, `/mnt` exists **and** is readable.
4. `hardware_present` = scan `/sys/class/block/*`, which is mounted read-only inside the sandbox: a device qualifies if `removable` reads `1`, or its `device` symlink path contains `/usb`, and it has at least one partition child with a non-zero `size`. This detects that a USB mass-storage device is attached even when its mount point is invisible.
5. Classify:
   - mounts found → normal operation.
   - no mounts, `!sandboxed`, `!hardware_present` → **P8**.
   - no mounts, `sandboxed`, and (`!granted(/run/media)` or `!media_roots_visible` or `hardware_present`) → **P9**.
   - no mounts, `!sandboxed`, `hardware_present` → **P9 variant**: the drive is attached but not mounted; wording differs (§4.6).

```
│   Drives                                        [ Choose Folder… ]  [ ↻ ]   │
│   ┌────────────────────────────────────────────────────────────────────┐    │
│   │                              ⚠                                     │    │
│   │                     Can't See Your Drives                          │    │
│   │                                                                    │    │
│   │    A USB drive is connected, but this app has not been given        │    │
│   │    permission to read it. Choose the drive's folder to continue,    │    │
│   │    or grant permission once and it will appear on its own.          │    │
│   │                                                                    │    │
│   │        ┌──────────────────────┐   ┌──────────────────────┐         │    │
│   │        │   Choose Folder…     │   │  How to Fix This…    │         │    │
│   │        └──────────────────────┘   └──────────────────────┘         │    │
│   └────────────────────────────────────────────────────────────────────┘    │
```

`Adw.StatusPage` `icon-name: dialog-warning-symbolic`, child = `Gtk.Box` horizontal `spacing 12` `halign center` containing `Choose Folder…` (`.pill .suggested-action` — here it *is* the way forward) and `How to Fix This…` (`.pill`) which opens **D9**.

**D9 Sandbox permission help** — `Adw.AlertDialog`, heading `Allow Access to USB Drives`, body per §4.6, `extra-child` = a `Gtk.Box` containing a selectable, `.monospace` `Gtk.Label` holding the exact command, and a `Gtk.Button` `Copy Command` (`.flat`, `edit-copy-symbolic`). Command text:

```
flatpak override --user --filesystem=/run/media io.github.yfilali.DynonUSBUpdater
```

Responses: `close` → `Close` (default, close response). No "run it for me" — the app cannot and must not escalate its own sandbox.

**Choose Folder… — the portal fallback, designed as a real feature.**
This path always works, because the `org.freedesktop.portal.FileChooser` grant covers whatever the user picks, sandbox permissions notwithstanding. It is therefore not a debug hatch; it is the supported route on a locked-down system, and it is present in *four* places: the Drives section header (always), the P8/P9/P10 status pages, the main menu (`Choose Drive Folder…`, `Ctrl+Shift+D`), and the keyboard shortcuts window.

- Opens `Gtk.FileDialog::select_folder` with title `Select Your Drive`, initial folder `/run/media/$USER` if readable, else `$HOME`.
- On selection: `statfs` the path for capacity, run the SkyView recognition test (§6.2) on it exactly as for a mounted drive, and add it to the card grid as a **folder target**: `folder-symbolic` icon, name = the folder's basename (`DYNON` — never the doc-portal path, which is unreadable noise like `/run/user/1000/doc/a1b2c3/DYNON`), cycle/space/fit lines identical to a real drive. Tooltip carries the real path.
- Folder targets differ from mounted drives in exactly two ways: they show a `Remove` item in their card menu, and they offer no eject affordance on the result page.
- Persistence: the chosen path is stored in GSettings `manual-targets`. On the next launch the app re-`statfs`es it; if it is no longer reachable (portal grant expired, drive not plugged in) the card renders as **P16**: title unchanged, `.dimmed`, cycle line replaced by `Not connected`, and the card's only action is `Choose Again…`. It is never silently dropped and never auto-selected while unreachable.

### 3.4 Prepare — P10 Drives present, none recognised

Cards render normally, all unselected, each showing the `Not a SkyView drive` caution (§4.7). Above the FlowBox an `Adw.Banner`: `No SkyView drives found. Select a drive to write to it anyway.` with no button. The primary button is insensitive with `Select a drive to continue`. This is the state the safety default produces when someone has a photo stick plugged in — and it is deliberately a little uncomfortable.

### 3.5 Running

```
┌──────────────────────────────────────────────────────────────────────────────┐
│  ☰                        Dynon USB Updater                        ─  □  ✕  │
│                     37% — about 14 minutes left                              │
├──────────────────────────────────────────────────────────────────────────────┤
│                                                                              │
│                                                                              │
│                                  37%                                         │
│                                                                              │
│              ▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░                │
│                                                                              │
│                       Extracting plates to DYNON                             │
│                          8,421 of 23,104 files                               │
│                        About 14 minutes left                                 │
│                                                                              │
│   ┌────────────────────────────────────────────────────────────────────┐    │
│   │  DYNON                                          Extracting plates  │    │
│   │  ▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓░░░░░░░░░░░░░░░░                              │    │
│   ├────────────────────────────────────────────────────────────────────┤    │
│   │  DYNON2                                                   Waiting  │    │
│   └────────────────────────────────────────────────────────────────────┘    │
│                                                                              │
│   ▸ Details                                                                  │
│                                                                              │
│                          ┌──────────────┐                                    │
│                          │    Cancel    │                                    │
│                          └──────────────┘                                    │
└──────────────────────────────────────────────────────────────────────────────┘
```

- Header bar: same menu button; menu items that mutate state (`Choose Drive Folder…`, `Rescan Drives`, `Preferences`) are **insensitive**, not hidden, so the menu does not reshuffle mid-run. `Adw.WindowTitle` subtitle becomes `37% — about 14 minutes left`, so the state is legible from the shell's window list and overview.
- Content: `Adw.Clamp` (max 560) → `Gtk.Box` vertical `spacing 12`, `valign center`, margins 24.
  - `Gtk.Label` percentage — classes `.title-1 .numeric`. `.numeric` is mandatory: without tabular figures the digits jitter every tick.
  - `Gtk.ProgressBar` — `show-text false`, `hexpand`. During R0 it `pulse()`s at 100 ms and the percentage label reads `—`.
  - `Gtk.Label` step — class `.title-4`, `wrap`, `justify center`. Text per §4.9.
  - `Gtk.Label` detail — classes `.body .dimmed .numeric`, e.g. `8,421 of 23,104 files`, or the byte counter during database copy.
  - `Gtk.Label` ETA — classes `.caption .dimmed .numeric`.
  - Per-drive `Adw.PreferencesGroup` with one `Adw.ActionRow` per drive in the job: `title` = drive name; suffix = a state `Gtk.Label` (`.caption`, semantic class per state) or, for the active drive, an `Adw.Spinner` (libadwaita 1.7+; do not use the deprecated `Gtk.Spinner`); the active row additionally shows a thin `Gtk.ProgressBar` (`height-request 4`) as its `Adw.ActionRow` subtitle slot replacement via `add_suffix` on a vertical box. Completed rows show `emblem-ok-symbolic` `.success`; failed rows show `dialog-error-symbolic` `.error` and the reason as the row subtitle.
  - `Adw.ExpanderRow` `Details` (collapsed by default; expansion state persists for the session only), containing the log list (§5.3) in a `Gtk.ScrolledWindow` `min-content-height 180`, `max-content-height 300`, `propagate-natural-height true`, plus header-suffix buttons `Copy` and `Save…`.
  - `Gtk.Button` `Cancel` — class `.pill` only. **Not** `.destructive-action`: stopping is not the destructive act here; the update is. Colouring Cancel red teaches the wrong lesson about which button is dangerous.
- The prepare form is not present in any form. Nothing configurable is on screen.

### 3.6 Result

```
┌──────────────────────────────────────────────────────────────────────────────┐
│  ☰                        Dynon USB Updater                        ─  □  ✕  │
├──────────────────────────────────────────────────────────────────────────────┤
│                                                                              │
│                                    ✓                                         │
│                                                                              │
│                            2 Drives Updated                                  │
│                                                                              │
│           Cycle 2608 is installed. Eject the drives before unplugging.       │
│                                                                              │
│   ┌────────────────────────────────────────────────────────────────────┐    │
│   │  ✓  DYNON       Cycle 2608 · 23,104 plates · 9 min 41 s            │    │
│   ├────────────────────────────────────────────────────────────────────┤    │
│   │  ✓  DYNON2      Cycle 2608 · 23,104 plates · 9 min 12 s            │    │
│   └────────────────────────────────────────────────────────────────────┘    │
│                                                                              │
│              ╭──────────────────╮   ┌──────────────────┐                     │
│              │    Eject Both    │   │      Done        │                     │
│              ╰──────────────────╯   └──────────────────┘                     │
│                                                                              │
│   ▸ Details                                                                  │
└──────────────────────────────────────────────────────────────────────────────┘
```

`Adw.StatusPage` in a `Gtk.ScrolledWindow`. `icon-name`, `title`, `description` per state (§4.10). Child box holds:
- `Adw.PreferencesGroup` of per-drive `Adw.ActionRow`s: prefix `Gtk.Image` (`emblem-ok-symbolic .success` / `dialog-error-symbolic .error` / `dialog-warning-symbolic .warning`), title = drive name, subtitle = outcome detail or failure reason (`wrap`, up to 2 lines). Failed rows carry a suffix `Gtk.Button` `Retry` (`.flat`) that re-enters the confirm flow for that drive alone.
- Action row of buttons, `halign center`, `spacing 12`: `Eject Both` / `Eject DYNON` (`.pill .suggested-action`, only for mounted drives that succeeded, never for folder targets) and `Done` (`.pill`).
- `Adw.ExpanderRow` `Details` — expanded by default in F1/F2/F3, collapsed in F0.

Nothing on this page auto-dismisses. The page persists until `Done`, `Retry`, or window close.

### 3.7 Dialogs

**D1 Confirm update** — `Adw.AlertDialog`.
- `heading` and `body` per §4.11. Body is **one or two plain sentences, no markup, no bullets** — `AlertDialog` bodies are centre-aligned and bulleted lists render as ragged garbage there.
- `extra-child`: `Adw.PreferencesGroup` (left-aligned, which is the entire point) with one `Adw.ActionRow` per drive: title = drive name, subtitle = the per-drive action sentence (§4.11), prefix icon `drive-removable-media-symbolic` or `folder-symbolic`. Mount paths never appear here; they live in each row's tooltip.
- If any selected drive failed the SkyView recognition test, the `extra-child` additionally contains a `Gtk.CheckButton` with label `I understand that DATA-BACKUP is not a SkyView drive` (name substituted; for several, `…that 2 of these are not SkyView drives`), and the destructive response stays **insensitive until it is checked**. This is the second lock on the safety default.
- Responses: `cancel` → `Cancel`; `update` → `Replace Plates` (`DESTRUCTIVE`) when an archive is included, or `Update` (`SUGGESTED`) when it is databases-only. `default-response: cancel`, `close-response: cancel`.

**D3 Preferences** — `Adw.PreferencesDialog`, one page `General`, one group `Updating`, three `Adw.SwitchRow`s (§4.12), plus a group `Logs` with one `Adw.ActionRow` `Activity Logs` / subtitle `Every run is saved here` / suffix `Gtk.Button` `Open Folder` (`.flat`, `folder-open-symbolic`) using `Gtk.FileLauncher::open_containing_folder`.

**D6 Archive contents** — `Adw.Dialog`, `content-width 520`, `content-height 480`, title `Archive Contents`. `Adw.ToolbarView` with `Adw.HeaderBar`; content = `Adw.PreferencesGroup` `description` = `Files will be written to ChartData/Plates on each drive.`, an `Adw.SwitchRow` `Remove Extra Folder` (visible **only** when a single wrapping folder is detected; §4.5), then a `Gtk.ListBox` `.boxed-list` of the first 20 resulting relative destination paths as single-line `.monospace .caption` labels, then a `.caption .dimmed` footer `…and 23,084 more · 12 junk files skipped`. The list updates live when the switch flips. This is the "preview of resulting destination paths" requirement, and it is the only place the wrapper option appears — it is contextual to an archive, not a global preference.

**D7 Activity log** — `Adw.Dialog`, `content-width 700`, `content-height 560`, title `Activity Log`, header-bar `pack_end` buttons `Copy` (`.flat`, `edit-copy-symbolic`) and `Save…` (`.flat`, `document-save-symbolic`). Content = the same log list widget as the running page's Details, unbounded height. Opened with `Ctrl+L` from anywhere.

---

## 4. Copy deck

Every user-visible string. GNOME HIG capitalisation: **Header Capitalization** for window/dialog/group titles, buttons, menu items, and switch/row titles; **sentence case** for descriptions, subtitles, banners, status text, and tooltips. Placeholders in `{braces}`. Numbers are locale-formatted with group separators. Sizes use `g_format_size` (SI: GB, MB) — never a hand-rolled 1024-based "GB".

### 4.1 Window and identity

| Key | String |
| --- | --- |
| Window title | `Dynon USB Updater` |
| Window subtitle (prepare) | *(empty)* |
| Window subtitle (running) | `{percent}% — {eta}` |
| Window subtitle (result) | *(empty)* |
| Desktop `Comment` | `Prepare USB drives for your avionics` |
| About: comments | `Copies each AIRAC cycle's aviation and obstacle databases and approach plates onto the USB drives you carry to the aircraft.` |
| About: legal footer | `Not affiliated with, endorsed by, or supported by Dynon Avionics.` |

### 4.2 First run (P0)

Dismissible `Adw.Banner`, shown once (`first-run-done`), no button:

> `Choose this cycle's files, pick the drive you fly with, then update.`

### 4.3 Main menu

```
Choose Drive Folder…            Ctrl+Shift+D
Rescan Drives                   Ctrl+R
─────────────────────────────
Activity Log                    Ctrl+L
Preferences                     Ctrl+comma
Keyboard Shortcuts              Ctrl+question
About Dynon USB Updater
```

### 4.4 Source row menus (`⋯`)

Aviation / Obstacle Database:
```
Choose a Different File…
Do Not Copy
─────────────────────────────
Show in Files
```
Approach Plates:
```
Choose a Different Archive…
Preview Contents…
Do Not Replace Plates
─────────────────────────────
Show in Files
```
When no archive is selected the plates menu is replaced by a single item `Choose an Archive…`.

### 4.5 Source rows — every subtitle and value

| Condition | Value label | Subtitle |
| --- | --- | --- |
| Database found | `Cycle {cycle}` `.heading .accent` | `{filename} · {size}` |
| Database found, no cycle parseable | `{date}` `.heading .accent` | `{filename} · {size}` |
| No file of this kind in folder | `Not found` `.dimmed` | `No {aviation\|obstacle} database in {folder}` |
| User chose "Do Not Copy" | `Not copying` `.dimmed` | `This database will be left as it is on the drive` |
| Archive selected | `Cycle {cycle}` or *(empty)* | `{filename} · {n} files · {size}` |
| No archive selected | `Not replacing` `.dimmed` | `Plates already on the drives will be left as they are` |
| Archive being read | *(empty)* + `Adw.Spinner` | `Reading {filename}…` |
| Archive unreadable | `Can't read` `.error` | `This file is not a readable zip archive` |
| Archive has no usable files | `Empty` `.error` | `This archive contains no plate files` |
| Archive has a wrapping folder | `Cycle {cycle}` | `{filename} · {n} files · {size} · in a folder named {folder}` |

`Remove Extra Folder` switch (D6 only) — title `Remove Extra Folder`, subtitle `Everything in this archive sits inside “{folder}”. Remove it so plates land directly in ChartData/Plates.` Default **off**; flattening a genuine data folder is worse than leaving a wrapper.

### 4.6 Banners and empty states

| ID | Title / body | Button |
| --- | --- | --- |
| P2 | `The folder {folder} is no longer available.` | `Choose Folder…` |
| P3 | `No databases found in {folder}.` | `Choose Folder…` |
| P4 | `Only the {aviation\|obstacle} database was found in {folder}.` | `Choose Folder…` |
| P7 | `{filename} could not be read as a plates archive.` | `Choose Another…` |
| P10 | `No SkyView drives found. Select a drive to write to it anyway.` | — |
| P13 | `{drive} does not have room for this update.` | `Deselect` |
| P12 | `{drive} cannot be written to.` | `Deselect` |
| P15 | `{drive} is registered to a different SkyView.` | `Details` |
| P16 | `{name} is not connected.` | `Choose Again…` |

P8 status page — title `No Drives Connected`; description `Plug in the USB drive you use with your SkyView. It will appear here automatically.`; button `Choose Folder…`.

P9 status page (sandboxed, permission missing) — title `Can't See Your Drives`; description `A USB drive is connected, but this app has not been given permission to read it. Choose the drive's folder to continue, or grant permission once and it will appear on its own.`; buttons `Choose Folder…`, `How to Fix This…`.

P9 variant (not sandboxed, hardware present, nothing mounted) — title `Drive Not Ready`; description `A USB drive is connected but has not been mounted yet. Open it once in Files, or choose its folder here.`; buttons `Choose Folder…`, `Rescan`.

D9 — heading `Allow Access to USB Drives`; body `This app is sandboxed and cannot list removable drives until it is given permission. Run this command in a terminal, then restart the app. Choosing a drive folder works without it.`; response `Close`.

P15 `Details` opens an `Adw.AlertDialog`: heading `Registered to a Different SkyView`; body `{drive} carries a chart key for SkyView {drive_serial}, but this cycle's files are for {file_serial}. If this drive belongs to another aircraft, its charts may not open. Check before you use it.`; responses `Deselect Drive` (default) and `Use It Anyway`.

### 4.7 Drive card states

| State | Cycle line | Verdict line |
| --- | --- | --- |
| Recognised, older cycle | `Cycle {old} → {new}` `.caption` | `{free} free · fits` `.dimmed` |
| Recognised, up to date | `Cycle {new} installed` `.caption .success` | `Already up to date` `.dimmed` |
| Recognised, no databases | `No databases installed` `.caption .dimmed` | `{free} free · fits` `.dimmed` |
| Not recognised | `Not a SkyView drive` `.caption .warning` | `{free} free · {total} total` `.dimmed` |
| Read-only | `Cycle {old}` `.caption` | `Read-only — can't be written to` `.error` |
| Won't fit | `Cycle {old} → {new}` `.caption` | `Needs {needed}, {available} available` `.error` |
| Different SkyView | `Cycle {old} → {new}` `.caption` | `Registered to SkyView {serial}` `.warning` |
| Folder target | `Folder on this computer` `.caption .dimmed` | `{free} free · fits` `.dimmed` |
| Folder target, unreachable | `Not connected` `.caption .dimmed` | `Choose this folder again to use it` `.dimmed` |
| Being scanned | `Checking…` `.caption .dimmed` + `Adw.Spinner` | *(empty)* |

Card menu (right-click / `Menu` key): `Deselect` or `Select`; `Show in Files`; and for folder targets `Choose Again…`, `Remove`.
Card tooltip: `{mount path}` for drives, `{real path}` for folder targets.

### 4.8 Primary button and its reason label

Button label:

| Selection | Label |
| --- | --- |
| 1 drive, name ≤ 16 chars | `Update {name}` |
| 1 drive, longer name | `Update 1 Drive` |
| n > 1 | `Update {n} Drives` |
| 0 drives | `Update Drives` (insensitive) |

Reason label under the button, **exactly one**, evaluated in this fixed priority order:

1. `Choose a folder that contains this cycle's files`
2. `No aviation or obstacle database found in {folder}`
3. `Choose at least one database or a plates archive to copy`
4. `Still reading {filename}…`
5. `Plug in a USB drive, or choose a drive folder, to continue`
6. `Select a drive to continue`
7. `{drive} can't be written to — deselect it to continue`
8. `{drive} doesn't have room for this update — deselect it to continue`

Ready-state summary (`.dimmed`), composed from parts:

> `Writes {total} to {drive list} · about {duration} · copies verified`

- `{drive list}`: `DYNON`, `DYNON and DYNON2`, or `3 drives`.
- `· copies verified` is appended only when the verify preference is on.
- `· old databases replaced` is appended only when that preference is on and the drive has older `.dup` files.
- Databases-only example: `Writes 10.2 MB to DYNON and DYNON2 · about 30 seconds · copies verified`.

Duration estimate before the run: `bytes ÷ 12 MB/s`, rounded per §4.9's vocabulary, with the words `about`. 12 MB/s is a deliberately pessimistic USB 2.0-class figure for many small files; over-promising here is worse than under-promising.

### 4.9 Running copy

Step lines (`.title-4`), where `{drive}` is the drive currently being written:

| State | Step line | Detail line |
| --- | --- | --- |
| R0 | `Preparing…` | `Checking space on {n} drives` |
| R1 | `Copying aviation database to {drive}` | `{done} of {total}` (bytes) |
| R1 | `Copying obstacle database to {drive}` | `{done} of {total}` |
| R2 | `Checking the copy on {drive}` | `Comparing checksums` |
| R3 | `Erasing old plates on {drive}` | `{n} files removed` |
| R4 | `Extracting plates to {drive}` | `{done} of {total} files` |
| R5 | `Finishing writes to {drive}` | `Do not unplug the drive` |
| R6 | `Stopping…` | `Finishing the current file` |

ETA line, matching GNOME Files' vocabulary:

| Condition | String |
| --- | --- |
| < 20 s of samples | `Estimating time left…` |
| < 60 s | `Less than a minute left` |
| 1 min | `About 1 minute left` |
| < 60 min | `About {n} minutes left` |
| ≥ 60 min | `About {h} hours {m} minutes left` |

ETA is computed from a 30-second rolling throughput window, refreshed at most once per second, and is **monotonically damped**: it may rise, but never by more than 20% in one update, to stop it flickering when a drive stalls.

Per-drive row states: `Waiting`, `Copying databases`, `Checking copies`, `Erasing plates`, `Extracting plates`, `Finishing`, `Done`, `Failed`, `Skipped`, `Stopped`.

### 4.10 Result copy

| State | Icon | Title | Description |
| --- | --- | --- | --- |
| F0, n>1 | `emblem-ok-symbolic` | `{n} Drives Updated` | `Cycle {cycle} is installed. Eject the drives before unplugging them.` |
| F0, n=1 | `emblem-ok-symbolic` | `{drive} Updated` | `Cycle {cycle} is installed. Eject the drive before unplugging it.` |
| F1 | `dialog-warning-symbolic` | `{ok} of {total} Drives Updated` | `{failed list} was not updated. Check the reason below before you fly.` |
| F2 | `dialog-error-symbolic` | `No Drives Were Updated` | `Nothing was written. Check the reason below.` |
| F3 | `dialog-warning-symbolic` | `Update Stopped` | `{drive}'s plates were being replaced when you stopped. Its plates folder is incomplete — run the update again before you fly with it.` |

Per-drive result subtitles: `Cycle {cycle} · {n} plates · {duration}` on success; the error sentence from §7 on failure; `Not started` for skipped drives; `Interrupted — plates folder is incomplete` for the stopped drive.

Buttons: `Eject Both` / `Eject All` / `Eject {drive}`; `Retry`; `Done`.

### 4.11 Confirmation dialog (D1)

With an archive:
- heading: `Replace Plates on {n} Drives?` (or `Replace Plates on {drive}?`)
- body: `The plates folder on {drive list} will be erased and rebuilt from {archive}. This takes about {duration} and cannot be undone.`

Databases only:
- heading: `Update {n} Drives?`
- body: `The aviation and obstacle databases on {drive list} will be replaced. Plates are left as they are.`

Per-drive `extra-child` rows:
- title `{drive}`
- subtitle, assembled: `Copy 2 databases · erase 27,490 plates · write 23,104 plates` — counts are real, from the scan.
- for an unrecognised drive the subtitle is prefixed `Not a SkyView drive · `.

Checkbox (only when an unrecognised drive is included): `I understand that {drive} is not a SkyView drive` / `I understand that {n} of these are not SkyView drives`.

Responses: `Cancel` · `Replace Plates` (destructive) or `Update` (suggested).

### 4.12 Stop / close guard (D2)

Before the erase begins (R0–R2): no dialog. Cancel stops immediately; toast `Update stopped. Nothing was changed on your drives.`

After the erase begins (R3–R5), for both the Cancel button and any window-close attempt:
- heading: `Stop the Update?`
- body: `{drive}'s plates folder has already been erased. Stopping now leaves it incomplete, and your SkyView will not find your approach plates until you run the update again.`
- responses: `Keep Updating` (default, close response) · `Stop Update` (destructive).

### 4.13 Preferences (D3)

| Row | Title | Subtitle | Default |
| --- | --- | --- | --- |
| Switch | `Verify Copies` | `Read each database back from the drive and compare checksums. Adds about a minute.` | on |
| Switch | `Replace Older Databases` | `Delete previous cycles from the drive so your SkyView sees only the new one.` | on |
| Switch | `Eject When Finished` | `Eject each drive automatically after it is updated successfully.` | off |
| Action | `Activity Logs` | `Every run is saved here` | — |

Cut deliberately: a preference for the archive wrapper (contextual, lives in D6), a preference for the entitlement warning (an option to disable a safety warning is an option to be surprised in the cockpit), a "confirm before writing" toggle (never optional), and a theme selector (the platform's job).

### 4.14 Tooltips

Only where a control has no visible label:

| Control | Tooltip |
| --- | --- |
| Menu button | `Main Menu` |
| Rescan button | `Rescan drives` |
| Source row `⋯` | `More options` |
| Log `Copy` | `Copy the log to the clipboard` |
| Log `Save…` | `Save the log to a file` |
| Drive card | `{path}` |
| Source path label | `{full folder path}` |

No tooltip ever carries information that exists nowhere else — a tooltip is unreachable by keyboard and unspoken to a screen reader that is not hovering.

### 4.15 Toasts

Transient confirmations only, never outcomes of the run:

| Event | Toast | Button |
| --- | --- | --- |
| Folder target added | `{name} added` | `Undo` |
| Folder target removed | `{name} removed` | `Undo` |
| Archive removed | `Plates will not be replaced` | `Undo` |
| Log copied | `Log copied to clipboard` | — |
| Log saved | `Log saved` | `Open` |
| Drive ejected | `{drive} can be unplugged` | — |
| Eject failed | `Could not eject {drive} — eject it from Files` | — |
| Update stopped pre-erase | `Update stopped. Nothing was changed on your drives.` | — |

All toasts `timeout 5` except `Could not eject` (`timeout 0`, dismissible), because it requires the user to do something.

---

## 5. Type scale and colour

### 5.1 Typography

| Where | Classes |
| --- | --- |
| Running percentage | `.title-1 .numeric` |
| Result page title | *(from `Adw.StatusPage`)* |
| Drive card name | `.title-4` |
| Running step line | `.title-4` |
| Section headings (`Drives`) | `.heading` |
| Cycle badges on source rows | `.heading .accent` |
| Row titles | *(default `Adw.ActionRow`)* |
| Row subtitles | *(default)* |
| Card cycle/verdict lines, ETA, counts, source path | `.caption` (+ `.dimmed`/`.warning`/`.error`) |
| Any label containing digits that change | additionally `.numeric` |
| Log message text | `.monospace` |
| Log timestamps | `.caption .dimmed .numeric` |

Four levels total — `title-1`, `title-4`, body, `caption` — and one accent treatment. Use `.dimmed`; `.dim-label` is deprecated in libadwaita 1.7+ and must not appear anywhere in the codebase.

### 5.2 Colour and semantic classes

Only libadwaita's semantic classes; **no hardcoded hex anywhere in CSS or code**, so the palette follows light/dark, the user's accent colour, and high contrast automatically.

| Meaning | Class | Used on |
| --- | --- | --- |
| Primary commit | `.suggested-action .pill` | Update button, Eject button, `Choose Folder…` in P9 |
| Irreversible | `.destructive-action` | D1's `Replace Plates`, D2's `Stop Update` — **only inside dialogs** |
| Success | `.success` | Result icons, selection ticks, `Cycle installed` |
| Caution | `.warning` | Entitlement mismatch, unrecognised drive, blocked-reason label |
| Failure | `.error` | Read-only, won't fit, failed rows |
| Identity/version | `.accent` | Cycle badges |
| De-emphasis | `.dimmed` | Metadata, paths, counts |

`Adw.LevelBar` offsets `high`/`full` provide the "nearly out of space"/"over capacity" colouring with zero custom colour code.

### 5.3 Log severity without hex

The log is a `Gtk.ListBox` (`.boxed-list-separate` inside the dialog, plain inside the Details expander) of at most 500 rows — entries are per *step*, not per file, so a full 2-drive run produces well under 100. Each row is `Gtk.Box` horizontal, `spacing 8`, margins 6/12:

| Part | Widget | Classes |
| --- | --- | --- |
| Severity | `Gtk.Image`, 16px | `info` → no image (a 16px spacer keeps alignment); `success` → `emblem-ok-symbolic` + `.success`; `warning` → `dialog-warning-symbolic` + `.warning`; `error` → `dialog-error-symbolic` + `.error` |
| Time | `Gtk.Label` `14:12:07` | `.caption .dimmed .numeric` |
| Message | `Gtk.Label`, `wrap`, `xalign 0`, `hexpand`, `selectable` | `.monospace` + the same semantic class as the icon |

Severity therefore survives dark mode, high contrast, and colour-blind users (icon shape carries it independently of hue). The plain-text export prepends `[ok] [warn] [error]` markers so severity survives copy-paste too. Export format:

```
Dynon USB Updater 1.0 — log started 2026-08-23 16:19:26
Source folder: /home/yacine/Downloads
Aviation:  airmate_av_data_us_2608_013712.dup (Cycle 2608, 8.6 MB)
Obstacle:  airmate_obstacle_data_us_2608_013712.dup (Cycle 2608, 2.0 MB)
Plates:    US-Plates-2608.zip (23,104 files, 7.3 GB)
Drives:    DYNON (/run/media/yacine/DYNON), DYNON2 (/run/media/yacine/DYNON2)
Options:   verify=on replace-old=on eject=off
---
16:19:26        DYNON: 12.0 GB free, 7.3 GB needed, 6.8 GB reclaimable — fits
16:19:27        DYNON: removing airmate_av_data_us_2607_013712.dup
16:19:31 [ok]   DYNON: copied airmate_av_data_us_2608_013712.dup (8.6 MB)
16:19:44 [ok]   DYNON: checksum matches
16:20:02 [warn] DYNON: erased 27,490 files from ChartData/Plates
...
16:41:18 [ok]   DYNON: 23,104 plates written, Cycle 2608 installed
```

Every run is written to `$XDG_DATA_HOME/io.github.yfilali.DynonUSBUpdater/logs/YYYY-MM-DD-HHMMSS.log` unconditionally — this is an aviation task and provenance is part of the product, not a preference. The last 20 files are kept.

---

## 6. Interaction

### 6.1 Selection model

Selection lives on the `Gtk.ToggleButton` inside each `FlowBox` child; the `FlowBox` itself has `selection-mode: none`. This keeps keyboard focus (which moves freely) and selection (which only changes on activation) independent — a `FlowBox` in `MULTIPLE` mode selects on arrow-key focus, which for a control that decides which physical device gets erased is unacceptable.

- Click anywhere on a card toggles it. `Space` or `Enter` toggles the focused card.
- No drag-select, no shift-range, no select-all-by-default. `Ctrl+A` selects **only drives that are recognised, writable, and fit** — it can never sweep an unrelated stick into the job.
- A card that is read-only or does not fit can still be selected (so the reason surfaces adjacent to the button rather than being silently unavailable), but it blocks the primary action and its banner offers `Deselect`.

**The safe default — and why I agree with the correction.** My earlier "select every connected drive" recommendation was wrong. The failure modes are asymmetric: the cost of a default that under-selects is one click; the cost of a default that over-selects is an unrelated USB stick having its `ChartData/Plates` erased and 7.3 GB of plates written to it. Defaults must be safe against the worst outcome, not optimised for the median one. The rule shipped is:

> **A drive is pre-selected if and only if it (a) passes the SkyView recognition test, (b) is writable, (c) has room, and (d) is not already at the target cycle.**

For the real machine that means: both DYNON and DYNON2 recognise, both are writable and fit, both are at 2607, so both start selected and the ready state is reached in **zero clicks** — the safe default and the convenient one coincide, which is the point of making the test evidence-based rather than count-based. An unrelated stick never pre-selects; a drive already at 2608 never pre-selects (a 20-minute no-op is a real cost too), and its card says `Already up to date` so the omission is explained rather than mysterious. Selection is remembered per drive in GSettings (§6.6) and a remembered selection is re-applied only if the drive still passes (a)–(c).

### 6.2 SkyView recognition test

Scored on the drive root; **recognised at score ≥ 2**. Every check is a cheap `readdir`/`stat` — no file contents are read, nothing is written.

| Signal | Score |
| --- | --- |
| A `ChartData/` directory exists | 2 |
| One or more `*.dup` files at the root | 2 |
| A `CHARTS-*.key` file at the root | 2 |
| A `FACTORY/`, `User Settings/`, or `settings_archive/` directory | 1 |
| One or more `*.duc` files at the root | 1 |
| Volume label matches `/dynon|skyview/i` | 1 |

Both real drives score 8. The test result is cached per mount and invalidated on `mount-changed`.

Installed cycle: parse every root `*.dup` name with the cycle regex `(?<![0-9])([0-9]{2})(0[1-9]|1[0-3])(?![0-9])`; the highest cycle across aviation-classified files is the drive's installed cycle. Entitlement ID: the trailing digit group of a root `.dup` name, cross-checked against `CHARTS-(\d+)\.key`; a mismatch between the drive's key and the *source* file's ID raises P15.

### 6.3 Pre-flight validation

Computed on every change to selection, sources, or drive state — never at commit time only.

| Check | Rule |
| --- | --- |
| Something to do | at least one of {aviation, obstacle, archive} is selected |
| Somewhere to put it | ≥ 1 drive selected |
| Sources readable | each selected source `stat`s and opens |
| Archive valid | central directory parses, ≥ 1 usable member after junk filtering, no `..` or absolute members |
| Writable | mount is not `ro` and `access(W_OK)` succeeds |
| Fits | `needed ≤ free + reclaimable`, where `needed = Σ dup_sizes + zip_uncompressed_size` and `reclaimable = du(ChartData/Plates)` when an archive is included, else 0 |
| No duplicates | a folder target resolving to the same device+inode as a mounted drive is merged, not listed twice |

`reclaimable` is computed asynchronously per drive on scan (27,490 files on FAT takes a moment); until it lands, the card shows `Checking…` and the drive cannot be part of a commit.

### 6.4 Keyboard map

| Shortcut | Action | Scope |
| --- | --- | --- |
| `Ctrl+O` | Choose update folder | prepare |
| `Ctrl+Shift+O` | Choose plates archive | prepare |
| `Ctrl+Shift+D` | Choose drive folder | prepare |
| `Ctrl+R` / `F5` | Rescan drives | prepare |
| `Ctrl+A` | Select all ready drives | prepare |
| `Ctrl+Shift+A` | Deselect all drives | prepare |
| `Ctrl+Return` | Update (primary action) | prepare, when sensitive |
| `Space` / `Enter` | Toggle focused drive card | prepare |
| `Escape` | Cancel the run (via D2 when past the erase) | running |
| `Ctrl+L` | Activity log dialog | anywhere |
| `Ctrl+Shift+C` | Copy log to clipboard | log dialog / Details |
| `Ctrl+comma` | Preferences | anywhere except running |
| `Ctrl+question` | Keyboard shortcuts | anywhere |
| `F10` | Main menu | anywhere |
| `Ctrl+W` | Close window (guarded during a run) | anywhere |
| `Ctrl+Q` | Quit (guarded during a run) | anywhere |
| `Menu` / `Shift+F10` | Context menu on focused drive card | prepare |

Every one appears in the `Gtk.ShortcutsWindow` (D4) under three groups: *Files*, *Drives*, *General*. All menu items show their accelerator.

### 6.5 Focus order and transitions

**Prepare, initial focus:** the primary Update button when it is sensitive (the common zero-click case), otherwise the first drive card, otherwise the `Change…` button. Tab order: menu button → `Change…` → aviation `⋯` → obstacle `⋯` → plates `⋯` → `Choose Folder…` → rescan → card 1 → card 2 → … → Update button. The reason label is not focusable but is the button's `DESCRIBED_BY`.

**Running, initial focus:** the `Cancel` button. Tab order: menu button → Details expander → `Copy` → `Save…` → Cancel.

**Result, initial focus:** the first eject button if present, otherwise `Done`. Tab order: menu button → per-drive `Retry` buttons → eject → Done → Details.

Transitions:

| From | Trigger | To | Motion |
| --- | --- | --- | --- |
| prepare | D1 `Replace Plates`/`Update` | running | `crossfade` 200 ms |
| running | job completes | result | `crossfade` 200 ms |
| running | D2 `Stop Update` | result (F3) | `crossfade` 200 ms |
| result | `Done` | prepare | `crossfade` 200 ms |
| result | `Retry` | D1 (single drive) → running | as above |
| any | drive plugged/unplugged | card added/removed | `FlowBox` child add/remove; no animation beyond libadwaita defaults |
| prepare | banner condition changes | banner reveal | `Adw.Banner`'s built-in slide |

No custom animations. `Adw.Spinner` for indeterminate work; `Gtk.ProgressBar::pulse` only during R0.

### 6.6 Persistence (GSettings, schema `io.github.yfilali.DynonUSBUpdater`)

| Key | Type | Default | Notes |
| --- | --- | --- | --- |
| `source-folder` | `s` | `""` | Empty → XDG Download |
| `plates-archive` | `s` | `""` | Cleared if unreadable at launch |
| `verify-copies` | `b` | `true` | |
| `replace-old-databases` | `b` | `true` | |
| `eject-when-finished` | `b` | `false` | |
| `selected-drives` | `as` | `[]` | Entries `"{uuid}\|{label}"`; matched by UUID first, label second |
| `manual-targets` | `as` | `[]` | Folder-target paths |
| `window-width` / `window-height` | `i` | `820` / `700` | |
| `window-maximized` | `b` | `false` | |
| `first-run-done` | `b` | `false` | |

Written on change (not only on close), so a crash cannot lose them. The application is **single-instance** (`Gio.ApplicationFlags::default`); a second launch presents the existing window. Two instances writing the same drive concurrently must be impossible.

---

## 7. Error taxonomy

### 7.1 Blocking, pre-flight (prevent the run)

| # | Condition | Detection | Surface | User can |
| --- | --- | --- | --- | --- |
| E1 | No source folder / unreadable | launch, folder change | Banner P2 + reason 1 | Choose another folder |
| E2 | No `.dup` in folder | folder scan | Banner P3 + reason 2 | Choose another folder |
| E3 | Only one kind found | folder scan | Banner P4, non-blocking | Continue, or choose another folder |
| E4 | Nothing selected to copy | continuous | Reason 3 | Re-enable a source |
| E5 | Archive unreadable / not a zip | async archive read | Row value `Can't read` + Banner P7 | Choose another archive |
| E6 | Archive has zero usable members | after filtering | Row value `Empty` + Banner P7 | Choose another archive |
| E7 | Archive contains `..` or absolute paths | member sanitisation | Banner: `{filename} contains unsafe file paths and will not be used.` | Choose another archive |
| E8 | No drives at all | scan | P8 status page + reason 5 | Plug in, or Choose Folder |
| E9 | Drives invisible (sandbox) | §3.3 algorithm | P9 status page + D9 + reason 5 | Choose Folder, or grant permission |
| E10 | Nothing selected | continuous | Reason 6 | Select a drive |
| E11 | Selected drive read-only | scan | Card `.error` + Banner P12 + reason 7 | Deselect, or remount rw |
| E12 | Selected drive won't fit | scan + archive size | Card `.error` + Banner P13 + reason 8 | Deselect, or free space |
| E13 | Folder target unreachable | launch | Card P16 + Banner | Choose again, or remove |

### 7.2 Non-blocking advisories

| # | Condition | Surface | User can |
| --- | --- | --- | --- |
| E14 | Drive not recognised as SkyView | Card `.warning`; if selected, the D1 checkbox gate | Deselect, or acknowledge in D1 |
| E15 | Entitlement mismatch | Banner P15 + card `.warning` + Details dialog | Deselect, or use anyway |
| E16 | Drive already at target cycle | Card `.success`, not pre-selected | Select it to rewrite |
| E17 | Archive cycle ≠ database cycle | Banner: `The plates archive is Cycle {a} but the databases are Cycle {b}.` | Continue, or change a source |

### 7.3 Banner priority

Exactly one banner is shown, first match wins: E7 → E9 → E5/E6 → E1 → E11 → E12 → E13 → E2 → E15 → E17 → E3 → P10 → P0.

### 7.4 Runtime failures (during the run)

| # | Condition | Behaviour | Result surface | Retry |
| --- | --- | --- | --- | --- |
| E18 | Drive unplugged mid-write | Abort that drive, continue others | Row: `The drive was unplugged before it finished. Its plates folder is incomplete.` | Yes, when reconnected |
| E19 | Out of space mid-write (bad estimate, other writer) | Abort that drive | `Ran out of space. {n} of {total} plates were written.` | Yes |
| E20 | I/O error / bad sector | Abort that drive, `errno` into the log | `The drive reported a write error. It may be failing.` | Yes |
| E21 | Checksum mismatch after copy | Abort that drive; the bad file is deleted | `The copy of {filename} did not match the original. Nothing was left half-written.` | Yes |
| E22 | Permission denied mid-write | Abort that drive | `The drive stopped accepting writes.` | Yes |
| E23 | Archive changed/vanished mid-run | Abort the whole job | F2: `The plates archive was changed while the update was running.` | No — restart |
| E24 | Corrupt zip member | Skip the member, log a warning, continue; if > 1% of members fail, abort that drive | `{n} plates could not be read from the archive.` | Yes |
| E25 | User stopped after the erase | Unwind, mark that drive incomplete | F3 wording (§4.10) | Yes |
| E26 | Eject failed | Toast `Could not eject {drive} — eject it from Files` | — | Retry from Files |

Every runtime failure isolates to its drive: the remaining drives continue. Every one lands in the log with `[error]` and the raw OS message, so the exported log is diagnosable.

---

## 8. Accessibility

### 8.1 Names, roles, relations

| Widget | Accessible treatment |
| --- | --- |
| Drive card `Gtk.ToggleButton` | Role `ToggleButton` (implicit). `LABEL` composed and updated on every state change: `"DYNON, Cycle 2607, updating to 2608, 11.1 gigabytes free, this update fits"`. `DESCRIPTION` carries the caution when present: `"Registered to a different SkyView"`. Checked state exposed automatically. |
| Card `Gtk.LevelBar` | `LABEL` `"Space on DYNON after this update"`; `VALUE_TEXT` `"18.9 of 28.7 gigabytes used"`. Never left to announce as a bare level bar. |
| Card selection tick `Gtk.Image` | `HIDDEN=true` — decorative; the toggle's checked state already conveys it. |
| Card drive `Gtk.Image` | `HIDDEN=true` — decorative. |
| Primary Update button | `DESCRIBED_BY` → the reason label. Mandatory whenever insensitive. |
| Reason label | Role `Status`, `LIVE=polite`, so a changing reason is announced without stealing focus. |
| Source row value labels | Included in the row's accessible name by `Adw.ActionRow`; the `⋯` button gets `LABEL` `"More options for aviation database"` etc. |
| Icon-only buttons (rescan, log copy/save, row `⋯`) | Explicit `LABEL` on every one. A tooltip is not an accessible name. |
| `Gtk.ProgressBar` (running) | `LABEL` `"Update progress"`; `VALUE_TEXT` updated to `"37 percent"`; `DESCRIBED_BY` → the step label. `show-text` stays false visually. |
| Step label (running) | Role `Status`, `LIVE=polite`. |
| ETA label | `HIDDEN=true` for the screen reader — it is folded into the throttled announcement (§8.2) instead, so it does not chatter every second. |
| Per-drive running rows | `Adw.ActionRow` name = `"DYNON, extracting plates"`; the row's inner progress bar gets `LABEL` `"DYNON progress"` and `VALUE_TEXT`. |
| Result `Adw.StatusPage` | Title is the heading; the whole page gets `LIVE=assertive` on entry so the outcome is announced immediately. |
| Failed result rows | Role `Alert`. |
| Log `Gtk.ListBox` | Role `Log`. Each row's accessible name is prefixed with the severity word: `"Error. DYNON: the drive reported a write error."` |
| Log rows | `LIVE=off` — the log is a reference, not a narration; announcing 100 entries would be hostile. |
| Banners | `Adw.Banner` role `Status`, `LIVE=polite`. |
| Dialogs | `Adw.AlertDialog` handles roles; the D1 checkbox gets `DESCRIBED_BY` → the heading so the acknowledgement's subject is unambiguous. |

### 8.2 Live-region strategy

Three channels, deliberately separated so they cannot talk over each other:

1. **Validation (polite, prepare page).** The single reason label. Debounced 400 ms so rapid toggling does not produce a stream.
2. **Progress (polite, running page).** The step label is the live region, but announcements are **throttled to one every 15 seconds** and to every phase change, and the announced string is composed: `"Extracting plates to DYNON, 37 percent, about 14 minutes left."` The percentage and file-count labels are themselves `HIDDEN` from the a11y tree to prevent per-second chatter.
3. **Outcome (assertive).** Phase changes into the result page, and any per-drive failure at the moment it happens, are announced assertively. A failure is the one thing that may interrupt.

### 8.3 Other requirements

- Every interactive control is reachable by Tab in the order given in §6.5; nothing is mouse-only. The card context menu is reachable by `Menu`/`Shift+F10`.
- Focus is visible on every control, including the `.card` toggles (they must not suppress the focus ring).
- No information is conveyed by colour alone: every semantic colour is paired with an icon shape or a word (`fits`, `Read-only`, `Not a SkyView drive`).
- Contrast is inherited from libadwaita; the app defines no colours, so high-contrast mode works with no extra code.
- Text scaling: nothing has a fixed pixel height that contains text. Card `height-request 132` is a *minimum*; the card grows with the label heights.
- Reduced motion: honour `Gtk.Settings::gtk-enable-animations`; when off, the stack transition becomes `none` and the spinner remains (it is a status indicator, not decoration).
- The 200%-scale target: all sizes above are logical px; nothing is a raster asset except the app icon, which is SVG.

---

## 9. Responsive behaviour

`Adw.Breakpoint`s on the `Adw.ApplicationWindow`:

| Breakpoint | Condition | Reflow |
| --- | --- | --- |
| Wide | default | `FlowBox` `max-children-per-line 3`; source-path caption visible in the group header; page margins 24 |
| Medium | `max-width: 700sp` | `FlowBox` `max-children-per-line 2`; the `Change…` button and the path caption collapse into a single `⋯` menu on the group header |
| Narrow | `max-width: 500sp` | `FlowBox` `max-children-per-line 1`; card `height-request` drops to 112 and the capacity line wraps to two `.caption` lines; page margins 12; primary button `hexpand true` (full-width pill); result page action buttons stack vertically |
| Short | `max-height: 500sp` | Running page switches `valign` from `center` to `fill` and drops the percentage to `.title-2`; the per-drive group and Details move into a `Gtk.ScrolledWindow` |
| Compact | `max-width: 400sp` | Window subtitle hidden entirely (progress remains on the page); drive cards lose the capacity `LevelBar`, keeping the verdict text |

At the minimum 360 × 480 the prepare page still shows: the banner slot, the three source rows, one drive card, and the primary button with its reason — nothing essential is behind a scroll except additional drives. The window never scrolls horizontally at any size; every wide element (log rows, archive preview paths) lives in its own `overflow-x` scroller.

---

## 10. Acceptance criteria

An implementer verifies each item by direct observation. Items marked **[SAFETY]** are release blockers.

**Identity and packaging**

1. App ID is exactly `io.github.yfilali.DynonUSBUpdater`; the desktop file, metainfo, icons, GResource prefix, and GSettings schema all use it.
2. Both icons ship at the specified paths; the symbolic renders legibly at 16 px without scaling down the colour icon.
3. Metainfo validates with `appstreamcli validate --strict`; summary is ≤ 35 characters and does not repeat the app name; the Dynon disclaimer sentence appears in the description.
4. Flatpak manifest carries `--filesystem=/run/media` and `--filesystem=xdg-download:ro` and no broader filesystem permission; `--socket=wayland`, `--socket=fallback-x11`, `--share=ipc`, `--device=dri`.
5. `flatpak run … ls /run/media/$USER` inside the built app lists the drives.

**Prepare page**

6. With `~/Downloads` as source and both real files present, both databases are auto-identified as Cycle 2608 and `US-Plates-2608.zip` is *not* confused with `324-Jaunell-Road.zip`.
7. The archive row reports `23,104 files` (not 23,105) and skips `ChartData/.DS_Store`.
8. **[SAFETY]** With DYNON and DYNON2 connected, both are pre-selected, and the app reaches the ready state with **zero clicks**.
9. **[SAFETY]** With a non-Dynon USB stick connected, it renders `Not a SkyView drive` and is **not** pre-selected.
10. **[SAFETY]** Selecting an unrecognised drive and pressing Update shows D1 with the acknowledgement checkbox, and the destructive response is insensitive until it is ticked.
11. A drive already at Cycle 2608 shows `Already up to date` and is not pre-selected, but can be selected manually.
12. **[SAFETY]** The primary button is never insensitive without a populated reason label immediately beneath it; verified for all eight reasons in §4.8.
13. The reason label is the button's `DESCRIBED_BY`; Orca reads the reason when focus lands on the disabled button.
14. Unplugging a drive removes its card within 2 s with no user action; replugging restores it with its remembered selection.
15. Free-space verdicts are correct: `needed` includes the uncompressed archive size and `available` includes the reclaimable existing `ChartData/Plates`.
16. The entitlement mismatch warning appears only when both IDs are present and differ, and never for the real matched pair (`013712` on both sides).

**Sandbox fallback**

17. With `--filesystem=/run/media` removed, the app shows **P9** (`Can't See Your Drives`), not P8, while a USB drive is attached.
18. D9 shows the exact override command and `Copy Command` puts it on the clipboard.
19. `Choose Folder…` reaches a drive through the portal and produces a fully functional drive card, with the folder's basename as the title (never a `/run/user/*/doc/*` path).
20. A remembered folder target that is no longer reachable renders as `Not connected` with `Choose Again…`, and is never auto-selected.

**Confirmation and the run**

21. D1's body is plain sentences; the per-drive manifest is in `extra-child` and is left-aligned; no mount paths appear in visible text.
22. D1's default and close response is `Cancel`.
23. Entering the running page hides every configurable control; nothing on screen can alter the job in flight.
24. **[SAFETY]** `Gtk.Application::inhibit` is called with `LOGOUT | SUSPEND | IDLE` and reason `Writing avionics data to USB drives` when the job starts, and uninhibited exactly once when it ends (including on failure and cancel).
25. **[SAFETY]** Attempting to close the window during R3–R5 shows D2 and does **not** close; `Keep Updating` is the default response.
26. **[SAFETY]** Cancelling during R0–R2 stops immediately with no dialog and a toast stating nothing was changed.
27. The percentage, step, detail, and ETA all update at least once per second and never regress except within the damping rule.
28. ETA reads `Estimating time left…` for the first 20 s, then follows the §4.9 vocabulary.
29. The window subtitle shows `{percent}% — {eta}` and is legible in the GNOME overview.
30. Per-drive rows show the correct state at all times, including `Waiting` for drives not yet started.
31. A failure on one drive does not stop the others.

**Result**

32. No result state auto-dismisses; the page persists until an explicit action.
33. F1 names each failed drive and gives a plain-language reason from §7.4, with a working `Retry`.
34. F3's description explicitly states that the interrupted drive's plates folder is incomplete.
35. `Eject` appears only for successfully-updated mounted drives; a failed eject produces a persistent toast, never a silent no-op.
36. The result page announces itself assertively to a screen reader on entry.

**Log**

37. The log conveys severity by icon *and* style class; a grep of the codebase for `#[0-9a-fA-F]{6}` in CSS/Rust returns nothing.
38. `Copy` and `Save…` both work; the exported text carries the header block and `[ok]/[warn]/[error]` markers.
39. Every run writes a log file under `$XDG_DATA_HOME/…/logs/` with no preference required; the last 20 are retained.

**Accessibility and responsive**

40. Every icon-only button has an explicit accessible label; `Gtk.Inspector`'s accessibility tab shows no unnamed interactive widgets.
41. Progress announcements are throttled to ≤ 1 per 15 s plus phase changes; the percentage label is hidden from the a11y tree.
42. Full keyboard operation start to finish: choose folder, choose archive, select drives, confirm, cancel — no pointer.
43. Every shortcut in §6.4 works and appears in the shortcuts window with a matching accelerator in the menu.
44. At 360 × 480 nothing overlaps, the window never scrolls horizontally, and the primary button plus its reason are visible.
45. With `gtk-enable-animations` off, stack transitions are instant and no animation runs.
46. Light, dark, and high-contrast all render correctly with no app-defined colours.
47. `.dim-label` appears nowhere; `.dimmed` is used instead.
48. At 200 % scale on 4K, the default 820 × 700 window shows the full prepare page without scrolling.

---

## Appendix A — behaviours carried over from the previous implementation

Preserved because they are correct, not because they exist:

- `GVolumeMonitor`-driven live drive detection with real volume labels.
- Cycle-aware `.dup` ranking, including the digit-boundary regex that stops `2608` matching inside `013712`.
- Defaulting the wrapper-removal option to **off** — flattening a real data folder is worse than leaving a wrapper.
- Confirmation before any destructive write, defaulting to Cancel, with `DESTRUCTIVE` appearance when plates are erased.
- Junk filtering (`.DS_Store`, `Thumbs.db`, `desktop.ini`, `._*`, `__MACOSX`) and outright rejection of `..`/absolute archive members.
- `.part` + `fsync` + `rename` for database writes, SHA-256 read-back verification, per-drive free-space checks with reclaimed-space accounting, and per-drive failure isolation.
- Timestamped, severity-tagged activity logging.
- Background execution with the UI fed from a channel.

Discarded deliberately: the preferences-page container, the header-bar primary action, the window subtitle as a validation channel, prefix icons on source rows, the always-visible options group, the disabled wrapper switch, the `Details` expander as the only outcome surface, hardcoded log colours, and the "select every drive" default.
