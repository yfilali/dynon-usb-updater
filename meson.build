project(
  'dynon-usb-updater',
  'rust',
  version: '1.0.0',
  meson_version: '>= 0.62',
  license: 'GPL-3.0-or-later',
)

app_id = 'io.github.yfilali.DynonUSBUpdater'

dependency('glib-2.0', version: '>= 2.80')
dependency('gtk4', version: '>= 4.16')
dependency('libadwaita-1', version: '>= 1.7')

cargo = find_program('cargo')
cargo_build_script = find_program('build-aux' / 'cargo-build.sh')

# meson wraps cargo rather than replacing it: `cargo build`/`cargo test` at
# the repository root keep working unmodified for day-to-day development.
cargo_build = custom_target(
  'cargo-build',
  build_by_default: true,
  build_always_stale: true,
  output: 'dynon-usb-updater',
  console: true,
  install: true,
  install_dir: get_option('bindir'),
  command: [
    cargo_build_script,
    meson.project_source_root(),
    meson.project_build_root() / 'target',
    get_option('buildtype'),
    '@OUTPUT@',
  ],
)

subdir('data')

install_data(
  'README.md',
  install_dir: get_option('datadir') / 'doc' / meson.project_name(),
)

test(
  'cargo-test',
  cargo,
  args: ['test', '--manifest-path', meson.project_source_root() / 'Cargo.toml'],
  timeout: 300,
)
