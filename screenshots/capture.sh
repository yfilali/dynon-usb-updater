#!/bin/bash
# Regenerates the screenshots in this directory (and the ones the metainfo
# and README reference) against FIXTURE data only — never the real DYNON
# sticks. See docs/PUBLISHING.md §5.
#
# Requires: an X11 (or XWayland) DISPLAY, xdotool, ImageMagick's `magick`,
# and a built `dynon-usb-updater` binary (built automatically below).
#
# Two escape hatches in the app make this possible without simulating real
# clicks (which turned out to be unreliable in a virtual/remote display):
#   DYNON_TEST_DRIVE_ROOTS  a `:`-separated list of directories that stand
#                           in for real GVolumeMonitor mounts (empty means
#                           zero drives, for the "no drives" state).
#   DYNON_AUTO_RUN          skips the confirm dialog and starts a real run
#                           shortly after the ready state renders, so the
#                           running/result states are reachable
#                           deterministically.
set -euo pipefail
cd "$(dirname "$0")/.."
REPO_ROOT="$PWD"
OUT_DIR="$REPO_ROOT/screenshots"

SCHEMA_COMPILER="/usr/bin/glib-compile-schemas"
[ -x "$SCHEMA_COMPILER" ] || SCHEMA_COMPILER="$(command -v glib-compile-schemas)"

echo "==> Building dynon-usb-updater (release)"
cargo build --release --manifest-path "$REPO_ROOT/Cargo.toml"
BINARY="$REPO_ROOT/target/release/dynon-usb-updater"

WORK="$(mktemp -d)"
trap 'kill_app; rm -rf "$WORK"' EXIT

kill_app() {
  local pid
  pid="$(pgrep -f "^$BINARY" || true)"
  if [ -n "$pid" ]; then
    kill -9 $pid 2>/dev/null || true
  fi
  # Wait for the window to actually disappear. Without this the next stage's
  # `xdotool search` finds the window we just killed and screenshots the
  # previous state under the next state's name.
  for _ in $(seq 1 50); do
    if [ -z "$(DISPLAY="${DISPLAY:-:0}" xdotool search --name "Dynon USB Updater" 2>/dev/null || true)" ]; then
      return 0
    fi
    sleep 0.2
  done
}

# --- Fixture data -----------------------------------------------------
FIX="$WORK/fixture"
mkdir -p "$FIX/downloads" "$FIX/DYNON/ChartData/Plates/US" "$FIX/DYNON/FACTORY" \
         "$FIX/DYNON2/ChartData/Plates/US"

echo "==> Building fixture data in $FIX"
head -c 8630943 /dev/urandom > "$FIX/downloads/airmate_av_data_us_2608_013712.dup"
head -c 1999540 /dev/urandom > "$FIX/downloads/airmate_obstacle_data_us_2608_013712.dup"

ZIPSTAGE="$WORK/zipstage/ChartData/Plates/US"
mkdir -p "$ZIPSTAGE"
python3_bin=/usr/bin/python3
command -v "$python3_bin" >/dev/null 2>&1 || python3_bin=python3
"$python3_bin" - "$ZIPSTAGE" << 'PYEOF'
import os, sys
d = sys.argv[1]
for i in range(20000):
    with open(os.path.join(d, f"PLATE_{i:05d}.png"), "wb") as f:
        f.write(os.urandom(400))
PYEOF
head -c 4096 /dev/urandom > "$WORK/zipstage/ChartData/Plates/Plates.sqlite"
touch "$WORK/zipstage/ChartData/.DS_Store"
"$python3_bin" - "$WORK/zipstage" "$FIX/downloads/US-Plates-2608.zip" << 'PYEOF'
import os, sys, zipfile
base, out = sys.argv[1], sys.argv[2]
zf = zipfile.ZipFile(out, "w", zipfile.ZIP_STORED)
for root, _dirs, files in os.walk(base):
    for f in files:
        p = os.path.join(root, f)
        zf.write(p, os.path.relpath(p, base))
zf.close()
PYEOF

# A decoy archive that must never be auto-selected (§UX-SPEC ground truth).
mkdir -p "$WORK/decoy"
head -c 500 /dev/urandom > "$WORK/decoy/note.txt"
"$python3_bin" -c "
import zipfile
zf = zipfile.ZipFile('$FIX/downloads/324-Jaunell-Road.zip', 'w')
zf.write('$WORK/decoy/note.txt', 'note.txt')
zf.close()
"

