#!/bin/sh
# Launch the updater with a Python that has PyGObject + GTK4 + libadwaita.
# Anaconda/pyenv interpreters usually do not, so try the system one first.
here=$(dirname "$(readlink -f "$0")")
for py in /usr/bin/python3 /usr/local/bin/python3 python3; do
    command -v "$py" >/dev/null 2>&1 || continue
    "$py" -c "import gi; gi.require_version('Gtk','4.0'); gi.require_version('Adw','1'); from gi.repository import Gtk, Adw" 2>/dev/null || continue
    exec "$py" "$here/dynon_usb_updater.py" "$@"
done
echo "No Python with GTK4 + libadwaita found. On Arch/Manjaro:" >&2
echo "  sudo pacman -S python-gobject gtk4 libadwaita" >&2
exit 1
