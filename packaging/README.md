# Packaging

How chukcut gets onto a machine, and what each route ships.

| Route | For | Command |
|---|---|---|
| Install script | anyone building from source | `scripts/install.sh` |
| Release tarball | a machine with the same distribution family as the build machine | `packaging/tarball.sh` |
| Dev runner | contributors | `scripts/run-dev.sh` |

## `packaging/linux/`

| File | Installed as | Purpose |
|---|---|---|
| `chukcut.desktop` | `applications/chukcut.desktop` | menu entry; opens `.chukcut` files |
| `chukcut-mime.xml` | `mime/packages/chukcut.xml` | the `application/x-chukcut` type, glob `*.chukcut` |
| `io.github.chuk_development.chukcut.metainfo.xml` | `metainfo/…` | AppStream data for software centres |
| `icons/chukcut.svg` | `icons/hicolor/scalable/apps/chukcut.svg` | master icon |
| `icons/hicolor/<size>/apps/chukcut.png` | same path under `icons/` | 16 to 512 px renders of the master |

The desktop file is `chukcut.desktop`, not a reverse-DNS name, because it must
match the Wayland `app_id` the window sets (`crates/app/src/main.rs`).
Otherwise the shell cannot pair the window with its icon. The AppStream
component has a reverse-DNS id and points at the desktop file with
`<launchable>`. If you change the app id, change all three together.

The icon is the logo glyph from `crates/app/src/ui/icons.rs` in the accent
colour on a graphite tile (`docs/design/language.md`). After you edit
`icons/chukcut.svg`, run `packaging/linux/make-icons.sh` (needs Inkscape or
`rsvg-convert`) and commit the PNGs. `assets/icons/` still holds the stock
Tauri artwork from before the GPUI rewrite. Nothing uses it.

Validate after an edit:

```bash
desktop-file-validate packaging/linux/chukcut.desktop
appstreamcli validate --no-net packaging/linux/io.github.chuk_development.chukcut.metainfo.xml
xmllint --noout packaging/linux/chukcut-mime.xml
```

CI runs these three checks in the `packaging` job.

## The release tarball

`packaging/tarball.sh` builds the release binary (or takes the one in
`target/release` with `--no-build`) and writes
`target/dist/chukcut-<version>-x86_64-linux.tar.xz`:

```
chukcut-<version>-x86_64-linux/
  bin/chukcut
  scripts/install.sh       the same installer; it finds bin/chukcut and skips the build
  packaging/linux/…        desktop entry, MIME type, metainfo, icons
  LICENSE NOTICE.md README.md CHANGELOG.md
```

The user unpacks it and runs `scripts/install.sh`. It installs into `~/.local`
without sudo. `--uninstall` removes it again.

### What is bundled, and what is not

Decisions [0002](../docs/decisions/0002-ffmpeg-and-hardware-encoding.md) and
[0010](../docs/decisions/0010-open-source-under-gpl.md) set the rules. As GPL
software, chukcut may link the distribution's `--enable-gpl` FFmpeg. "Users
should not have to install anything" is a packaging goal, not a licence
requirement. The rule for this recipe: **ship only our own binary, and nothing
whose redistribution terms are unclear.**

**Inside the binary (statically linked, built from source by cargo):**

| Component | Licence | Why it is static |
|---|---|---|
| whisper.cpp and ggml (via `whisper-rs-sys`) | MIT | built with CMake; no distribution packages it |
| libjpeg-turbo (via `turbojpeg-sys`) | IJG, BSD-3-Clause, zlib | distribution packages are too old to link |
| Lua 5.4 (via `mlua`, `vendored`) | MIT | the effect runtime pins one version |
| RNNoise model (via `nnnoiseless`) | BSD-3-Clause | compiled in, no model file |
| Rust crates (wgpu, GPUI, …) | MIT / Apache-2.0 and compatible | the normal Rust case; `cargo tree` lists them |

**From the system (dynamically linked, never bundled):**

- **FFmpeg** (`libavcodec`, `libavformat`, `libavutil`, `libavfilter`,
  `libavdevice`, `libswscale`, `libswresample`). The binary records the
  sonames of the build machine (`libavcodec.so.60` for FFmpeg 6.1 on Ubuntu
  24.04 and Mint 22). On a distribution with another FFmpeg major, the
  tarball does not start: build from source there. We do not bundle FFmpeg,
  because a distribution build carries H.264/HEVC implementations
  (and x264/x265), and their patent terms are not ours to pass on. See
  `NOTICE.md`.
- **libva** and the VAAPI driver (`iHD`, `radeonsi`). The driver is the
  hardware vendor's.
- **The Vulkan loader** (`libvulkan.so.1`) and the GPU's Vulkan driver.
- **NVIDIA libraries** (`libcuda`, `libnvidia-encode`, NVDEC). These are
  **never** bundled. They come from the installed NVIDIA driver and must match
  its version exactly. FFmpeg loads them at runtime when NVENC or NVDEC is used.
- ALSA, X11/xcb, Wayland, xkbcommon, fontconfig, FreeType: the desktop stack.
- libshaderc (Apache-2.0), linked from the system's `libshaderc-dev`.
- The C++ runtime (`libstdc++`) for whisper.cpp.

`readelf -d target/release/chukcut | grep NEEDED` lists the direct
dependencies. In October 2026 that was FFmpeg, ALSA, xcb, xkbcommon,
fontconfig and libstdc++. wgpu loads Vulkan, and GPUI loads Wayland, with
`dlopen`, so `ldd` does not show them.

A CUDA build of whisper.cpp (`scripts/install.sh --cuda`) links the CUDA
runtime from the CUDA toolkit. A tarball made from such a build would carry a
dependency on the toolkit's `libcudart`. Build release tarballs without
`--cuda`.

### AppImage and Flatpak

There is no AppImage recipe, on purpose. An AppImage that does not bundle
FFmpeg is as tied to one distribution as the tarball. One that does bundle it
has to ship a codec build whose patent position we cannot vouch for. The
portable route that decision 0010 names is a **Flatpak** on the freedesktop
runtime. There, the runtime's `codecs-extra` extension supplies H.264 and HEVC
and carries their terms. That is the next packaging step, and nothing in this
directory blocks it: the desktop file, MIME type, metainfo and icons are
already what a Flatpak manifest installs.
