# Packaging

How chukcut gets onto a machine, and what each route ships.

| Route | For | Command |
|---|---|---|
| Install script | anyone building from source | `scripts/install.sh` |
| `.deb` | Ubuntu 24.04, Mint 22 and relatives (amd64, arm64) | `packaging/deb/build-deb.sh` |
| AppImage | any Linux with glibc 2.39 or newer (x86_64, aarch64); FFmpeg bundled | `packaging/appimage/build-appimage.sh` |
| Release tarball | a machine with the same distribution family as the build machine | `packaging/tarball.sh` |
| macOS `.dmg`, Windows `.zip` and installer | **experimental, does not build yet** | `packaging/macos/`, `packaging/windows/` |
| Dev runner | contributors | `scripts/run-dev.sh` |

All of them except the install script and the dev runner come out of one
GitHub Actions workflow, `.github/workflows/release.yml`, described next.

## The release workflow

`.github/workflows/release.yml` runs only when someone starts it: on GitHub,
**Actions › Release › Run workflow**, or from a shell with
`gh workflow run release.yml -f version=0.2.0 -f draft_release=true`.

| Input | Default | Effect |
|---|---|---|
| `version` | empty | The version in file names and package metadata. Empty: the version in `crates/app/Cargo.toml`. A leading `v` is dropped. |
| `deb` | on | `.deb` for amd64 and arm64 |
| `appimage_x86_64` | on | AppImage for x86_64 |
| `appimage_aarch64` | on | AppImage for aarch64, built on the `ubuntu-24.04-arm` runner |
| `macos_dmg` | on | universal `.dmg` (experimental) |
| `windows_zip` | on | portable `.zip` (experimental) |
| `windows_installer` | on | Inno Setup installer `.exe` (experimental) |
| `draft_release` | off | With a `version`: attach every result to a **draft** release `v<version>` |

Every result is a workflow artifact on the run's page (kept 90 days, the
repository default). The draft release is never published by the workflow:
a person reads it and presses Publish. If a draft for the same tag exists,
its files are replaced; a published release is never touched.

| Artifact | Job | Contents |
|---|---|---|
| `chukcut-linux-x86_64` | Linux x86_64 (ubuntu-24.04) | `chukcut_<v>_amd64.deb`, `chukcut-<v>-x86_64.AppImage`, `.sha256` files |
| `chukcut-linux-aarch64` | Linux aarch64 (ubuntu-24.04-arm) | `chukcut_<v>_arm64.deb`, `chukcut-<v>-aarch64.AppImage`, `.sha256` files |
| `chukcut-macos-arm64`, `chukcut-macos-x86_64` | macOS per architecture | `chukcut-<v>-<arch>-macos.app.tar.gz` (input to the next row) |
| `chukcut-macos-universal` | macOS universal | `chukcut-<v>-universal-macos.dmg` |
| `chukcut-windows-x86_64-zip` | Windows | `chukcut-<v>-x86_64-windows.zip` |
| `chukcut-windows-x86_64-setup` | Windows | `chukcut-<v>-x86_64-windows-setup.exe` |

The Linux jobs test what they built before they upload it. The `.deb` is
installed with `apt` (which resolves its Depends line), `chukcut-cli
--version` and `chukcut-cli ml status` must run, and the worker must be in
`/usr/libexec/chukcut/`. The AppImage runs `--cli --version` and `--cli ml
status`, and the editor must still be running after ten seconds on a virtual
X display with lavapipe.

**macOS and Windows are experimental.** Their jobs are `continue-on-error`,
so a red macOS or Windows job does not fail the run. Today they stop at the
compile of the engine: `docs/decisions/0033-release-builds.md` lists what
blocks them. The jobs stay in place so that they produce a build once the
code allows it. Nothing is signed: macOS builds are signed ad hoc (Gatekeeper
asks the user to allow them; notarisation needs a paid Developer ID), and the
Windows installer triggers SmartScreen until a code-signing certificate signs
it. Signing needs secrets the workflow does not have yet.

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

`packaging/tarball.sh` builds the three release binaries (or takes the ones
in `target/release` with `--no-build`, and stops if one is missing) and
writes `target/dist/chukcut-<version>-x86_64-linux.tar.xz`:

