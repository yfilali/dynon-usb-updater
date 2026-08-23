# Distribution &amp; Publishing Plan

App ID: **`io.github.yfilali.DynonUSBUpdater`** (fixed — Flathub requires a domain you
control or the `io.github.<user>` prefix; the earlier `org.yacine.*` would be rejected).

## 1. Repository

`github.com/yfilali/dynon-usb-updater`, public (Flathub requires public sources).

```
dynon-usb-updater/
├── Cargo.toml               workspace; the app crate
├── meson.build              top-level; drives cargo, installs data, gettext
├── src/
│   ├── main.rs              app entry, resources, gettext init
│   ├── application.rs       AdwApplication subclass, actions, accels
│   ├── window.rs            AdwApplicationWindow subclass (CompositeTemplate)
│   ├── drive.rs             GObject wrapper: volume, label, cycle, capacity, fit
│   ├── scan.rs              .dup discovery, cycle parsing, archive inspection
│   ├── job.rs               copy/verify/extract worker + cancellation + progress
│   └── ui/*.ui              GTK4 XML templates (no Blueprint dependency)
├── data/
│   ├── io.github.yfilali.DynonUSBUpdater.desktop.in
│   ├── io.github.yfilali.DynonUSBUpdater.metainfo.xml.in
│   ├── icons/hicolor/scalable/apps/*.svg   (+ symbolic)
│   └── resources.gresource.xml
├── build-aux/
│   └── io.github.yfilali.DynonUSBUpdater.yaml   Flatpak manifest
├── po/                      translation scaffolding (LINGUAS, POTFILES)
├── screenshots/             README + metainfo screenshots (generated, see §5)
└── docs/                    UX-SPEC.md, PUBLISHING.md
```

License: **GPL-3.0-or-later** (GNOME ecosystem norm; compatible with the LGPL runtime).
Say the word if you'd rather have MIT — it changes only the headers and metainfo field.

## 2. Build system

Meson wraps Cargo so that `meson install` places the binary, desktop file, metainfo,
icons and GResource bundle in the right prefix. This is the layout Flathub expects
and it keeps `cargo build` working for day-to-day development.

## 3. Continuous integration (GitHub Actions)

| Workflow | Runs | Gate |
| --- | --- | --- |
| `ci.yml` | `cargo fmt --check`, `cargo clippy -D warnings`, `cargo test` | every push/PR |
| `flatpak.yml` | `flatpak-builder` in the `org.gnome.Sdk//49` container, then `appstreamcli validate` and `desktop-file-validate` | every push/PR |
| `release.yml` | on tag `v*`: build, attach the `.flatpak` bundle to a GitHub Release | tags |

Validating the metainfo in CI matters: a malformed `<releases>` block is the most
common Flathub rejection.

## 4. Channels

1. **Flathub — primary.** Fork `flathub/flathub`, push the manifest to a branch named
   for the app ID, open a PR against `new-pr`. Review is manual and typically iterates
   on permissions. Our justification, ready in advance:
   > The app writes avionics databases to FAT-formatted USB drives mounted by udisks
   > at `/run/media/$USER`. No XDG portal exposes removable-drive enumeration or
   > write access, so `--filesystem=/run/media` is required. A file-chooser-portal
   > fallback is offered in-app for users who prefer to grant one directory at a time.
   Permissions requested, and nothing more:
   `--share=ipc --socket=wayland --socket=fallback-x11 --device=dri`
   `--filesystem=/run/media --filesystem=xdg-download:ro`
   Once merged, Flathub creates `flathub/io.github.yfilali.DynonUSBUpdater`; updates
   are PRs there, and the build bot rebuilds on manifest changes.
2. **AUR** — `dynon-usb-updater` PKGBUILD (and `-git`) for Arch/Manjaro, which is what
   this machine runs. Native package, no sandbox, no permission argument.
3. **Source** — `meson setup build && meson install -C build`, documented in the README.

## 5. Screenshots (README + metainfo)

Both the README and the AppStream metainfo need real screenshots, and Flathub renders
the metainfo ones on the app page. They must be regenerable, not hand-captured once:

`screenshots/capture.sh` launches the app under XWayland with seeded fixture data
(a temp folder of `.dup` files, a small plates archive, and a fake drive directory —
never the real DYNON sticks), drives it through each state with `xdotool`, and writes
`ready.png`, `working.png`, `result.png`, `drives-empty.png`. GNOME on Wayland blocks
`grim`, so capture is `xwd -id $(xdotool search --name …)` piped through `magick`.

Metainfo references them by raw.githubusercontent URL at a tagged commit, with a
`<caption>` each — Flathub requires captions.

## 6. README structure (the "rock solid" one)

1. Title, one-line description, badges (Flathub version + downloads, CI, license)
2. **Hero screenshot** immediately under the title
3. What it does — the three things, in pilot language
4. Install — Flathub button, AUR, source build
5. Using it — a walkthrough with a screenshot per phase, ending in the result screen
6. What it will and will not touch on your drives (the safety contract, explicit)
7. How it decides which files are "newest" (cycle parsing) and how the archive is unpacked
8. Troubleshooting — drive not detected (incl. the Flatpak permission case), not enough
   space, interrupted update, incomplete plates
9. Building from source, project layout, running the tests
10. License and credits
11. **Screenshot gallery at the end** — every state, captioned

## 7. Release checklist

- [ ] `cargo test` green, clippy clean
- [ ] Version bumped in `meson.build`, `Cargo.toml`, and a new `<release>` in metainfo
- [ ] `appstreamcli validate` and `desktop-file-validate` pass
- [ ] Screenshots regenerated if the UI changed
- [ ] Tag `vX.Y.Z`, GitHub Release with notes
- [ ] Flathub PR (first time) or manifest bump PR (subsequent)
