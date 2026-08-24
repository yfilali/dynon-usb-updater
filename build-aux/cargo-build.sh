#!/bin/sh
# Invoked by meson's custom_target so `meson install` produces the binary at
# the path Meson expects, while the actual compilation stays plain Cargo.
#
# Args: <source-root> <cargo-target-dir> <buildtype> <output-path>
set -eu

source_root="$1"
target_dir="$2"
buildtype="$3"
output="$4"

profile_dir="debug"
extra_args=""
if [ "$buildtype" != "debug" ]; then
  profile_dir="release"
  extra_args="--release"
fi

CARGO_TARGET_DIR="$target_dir" cargo build $extra_args \
  --manifest-path "$source_root/Cargo.toml"

cp "$target_dir/$profile_dir/dynon-usb-updater" "$output"