```
chukcut-<version>-x86_64-linux/
  bin/chukcut              the editor
  bin/chukcut-ml-worker    the AI models' process; the editor finds it next to itself
  bin/chukcut-cli          the command line and MCP server
  bin/chukcut-whisper-cuda transcription on an NVIDIA GPU; only when the build had nvcc
  scripts/install.sh       the same installer; it finds bin/chukcut and skips the build
  packaging/linux/…        desktop entry, MIME type, metainfo, icons
  LICENSE NOTICE.md README.md CHANGELOG.md
```

The user unpacks it and runs `scripts/install.sh`. It installs the three
binaries into `~/.local/bin` and the desktop files into `~/.local/share`,
without sudo. `--uninstall` removes them again. The CI `release` job checks
that all three binaries are in the tarball.

**Where the editor finds the worker.** `CHUKCUT_ML_WORKER` if set; else
next to the editor's own (resolved) binary, one directory up, or in
`<prefix>/libexec/chukcut/` or `<prefix>/lib/chukcut/`; else on `PATH`
(`engine::modules::ml::worker::beside`). The worker loads ONNX Runtime with
`dlopen` and downloads it on first use, so the tarball carries no ONNX
Runtime and no models.

### What is bundled, and what is not

This is the rule for the tarball and the `.deb`. The AppImage bundles FFmpeg
and its codec libraries; see "The AppImage" below.

Decisions [0002](../docs/decisions/0002-ffmpeg-and-hardware-encoding.md) and
[0010](../docs/decisions/0010-open-source-under-gpl.md) set the rules. As GPL
software, chukcut may link the distribution's `--enable-gpl` FFmpeg. "Users
should not have to install anything" is a packaging goal, not a licence
requirement. The rule for this recipe: **ship only our own binary, and nothing
whose redistribution terms are unclear.**

**Inside the binaries (statically linked, built from source by cargo):**

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

**The CUDA transcription helper** (`chukcut-whisper-cuda`, decision 0036)
is whisper.cpp with its CUDA backend in a process of its own. The engine's
build script builds it whenever `nvcc` is on the build machine
(`CHUKCUT_WHISPER_CUDA=0` skips it, `=1` requires it); CI and the release
builders have no `nvcc`, so their packages do not contain it. When it is
there, every recipe ships it as an **optional** binary: it links
`libcudart.so.12`, `libcublas.so.12` and `libcublasLt.so.12` from the CUDA
toolkit, which are never bundled and never a dependency. On a machine
without them (or without an NVIDIA driver) the helper does not start and the
editor transcribes on the CPU in its own process; with chukcut's NVIDIA
bundle installed (Settings › AI acceleration), the editor puts the bundle's
CUDA libraries on the helper's library path, so the toolkit is not needed
either. The helper's CPU code is compiled for the build machine's CPU and
its CUDA kernels for the build machine's GPU (ggml's `GGML_NATIVE` default);
a packager who ships it sets `GGML_NATIVE=OFF` and
`CMAKE_CUDA_ARCHITECTURES` (for example `"75;86;89"`) in the environment of
the build, which whisper.cpp's CMake run reads.

## The .deb

`packaging/deb/build-deb.sh [--no-build] [--version V]` writes
`target/dist/chukcut_<version>_<arch>.deb` (`amd64` or `arm64`, from dpkg).
A version `0.2.0-rc.1` becomes `0.2.0~rc.1`, so that it sorts before
`0.2.0`.

```
/usr/bin/chukcut                              the editor
/usr/bin/chukcut-cli                          the command line and MCP server
/usr/libexec/chukcut/chukcut-ml-worker        the AI worker; the editor looks here
/usr/libexec/chukcut/chukcut-whisper-cuda     the CUDA transcription helper, when built
/usr/share/applications/chukcut.desktop       and the MIME type, metainfo, icons
/usr/share/doc/chukcut/                       copyright, NOTICE.md, README.md
```

It links the system FFmpeg, like the tarball: the Depends line comes from
`dpkg-shlibdeps` (`libavcodec60 (>= 7:6.0)`, `libasound2t64`, …, about 40 MB
as a package), plus `libvulkan1`, which wgpu opens with `dlopen`. A package
built on Ubuntu 24.04 installs on 24.04, Mint 22 and their relatives, and apt
refuses it elsewhere. No maintainer scripts: dpkg triggers refresh the
desktop, MIME and icon caches.

## The AppImage

