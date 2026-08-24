install_data(
  app_id + '.desktop',
  install_dir: get_option('datadir') / 'applications',
)

install_data(
  app_id + '.metainfo.xml',
  install_dir: get_option('datadir') / 'metainfo',
)

install_data(
  app_id + '.gschema.xml',
  install_dir: get_option('datadir') / 'glib-2.0' / 'schemas',
)

install_data(
  'icons' / 'hicolor' / 'scalable' / 'apps' / (app_id + '.svg'),
  install_dir: get_option('datadir') / 'icons' / 'hicolor' / 'scalable' / 'apps',
)

install_data(
  'icons' / 'hicolor' / 'symbolic' / 'apps' / (app_id + '-symbolic.svg'),
  install_dir: get_option('datadir') / 'icons' / 'hicolor' / 'symbolic' / 'apps',
)

desktop_file_validate = find_program('desktop-file-validate', required: false)
if desktop_file_validate.found()
  test(
    'validate-desktop-file',
    desktop_file_validate,
    args: [app_id + '.desktop'],
    workdir: meson.current_source_dir(),
  )
endif

appstreamcli = find_program('appstreamcli', required: false)
if appstreamcli.found()
  test(
    'validate-metainfo',
    appstreamcli,
    args: ['validate', '--strict', '--no-net', app_id + '.metainfo.xml'],
    workdir: meson.current_source_dir(),
  )
endif

# Looked up by absolute path: on at least one known dev machine, an Anaconda
# Python environment shadows the real /usr/bin/glib-compile-schemas earlier
# on PATH with an incompatible build.
glib_compile_schemas = find_program(
  '/usr/bin/glib-compile-schemas', 'glib-compile-schemas',
  required: false,
)
if glib_compile_schemas.found()
  meson.add_install_script(
    find_program(meson.project_source_root() / 'build-aux' / 'compile-schemas.sh'),
    get_option('datadir') / 'glib-2.0' / 'schemas',
    glib_compile_schemas.full_path(),
  )
endif
