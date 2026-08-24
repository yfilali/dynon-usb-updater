#!/bin/bash
# Run a command with the USB drive mounts masked away.
#
# (/media is absent on this host; /run/media is where udisks mounts them.)
# The app writes to removable drives, so anything that builds, tests or launches
# it can in principle reach the real avionics drives at /run/media. This wraps a
# command in a bubblewrap namespace where those paths are empty tmpfs mounts:
# the drives are not merely off-limits, they are not there. Everything else —
# the project, ~/.cargo, /tmp, the session bus, the display — is untouched, so
# builds, tests and screenshot capture work normally.
#
#   scripts/no-drives.sh cargo test
#   scripts/no-drives.sh bash screenshots/capture.sh
set -euo pipefail
exec bwrap \
  --bind / / \
  --dev-bind /dev /dev \
  --proc /proc \
  --tmpfs /run/media \
  --die-with-parent \
  "$@"
