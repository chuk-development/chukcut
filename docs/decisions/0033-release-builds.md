# 0033 — Release builds: a manual workflow, Linux packages that work, macOS and Windows as experiments

Date: 2026-10-04. Status: accepted (release agent).

## What was decided

`.github/workflows/release.yml` builds every release artifact. It runs only
when someone starts it (`workflow_dispatch`); there is no tag or push
trigger. Inputs: a version string and one checkbox per target, all on, plus
"draft release", off. Every result is a workflow artifact; with a version
and the checkbox it is also attached to a **draft** release `v<version>`.
The workflow never publishes. `packaging/README.md` is the user-facing
description.

| Target | Runner | Recipe | State |
|---|---|---|---|
| `.deb` amd64 | ubuntu-24.04 | `packaging/deb/build-deb.sh` (dpkg-deb, Depends from dpkg-shlibdeps) | built and installed locally (clean `ubuntu:24.04` container); the job installs and runs it too |
| `.deb` arm64 | ubuntu-24.04-arm | the same script | not run: no ARM machine; the workspace type-checks for `aarch64-unknown-linux-gnu` |
| AppImage x86_64 | ubuntu-24.04 | `packaging/appimage/build-appimage.sh` (linuxdeploy + appimagetool, pinned) | built locally, run in `debian:trixie` and `fedora:42` containers and on Xvfb; the job runs it too |
| AppImage aarch64 | ubuntu-24.04-arm | the same script | as for the arm64 .deb |
| macOS universal `.dmg` | macos-15 + macos-15-intel | `packaging/macos/build-app.sh`, `make-universal.sh` | **experimental**: stops at the engine compile |
| Windows `.zip` + Inno Setup `.exe` | windows-2022 | `packaging/windows/package.ps1`, `chukcut.iss` | **experimental**: stops at the engine compile |

### Linux

