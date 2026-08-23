#!/usr/bin/env python3
"""
Dynon USB Updater — GTK4 / libadwaita front end.

Prepares one or more USB thumb drives for a Dynon SkyView data update: copies
the latest aviation- and obstacle-data .dup files to the drive root and replaces
ChartData/Plates from a selected plates .zip.

Run with:  ./run.sh   (or:  /usr/bin/python3 dynon_usb_updater.py)
"""

from __future__ import annotations

import json
import os
import queue
import shutil
import sys
import threading
from pathlib import Path

import gi

gi.require_version("Gtk", "4.0")
gi.require_version("Adw", "1")
from gi.repository import Adw, Gdk, Gio, GLib, Gtk, Pango  # noqa: E402

from dynon_core import (  # noqa: E402
    CHART_SUBPATH, Drive, Job, Runner, classify, human, list_drives, newest,
    safe_members, scan_dup_files, strip_common_prefix,
)
import zipfile  # noqa: E402

APP_ID = "org.yacine.DynonUSBUpdater"
NONE_LABEL = "None"
STATE_FILE = (Path(os.environ.get("XDG_CONFIG_HOME", Path.home() / ".config"))
              / "dynon-usb-updater.json")


def downloads_dir() -> Path | None:
    path = GLib.get_user_special_dir(GLib.UserDirectory.DIRECTORY_DOWNLOAD)
    for candidate in (Path(path) if path else None, Path.home() / "Downloads"):
        if candidate and candidate.is_dir():
            return candidate
    return None


def load_state() -> dict:
    try:
        return json.loads(STATE_FILE.read_text())
    except (OSError, ValueError):
        return {}


def save_state(data: dict) -> None:
    try:
        STATE_FILE.parent.mkdir(parents=True, exist_ok=True)
        STATE_FILE.write_text(json.dumps(data, indent=2))
    except OSError:
        pass


# --------------------------------------------------------------------------
# Drive discovery through GIO (names, icons and live plug/unplug signals)
# --------------------------------------------------------------------------

def gio_drives() -> list[Drive]:
    drives: list[Drive] = []
    for mount in Gio.VolumeMonitor.get().get_mounts():
        root = mount.get_root()
        path = root.get_path() if root else None
        if not path or not mount.can_eject():
            continue
        d = next((x for x in list_drives(include_all=True) if str(x.path) == path), None)
        drive = Drive(Path(path), mount.get_name() or Path(path).name,
                      d.fstype if d else "?")
        if d:
            drive.total, drive.free = d.total, d.free
        drive.icon = mount.get_icon()
        drives.append(drive)
    if not drives:                       # non-GIO systems, or nothing ejectable
        drives = list_drives()
    return sorted(drives, key=lambda d: str(d.path))


# --------------------------------------------------------------------------
# Window
# --------------------------------------------------------------------------