`packaging/appimage/build-appimage.sh [--no-build] [--version V]` writes
`target/dist/chukcut-<version>-<arch>.AppImage` (x86_64 or aarch64). It
downloads linuxdeploy, appimagetool and the type 2 runtime at pinned
versions into `target/appimage-tools/` (or takes `LINUXDEPLOY`,
`APPIMAGETOOL`, `APPIMAGE_RUNTIME`) and needs no FUSE.

**FFmpeg is inside**, with every library it links (x264, x265, dav1d, libvpx,
SVT-AV1, …): 188 libraries, about 145 MB as an AppImage in October 2026.
`BUILD-INFO.txt` at the AppImage's root lists them and the Ubuntu source
packages they came from, at their exact versions.

**From the system** (the AppImage does not start, or loses hardware video,
without them):

- glibc 2.39 or newer, because the build runs on Ubuntu 24.04.
- libva and libdrm. The VAAPI driver is loaded into the process and binds to
  the libva and libdrm already there; a bundled older libva cannot load a
  newer driver.
- The Vulkan loader and driver, GL, EGL, GBM and the Wayland libraries,
  which must match the installed Mesa or NVIDIA driver.
- libstdc++ and libgcc_s: the system's Mesa needs its own (newer) C++
  runtime, and a process has only one.
- What linuxdeploy's exclude list assumes on every desktop: X11 and xcb,
  fontconfig, FreeType, HarfBuzz, FriBidi, ALSA, zlib, expat and a few more.
  The exception is JACK: the list assumes it, Debian and Fedora often lack
  it, and libavdevice links it, so the script bundles `libjack.so.0`
  anyway.

The script fails if one of the system-only libraries ended up inside.

The entry point is `packaging/appimage/AppRun`: it starts the editor, or the
CLI with `--cli` as the first argument or when the AppImage is called through
a symlink named `chukcut-cli`. All three binaries sit in `usr/bin`, so the
editor finds the worker next to itself. The libraries are found through the
binaries' RPATH, not `LD_LIBRARY_PATH`, so the worker's own `dlopen` of ONNX
Runtime and CUDA sees the system unchanged.

Checked by hand on 2026-10-04: the x86_64 AppImage, unpacked into a
`debian:trixie` and a `fedora:42` container that have no FFmpeg at all (only
the system libraries above and Mesa's lavapipe), imports an H.264 file and
exports a 1080x1920 H.264 MP4; the editor opens its start screen on Xvfb.

On a machine with AppImageLauncher, its binfmt hook intercepts every
AppImage, also in containers and scripts, and can stop to ask a question.
Set `APPIMAGELAUNCHER_DISABLE=1` when running one from a script.

### The codec question

The AppImage, the macOS bundle and the Windows zip carry FFmpeg with x264
and x265. This file used to say "no AppImage, on purpose" for that reason;
the owner decided otherwise (decision 0033). Our binaries are now in the
position of Kdenlive's, Shotcut's and VLC's: the H.264 and HEVC patent pools
license implementations, and a free build pays nothing. The `.deb` and the
tarball still link the distribution's FFmpeg and carry none of it.

### Flatpak

A **Flatpak** on the freedesktop runtime remains the cleaner portable route
(decision 0010): the runtime's `codecs-extra` extension supplies H.264 and
HEVC and carries their terms. Nothing here blocks it: the desktop file, MIME
type, metainfo and icons are what a Flatpak manifest installs.

## macOS and Windows (experimental)

`packaging/macos/build-app.sh` makes `chukcut.app` for the Mac's own
architecture: the three binaries in `Contents/MacOS`, Homebrew's FFmpeg and
every other non-system dylib copied into `Contents/Frameworks` by
`dylibbundler`, an `.icns` made from the Linux icon renders, and an ad-hoc
signature. `packaging/macos/make-universal.sh` joins an arm64 and an x86_64
app with `lipo` and packs a `.dmg`. Universal needs both architectures' own
dylibs, which is why the workflow builds natively on `macos-15` (arm64) and
`macos-15-intel` and joins the results; both must have bundled the same
dylib set, or the script stops and names the difference.

`packaging/windows/package.ps1` packs the MSVC build with BtbN's shared
FFmpeg DLLs into a portable zip; `packaging/windows/chukcut.iss` (Inno Setup
6) installs the same files, per user by default, and registers `.chukcut`.
`ffmpeg-sys-next` finds FFmpeg through `FFMPEG_DIR`.

Neither builds today: the engine does not compile for either platform.
[Decision 0033](../docs/decisions/0033-release-builds.md) lists what blocks
it, file by file.