for drive in DYNON DYNON2; do
  head -c 8000000 /dev/urandom > "$FIX/$drive/airmate_av_data_us_2607_013712.dup"
  head -c 1900000 /dev/urandom > "$FIX/$drive/airmate_obstacle_data_us_2607_013712.dup"
  touch "$FIX/$drive/CHARTS-013712.key"
  for i in $(seq 1 2500); do
    head -c 300 /dev/urandom > "$FIX/$drive/ChartData/Plates/US/old_$i.png"
  done
done

# --- GSettings schema + isolated config/data dirs ----------------------
SCHEMA_DIR="$WORK/schemas"
mkdir -p "$SCHEMA_DIR"
cp "$REPO_ROOT/data/io.github.yfilali.DynonUSBUpdater.gschema.xml" "$SCHEMA_DIR/"
"$SCHEMA_COMPILER" "$SCHEMA_DIR"

CFG="$WORK/xdg-config"
mkdir -p "$CFG/glib-2.0/settings"
cat > "$CFG/glib-2.0/settings/keyfile" << KEOF
[io/github/yfilali/DynonUSBUpdater]
source-folder='$FIX/downloads'
plates-archive='$FIX/downloads/US-Plates-2608.zip'
window-width=820
window-height=1150
system-type='certified'
data-provider='airmate'
KEOF
mkdir -p "$WORK/xdg-data"

run_app() {
  local drive_roots="$1"
  shift
  GDK_BACKEND=x11 GDK_SCALE="${CAPTURE_SCALE:-2}" DISPLAY="${DISPLAY:-:0}" \
  GSETTINGS_SCHEMA_DIR="$SCHEMA_DIR" \
  GSETTINGS_BACKEND=keyfile \
  XDG_CONFIG_HOME="$CFG" \
  XDG_DATA_HOME="$WORK/xdg-data" \
  XDG_DATA_DIRS="$HOME/.local/share:/usr/local/share:/usr/share" \
  DYNON_TEST_DRIVE_ROOTS="$drive_roots" \
  "$@" \
  "$BINARY" > "$WORK/app.log" 2>&1 &
  disown
}

wait_for_window() {
  local id=""
  for _ in $(seq 1 50); do
    id="$(DISPLAY="${DISPLAY:-:0}" xdotool search --name "Dynon USB Updater" 2>/dev/null | tail -1 || true)"
    [ -n "$id" ] && { echo "$id"; return 0; }
    sleep 0.2
  done
  echo "error: window never appeared" >&2
  cat "$WORK/app.log" >&2
  return 1
}

shoot() {
  local name="$1" id
  # Re-resolve the window id right before capturing rather than trusting one
  # cached from earlier in the run — the id has been observed to go stale
  # (a wm/XWayland quirk in some remote display setups, not an app crash;
  # the app process itself is still alive and unchanged when this happens).
  id="$(wait_for_window)"
  DISPLAY="${DISPLAY:-:0}" xwd -id "$id" -out "$WORK/$name.xwd"
  magick "$WORK/$name.xwd" "$OUT_DIR/$name.png"
  echo "==> wrote $OUT_DIR/$name.png"
}

# --- Ready state --------------------------------------------------------
echo "==> Capturing ready.png"
run_app "$FIX/DYNON:$FIX/DYNON2"
wait_for_window > /dev/null
sleep 0.5
shoot ready

kill_app
sleep 0.5

# --- Running + result states (DYNON_AUTO_RUN bypasses D1 deterministically) --
echo "==> Capturing running.png / result.png"
run_app "$FIX/DYNON:$FIX/DYNON2" env DYNON_AUTO_RUN=1
wait_for_window > /dev/null
sleep 1.8
shoot running

# Result never auto-dismisses, so a generous fixed wait is fine.
sleep 10
shoot result

kill_app
sleep 0.5

# --- No drives connected --------------------------------------------------
echo "==> Capturing drives-empty.png"
run_app ""
wait_for_window > /dev/null
sleep 0.5
shoot drives-empty

kill_app
echo "==> Done. Screenshots written to $OUT_DIR"