class UpdaterWindow(Adw.ApplicationWindow):
    def __init__(self, app: Adw.Application, start_folder: str | None = None):
        super().__init__(application=app, title="Dynon USB Updater")
        self.set_default_size(*self._preferred_size())

        self.state = load_state()
        self.dups: list = []
        self.dup_paths: list[Path | None] = []
        self.zip_file: Path | None = None
        self.zip_members: list = []
        self.drive_rows: dict[str, tuple[Adw.ActionRow, Gtk.CheckButton, Drive]] = {}
        self.runner: Runner | None = None
        self.cancel = threading.Event()
        self.q: queue.Queue = queue.Queue()
        self.src_folder: Path | None = None
        self.manual_drives: dict[str, Drive] = {}

        self.toasts = Adw.ToastOverlay()
        toolbar = Adw.ToolbarView()
        self.toasts.set_child(toolbar)
        self.set_content(self.toasts)

        self.window_title = Adw.WindowTitle(title="Dynon USB Updater",
                                            subtitle="Choose an update folder")
        header = Adw.HeaderBar(title_widget=self.window_title)

        self.update_btn = Gtk.Button(
            child=Adw.ButtonContent(label="Update Drives",
                                    icon_name="drive-harddisk-usb-symbolic"),
            sensitive=False, tooltip_text="Write the selected files to the drives")
        self.update_btn.add_css_class("suggested-action")
        self.update_btn.connect("clicked", self.on_update_clicked)
        header.pack_end(self.update_btn)

        menu = Gio.Menu()
        menu.append("Rescan Drives", "win.rescan")
        menu.append("About Dynon USB Updater", "win.about")
        header.pack_start(Gtk.MenuButton(icon_name="open-menu-symbolic",
                                         menu_model=menu, tooltip_text="Main Menu"))
        for name, handler in (("rescan", lambda *_: self.refresh_drives()),
                              ("about", lambda *_: self.show_about())):
            action = Gio.SimpleAction.new(name, None)
            action.connect("activate", handler)
            self.add_action(action)
        toolbar.add_top_bar(header)

        page = Adw.PreferencesPage()
        toolbar.set_content(page)
        page.add(self._build_sources())
        page.add(self._build_drives())
        page.add(self._build_options())
        page.add(self._build_log())

        toolbar.add_bottom_bar(self._build_progress())

        monitor = Gio.VolumeMonitor.get()
        for signal in ("mount-added", "mount-removed", "mount-changed"):
            monitor.connect(signal, lambda *_: self.refresh_drives())
        self.refresh_drives()

        folder = start_folder or self.state.get("folder") or downloads_dir()
        if folder and Path(folder).is_dir():
            self.load_folder(Path(folder))
        last_zip = self.state.get("zip")
        if last_zip and Path(last_zip).is_file():
            self.load_zip(Path(last_zip))
        self.remove_row.set_active(self.state.get("remove_old", True))
        self.verify_row.set_active(self.state.get("verify", True))
        self.connect("close-request", self.on_close)
        GLib.timeout_add(120, self._pump)

    @staticmethod
    def _preferred_size() -> tuple[int, int]:
        """Tall enough for the whole page, but never taller than the screen."""
        wanted_w, wanted_h = 860, 1180
        display = Gdk.Display.get_default()
        if display:
            monitors = display.get_monitors()
            monitor = monitors.get_item(0) if monitors.get_n_items() else None
            if monitor:
                area = monitor.get_geometry()
                return (min(wanted_w, int(area.width * 0.9)),
                        min(wanted_h, int(area.height * 0.9)))
        return wanted_w, wanted_h

    def show_about(self):
        about = Adw.AboutDialog(
            application_name="Dynon USB Updater",
            application_icon="drive-harddisk-usb-symbolic",
            developer_name="Built with Claude Code",
            version="1.0",
            comments=("Copies the latest Dynon aviation and obstacle .dup databases "
                      "to your USB drives and replaces ChartData/Plates from a "
                      "plates archive."),
            license_type=Gtk.License.MIT_X11)
        about.present(self)

    # -- sources -----------------------------------------------------------
    def _build_sources(self) -> Adw.PreferencesGroup:
        group = Adw.PreferencesGroup(title="Update Files")

        self.folder_row = Adw.ActionRow(title="Update Folder",
                                        subtitle="No folder selected",
                                        subtitle_lines=1, title_lines=1)
        self.folder_row.add_prefix(Gtk.Image.new_from_icon_name("folder-open-symbolic"))
        browse = Gtk.Button(label="Choose…", valign=Gtk.Align.CENTER)
        browse.connect("clicked", self.on_choose_folder)
        self.folder_row.add_suffix(browse)
        self.folder_row.set_activatable_widget(browse)
        group.add(self.folder_row)

        self.av_row = Adw.ComboRow(title="Aviation Data", model=Gtk.StringList())
        self.av_row.add_prefix(Gtk.Image.new_from_icon_name("airplane-mode-symbolic"))
        self.av_row.connect("notify::selected", lambda *_: self.update_ready())
        group.add(self.av_row)

        self.ob_row = Adw.ComboRow(title="Obstacle Data", model=Gtk.StringList())
        self.ob_row.add_prefix(Gtk.Image.new_from_icon_name("find-location-symbolic"))
        self.ob_row.connect("notify::selected", lambda *_: self.update_ready())
        group.add(self.ob_row)

        self.zip_row = Adw.ActionRow(
            title="Plates Archive", subtitle_lines=1, title_lines=1,
            subtitle="No archive selected — plates will be left alone")
        self.zip_row.add_prefix(Gtk.Image.new_from_icon_name("package-x-generic-symbolic"))
        zip_box = Gtk.Box(spacing=6, valign=Gtk.Align.CENTER)
        self.zip_clear = Gtk.Button(icon_name="edit-clear-symbolic", visible=False)
        self.zip_clear.add_css_class("flat")
        self.zip_clear.set_tooltip_text("Clear selection")
        self.zip_clear.connect("clicked", lambda *_: self.clear_zip())
        zip_choose = Gtk.Button(label="Choose…")
        zip_choose.connect("clicked", self.on_choose_zip)
        zip_box.append(self.zip_clear)
        zip_box.append(zip_choose)
        self.zip_row.add_suffix(zip_box)
        self.zip_row.set_activatable_widget(zip_choose)
        group.add(self.zip_row)

        self.strip_row = Adw.SwitchRow(
            title="Strip Top-Level Folder", subtitle_lines=1,
            subtitle="Unwrap a cycle folder that wraps the whole archive",
            sensitive=False)
        self.strip_row.connect("notify::active", lambda *_: self.describe_zip())
        group.add(self.strip_row)
        return group

    # -- drives ------------------------------------------------------------
    def _build_drives(self) -> Adw.PreferencesGroup:
        self.drives_group = Adw.PreferencesGroup(title="Target Drives")
        header_buttons = Gtk.Box(spacing=4, valign=Gtk.Align.CENTER)
        add = Gtk.Button(icon_name="list-add-symbolic",
                         tooltip_text="Add a folder as a target")
        add.add_css_class("flat")
        add.connect("clicked", self.on_add_folder)
        rescan = Gtk.Button(icon_name="view-refresh-symbolic", tooltip_text="Rescan")
        rescan.add_css_class("flat")
        rescan.connect("clicked", lambda *_: self.refresh_drives())
        header_buttons.append(add)
        header_buttons.append(rescan)
        self.drives_group.set_header_suffix(header_buttons)

        self.no_drives = Adw.ActionRow(
            title="No removable drives found",
            subtitle="Plug in a USB drive — it will appear here automatically")
        self.no_drives.add_prefix(Gtk.Image.new_from_icon_name("drive-removable-media-symbolic"))
        self.drives_group.add(self.no_drives)
        return self.drives_group

    def on_add_folder(self, *_):
        dialog = Gtk.FileDialog(title="Select a folder to update", modal=True)
        dialog.select_folder(self, None, self._manual_chosen)

    def _manual_chosen(self, dialog, result):
        try:
            folder = dialog.select_folder_finish(result)
        except GLib.Error:
            return
        if not folder or not folder.get_path():
            return
        path = Path(folder.get_path())
        try:
            usage = shutil.disk_usage(path)
        except OSError as exc:
            self.toasts.add_toast(Adw.Toast(title=f"Cannot use that folder: {exc}"))
            return
        drive = Drive(path, path.name or str(path), "manual", usage.total, usage.free)
        self.manual_drives[str(path)] = drive
        self.refresh_drives()
        self.log(f"Added {path} as a manual target")

    def on_remove_manual(self, _button, key):
        self.manual_drives.pop(key, None)
        self.refresh_drives()

    # -- options -----------------------------------------------------------
    def _build_options(self) -> Adw.PreferencesGroup:
        group = Adw.PreferencesGroup(title="Options")
        self.remove_row = Adw.SwitchRow(
            title="Replace Older Databases",
            subtitle="Delete existing .dup files of the same type from the drive",
            active=True)
        group.add(self.remove_row)
        self.verify_row = Adw.SwitchRow(
            title="Verify After Copying",
            subtitle="Read each copied .dup back and compare checksums", active=True)
        group.add(self.verify_row)
        return group

    # -- log ---------------------------------------------------------------
    def _build_log(self) -> Adw.PreferencesGroup:
        group = Adw.PreferencesGroup()
        self.log_row = Adw.ExpanderRow(title="Details", subtitle="Activity log")
        self.log_row.add_prefix(Gtk.Image.new_from_icon_name("text-x-generic-symbolic"))
        self.log_view = Gtk.TextView(editable=False, cursor_visible=False,
                                     wrap_mode=Gtk.WrapMode.WORD_CHAR,
                                     top_margin=8, bottom_margin=8,
                                     left_margin=12, right_margin=12,
                                     monospace=True)
        self.log_buffer = self.log_view.get_buffer()
        for name, colour in (("error", "#c01c28"), ("warn", "#c64600"),
                             ("good", "#26a269")):
            self.log_buffer.create_tag(name, foreground=colour)
        self.log_buffer.create_tag("head", weight=Pango.Weight.BOLD)
        scroller = Gtk.ScrolledWindow(min_content_height=220, max_content_height=340,
                                      propagate_natural_height=True)
        scroller.set_child(self.log_view)
        row = Adw.ActionRow(activatable=False)
        row.set_child(scroller)
        self.log_row.add_row(row)
        group.add(self.log_row)
        return group

    # -- progress bar ------------------------------------------------------
    def _build_progress(self) -> Gtk.Widget:
        box = Gtk.Box(orientation=Gtk.Orientation.HORIZONTAL, spacing=12,
                      margin_top=10, margin_bottom=10, margin_start=16, margin_end=16)
        self.progress = Gtk.ProgressBar(hexpand=True, valign=Gtk.Align.CENTER,
                                        show_text=False)
        self.status_label = Gtk.Label(label="", xalign=0,
                                      ellipsize=Pango.EllipsizeMode.MIDDLE)
        self.status_label.add_css_class("dim-label")
        column = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=6, hexpand=True)
        column.append(self.status_label)
        column.append(self.progress)
        self.cancel_btn = Gtk.Button(label="Cancel", valign=Gtk.Align.CENTER,
                                     visible=False)
        self.cancel_btn.add_css_class("destructive-action")
        self.cancel_btn.connect("clicked", lambda *_: self.request_cancel())
        box.append(column)
        box.append(self.cancel_btn)
        self.progress.set_fraction(0.0)
        self.progress_bar_box = box
        box.set_visible(False)
        return box

    # -- file selection (GTK native dialogs, portal-backed) ----------------
    def on_choose_folder(self, *_):
        dialog = Gtk.FileDialog(title="Select the folder containing the .dup files",
                                modal=True)
        start = self.src_folder or downloads_dir()
        if start:
            dialog.set_initial_folder(Gio.File.new_for_path(str(start)))
        dialog.select_folder(self, None, self._folder_chosen)

    def _folder_chosen(self, dialog, result):
        try:
            folder = dialog.select_folder_finish(result)
        except GLib.Error:
            return
        if folder and folder.get_path():
            self.load_folder(Path(folder.get_path()))

    def on_choose_zip(self, *_):
        dialog = Gtk.FileDialog(title="Select the plates archive", modal=True)
        filters = Gio.ListStore.new(Gtk.FileFilter)
        zips = Gtk.FileFilter(name="Zip archives")
        zips.add_pattern("*.zip")
        zips.add_mime_type("application/zip")
        filters.append(zips)
        every = Gtk.FileFilter(name="All files")
        every.add_pattern("*")
        filters.append(every)
        dialog.set_filters(filters)
        dialog.set_default_filter(zips)
        if self.zip_file:
            dialog.set_initial_file(Gio.File.new_for_path(str(self.zip_file)))
        else:
            start = self.src_folder or downloads_dir()
            if start:
                dialog.set_initial_folder(Gio.File.new_for_path(str(start)))
        dialog.open(self, None, self._zip_chosen)

    def _zip_chosen(self, dialog, result):
        try:
            gfile = dialog.open_finish(result)
        except GLib.Error:
            return
        if gfile and gfile.get_path():
            self.load_zip(Path(gfile.get_path()))

    # -- loading -----------------------------------------------------------
    def load_folder(self, folder: Path):
        self.src_folder = folder
        self.dups = scan_dup_files(folder)
        self.folder_row.set_subtitle(str(folder))
        self.folder_row.set_tooltip_text(str(folder))

        labels = [NONE_LABEL] + [f.path.name for f in self.dups]
        self.dup_paths = [None] + [f.path for f in self.dups]
        for row, kind in ((self.av_row, "avdata"), (self.ob_row, "obstacle")):
            model = Gtk.StringList()
            for label in labels:
                model.append(label)
            row.set_model(model)
            pick = newest(self.dups, kind)
            row.set_selected(self.dup_paths.index(pick.path) if pick else 0)
            row.set_subtitle(self._dup_subtitle(pick))

        self.av_row.connect_after("notify::selected", self._sync_subtitles)
        self.ob_row.connect_after("notify::selected", self._sync_subtitles)
        n = len(self.dups)
        self.log(f"Found {n} .dup file{'' if n == 1 else 's'} in {folder}")
        if not n:
            self.log("No .dup files in that folder", "warn")
        self.update_ready()

    def _dup_subtitle(self, dup) -> str:
        if not dup:
            return "Not selected"
        return f"{dup.version} · {human(dup.size)}"

    def _sync_subtitles(self, *_):
        for row, kind in ((self.av_row, "avdata"), (self.ob_row, "obstacle")):
            path = self.selected_path(row)
            dup = next((d for d in self.dups if d.path == path), None)
            row.set_subtitle(self._dup_subtitle(dup))

    def selected_path(self, row: Adw.ComboRow) -> Path | None:
        index = row.get_selected()
        if index == Gtk.INVALID_LIST_POSITION or index >= len(self.dup_paths):
            return None
        return self.dup_paths[index]

    def load_zip(self, path: Path):
        try:
            with zipfile.ZipFile(path) as zf:
                members = safe_members(zf)
        except Exception as exc:                      # noqa: BLE001
            self.zip_row.set_subtitle(f"Cannot read archive: {exc}")
            self.log(f"Cannot read {path.name}: {exc}", "error")
            return
        self.zip_file = path
        self.zip_members = members
        plain = [str(p) for _, p in strip_common_prefix(members, False)]
        single = [str(p) for _, p in strip_common_prefix(members, True)]
        applicable = plain != single
        self.strip_row.set_sensitive(applicable)
        self.strip_row.set_active(False)   # flattening a real folder (e.g. US/) is
                                           # far worse than leaving a wrapper in place
        self.zip_clear.set_visible(True)
        self.describe_zip()
        self.update_ready()

    def clear_zip(self):
        self.zip_file = None
        self.zip_members = []
        self.zip_clear.set_visible(False)
        self.strip_row.set_sensitive(False)
        self.strip_row.set_active(False)
        self.zip_row.set_subtitle("No archive selected — plates will be left alone")
        self.update_ready()

    def describe_zip(self):
        if not self.zip_file:
            return
        members = strip_common_prefix(self.zip_members, self.strip_row.get_active())
        size = sum(i.file_size for i, _ in members)
        sample = str(members[0][1]) if members else ""
        dest = "/".join(CHART_SUBPATH)
        self.zip_row.set_subtitle(
            f"{self.zip_file.name} · {len(members)} files, {human(size)}")
        self.zip_row.set_tooltip_text(
            f"{self.zip_file}\n\nFirst file installs as:\n{dest}/{sample}")

    # -- drives ------------------------------------------------------------
    def refresh_drives(self):
        if self.runner is not None:
            return
        found = {str(d.path): d for d in gio_drives()}
        found.update({k: v for k, v in self.manual_drives.items() if k not in found})
        checked = {key for key, (_, check, _) in self.drive_rows.items()
                   if check.get_active()}

        for key, (row, _, _) in list(self.drive_rows.items()):
            if key not in found:
                self.drives_group.remove(row)
                del self.drive_rows[key]

        for key, drive in found.items():
            if key in self.drive_rows:
                row, check, _ = self.drive_rows[key]
                row.set_subtitle(self._drive_subtitle(drive))
                if hasattr(row, "level"):
                    row.level.set_value(self._used_fraction(drive))
                self.drive_rows[key] = (row, check, drive)
                continue
            check = Gtk.CheckButton(valign=Gtk.Align.CENTER)
            check.connect("toggled", lambda *_: self.update_ready())
            row = Adw.ActionRow(title=drive.label, subtitle_lines=1, title_lines=1,
                                subtitle=self._drive_subtitle(drive),
                                tooltip_text=str(drive.path),
                                activatable_widget=check)
            row.add_prefix(check)
            row.add_prefix(Gtk.Image.new_from_icon_name(
                "drive-harddisk-usb-symbolic" if drive.fstype != "manual"
                else "folder-symbolic"))
            level = Gtk.LevelBar(min_value=0.0, max_value=1.0, width_request=90,
                                 valign=Gtk.Align.CENTER,
                                 mode=Gtk.LevelBarMode.CONTINUOUS)
            level.set_value(self._used_fraction(drive))
            row.add_suffix(level)
            row.level = level
            if drive.fstype == "manual":
                remove = Gtk.Button(icon_name="user-trash-symbolic",
                                    valign=Gtk.Align.CENTER, tooltip_text="Remove")
                remove.add_css_class("flat")
                remove.connect("clicked", self.on_remove_manual, key)
                row.add_suffix(remove)
            self.drives_group.add(row)
            self.drive_rows[key] = (row, check, drive)
            check.set_active(key in checked or len(found) == 1)

        self.no_drives.set_visible(not found)
        self.update_ready()

    @staticmethod
    def _used_fraction(drive: Drive) -> float:
        if not drive.total:
            return 0.0
        return max(0.0, min(1.0, (drive.total - drive.free) / drive.total))

    @staticmethod
    def _drive_subtitle(drive: Drive) -> str:
        kind = "Folder" if drive.fstype == "manual" else drive.fstype.upper()
        return f"{human(drive.free)} free of {human(drive.total)} · {kind}"

    def selected_drives(self) -> list[Drive]:
        return [drive for _, (_, check, drive) in self.drive_rows.items()
                if check.get_active()]

    # -- run ---------------------------------------------------------------
    def update_ready(self, *_):
        has_work = bool(self.selected_path(self.av_row) or
                        self.selected_path(self.ob_row) or self.zip_file)
        drives = self.selected_drives()
        self.update_btn.set_sensitive(bool(drives) and has_work and self.runner is None)
        if self.runner is not None:
            return
        if not self.dups and not self.zip_file:
            summary = "Choose an update folder"
        elif not has_work:
            summary = "Choose a .dup file or a plates archive"
        elif not drives:
            summary = "Select at least one drive"
        else:
            files = sum(1 for x in (self.selected_path(self.av_row),
                                    self.selected_path(self.ob_row), self.zip_file) if x)
            summary = (f"{files} item{'' if files == 1 else 's'} → "
                       f"{len(drives)} drive{'' if len(drives) == 1 else 's'}")
        self.window_title.set_subtitle(summary)

    def on_update_clicked(self, *_):
        drives = self.selected_drives()
        av, ob = self.selected_path(self.av_row), self.selected_path(self.ob_row)

        actions = []
        if av:
            actions.append(f"Copy {av.name}")
        if ob:
            actions.append(f"Copy {ob.name}")
        if self.zip_file:
            actions.append(f"Erase {'/'.join(CHART_SUBPATH)} and extract "
                           f"{self.zip_file.name}")
        body = ("<b>Drives</b>\n" + "\n".join(f"• {d.label} ({d.path})" for d in drives) +
                "\n\n<b>Actions</b>\n" + "\n".join(f"• {a}" for a in actions))
        if self.zip_file:
            body += ("\n\nEverything currently in " + "/".join(CHART_SUBPATH) +
                     " on those drives will be deleted.")

        dialog = Adw.AlertDialog(heading="Update these drives?", body=body,
                                 body_use_markup=True)
        dialog.add_response("cancel", "Cancel")
        dialog.add_response("update", "Update")
        dialog.set_response_appearance(
            "update", Adw.ResponseAppearance.DESTRUCTIVE if self.zip_file
            else Adw.ResponseAppearance.SUGGESTED)
        dialog.set_default_response("cancel")
        dialog.set_close_response("cancel")
        dialog.connect("response", self._confirm_response, drives, av, ob)
        dialog.present(self)

    def _confirm_response(self, dialog, response, drives, av, ob):
        if response != "update":
            return
        self.cancel = threading.Event()
        job = Job(drives, av, ob, self.zip_file,
                  self.remove_row.get_active(), self.verify_row.get_active(),
                  self.strip_row.get_active())
        self.runner = Runner(job, self.q, self.cancel)
        self.progress.set_fraction(0.0)
        self.progress_bar_box.set_visible(True)
        self.update_btn.set_sensitive(False)
        self.cancel_btn.set_visible(True)
        self.log_row.set_expanded(True)
        self.log(f"Updating {len(drives)} drive(s)", "head")
        self.runner.start()

    def request_cancel(self):
        self.cancel.set()
        self.status_label.set_label("Cancelling…")

    # -- queue pump --------------------------------------------------------
    def _pump(self) -> bool:
        try:
            while True:
                kind, payload = self.q.get_nowait()
                if kind == "log":
                    self.log(*payload)
                elif kind == "status":
                    self.status_label.set_label(payload)
                elif kind == "progress":
                    self.progress.set_fraction(min(1.0, payload / 100.0))
                elif kind == "done":
                    self._finish(*payload)
        except queue.Empty:
            pass
        return GLib.SOURCE_CONTINUE

    def _finish(self, ok: list[str], failed: list[str]):
        self.runner = None
        self.cancel_btn.set_visible(False)
        if not failed:
            self.progress.set_fraction(1.0)
        if failed:
            message = f"{len(ok)} updated, {len(failed)} failed: {', '.join(failed)}"
            self.log(message, "error")
        else:
            message = (f"{len(ok)} drive{'' if len(ok) == 1 else 's'} updated — "
                       "safe to eject")
            self.log(message, "good")
        self.toasts.add_toast(Adw.Toast(title=message, timeout=6))
        self.progress_bar_box.set_visible(False)
        self.refresh_drives()               # this recomputes the header subtitle…
        self.window_title.set_subtitle(message)   # …so restate the outcome after it

    def on_close(self, *_) -> bool:
        save_state({
            "folder": str(self.src_folder) if self.src_folder else "",
            "zip": str(self.zip_file) if self.zip_file else "",
            "remove_old": self.remove_row.get_active(),
            "verify": self.verify_row.get_active(),
        })
        return False

    def log(self, text: str, tag: str = "info"):
        end = self.log_buffer.get_end_iter()
        stamp = GLib.DateTime.new_now_local().format("%H:%M:%S")
        if tag == "info":
            self.log_buffer.insert(end, f"{stamp}  {text}\n")
        else:
            self.log_buffer.insert_with_tags_by_name(end, f"{stamp}  {text}\n", tag)
        mark = self.log_buffer.create_mark(None, self.log_buffer.get_end_iter(), False)
        self.log_view.scroll_mark_onscreen(mark)
        self.log_buffer.delete_mark(mark)


class UpdaterApp(Adw.Application):
    def __init__(self, start_folder: str | None):
        super().__init__(application_id=APP_ID,
                         flags=Gio.ApplicationFlags.NON_UNIQUE)
        self.start_folder = start_folder

    def do_activate(self):
        UpdaterWindow(self, self.start_folder).present()


def main() -> int:
    folder = sys.argv[1] if len(sys.argv) > 1 and Path(sys.argv[1]).is_dir() else None
    return UpdaterApp(folder).run([sys.argv[0]])


if __name__ == "__main__":
    sys.exit(main())
