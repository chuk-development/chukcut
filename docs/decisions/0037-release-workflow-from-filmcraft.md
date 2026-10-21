# 0037 — The release workflow is FilmCraft's, taken over on purpose and credited

Date: 2026-10-21. Status: accepted. Supersedes the "how a release is started
and published" part of [0033](0033-release-builds.md); 0033's Linux
packaging (what the AppImage bundles and why, the .deb on the system
FFmpeg) and its macOS and Windows findings still stand.

## What was decided

FilmCraft ([storytold/filmcraft](https://github.com/storytold/filmcraft),
MIT OR Apache-2.0) releases through a shared workflow,
[`storytold/craft-common-actions/.github/workflows/release.yml`](https://github.com/storytold/craft-common-actions/blob/main/.github/workflows/release.yml),
with small composite actions beside it. Their pipeline is better than the
one 0033 built, and the owner's instruction was to copy it and to say openly
that we took it from them. `.github/workflows/release.yml` is now that
pipeline, rebuilt around chukcut's own packaging scripts, with
`.github/actions/resolve-version` and `.github/actions/draft-release` as
adapted copies of their actions of the same name. The credit is in the
header of each of those files, in `NOTICE.md`, and in the README's Credits
section. Their reference versions: filmcraft `70f0fc7`, craft-common-actions
`d18075e`, both 2026-10-20.

We cannot call their workflow with `uses:`. It belongs to another
organisation and moves at `@main`, and it assumes their crates: `cargo xtask
version set`, a `packaging/linux/package.sh` that drives nfpm, WiX for
Windows, a craft-fonts checkout. So the file is ours, and the reasoning is
theirs.

### Taken over

| From FilmCraft / craft-common-actions | In chukcut |
|---|---|
| Trigger: a push to the `release` branch makes a release; `workflow_dispatch` with a version override is a test run | The same. A test run makes a draft only when "draft release" is ticked (FilmCraft's always drafts) |
| One version, checked as strict semver (`1.2.3`, `1.2.3-rc.1`); a pre-release part marks the draft as a pre-release | `.github/actions/resolve-version`; the version comes from `crates/app/Cargo.toml` |
| "Apply version override" with `cargo xtask version set`, so the binaries report the version the files carry | `packaging/set-version.sh` (the five `Cargo.toml` files and `Cargo.lock`); also the way to bump before a release |
| Linux x86_64 and aarch64 as a matrix on native runners (`ubuntu-*-arm` for ARM) | The same, both must pass for a draft |
| AppImage update information `gh-releases-zsync\|owner\|repo\|latest\|<app>-*-linux-<arch>.AppImage.zsync` and a `.zsync` next to every AppImage, so AppImageUpdate fetches only the changed blocks | `packaging/appimage/build-appimage.sh`; the job checks both are there |
| One file name scheme: `<app>-<version>-<os>-<arch>.<ext>` | `chukcut-<v>-linux-<arch>.{AppImage,AppImage.zsync,deb,tar.xz}`, `chukcut-<v>-macos-universal.dmg`, `chukcut-<v>-windows-x86_64{.zip,-setup.exe}` |
| Artifacts named `release-*`; the release job downloads that pattern merged, so intermediate artifacts (the per-arch macOS apps) never reach the release | The same |
| One `SHA256SUMS.txt` over every release file, also in the run's summary | The same; the per-file `.sha256` files are no longer attached |
| A **draft** with `--generate-notes`, titled after the version, targeted at the built commit; an existing draft is emptied and refilled; a published release is never touched | `.github/actions/draft-release`, with chukcut's lines in front of the generated notes |
| Actions pinned to commit SHAs with the version in a comment | The same SHAs as theirs (checkout v7.0.1, rust-toolchain v1, rust-cache v2.9.2, upload-artifact v7.0.2, download-artifact v8.0.2) |
| `timeout-minutes` on every job, `CARGO_INCREMENTAL: 0`, one Rust cache key per target, `concurrency` without cancelling | The same |
| The release job runs unless something it needs failed | Spelled out as "version and Linux succeeded", because the experimental jobs may fail |

Two things are better than in their version: the inputs reach the shell
through `env:` instead of `${{ }}` inside the script (no injection through a
version string), and a version that is already a published release fails in
the first job, in seconds, instead of after an hour of builds.

### Deliberately not taken over

- **The craft-fonts embedding** (`craft-fonts-ref`, `CRAFT_FONTS_DIR`,
  `CRAFT_FONTS_REQUIRED`, the `checkout-craft-fonts` action). Every Craft
  release build checks out storytold/craft-fonts and embeds its Japanese
  fonts (BIZ UDPGothic, Shippori Mincho, BIZ UDMincho) into the binary,
  about 23 MB, and FilmCraft's GUI and CLI each carry them, so a package
  holds them twice. chukcut lays text out with the system's fonts through
  fontconfig and has nothing to gain from 46 MB of fonts per package. Left
  out on purpose.
- **rpm and the nfpm packaging.** Their binaries link only glibc, so one rpm
  fits every distribution. Ours link the system FFmpeg (`libavcodec.so.60`
  on Ubuntu 24.04); an rpm of them would ask Fedora for a soname it does not
  ship. The AppImage is the route for Fedora and everything else. Our .deb
  stays `dpkg-deb` with `dpkg-shlibdeps`, which writes the Depends line from
  the binaries.
- **Ubuntu 22.04 as the build base** (their glibc floor of 2.35). 22.04
  ships FFmpeg 4.4, which the engine does not build against (0033).
- **Signing and notarisation** (Apple Developer ID, Authenticode, Azure
  Trusted Signing, and their "require signing secrets on the release
  branch" gate) and the `release` environment that holds those secrets. We
  have no certificates, and macOS and Windows do not compile yet. When they
  do and someone pays for certificates, their gate is the pattern to copy.
- **WiX `.msi`** for Windows: we keep the Inno Setup installer from 0033;
  the job does not get far enough for the choice to matter.
- **Flatpak, Arch package, FreeBSD, riscv64, web (wasm), headless server.**
  Their Flatpak and Arch jobs repackage the tarball, which for us links one
  distribution's FFmpeg; a Flatpak on the freedesktop runtime with its codec
  extension is still 0010's and 0033's open item. The engine is Linux-only
  (VAAPI, dmabuf), has no wasm build and no server.

### Licence

FilmCraft is MIT OR Apache-2.0; we use it under Apache-2.0, which is
compatible with GPL-3.0-or-later. Its NOTICE line and the Apache licence text
(`.github/LICENSE-APACHE-2.0.txt`) go with the adapted files, and each file
says that it was changed. **craft-common-actions has no licence file** (the
GitHub API reports none as of 2026-10-21), so by default its text is not
licensed to anyone. That is why the workflow and the two actions were
written anew for chukcut, following its structure and behaviour, not
pasted: the shared workflow's text is a few hundred lines of `env:` blocks
for fourteen apps that we would not want anyway. If storytold adds a licence
there, nothing needs to change here; if they object, the structure is
generic GitHub Actions practice and the credit stays.

## What it costs

- A release is now "push to `release`", not "remember the Run workflow
  form". A push by mistake costs runner time and a draft, nothing public.
- AppImage file names changed (`-linux-` in the middle) and the release no
  longer carries per-file `.sha256` files. Anyone scripting downloads from
  the old names has to follow; there was no published release yet.
- The pinned action SHAs need updating by hand (or by Dependabot, which can
  update SHA pins).

## What would change our minds

- If craft-common-actions publishes a licence and a versioned tag, calling
  parts of it directly (resolve-version, draft-release) instead of keeping
  copies.
- A Flatpak, or a statically built FFmpeg that lets one binary set fit all
  distributions, would bring back rpm and an older build base.