- **The .deb links the system FFmpeg** (Ubuntu 24.04's 6.1, `libavcodec60`)
  like the tarball does. `dpkg-shlibdeps` writes the Depends line from the
  binaries' NEEDED entries; `libvulkan1` is added by hand because wgpu opens
  it with `dlopen`. The worker goes to `/usr/libexec/chukcut/`, where
  `ml::worker::beside` already looks, so it is not on PATH.
- **The AppImage bundles FFmpeg** and everything else linuxdeploy finds,
  minus a list of libraries that must come from the system because a driver
  is loaded into the process and binds to them: libva, libdrm, the Vulkan,
  GL, EGL and GBM loaders, the Wayland libraries, libstdc++ and libgcc_s (the system's Mesa needs
  its own, newer C++ runtime), and anything NVIDIA. The script fails if one
  of those ends up inside. A custom `AppRun` starts the editor, or the CLI
  with `--cli` or when called through a `chukcut-cli` symlink; the libraries
  are found through RPATH, so the environment, and with it the worker's own
  `dlopen` of ONNX Runtime and CUDA, is left alone.
- **Both are built on Ubuntu 24.04**, so the AppImage needs glibc 2.39. An
  older base (22.04) would widen the reach but ships FFmpeg 4.4, which the
  engine has never been built against (it uses the FFmpeg 5.1+ channel
  layout API, for one); building FFmpeg ourselves for the AppImage is the
  way to lower that floor.

This reverses the line in `packaging/README.md` that said "no AppImage, on
purpose", at the owner's request. The reason given there was the codec patent
position of a bundled FFmpeg. The AppImage, the macOS bundle and the Windows
zip now carry FFmpeg with x264 and x265, which puts the project's binaries in
the same position as Kdenlive's, Shotcut's and VLC's: patent pools license
implementations, nobody pays royalties for free builds, and that has not been
pursued against free projects; the exposure is a claim against distributed
binaries, not against the source. The GPL side is handled by recording the
exact source packages of every bundled library in the AppImage's
`BUILD-INFO.txt`; attaching those sources to a release would complete it.

### macOS and Windows

The owner's words were "rein theoretisch möglich": not a focus. The jobs are
`continue-on-error`, build what they can and upload what they make. They
were written against the runners' documented images, but have never run:
nothing past the engine compile can be tested until the engine compiles.

- **macOS**: one native job per architecture (Homebrew bottles are per
  architecture, so one machine cannot link both), Homebrew's FFmpeg and
  shaderc, `dylibbundler` copies the dylibs into `Contents/Frameworks`; a
  third job joins the two `.app`s with `lipo` and packs a `.dmg`. Signed ad
  hoc only. Homebrew's bottles are built for the runner's macOS, so the app
  needs macOS 15 in practice, whatever `LSMinimumSystemVersion` says.
- **Windows**: MSVC, BtbN's GPL shared FFmpeg 7.1 build through
  `FFMPEG_DIR`, NASM from Chocolatey for libjpeg-turbo, the image's LLVM for
  bindgen, shaderc built from source. The zip carries the FFmpeg DLLs next to
  the executables; the Inno Setup script installs per user by default and
  registers `.chukcut`. Unsigned.

## What blocks macOS and Windows

How this was found (2026-10-04): no macOS SDK or Windows toolchain is on the
development machine, so the engine was type-checked for
`x86_64-linux-android` with the NDK as a stand-in. That target has
`target_os = "android"`, so every `cfg(target_os = "linux")` gate and every
Linux-only dependency (`ash`, `wgpu-hal`, `libc` in the engine's
`[target.'cfg(target_os = "linux")'.dependencies]`) falls away exactly as on
macOS. Host FFmpeg headers stood in for a target build; nothing was linked.
The same trick for `aarch64-unknown-linux-gnu` (clang with the arm64 glibc
sysroot) type-checks the whole workspace cleanly.

**The engine does not compile on any non-Linux target.** rustc stops at name
resolution, so these are the first errors, not all of them:

| Where | Why it fails off Linux |
|---|---|
| `render/shared_frame.rs:40` | imports `render::dmabuf`, which is `#[cfg(target_os = "linux")]`; the module itself is not gated, and `render/mod.rs` re-exports `SharedFrame`, `SharedFrames`, `SharedBuffer` |
| `preview/zerocopy.rs:61` | imports `render::dmabuf` (VAAPI JPEG preview from exported NV12) |
| `preview/vasurface.rs:75` | imports `render::dmabuf` (VAAPI JPEG preview into a VA surface) |
| `export/hwframes.rs:529` | names `render::dmabuf` (zero-copy VAAPI export) |
| `export/job.rs:991` | uses `ZeroCopy`, which is defined only on Linux (`job.rs:1427`) |

Behind those, by reading the code, the next layer:

- `preview/player.rs` hands out `Collected::Shared(SharedFrame)` and owns a
  `SharedFrames` pool; `preview/server.rs` has `Claim` variants holding
  `vasurface::Claim` and `zerocopy::Claim`; `media/decoder.rs` stores a
  `DmabufFrame` in `MappedFrame`; `media/provider.rs` imports planes through
  `render::dmabuf` (already gated there, line 1147). Each needs its type or
  variant gated, and every `match` over it a Linux-only arm.
- `media/dmabuf.rs` and `export/hwframes.rs` use `std::os::fd`, which exists
  on macOS but not on Windows.
- `crates/app/src/player.rs` draws shared frames with
  `Window::paint_external_buffer` and `ExternalBuffer`, which exist only in
  the Linux build of the patched GPUI (`vendor/gpui-pre/src/window.rs:4993`);
  on macOS and Windows GPUI does not use `gpui-pre-wgpu` at all. The app
  would need the readback path only.
- Windows only: `ml/download.rs` (unpacking runtime packs: Unix permissions
  and symlinks, three places) and `crates/ml-worker/src/registry.rs:928` (the
  TensorRT provider symlink) use `std::os::unix`. The worker otherwise
  compiles on a non-Linux Unix.

That is about twenty engine files plus the app's player, all on the preview
and export hot paths, which is not a gate-here-and-there change. It was not
done: Linux is the product (CLAUDE.md), and a port that
touches the preview pipeline on Linux's behalf has to be its own piece of
work, measured on Linux before and after.

**What works off Linux even after a compile fix:** software decode and
encode, the readback preview (`BgraReadback`), wgpu on Metal or DirectX 12,
cpal on CoreAudio or WASAPI. **What would still be missing:** hardware
decode and encode (VideoToolbox, D3D11VA/MediaFoundation are not wired up;
NVENC on Windows would work through FFmpeg), and the AI models: the ONNX
Runtime packs in `crates/ml-worker/src/registry.rs` are Linux `.so` archives
pinned by URL and hash, so the worker would find no runtime.

One small change was made, because it costs Linux nothing:
`ml::worker::BINARY` is `chukcut-ml-worker.exe` on Windows, so the editor
can find the worker next to itself there.

## What it costs

- Four more scripts and one workflow to keep working. The Linux ones are
  tested in every run; the macOS and Windows ones rot unnoticed until the
  code compiles there.
- An ARM runner build per release: free for a public repository, about as
  slow as the x86_64 one.
- The AppImage is large (145 MB, 188 libraries) because Ubuntu's FFmpeg links every codec
  library Ubuntu enables.

## What would change our minds

- A Flatpak on the freedesktop runtime (codecs from its extension) would
  replace the AppImage as the portable route, as decision 0010 suggested.
- A decision to support macOS or Windows for real would need the port listed
  above, a second preview path, and signing money (Apple Developer ID,
  Windows code-signing certificate).
