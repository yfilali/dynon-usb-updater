#!/bin/sh
# Run by meson as a post-install script. $MESON_INSTALL_DESTDIR_PREFIX is set
# by meson to <destdir><prefix> at install time; $1 is the schemas directory
# relative to the prefix.
set -eu
schemas_dir="$MESON_INSTALL_DESTDIR_PREFIX/$1"
"$2" "$schemas_dir"
