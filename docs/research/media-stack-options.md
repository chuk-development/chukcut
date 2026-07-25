# Media stack: build or adopt

A decision document, written because "we already built it" is not an argument
and neither is "a library exists". The question is whether GStreamer/GES, MLT,
a higher-level Rust crate, or an existing open-source editor would get chukcut
to a shipping product faster than continuing to write the media layer
ourselves.

Everything below was checked against primary sources in July 2026 —
crates.io and GitHub APIs, the GStreamer monorepo, MLT's installed headers on
this machine — rather than recalled. Version numbers and dates are as of
2026-07-25. Where a claim could not be verified it says so.

## Verdict

**Keep the custom stack. Adopt nothing wholesale. Borrow narrowly.**

The framing of the question contains a mistaken premise: that these frameworks
solve the parts we have not built. They do not. What GES and MLT give away for
free is a *timeline model with clips, ranges and transitions* — which is
precisely the part chukcut already has, tested, in 2,279 lines of
`project` + `timeline` + `workspace`. What they do not give away is hardware
decode wired into a GPU compositor, reduced-resolution preview, proxy media, or
a render cache: Pitivi builds its own proxies on top of GES, and Kdenlive and
Shotcut build their own on top of MLT, because the frameworks do not have them.
So the trade on offer is to discard roughly 11,700 lines of working media code
in order to acquire a timeline we do not need, and then rebuild the proxy and
preview machinery anyway, on top of an engine whose internals we would no
longer control. Both migrations cost somewhere between 95 and 175
developer-days to arrive back at the feature set we have today. Closing our own
gaps — audio, hardware decode, a render graph, proxies, cross-platform FFmpeg —
costs 52 to 92 days and every one of those days buys a capability rather than
parity.

The maturity evidence points the same way, though not for the reason one would
guess, and getting this part right matters. **GES is not dying** — it took 98
commits in twelve months against 71 the year before, and the work is
architectural, funded by Igalia for what is almost certainly one commercial
customer. It is simply *small and unreleased*: its Rust binding crate pulled
**1,330 downloads in the last 90 days** against 1.19 million for
`gstreamer-video` from the same repository, with zero reverse dependencies; its
only desktop consumer, Pitivi, has taken sixteen commits in a year, four of the
last five being translations; and **both features that would justify adopting it
— GPU compositing and thread safety — exist only in `main`, targeted at 1.30,
due "late 2026".** On 1.28 you get software compositing, and on Ubuntu 24.04 you
get 1.24.2. Worse for us specifically: GES's Rust types are **not `Send` and not
`Sync`**, so a `ges::Timeline` cannot live in `tauri::State` at all, and the
whole IPC surface would have to become message passing to an engine thread. MLT, by
contrast, is genuinely healthy — v7.40.0 on 2026-06-25, 323 commits in twelve
months, maintained jointly by Shotcut and Kdenlive developers — and its timeline
model maps onto our document almost exactly. It fails on three other axes: its
frame type is a CPU byte-plane struct and its position type is a 32-bit **frame
index**, both structurally incompatible with a wgpu compositor and a microsecond
document; both reference editors run timeline preview on **one** render thread
and stutter at 1080p on high-end hardware; and every module we would actually
need — `qtblend` for transform, `qtext` for text, `volume` for audio gain — is
GPL, so an LGPL-clean build cannot even express per-clip rotation. The only MLT
Rust binding ever published, `mlt-sys`, dates from **2018**; the one Rust project
with working bindings keeps them behind a default-off feature flag and uses
FFmpeg instead. The narrow borrow that *is* worth making is the one Gausian
made: use GStreamer as an optional **decode backend** behind our own trait, not
as the engine.

## What the hard part actually is

It helps to separate the layers, because the frameworks are sold as one thing
and are in fact several.

| Layer | Hard? | Do we have it? |
|---|---|---|
| Timeline document: clips, ranges, keyframes, undo | No — it is bookkeeping | Yes, tested |
| Demux/decode/encode | No — libavformat/libavcodec do it | Yes |
| Frame-accurate seek policy | Yes | Yes |
| A/V sync with audio as master | Yes | No |
| Hardware decode into a GPU texture | Yes | No |
| GPU compositing, multi-pass | Yes | Partially — single pass only |
| Reduced-res preview, proxies, render cache | Yes | Preview yes, proxies/cache no |
| Encoder flush correctness | Subtle, not hard | Yes |

Every framework in this document is strong on rows one and two and weak or
absent on rows four through seven. That asymmetry is the whole decision.

---

## Option 1 — GStreamer + GStreamer Editing Services

### What it gives us

GES is a real non-linear editing library, not a marketing claim, and — this is
worth stating up front because the download figures below invite the opposite
conclusion — **it is not abandoned**. The C library took 98 commits in the last
twelve months against 71 in the preceding year, a 38% increase, and the work is
architectural rather than janitorial: an internal-locking pass to make the API
thread-safe, a GPU backend auto-plugger, a task-pool context in `GESPipeline`, a
Valgrind leak sweep, use-after-free fixes in the asset cache. GES gets its own
section in every GStreamer release. The engine behind that is Igalia, whose
[2026 report](https://www.igalia.com/2026/01/05/Doing-Our-Share-for-the-Web-in-2025.html)
names GES enhancements explicitly; Thibault Saunier wrote 54 of the 67
substantive commits. The confusion arises because the GitHub repository named
`GStreamer/gst-editing-services` is a stale pre-monorepo mirror last pushed in
2018, and because Pitivi — GES's only desktop consumer — genuinely is dead.

`GESTimeline` is itself a `GstElement`, so a timeline drops into any pipeline;
`GESLayer` holds the user-visible arrangement, `GESTrack` the output streams,
`GESClip` and `GESTrackElement` the pieces, and `GESPipeline` is a
preview/render convenience wrapper
([docs](https://gstreamer.freedesktop.org/documentation/gst-editing-services/index.html)).
The composition engine underneath (`nlecomposition`) does frame-accurate seeking
across clip boundaries properly. Nested timelines, transitions, a project/asset
system with serialisation, and `GESEffect` — which builds an effect from a
`gst-parse-launch` string, so any GStreamer element becomes an effect — all
exist today.

Three capabilities are better than we assumed and should be recorded honestly:

- **Proxy editing is built in.** `ges_asset_set_proxy()` and the `proxy-target`
  property exist precisely to "substitute a `GESUriClipAsset` corresponding to a
  high resolution media file with the asset of a lower resolution stand in"
  ([GESAsset docs](https://gstreamer.freedesktop.org/documentation/gst-editing-services/gesasset.html)).
  GES does not *generate* the proxy — Pitivi transcodes them itself — but the
  substitution machinery, which is the fiddly part to retrofit, is there.
- **Speed and time remapping work**, through `ges_base_effect_register_time_property()`
  and `set_time_translation_funcs()` (since 1.18), with
  `scaletempo:rate`, `pitch:tempo`, `pitch:rate` and `videorate:rate`
  pre-registered as rate properties. Reverse playback landed in 1.26. There is
  no `GESClip:rate` convenience property; you attach an effect.
- **Audio automation is real**: the `GESAudioSource` bin is
  `audioconvert ! audioresample ! volume ! capsfilter`, with `volume` and `mute`
  exposed as keyframable child properties.

Per-clip geometry is where it starts to fall short. `GESVideoSource` exposes
exactly these child properties: `alpha`, `posx`/`fposx`, `posy`/`fposy`,
`width`/`fwidth`, `height`/`fheight`, `operator`, `zorder`, plus
`video-direction` from an inserted `videoflip`
([ges-video-source.c](https://github.com/GStreamer/gstreamer/blob/main/subprojects/gst-editing-services/ges/ges-video-source.c)).
That is position, scale, opacity, z-order — and nothing else.

Keyframes attach cleanly: every child property is a `GObject` property, so
`ges_track_element_set_control_source(elem, source, "posx", "direct-absolute")`
binds a `GstInterpolationControlSource` with `NONE`/`LINEAR`/`CUBIC`/
`CUBIC_MONOTONIC`.

Hardware decode is the strongest single reason to consider GStreamer. The old
`gstreamer-vaapi` plugin is gone — its tags stop at 1.19.2, and 1.28's release
notes state it "has been removed in favour of the va plugin" — replaced by `va`
in `gst-plugins-bad/sys/va`. On Linux the decoders `vah264dec`, `vah265dec`,
`vavp9dec`, `vaav1dec` carry `PRIMARY+1` and are auto-plugged; on Windows every
`va` element is `RANK_NONE` and the support is documented as experimental.
`d3d11`, `d3d12`, `nvcodec`, `qsv` and `applemedia` cover the other platforms,
and `vtdec_hw` on macOS outputs `memory:GLMemory` via IOSurface, which is
genuine zero-copy. On this machine's Raptor Lake iGPU (i7-1355U, iHD 24.1.0,
VA-API 1.20) `vainfo` reports VLD entrypoints for H.264, HEVC through
Main12/444, VP9 profiles 0–3 and AV1 Profile 0 — decode only for AV1, which
matches Raptor Lake's silicon.

### The Rust-specific blocker

This one deserves its own heading because it is specific to our stack and is not
a matter of degree.

**GES objects are not `Send` and not `Sync` in Rust.** In gstreamer-rs's
`Gir.toml` for the GES crate, `concurrency = "send+sync"` is set for exactly
seven types — `Asset`, `ClipAsset`, `EffectAsset`, `SourceClipAsset`,
`TrackElementAsset`, `UriClipAsset`, `UriSourceAsset` — and for none of the
twenty-odd others, including `Timeline`, `Clip`, `Layer`, `Track`,
`TrackElement`, `Pipeline`, `Project` and `Effect`. Plain `gst::Element` and
`gst::Pipeline` do carry `unsafe impl Send + Sync`; the GES types deliberately
do not.

`tauri::State` requires `Send + Sync`. So a `ges::Timeline` **cannot be held in
Tauri managed state at all**. The architecture that follows is forced: a
dedicated engine thread owning a `glib::MainContext`, every one of our
`#[tauri::command]` functions reduced to a message send plus a oneshot reply,
and our `EditCommand` layer re-expressed as messages rather than direct calls.
That is workable — it is what a GStreamer application looks like — but it is a
fixed cost imposed on the entire IPC surface, and it is at odds with
`CLAUDE.md`'s current rule that commands take the project lock, clone, and drop.

It is also being fixed upstream, in a way that is itself a risk: MR !10198,
"ges: add internal locking to enable multi-threaded GES usage", was merged
2026-04-15 and is described by its author as a **breaking change** — functions
returning borrowed references gain `_full` variants, and "the value macros can no
longer be used as lvalues". It is in `main`, targeted at 1.30. The Rust bindings
have not yet marked the types `Send`/`Sync` as of 0.25.3.

### What it costs us

The GPU compositing story is not what it appears. GES's own
[hardware-acceleration.md](https://github.com/GStreamer/gstreamer/blob/main/subprojects/gst-editing-services/docs/hardware-acceleration.md)
says plainly: "The default ranks pick the software `compositor`, so an HW
backend (Vulkan, GL, CUDA, D3D11/12, HIP, ...) only takes over after its
compositor is promoted." Promotion is done with `GST_PLUGIN_FEATURE_RANK` or
`gst_plugin_feature_set_rank()`. The element selector that makes this possible
(MR !11434, authored 2026-04-15, merged 2026-06-22) replaced every hardcoded
`gst_element_factory_make()` call in GES — and it is **absent from the `1.28.0`
tag** (released 2026-01-27), verified by fetching the file at that ref. It ships
in 1.30, still in development (`1.29.2`), due "late 2026". Ubuntu 24.04 is on
1.24.2. So adopting GES *today* means compositing in software; adopting it for
GPU compositing means shipping an unreleased GStreamer.

There is a trap inside the mechanism worth recording, from the design document:
promoting `glvideomixer` (a bin that also accepts system memory) yields
`strict=FALSE`, and every helper role gets wrapped as
`glupload ! gl-core ! gldownload` — an upload and download *per element*.
Promoting `glvideomixerelement` (which only accepts `memory:GLMemory`) yields
`strict=TRUE` and keeps the whole chain on the GPU. An editor wants the latter,
and nothing in the API makes that obvious.

Before the failure of nerve on which mixer to promote, there is a more basic
problem with the VA path: **`vacompositor` does not blend alpha**. Issue #5114
(2026-05-20) reports BGRA being treated as opaque, reproduced on 1.28.3 and
1.24. For an editor that is disqualifying — it is titles, overlays and every
crossfade. `vapostproc` has its own set (#4531: broken on Intel for NV21, ABGR,
xBGR, xRGB, ARGB and RGB16; Y42B crashes), and `vacompositor` additionally lacks
`sizing-policy` (#2755), composites wrongly at negative positions (#4245), and
stalls about two seconds on a flushing seek through a `tee` with
`force-live=true` (#5007) — which is precisely the scrubbing pattern.

**Correction to an assumption we started with:** proxy support does exist, in the
form of `ges_asset_set_proxy()`. What GES does not do is *generate* proxies or
render the timeline at reduced resolution; Pitivi transcodes its own, and its
manual explains that other containers are "slow and lack precision"
([manual](https://www.pitivi.org/manual/importing.html),
[0.96 release notes](https://blog.pitivi.org/2016/06/30/pitivi-0-96-cogito-ergo-proxy/)).
So the transcode pipeline and the proxy-management UI are still ours to write;
the asset-substitution plumbing is not.

The capability gaps against a CapCut-style inspector are broader than rotation
alone, and they compound:

- **No arbitrary rotation.** `framepositioner` has none; `videoflip` gives 90°
  steps and mirroring. Free rotation means an added effect per clip
  (`gltransformation` on GPU, `rotate` on CPU).
- **No crop.** Neither the positioner nor the compositor pads expose it. That is
  another added element (`videocrop`, CPU).
- **Three blend modes.** `GstCompositorOperator` is `source`, `over`, `add`.
  There is no multiply, screen or overlay.
- **No per-keyframe easing.** The interpolation mode belongs to the whole
  `GstControlSource`, not to individual control points, so a per-keyframe easing
  picker — which our format already supports and `ROADMAP.md` Phase 2 promises —
  cannot be expressed. Writing a custom `GstControlSource` is the escape hatch,
  and **it is not available from Rust**: `gstreamer-controller` ships no
  `subclass` module. The workaround is baking easing curves into dense control
  points.
- **Titles are `textoverlay`.** `GESTitleClip`'s own properties have been
  deprecated since 1.6. Animated titles mean rendering them ourselves into an
  image track — which is what we would have done anyway, but it is not something
  GES saves us.

Scalability has one known cliff worth knowing about before designing around it:
issue #4019 reports auto-transitions degrading exponentially with clip count —
800 clips taking over six minutes, 1,200 taking over twenty-one.

Binding maturity is the number that should stop the conversation.
`gstreamer-editing-services` 0.25.2 (2026-05-11) has **70,024 lifetime
downloads and 1,330 in the last 90 days**; `gstreamer` 0.25.3 has 9.1 million
and 1.5 million respectively, and `gstreamer-video` 6.7 million and 1.19
million ([crates.io](https://crates.io/crates/gstreamer-editing-services)).
Same repository, same maintainers, same release cadence — the GES binding is
maintained as part of the set, but effectively nobody is using it. Bugs in the
GES-specific surface will be found by us. The bindings are gir-generated from
GObject-Introspection metadata; we did **not** audit which GES functions have
manual overrides or are missing, so treat "complete" as unverified.

The API is documented as not thread-safe before 1.16 and, per GStreamer's own
documentation, developers are "strongly encouraged" to use ≥ 1.16. Our
architecture runs preview and export concurrently against one shared
compositor; GES would require a discipline we have not designed for. We did not
locate the current authoritative thread-safety statement in the 1.28 docs and
are not asserting more than this.

Packaging is a real, recurring tax, and 1.28 made it worse. The published
account of bundling GStreamer into a Tauri app on macOS
([altunenes](https://altunenes.github.io/posts/taurigst/)) requires
`install_name_tool` rewriting of every library to `@executable_path`, signing
each `.dylib` individually with a Developer ID, a wrapper script to set
`GST_PLUGIN_SYSTEM_PATH`/`GST_PLUGIN_PATH`/`DYLD_LIBRARY_PATH`, and three
entitlements: `com.apple.security.cs.allow-unsigned-executable-memory`,
`com.apple.security.cs.disable-library-validation` and
`com.apple.security.cs.allow-dyld-environment-variables`. Note what the second
of those means: we would ship an app with library validation disabled. Tauri
additionally does not sign `.dylib` files nested inside frameworks, which fails
notarization
([tauri#11992](https://github.com/tauri-apps/tauri/issues/11992),
[tauri#8075](https://github.com/tauri-apps/tauri/issues/8075)). GStreamer's own
macOS deployment page still contains the literal text
`FIXME: PackageMaker is dead we need a new solution` and references version
1.8.1, so there is no maintained upstream guidance. `GST_PLUGIN_SCANNER` is a
separate helper binary that gets `exec`'d, so it needs signing too and rubs
against the hardened runtime.

The size and shape of what gets bundled:

- Official 1.28.5 artefacts are a single Inno Setup **`.exe` of 840 MB** on
  Windows x86_64 and a **146 MB runtime `.pkg`** on macOS. **1.28 removed the
  MSIs and the merge modules entirely**, which is what the older
  "bundle the runtime MSI, run `msiexec` silently" guidance depended on.
- The full macOS framework unpacks to ~681 MB. An editor-relevant subset is
  about **343 MB universal2, so roughly 170 MB after thinning to one
  architecture**. GES itself is 3.2 MB of that; the codec and base-system
  packages are the weight.
- GES is a **separate, optional component** in cerbero
  (`('gstreamer-1.0-editing', False, False)`) and it drags in
  `gstreamer-1.0-devtools`/GstValidate as a dependency. Whether it is
  pre-selected in the shipped installers is something our two independent checks
  disagreed on; we did not settle it.
- **Static linking is not available to us.** `gst-full` exists, but the Rust
  path fails on `gst_init_static_plugins`, which Sebastian Dröge described on
  2024-11-30 as "currently WIP and does not work out of the box yet"; the
  enabling MR !7624 has been a draft since October 2024. Separately, GES's
  `meson.build` disables Python on static builds, which removes the OTIO and
  FCP-XML formatters, since 1.28 moved them into a Python plugin.
- The Cargo side has no vendoring and no fallback: `system-deps` requires
  `gst-editing-services-1.0.pc` on every build machine including CI, and
  `build.rs` exits on failure. There is no supported cross-compile path.

One more constraint specific to our shell: **Tauri v2 on Linux uses
webkit2gtk-4.1, which is GTK3**, so `gtk4paintablesink` — the sink every modern
GStreamer/Rust example uses, and the one Ferricast previews through — is not
available to us. The GTK3 `gtksink`/`gtkglsink` still exist, or we keep our own
`appsink` → protocol transport, which is what we already have.

Licensing, by contrast, is the one dimension where GES clearly beats MLT and it
is worth saying so plainly: GES and the GStreamer core, base, good and most of
bad are LGPL, and GPL is confined to `gst-plugins-ugly` and to `gst-libav` when
FFmpeg is built with x264/x265. Nothing in the *editing* surface — compositor,
positioner, control sources, the `va` decoders — is GPL. Under Option 2 the
equivalent modules are.

### What we would discard

`media/decoder.rs`, `media/provider.rs`, all of `render/`, all of `preview/`
except the protocol handler, and the encoder half of `export/` — call it 10,000
of the 11,725 lines in the media modules. The wgpu compositor goes, and with it
the plan in `ROADMAP.md` Phase 3 to run reverse-engineered GLSL ES effects as
WGSL, because those effects would have to become GStreamer elements instead.
The document model, the edit commands, the undo stack and the entire frontend
survive.

### The risk that bites in six months

We build the transform inspector. Free rotation is not expressible in GES's
positioner, so we add `gltransformation` per clip; crop needs `videocrop`;
neither is native to the memory type the software compositor uses, so each one
inserts an upload and a download. Preview drops below real time on a four-layer
timeline. The fix is the GPU compositor path, which requires promoting
`glvideomixerelement` specifically — and that machinery is in 1.30, unreleased,
so we are now building and shipping our own GStreamer on three platforms, with
per-dylib signing on macOS, no static linking, and no MSI on Windows. All of it
sits behind an engine thread we had to introduce because the types are
`!Send`, and every bug we hit in the GES-specific surface is ours to diagnose
first, because 1,330 downloads a quarter means nobody is ahead of us. The
underlying bet is on one contract at one consultancy continuing.

---

## Option 2 — MLT Framework

### What it gives us

MLT is in markedly better health than its reputation suggests, and this is the
option whose feature list comes closest to what a CapCut-style editor actually
needs. v7.40.0 was released 2026-06-25, v7.38.0 on 2026-04-22, v7.36.x in
December 2025 — a release roughly every two months, 323 commits in the last
twelve months, and the repository was pushed to on the day this document was
written ([github.com/mltframework/mlt](https://github.com/mltframework/mlt),
1,821 stars, LGPL-2.1). Maintenance is shared: Dan Dennedy (Shotcut) with 183
commits in that window, Brian Matherly with 55, and Kdenlive's Julius Künzel and
Jean-Baptiste Mardelle with 48 and 18 — Kdenlive's
[2026 status post](https://kdenlive.org/news/2026/state-2026/) describes
strengthened upstream collaboration and no intent to replace it. The bus factor
still leans heavily on Dennedy, who has roughly 57% of all commits since 2004.

The C API is small and pleasant to bind: 557 exported functions across 32 flat
headers, opaque structs, variadics confined to `mlt_log`, pkg-config
(`mlt-framework-7`) and a CMake config package. The model is producer / filter /
transition / consumer, with `mlt_playlist` as a track, `mlt_tractor` as
multitrack-plus-field, and the tractor itself being a producer again.

The document mapping is close to exact, and better than GES's:

| chukcut | MLT XML |
|---|---|
| Materials pool | top-level `<producer id>`, referenced many times |
| Segment with target/source range | `<entry producer in out/>`, position by order |
| Gap | `<blank length/>` |
| Track | `<playlist>` inside `<track>` |
| Keyframes | animated property strings |

`mlt_animation` is genuinely strong — stronger than our own keyframe model
today. The published
[property-animation docs](https://www.mltframework.org/docs/propertyanimation/)
describe discrete/linear/Catmull-Rom syntax with times expressible as frames,
clock or timecode and negative times relative to the end; the current
`mlt_types.h` in fact defines **34 interpolation types**, including full
sinusoidal/quadratic/cubic/quartic/quintic/exponential/circular/back/elastic/
bounce easing families. Animated types include `mlt_rect` and colour.

The service catalogue covers most of Phase 2: `qtblend` (keyframed rect,
keyframed `rotation` **and** `rotate_anchor`, 41 composition modes),
`link_timeremap` (keyframed speed map with pitch compensation), `qtext` /
`dynamictext` / `kdenlivetitle` / `typewriter` for text, a constant-power
crossfade `mix` transition, EBU R128 loudness, `rubberband`, and — the largest
lever — `filter_avfilter.c`, which registers **every FFmpeg avfilter**
automatically as an MLT service.

Hardware decode does exist, contrary to what MLT 7.22 (the version installed on
this machine) suggests. `producer_avformat.c` supports **vaapi, cuda/nvdec,
videotoolbox, d3d11va, dxva2 and vulkan**, selectable per-URL
(`file.mp4?hwaccel=vaapi&hwaccel_device=/dev/dri/renderD128`) or via
`MLT_AVFORMAT_HWACCEL` environment variables added in 7.32.0, with GPU-side
scaling (`scale_vaapi`, `scale_cuda`) added in 7.38.0.

### What it costs us

Two facts from the installed headers decide the architecture question, and
neither is recoverable by effort:

```c
/* /usr/include/mlt-7/framework/mlt_types.h */
typedef int32_t mlt_position;          /* frames, not time */

/* /usr/include/mlt-7/framework/mlt_image.h */
uint8_t *planes[MLT_IMAGE_MAX_PLANES]; /* system memory, always */
```

`mlt_position` is a frame index. `docs/architecture/project-format.md` gives the
reasons we chose microseconds — a 24 fps clip on a 30 fps timeline has no
integer frame position, and changing project frame rate would rewrite every
number in the document. Adopting MLT means either accepting that constraint or
maintaining a lossy translation at every boundary, forever.

`mlt_frame` carries CPU byte planes, so every filter, transition and the
compositor operate on system memory. MLT's own FAQ is candid about the
consequence: "MLT is not highly optimized for desktop playback … MLT must pull
decoded images back into system memory instead of just leaving it in the video
memory for display, and this memory transfer is a major bottleneck"
([FAQ](https://www.mltframework.org/faq/)). Hardware decode therefore lowers CPU
load without removing the readback. Shotcut's own release note for the feature
(v26.1.30, 2026-01-30) says so directly: "Do not expect to be blown away by
speediness … It does not seem to help much with seeking and scrubbing; proxies
are still key for that … It is not integrated with the GPU processing mode by
using so-called 0-copy." Kdenlive ships the same option **off** by default.

The preview threading model is the part that should worry us most. MLT
parallelises at frame level — each worker renders a whole frame through the
entire filter chain — and the consumer's `real_time` property sets both the
thread count and whether frames may be dropped. In practice **both reference
editors run preview with exactly one render thread**: Shotcut defaults to
`real_time=1`, Kdenlive hard-codes `±1`. Export uses `-N`. Dennedy stated the
position plainly in
[discussion #1078](https://github.com/mltframework/mlt/discussions/1078)
(2025-03-21): "my personal interest now is not in improving CPU-based speed as
most CPU-based [effects] in MLT are very limited: 8-bit and not color-managed."
The user-visible result is a Shotcut forum thread from February 2026 in which a
Ryzen 7 9700X with 64 GB and an RTX 3060 Ti stutters on **1080p** H.264 with
proxies enabled, 360p preview scaling and no filters
([thread](https://forum.shotcut.org/t/choppy-playback-even-with-proxy-and-preview-scaling-on/51102)).

The GPU story is more alive than commonly reported but not usable. Movit was
*not* removed from Shotcut — it was de-emphasised in 2018, **restored** in
v23.05.07, and folded into "Processing Mode" in v25.12.30 — but Kdenlive
hard-disables it (`m_gpuAllowed = false; // Disable movit until it's stable`),
and MLT issue #1095 ("MOVIT library is broken") was closed as *not planned*. A
new libplacebo module (`placebo.render`, `placebo.shader`, D3D11/Vulkan) was
merged **2026-07-12** — after v7.40.0, so it is in no release, and Shotcut builds
with `-DMOD_PLACEBO=OFF`.

**Licensing is the finding that decides this option, and it is a money risk
rather than an aesthetic one.** The core `libmlt-7`/`libmlt++-7` is LGPL-2.1, but
MLT's own `CMakeLists.txt` disables a list of modules when built with
`-DGPL=OFF`, and the list contains the things we need:

- **`MOD_QT6` is GPL.** Individual files such as `filter_qtblend.cpp` carry LGPL
  headers, but they are linked with a GPL-2.0+ `factory.c` into one
  `libmltqt6.so`. So **`qtblend` and `qtext` — the transform and the text
  renderer — are not usable in a proprietary or non-GPL product.**
- **`plus` is GPL-3** because of `subtitles.cpp` and `hsl.h`, which takes
  `affine`/`transition_affine` — the obvious Qt-free transform fallback — and
  `dynamictext` with it.
- **`volume` lives in `normalize`, which is GPL.** A non-GPL build has no volume
  filter.
- **frei0r itself is GPL-2.0-or-later**, so shipping the plugins ships GPL.
- `avformat` unconditionally compiles a GPL-2.0+ `link_avdeinterlace.c` that
  must be patched out by hand.

What survives an LGPL-clean build is core, avformat (patched), xml, sdl2,
rtaudio, gdk, oldfilm, kdenlive, opencv, sox, vorbis, rnnoise, placebo and
decklink. For per-clip transform that leaves only `transition_composite`:
position, size, opacity — **no rotation**. Which is to say the LGPL subset of
MLT cannot express our existing `Transform` struct. (There is a fourth path —
Meltytech LLC holds assigned copyright on core/avformat/normalize/resample/sdl/
xml and could in principle sell an exception — but we found no evidence such a
programme exists. See "what we could not determine".)

Rust bindability is thin but not the blocker it first appears. The only binding
published to crates.io is [`mlt-sys` 0.1.1](https://crates.io/crates/mlt-sys),
released **2018-07-25**, MLT-6 era, dead. A GitHub code search for
`mlt_factory_init` across all Rust repositories returns exactly one repo:
[`xirtus/Rook`](https://github.com/xirtus/Rook), a Rust editor created June 2026
with its own `mlt-sys` (bindgen 0.71 + pkg-config) and a safe wrapper — except
that the whole thing sits behind a feature flag that is **off by default**, with
a no-op FFI stub in its place, and Rook's real engine is FFmpeg decode plus a
CPU compositor. So it proves bindgen runs, not that anyone has driven MLT from
Rust. The C API is genuinely bindgen-friendly; the cost is the manual
refcounting (`mlt_properties_inc_ref`/`_dec_ref` with no ownership documented in
signatures) and a thread model MLT does not guarantee — Rook's blanket
`unsafe impl Send + Sync` is exactly the shortcut that fails later. A credible
safe wrapper is two to four weeks.

Non-Qt, non-C++ hosts do exist, and the two precedents are more useful than the
bindings question:

- **[Flowblade](https://github.com/jliljebl/flowblade)** — 3,076 stars, GPL-3,
  Python + GTK, v2.24.2 on 2026-05-29 — drives MLT in-process through the
  official SWIG Python bindings and has done for fourteen years. Tellingly it
  uses `cairoblend` and frei0r and references `qtblend` zero times.
- **[vean](https://github.com/Tshah-95/vean)** — 21 stars, AGPL-3, created
  2026-06-30 — is a **Tauri + TypeScript** editor over MLT and therefore the
  closest structural analogue to chukcut that exists. Its two decisions are the
  most valuable single input to this document. First, it **does not link
  libmlt**; it ships the `melt` binary as a sidecar and shells out, explicitly to
  keep the licence boundary ("that keeps vean AGPL-by-choice, not
  GPL-by-force"). Second, it **does not use MLT for preview at all**: its design
  notes reject rendering the timeline through `melt` as "the export pipeline
  misused for preview", with "seconds-to-minutes of latency for a
  sub-millisecond edit", and replace it with an in-house compositor. `melt`
  remains only the exact export path.

Packaging is the last cost. Shotcut builds everything from source (a 2,356-line
script for Linux/macOS, 1,423 lines of MSYS2 for Windows); Kdenlive uses KDE
Craft. There is **no vcpkg port and no Conan package**. MLT does ship a
`vcpkg.json` and MSVC compatibility code, but its `build-windows-msvc.yml`
workflow is currently `disabled_manually` and its last eight runs (2026-06-25 to
2026-07-03) all failed — so `x86_64-pc-windows-msvc`, which is what a Tauri app
targets, is aspirational. MinGW works, at the cost of CRT mismatch risk across
the FFI boundary. The Qt module manufactures its own `QApplication` when none
exists, needs `DISPLAY`/`WAYLAND_DISPLAY` or `QT_QPA_PLATFORM=offscreen`, and
formally expects to be constructed on the main thread — which `melt` handles
with a main-thread preflight and which would collide with NSApplication on macOS
and GTK/WebKitGTK on Linux inside a Tauri process.

One operational note: **CVE-2026-45184** allows remote code execution or file
exfiltration from a crafted project file
([KDE advisory](https://kde.org/info/security/advisory-20260508-1.txt)), and
Kdenlive 26.04.3 blocks command execution for MLT < 7.40. Any host that opens
`.mlt` files must validate them itself.

### What we would discard

Everything Option 1 discards, plus the microsecond time model at the engine
boundary, plus any near-term GPU effect path. And, if the licence forces
`transition_composite`, plus rotation — which would have to be reimplemented
anyway, meaning we would have discarded the wgpu compositor in order to rewrite
part of it in C.

### The risk that bites in six months

A licence audit before release finds `libmltqt6.so` in the bundle. `qtblend` is
the transform, so the transform goes; the fallback `transition_composite` has no
rotation, and `affine` is GPL-3 as well. We are now writing a rotation filter as
an LGPL MLT module — in C, against a 557-function API, with the world's only
Rust bindings — to recover a capability our wgpu compositor already had before
the migration. The alternative reading of the same risk is vean's: go
out-of-process with `melt` for licence safety, accept that preview cannot go
through it, and build our own compositor anyway — at which point MLT is our
exporter, not our engine, and the migration bought a file format.

---

## Option 3 — Higher-level Rust media crates

This option is covered at crate-by-crate depth in the companion
[`rust-crate-survey.md`](rust-crate-survey.md); what follows is only the part
that bears on the build-or-adopt decision. The short version: none of them is a
step up for an editor, and one of the names in the brief is not a video library
at all.

| Crate | Latest | Released | 90-day downloads | What it actually is |
|---|---|---|---|---|
| [`ffmpeg-next`](https://github.com/zmwangx/rust-ffmpeg) | 8.1.0 | 2026-03-18 | 3,216,463 | What we use. Safe wrapper over libav*. |
| [`rsmpeg`](https://github.com/larksuite/rsmpeg) | 0.18.0+ffmpeg.8.0 | 2025-08-24 | 34,293 | Thinner, fuller wrapper — **the only one with real HW wrappers**. Last commit eleven months ago. |
| [`rusty_ffmpeg`](https://github.com/CCExtractor/rusty_ffmpeg) | 0.17.0+ffmpeg.8.1 | 2026-04-10 | 47,247 | The `-sys` layer under rsmpeg; handles static/vcpkg/dylib linking. |
| [`ez-ffmpeg`](https://github.com/YeautyYE/ez-ffmpeg) | 0.15.0 | 2026-07-23 | 32,727 | `fftools/ffmpeg` ported to Rust — **built on top of `ffmpeg-next ^8.1`**, not instead of it. |
| [`video-rs`](https://github.com/oddity-ai/video-rs) | 0.11.0 | 2026-02-24 | 110,245 | ~3.6k lines over `ffmpeg-next`. Has hwaccel and seek; **no audio module at all**. |
| [`ffmpeg-sidecar`](https://github.com/nathanbabcock/ffmpeg-sidecar) | 2.5.2 | 2026-05-30 | 438,687 | Spawns the `ffmpeg` CLI and parses its output. |
| [`symphonia`](https://github.com/pdeljanov/Symphonia) | 0.6.0 | 2026-05-15 | 3,005,050 | Pure-Rust audio demux/decode. 0.6 is a two-year rework toward video/subtitles. |
| [`cpal`](https://github.com/RustAudio/cpal) | 0.18.1 | 2026-06-07 | 4,004,737 | Cross-platform audio device I/O. (0.17.2 is yanked.) |
| [`rubato`](https://github.com/HEnquist/rubato) | 4.0.0 | 2026-07-09 | 3,045,925 | The standard Rust resampler, for when speed ramps need one. |
| `vidyut` | 0.0.0 | 2023-09-01 | — | **A Sanskrit toolkit.** Not a media crate; the brief's inclusion of it is a name collision. |

Two corrections to assumptions this document started with, both against my own
earlier draft:

**`ez-ffmpeg` is not an alternative to `ffmpeg-next` — it depends on it.** Its
`Cargo.toml` requires `ffmpeg-next ^8.1.0` and `ffmpeg-sys-next ^8.1.0`. It is a
hand port of FFmpeg n7.1's `fftools/ffmpeg` (same function names: `ts_fixup`,
`video_sync_process`, `enc_open`), which makes it excellent at being the
`ffmpeg` command and wrong for an editor. Its own documentation disqualifies it:
`SeekMode::InputSeek` "does **not** promise keyframe alignment or exact-second
precision", and `frame_export`'s stated non-goals include "random access by
index/timestamp … out of scope". There is no `SwrContext` access, no
`AVFilterGraph` handle, and the raw FFI escape hatches under `src/raw/` are
deliberately `pub(crate)`. `VideoWriter` is documented as "constant frame rate,
video only, no per-frame PTS, no audio" — which by itself ends the conversation
for our exporter. It also shipped seven releases in twenty days fixing a
decoder-teardown double-free reproducing "at roughly 0.5% per run", a
worker-teardown use-after-free, and four shutdown deadlocks.

**`rsmpeg` is the right abstraction in the wrong maintenance state**, and the
"production at Lark/ByteDance" line — which I repeated — is **unproven**. The
author's email is `@bytedance.com` and the org is `larksuite`, but there is no
public statement of production use, and in issue #253 a contributor calls it "a
personal project of yours" to the maintainer directly. What is true is that
rsmpeg is the only one of these crates with genuine hardware wrappers:
`AVHWDeviceContext::create/create_derived`, `AVHWFramesContext::init`,
`set_hw_device_ctx`/`set_hw_frames_ctx`, and critically `set_get_format()` —
the callback without which hardware pixel-format negotiation cannot work. But
its last commit is **2025-08-24**; FFmpeg 8.1 support PRs #251 and #252 have sat
unreviewed since April and May 2026, as has a RUSTSEC advisory fix; the
maintainer is demonstrably still active elsewhere. That is neglect rather than
abandonment, which is worse to plan around. **Read
`rsmpeg/src/avutil/hwcontext.rs` and `src/avcodec/codec.rs` as a reference
implementation — roughly 500 lines of exactly the safe wrappers we should layer
over `ffmpeg-next` ourselves — and do not take the dependency.**

**`video-rs`** is ~3,600 lines over `ffmpeg-next`. It does have
`Decoder::seek`, `seek_to_frame`, `decode_raw` and
`DecoderBuilder::with_hardware_acceleration`, so it is not purely cosmetic — but
there is no audio module in the tree at all, which makes it structurally unable
to serve an editor. Its README calls it a work in progress that "will contain
bugs", and its authors intend to replace it with
[`rave`](https://github.com/oddity-ai/rave), itself "under heavy development and
not ready for use".

**`ffmpeg-sidecar`** drives the `ffmpeg` binary over stdio. Good for batch
transcoding, a downgrade from what we have: no in-process seek control, no
decode loop, a process spawn per request.

### The thing that actually matters here: our own crate has a soundness bug

`ffmpeg-next` woke up. Its nominal owner (zmwangx) has made one commit since
2020; since June 2026 the de-facto maintainer is **Adrian Eddy, the author of
Gyroflow**, who merged FFmpeg 9.0 support on 2026-07-20 and moved the crate to
edition 2024. Master is 34 commits ahead of v8.1.0 with `version = "9.0.0"`.

Among those 34 commits is one we need. The unreleased changelog reads: *"Fix
unsound `Send` impls: replace the non-atomic `Rc` keep-alive shared by format
contexts and stream-derived `codec::{Context, Parameters}` with `Arc`"* (PR
#267, merged 2026-06-10). **Our architecture is exactly the one that trips
this**: `media/provider.rs` caches a decoder per material behind a `Mutex` and
hands it to the preview thread, the exporter's worker threads and the waveform
reader. Master also fixes wrong audio plane pointers and linesize, and a leak in
`Sink::frame`/`Sink::samples` on frame reuse.

So the upgrade path is: move 6.1 → the 8.x line, but **do not ship released
8.1.0 if decode contexts cross threads** — pin a master revision or wait for
9.0.0. That is a real, dated, actionable finding about the stack we already
have, and it is worth more than any of the migrations in this document.

**The gap none of these crates fills** is still hardware decode. A code search
for `hw_device_ctx` across `zmwangx/rust-ffmpeg` **master** returns zero hits;
PR #274 has been open since 2026-07-17 and Eddy's own review says the proposed
API cannot express a decode-HW → encode-HW zero-copy path. The raw symbols are
present in our generated `ffmpeg-sys-next` bindings, so VAAPI decode is reachable
from where we stand — via unsafe FFI we write ourselves.

We do not have to write it from nothing, though, and this is the most useful
thing in this section. Three permissively-licensed implementations of precisely
that layer exist:

- **[`AdrianEddy/oxivideo`](https://github.com/AdrianEddy/oxivideo)**, branch
  `legacy`, crate `gpu-video` — **MIT OR Apache-2.0** — is explicitly "a refactor
  of Gyroflow's ffmpeg code": the FFmpeg and hardware-decode layer of a shipped
  9,000-star application, without Gyroflow's GPL. Includes
  `support/ffmpeg_hw.rs`, a buffer pool, and BRAW/R3D decoders. The encoder side
  is a stub and `main` was reset for a planned reboot, so take the `legacy`
  branch as source to read and lift.
- **[`jazzfool/ffgpu`](https://github.com/jazzfool/ffgpu)** — Apache-2.0 —
  "zero-copy hardware decoding bridge, from FFmpeg to WGPU". Small and stale, but
  exactly on target.
- **`ez-ffmpeg/src/wgpu_filter/hw_interop.rs`** — MIT/Apache — VAAPI decoder
  surface → `av_hwframe_map` to a DRM_PRIME dmabuf → two `VkImage` (R8 and RG88)
  → `wgpu::Texture` with an NV12 conversion pipeline, and an
  `av_hwframe_transfer_data` fallback. The crate is wrong for us; this file is
  the answer to the single-plane limitation noted above.

### What we would discard

Nothing. The useful actions here are an upgrade and a borrow, not a migration.

### The risk that bites in six months

We stay on `ffmpeg-next` 6.1 because it works, and then hit the unsound `Send`
under load — a heisenbug in the preview thread that only appears when an export
runs concurrently, which is the exact scenario `render_smoke` does not cover.
The mitigation is to schedule the 8.x upgrade deliberately rather than letting
the pin rot.

---

## Option 4 — Adopt or fork an existing Rust editor

We searched GitHub for Rust video editors sorted by stars, and separately for
`topic:nle`, `media framework language:Rust` and `compositing engine
language:Rust`. The headline is worth stating first: **the "MLT of Rust" slot is
empty.** Nobody in Rust has both in-process libav decode *and* a reusable engine
crate. Every project below is either an application rather than a framework,
shells out to the `ffmpeg` binary, or uses platform codecs only. The star mass
in this category is not in Rust at all —
[OpenCut](https://github.com/OpenCut-app/OpenCut) has 78,675 stars (TypeScript,
MIT, "the open-source CapCut alternative") and
[palmier-pro](https://github.com/palmier-io/palmier-pro) 12,149 (Swift,
GPL-3.0).

| Project | Stars | Last push | License | Verdict |
|---|---|---|---|---|
| [Cap](https://github.com/CapSoftware/Cap) | 20,341 | 2026-07-24 | **AGPL-3.0** (`cap-camera*`/`scap-*` MIT) | Serious, ~205k lines of Rust. AGPL on every crate we would want. |
| [Gausian](https://github.com/gausian-AI/Gausian_native_editor) | 1,003 | 2026-07-01 | NOASSERTION | Architecturally our mirror; see the caveat below. |
| [Gyroflow](https://github.com/gyroflow/gyroflow) | 9,225 | 2026-07-16 | GPL-3.0 + §7 exceptions | Not an editor, but the best GPU-buffer abstraction in Rust. |
| [Olive](https://github.com/olive-editor/olive) | 9,099 | **2024-12-05** | GPL-3.0 | C++. Dead; cause now established (below). Fork [`oak`](https://github.com/OakVideoEditorCommunity/oak) is alive. |
| [`lzw5399/video-editor`](https://github.com/lzw5399/video-editor) | 1 | **2026-06-27** | **MIT** | Abandoned after 11 days — and the closest architectural match that exists. Read it. |
| [`cutlass`](https://github.com/1mrnewton/cutlass) | 46 | 2026-07-25 | MIT/Apache-2.0 | Active, ~138k lines, **zero FFmpeg** — platform codecs only. No Linux backend. |
| [Rook](https://github.com/xirtus/Rook) | 0 | 2026 | — | MLT bindings **disabled by default**; real engine is FFmpeg + CPU compositor. |
| [dalang](https://github.com/iyxan23/dalang) | 16 | 2024-01-22 | GPL-3.0 | Rust + Actix; **MLT was never actually wired in**. |
| [moviola](https://github.com/rntrtul/moviola) | 7 | 2025-10-04 | MIT | GStreamer appsink → wgpu with Vulkan zero-copy. Technically the most interesting corpse. |
| [`opentimelineio`](https://crates.io/crates/opentimelineio) | — | 2020-10-20 | — | Name placeholder. No usable Rust OTIO binding exists. |

**Cap** is the most capable Rust editor codebase in existence — 46 crates, a real
wgpu compositor with twelve WGSL shaders, a ±45-frame decode cache with PTS-hole
tracking, a GPU-side RGBA→NV12 export path, and hardware-encoder selection with
*measured* throughput ceilings per backend. It is also AGPL-3.0 on precisely
those crates (`cap-rendering`, `cap-editor`, `cap-export`, `cap-video-decode`,
`cap-gpu-converters`), with no dual licence and no CLA, so none of it can be
copied. Two things in it are worth knowing anyway. First, its timeline is **not**
an NLE timeline — one video track of linear trims plus typed parallel segment
lists, two transition kinds, keyframes essentially only for masks. Second, and
more useful to us: `crates/rendering-skia/README.md` announces "a new rendering
backend for Cap using Skia, designed to replace the current wgpu-based
renderer", with the rationale "no manual shader management, built-in effects and
filters". **A well-funded team with twelve hand-written shaders has concluded
that hand-written WGSL does not scale**, which is a datapoint to weigh against
`ROADMAP.md` Phase 3.

**Gausian — with a caveat that deflates it.** Its architecture is a near-mirror
of ours: crates for `timeline` (graph, tracks, commands), `project` (SQLite,
assets, **proxy** and job tables), `renderer` (wgpu + WGSL), `media-io`, and
`native-decoder` = "VideoToolbox (macOS) + optional GStreamer backend". That
split — own the timeline and renderer, treat GStreamer as one decode backend
among several — is the same conclusion this document reaches, and arriving at it
independently is meaningful. But it is a young project, and the reading of its
tree is that the schema is far ahead of the engine: a rich node/automation/
keyframe model, with a preview that shows a single clip and an export that is
essentially `ffmpeg concat`. Treat it as corroboration of the *architecture*,
not as evidence that the architecture has been carried to a shipping product.
The lesson pointing back at us is the one it demonstrates by accident: schema
and renderer have to grow together.

**The reference nobody starred.**
[`lzw5399/video-editor`](https://github.com/lzw5399/video-editor) has one star,
was abandoned four weeks after it started, and is MIT-licensed — and its layering
is the design this document is arguing for, already built: `draft_model` (a
canonical schema on an **integer time model**, with schemars + ts-rs generating
the TypeScript types) → `draft_commands` (snapping, main-track magnet, undo/redo)
→ `engine_core` → `render_graph` (a typed render *intent*) → **two consumers**, an
`ffmpeg_compiler` for export and a `realtime_preview_runtime` for preview, with
**parity tests between them**. Roughly 75k lines of source and 50k of tests,
including a 48 KB `preview_export_parity.rs`. It also has an explicit CapCut
draft-compatibility track. Whatever one thinks of how it was produced, it is MIT
and it is the closest thing to a blueprint for our exact problem. Our own
guarantee that preview and export agree comes from `render_frame` being a pure
function; theirs comes from testing two backends against each other. Both are
defensible and ours is cheaper — but their test corpus is worth reading.

**Olive's death is no longer undetermined.** Matt Hyland said it himself in a
March 2025 development update: "in 2023 I paused work on Olive because at the
time I was going through a rough time both **personally and financially**", and
"Olive in its last incarnation was **becoming a bit too big for one person to
manage**, especially when **it wasn't even my full-time job**". Core contributor
ThomasWilshaw corroborated it in issue #2395 (2026-04-02). Solo maintainer, side
project, financial crisis, scope explosion — no fork war, no licence dispute.
The stated plan was another rethink (node compositing in C#, rendering on
Godot); nothing public has appeared since.

What is worth taking from Olive is a design, not code. Its **`NodeTraverser`
emits jobs rather than pixels** — `ShaderJob`, `ColorTransformJob`, `SampleJob`,
`GenerateJob`, `FootageJob`, `CacheJob` — with the processing hooks empty in the
base class, so the traverser can compute timing, metadata and dependencies with
no GPU involved at all, and only the render processor implements them against a
real backend. In Rust that is a `trait Traverser`, an `enum Job`, and a wgpu
`impl`. It is the cleanest effect-graph design in any real NLE, and it is
directly applicable to the render-graph rewrite recommended below.

Two non-Rust projects belong here as structural precedents, both covered in
Option 2: [Flowblade](https://github.com/jliljebl/flowblade) proves a non-Qt host
can drive MLT in-process, and [vean](https://github.com/Tshah-95/vean) is the
only project with chukcut's exact shell — Tauri + TypeScript over MLT — and it
**stopped using MLT for preview**, calling that "the export pipeline misused for
preview", and wrote its own compositor. Adopting an engine did not spare it the
work we are debating.

**Why the dead Rust ones died**, and this is the most informative part of the
whole survey: across seven abandoned projects — dalang, `core_video_editor`,
`crocus`, `escher`, `stunts`, `moviola`, `ninve` — **not one died from Rust,
wgpu, or FFmpeg bindings**. They died from one person, spare time, and scope.
Four of the seven never had a working timeline model at all; they stalled at
"decode a frame and show it". `escher` is the sharpest example: its last three
commits are a refactored FFmpeg binding, an attempted scheduler refactor, and a
framebuffer experiment — a task scheduler and a scene graph built instead of an
editor. The two that survived did so by cutting scope to a single tool. Decode
and preview is a fortnight; timeline, compositing and export is not — and we are
already past that wall, which is the one genuine argument from our position that
is not sunk cost.

For the specific problems we have not solved, the most directly instructive
reference is a two-star toy:
[`rs-wgpu-video-player`](https://github.com/singh-ps/rs-wgpu-video-player), which
does hardware decode (D3D11VA/VAAPI/VideoToolbox) with software fallback and
audio-master sync against the cpal sample-consumption clock, and documents its
own weaknesses honestly — no PLL, so long playback drifts when the source rate
differs from the device rate, and `av_hwframe_transfer_data` into swscale rather
than a zero-copy path. That is exactly the shape of the code we need to write,
including the mistakes to avoid.

### The risk that bites in six months

Forking Cap: a licence audit before a commercial release finds AGPL-3.0 code in
the render path and the product cannot ship without opening the whole source
tree. This one is cheap to avoid — the answer is simply no.

---

## Comparison

| Dimension | Custom (today) | GStreamer + GES | MLT | Rust crates | Fork an editor |
|---|---|---|---|---|---|
| Timeline model | Ours, microseconds, tested | Nanoseconds, complete | Frame-indexed (int32) | None | Varies |
| Per-clip transform | Full (pos/scale/rot/opacity) | pos, size, alpha, z, 90° flips. **No free rotation, no crop** | `qtblend`: full incl. rotation + anchor — **but GPL** | None | Varies |
| Blend modes | Ours to define | **Three**: source, over, add | 41 via `qtblend` (GPL) | None | Varies |
| Keyframes | Ours, per-segment, easing | `GstControlSource` — **easing is per-curve, not per-keyframe** | `mlt_animation`, 34 easings — better than ours | None | Varies |
| Rust integration | Native | **GES types are `!Send`/`!Sync`** → engine thread + message passing | Bindings do not exist | Native | Native |
| Custom GPU effects | wgpu + WGSL, our roadmap | Must become GStreamer elements | Movit (OpenGL) or libplacebo (**unreleased**) | ez-ffmpeg has wgpu filters | Gyroflow: wgpu |
| GPU compositing | Yes, single-pass | Software by default; GPU path lands in **1.30** | No — CPU frames by construction | n/a | Gausian: wgpu |
| HW decode (VAAPI/RPL) | **No** | Yes, `va` plugin | Yes since 7.32, **no zero-copy** | No safe API in any crate | Gausian: yes |
| A/V sync | **Not built** | `GstClock`, solved | Solved (sdl/rtaudio consumer) | cpal gives a clock, not a policy | Yes |
| Preview throughput | Ours: readback + JPEG | Pipeline-paced | **One render thread** in Shotcut and Kdenlive | n/a | Gausian: wgpu |
| Proxies | Not built | Substitution API exists; **generation is yours** (Pitivi's is its own) | Not provided (Kdenlive/Shotcut roll their own) | n/a | Gausian: proxy tables |
| Reduced-res preview render | Yes (proxy render + ring) | Not provided | Not provided | n/a | — |
| Rust binding quality | n/a — native | gir-generated; **1,330 dl/90d** for GES | **None working**; 2018 crate, one stub | Native | Native |
| Upstream health | Ours | GES ~70–95 commits/yr; Pitivi ~dormant | 323 commits/yr, release every ~2 months | ffmpeg-next very active | Cap/Gausian active |
| Licence | Ours | LGPL core, GPL in bad/ugly | LGPL-2.1 core, **transform/text/volume are GPL** | MIT/Apache | **Cap is AGPL-3.0** |
| Win/macOS packaging | FFmpeg only (unsolved) | Heavy: dylib signing, entitlements, plugin paths | Heavy: 30 modules + Qt6; **MSVC CI red**, no vcpkg port | Moderate (vcpkg/static) | n/a |
| Code discarded | — | ~10,000 lines | ~10,000 lines + time model | ~2,500 lines | ~everything |

---

## The specific hard problems

**Frame-accurate seeking.** Solved here, and solved well. `media/decoder.rs`
seeks to the keyframe at or before the target, decodes forward, and retries from
progressively earlier when the container index lies — with a documented
threshold below which it decodes forward instead of seeking. GES solves it too,
inside `nlecomposition`, but through a pipeline whose seek behaviour we would
not control. MLT solves it at frame granularity by construction. This row is not
a reason to migrate.

**A/V sync with audio as clock master.** Not built. `preview/clock.rs` already
defines a `TimeSource` trait with a `MonotonicSource` implementation and
comments describing the intended audio master, so the seam exists — but there is
no audio in the tree at all: no `cpal`, no `rodio`, no `symphonia`, and
`export/audio.rs` mixes against a `SilentAudioSource`, meaning **every export
currently ships a silent audio track**. This is the single largest hole and the
strongest argument any framework has, because GES and MLT both solve it. It is
also 12–20 days of work with a known shape, and cpal's `StreamTrait::now()`
exposes the device clock for exactly this ([cpal
#279](https://github.com/RustAudio/cpal/issues/279)).

**Real-time preview at reduced resolution.** Solved here, and essentially not
provided by either framework — the point that most inverts the usual intuition.
GES has the one genuine exception: `ges_asset_set_proxy()` swaps a
high-resolution asset for a stand-in, which is the fiddly half of proxy support.
Neither framework generates the proxies or renders the timeline at reduced
resolution. Our
`preview-pipeline.md` design (proxy render resolution chosen from canvas aspect,
ring buffer, session ids invalidating stale frames, JPEG over
`chukcut-frame://`) is machinery Pitivi and Kdenlive each had to build for
themselves on top of their engines. The MLT case is sharper still: Shotcut and
Kdenlive both drive preview with a single render thread and still stutter at
1080p on hardware far better than our target, which is why proxies are
mandatory in both.

**Hardware-accelerated decode on Raptor Lake.** Not built, and no Rust crate
provides it. The hardware is capable — this machine's iHD 24.1.0 driver reports
VLD entrypoints for H.264, HEVC through Main12/444, VP9 profiles 0–3 and AV1
Profile 0. The path is `av_hwdevice_ctx_create(AV_HWDEVICE_TYPE_VAAPI)`, a
`get_format` callback selecting `AV_PIX_FMT_VAAPI`, then either
`av_hwframe_transfer_data` to system memory (simple, what
`rs-wgpu-video-player` does) or DMA-BUF export into a wgpu texture. All three
symbols exist in our generated `ffmpeg-sys-next` bindings.

The zero-copy half of that got materially easier very recently, and it is worth
recording because it changes the calculus. `wgpu-hal` **30.0.0 — the exact
version in our `Cargo.toml`** — added
`vulkan::Device::texture_from_dmabuf_fd(fd, desc, drm_modifier, stride, offset)`
(verified by reading `wgpu-hal-30.0.0/src/vulkan/device.rs:525` in our own
registry). Unix-only, Vulkan-only, and it needs `AV_PIX_FMT_DRM_PRIME` on the
FFmpeg side, but it means VAAPI-decoded frames can in principle reach a wgpu
texture without a round trip through system memory — in the stack we already
have, with no framework adopted. One caveat that matters: the method's own
documentation says it "currently only supports single-plane DMA-bufs", and
`vah264dec` gives us NV12, which is two planes. The way through is the one
`ez-ffmpeg`'s `hw_interop.rs` takes — map the DRM_PRIME descriptor to two
`VkImage`s (R8 and RG88) and convert in a shader — or convert to a single-plane
format on the GPU first. The companion
[`rust-crate-survey.md`](rust-crate-survey.md) in this directory works the path
out in crate-level detail.

So this is the one row where GES would genuinely save us effort — perhaps 8–15
days of it, against the cost of everything else in Option 1, and against a
do-it-ourselves path that has three permissively-licensed reference
implementations and just got shorter.

**GPU compositing of many layers.** Partially solved. The compositor is honest
about its limits in its own module documentation: "no render graph, no
intermediate pass and no per-segment target: a straight painter's-algorithm loop
into a single attachment". That is correct for source-over compositing and
insufficient for transitions, masks, multi-pass effects or a colour-managed
pipeline — all of which Phase 2 and Phase 3 require. GES composites in software
until 1.30. MLT composites on the CPU, period.

**Encoder flush correctness.** Solved, deliberately, and documented in
`export/encoder.rs`: `send_eof` followed by draining until `Eof`, for video and
audio separately, with the reasoning ("an encoder holds frames: lookahead,
B-frame reordering") written down. This is the class of bug that produces a file
missing its last second, and it is already right.

---

## Where our own stack actually stands

Stated without credit for effort already spent. The measurements: 14,172 lines
of Rust across eight modules with 258 `#[test]` functions, plus 9,394 lines of
TypeScript with 169 Vitest cases.

### Genuinely good

- **The document and edit layer** (`project` 1,015 + `timeline` 826 +
  `workspace` 438 lines). Microsecond times, target/source ranges, normalised
  transforms, per-segment keyframes, validation that separates "the world is
  wrong" from "we have a bug". It is orthogonal to the media layer and survives
  every option in this document — which also means no option can claim credit
  for replacing it.
- **The seek policy.** The retry ladder for containers with unreliable indexes
  is the kind of thing that is only written after it has failed once.
- **Compositor purity.** `render_frame` is a pure function of project and time,
  which is why preview and export cannot disagree. Neither GES nor MLT offers
  that guarantee structurally; they offer one pipeline run in two modes.
- **The preview transport decision.** `preview-pipeline.md` weighed four options
  and picked the one that keeps the engine boundary identical if we later switch
  to hole punching. That reasoning holds regardless of what sits behind it.
- **Encoder flush and hardware-encoder detection.** `export/hwaccel.rs` probes
  without initialising any device, inside `catch_unwind`, and defaults to
  software because hardware encoders are "meaningfully worse at the same
  bitrate" in a vendor-dependent way. That is a considered position, not a
  default.

### Genuinely weak

1. **Audio does not exist.** Not "is being built" — absent. Exports are silent.
   This is the biggest risk in the project and it is unrelated to which
   framework we choose.
2. **No hardware decode.** Software decode of 4K HEVC on a 15-watt U-series chip
   will not sustain multi-layer preview, and the iGPU that could do it is idle.
3. **The compositor is single-pass.** Transitions, masks, adjustment layers and
   the Phase 3 effect runtime all need intermediate targets. That is a rewrite
   of `render/compositor.rs`, not an extension — and it is the piece most likely
   to be underestimated.
4. **Preview throughput is bounded by readback plus JPEG.** Fine at 960×540 with
   two layers; the ceiling arrives with ten.
5. **Decode scheduling is naive.** One cached decoder per material, serviced
   serially. A ten-layer frame is ten sequential seek-and-decodes with no
   prefetch and no parallel budget.
6. **No cross-platform build story.** `ffmpeg-next = "6.1"` is pinned to the
   system FFmpeg, as `CLAUDE.md` records. Nothing in the tree builds or vendors
   FFmpeg, and Ubuntu 24.04's 6.1 is two major versions behind the crate's
   current 8.1.
7. **No proxies, no render cache.** Both are deferred to Phase 4 and both are
   what makes an editor feel fast.

Three of those seven — audio mixing policy, proxies, and the render cache — are
work we must do under *any* option, because no framework here supplies them.
Hardware decode is the one item a framework genuinely hands us: GES via the `va`
plugin, MLT via `producer_avformat`'s `hwaccel` since 7.32 but without
zero-copy. Everything else on the list is ours to write regardless. That is the
sunk-cost-free version of the argument, and it still points the same way.

---

## Migration cost estimates

Assumptions behind every number: one developer already fluent in this codebase,
counted in working days of focused work, excluding calendar overhead. "Done"
means feature parity with the app as it stands today — import, cut, transform,
preview, export — plus the audio work currently in flight, on Linux only.
Windows and macOS packaging is listed separately because it is deferrable in one
option and not in the others. Ranges are wide where the work depends on facts we
could not verify without building a prototype.

| Option | Days | Breakdown |
|---|---|---|
| **Stay and close the gaps** | **52–92** | Audio decode/resample/mix/device/clock 12–20 · VAAPI decode via FFI with fallback **6–12** (lowered: three permissive reference implementations exist) · render graph with intermediate targets 15–25 · proxy media + render cache 10–18 · `ffmpeg-next` 6.1 → 8.x/master upgrade **4–8** · cross-platform FFmpeg build 10–20, deferrable |
| **GStreamer + GES** | **100–175** | Learn GES/GObject from Rust 5–10 · **re-architect IPC around an engine thread because GES types are `!Send`/`!Sync`** 10–20 · document→timeline projection incl. incremental edits and undo mapping 15–25 · preview transport via appsink, no `gtk4paintablesink` under Tauri's GTK3 webview 8–15 · close the capability gap — free rotation, crop, blend modes, per-keyframe easing, animated titles 25–45 · export pipeline, presets, hw encoder selection 8–15 · Win/macOS bundling, per-dylib signing, entitlements, no static linking, no MSI 20–35 · rebuild test suite 10–15 |
| **MLT** | **95–175** | `mlt-sys` bindings + safe wrapper with correct refcounting and a defensible thread model 10–20 · frames↔microseconds reconciliation 10–20 · document→tractor/playlist projection 15–25 · preview transport from CPU frames 8–12 · re-express transforms as filters, abandon wgpu 15–30 · **licence remediation: LGPL transform/text/volume modules, or a commercial exception, or ship GPL** 5–15 · package MLT + modules (+Qt6) on Win/macOS with MSVC unresolved 20–35 · rebuild test suite 10–15 |
| **Upgrade the FFmpeg wrapper** (6.1 → 8.x) | **4–8** | Not optional in the long run, and it carries a soundness fix we need. Not a migration — an upgrade of the crate we already use. |
| **Swap to `rsmpeg`** | **10–20** | Rewrite `decoder.rs`, `probe.rs`, `encoder.rs` against a wrapper whose last commit is eleven months old. **No longer recommended** — read its hwcontext module instead. |
| **Fork an existing editor** | **n/a** | Cap is AGPL-3.0 on every crate we would want; Gausian's licence is unresolved and its engine is behind its schema. No candidate is adoptable. |

The comparison that decides it: **100–175 days to stand still (GES), or 52–92
days to move forward (stay).** Taking the most pessimistic figure for staying and
the most optimistic for GES, staying still wins — and the GES figure excludes the
permanent tax of being the party that finds the bugs in a binding downloaded
1,330 times a quarter, and of tracking an unreleased 1.30 for the two features
that make it worth adopting at all.

---

## What we could not determine

Stated explicitly rather than papered over:

- **Whether the GES 1.30 GPU compositing path performs.** The mechanism is
  documented, the merge date is known, and the strict/non-strict distinction is
  understood; nobody has published benchmarks and we did not build it.
- **Whether GES is pre-selected in the shipped installers.** It is an optional
  cerbero component (`('gstreamer-1.0-editing', False, False)`), but an
  inspection of the built macOS `.pkg` found the sub-package marked
  `start_enabled="True"`. Our two checks disagree and we did not resolve it. The
  Windows `.exe` could not be unpacked (Inno Setup).
- **Intel Raptor Lake with the `va` plugin in practice.** We verified the
  hardware's capabilities with `vainfo` and the plugin's ranks in source, but
  found no benchmarks or bug reports for this specific combination. Given
  `vacompositor`'s open alpha-blending bug, this would need a local test before
  being relied on.
- **DMA-BUF reliability end to end** — `va` → `glupload` zero-copy on Intel, and
  GL-texture import into a wgpu renderer. No public GStreamer↔wgpu zero-copy
  reference exists.
- **Whether Meltytech LLC sells a commercial exception for MLT's GPL modules.**
  They hold assigned copyright on several of them and could in principle, but no
  such programme was found. If MLT were ever seriously reconsidered, asking them
  is the first and cheapest step, because it decides whether `qtblend` is usable.
- **Why MLT's MSVC CI is failing** (logs not examined) and whether there is a
  target date for it going green. Also whether MSVC-built `mltqt6`/`mltfrei0r`
  render correctly at runtime — the CI only ever proved compilation.
- **Whether MLT's `hwaccel=vulkan` works end to end** in shipped builds. The code
  and build flags exist; no release note or user report confirms it.
- **Hard preview benchmarks for MLT.** No published figures exist for fps at a
  given resolution with and without hwaccel on defined hardware — only
  qualitative developer statements and forum reports. Measuring it would require
  building a prototype.
- **Who funds GES.** The evidence that Igalia's GES work is paid for by
  [Tella](https://www.tella.com) — a commercial screen-recorder/editor that
  maintains forks of both `gstreamer-rs` and `gst-plugins-rs`, into which
  Thibault Saunier and Carlos Bentzen commit directly — is circumstantial. There
  is no public statement. It matters, because it is the answer to "what happens
  to GES if one contract ends".
- **Gausian's licence.** GitHub reports `NOASSERTION` and we did not read the
  file, so it is not established that their code could be reused; and our
  characterisation of how far their engine has actually got is a reading of the
  repository, not a measurement.

Two items that *were* on this list and are now answered, recorded because the
answers changed the document: GES bundle sizes (about 170 MB thinned on macOS;
an 840 MB Windows installer) and whether anything commercial ships on GES
(Tella, probably). One that was answered from an unexpected direction: Olive's
stall, which its author explained himself.

One methodological caveat: GStreamer and GES were inspected through the
`GStreamer/gstreamer` GitHub mirror of the freedesktop GitLab monorepo, because
gitlab.freedesktop.org sits behind Anubis and could not be queried directly. The
mirror was current (pushed 2026-07-25) but a mirror can lag, and the
`GStreamer/gst-editing-services` repository on GitHub is a **stale** pre-monorepo
mirror last pushed in 2018 — which is exactly the sort of artefact that
generates false "GES is abandoned" conclusions. It is not abandoned. It is
small, it is funded by one company, and the parts of it we would need are
unreleased.

---

## Recommendation, restated as actions

1. **Finish audio**: decode, resample (`rubato` if speed ramps need it), mix,
   `cpal` output, and make the audio device the `TimeSource`. Nothing else in the
   roadmap is blocked by as much, and no option in this document removes the
   work.
2. **Upgrade `ffmpeg-next` 6.1 → the 8.x line**, and do not ship released 8.1.0
   if decode contexts cross threads — pin a master revision or wait for 9.0.0.
   The `Rc` → `Arc` soundness fix (PR #267) is master-only and our threading
   model is the one that trips it. This is the highest value-per-day item here.
3. **Add VAAPI decode** behind the existing `SourceProvider` seam, via
   `ffmpeg-sys-next` FFI, with software fallback and a per-machine capability
   probe modelled on `export/hwaccel.rs`. Do not write it from scratch: read
   `rsmpeg`'s `avutil/hwcontext.rs` for the safe-wrapper shape,
   [`oxivideo`](https://github.com/AdrianEddy/oxivideo)'s `legacy` branch
   (MIT/Apache — Gyroflow's hardware layer without the GPL) for the production
   version, and `ez-ffmpeg`'s `wgpu_filter/hw_interop.rs` for the DRM_PRIME →
   `VkImage` → wgpu path. Transfer to system memory first; pursue zero-copy
   through `wgpu-hal`'s `texture_from_dmabuf_fd` only if measurement demands it.
4. **Rewrite the compositor as a small render graph** with intermediate targets
   *before* building transitions, rather than after. Model it on Olive's
   `NodeTraverser`: a traversal that emits typed *jobs* rather than pixels, with
   the processing hooks empty in the base implementation, so timing and
   dependency analysis cost no GPU. In Rust that is a trait, an enum, and one
   wgpu implementation.
5. **Keep the decode backend behind a trait**, so a GStreamer or platform-native
   decode backend can be added per platform later — the Gausian and `lookout`
   split, the latter of which ships a 5 MB Windows installer by gating GStreamer
   to Linux — without any of it reaching the timeline, the compositor or the
   document.
6. **Read two codebases before writing more of our own**:
   [`lzw5399/video-editor`](https://github.com/lzw5399/video-editor) (MIT) for
   the draft → commands → render-graph → dual-backend layering with
   preview/export parity tests, and Cap's `PLAYBACK-FINDINGS.md` for what a
   decode cache and prefetch scheduler have to handle in practice. Reimplement,
   never copy, anything from Cap.
7. **Decide the effect model deliberately.** Cap's own team, twelve WGSL shaders
   in, is building a Skia backend to escape hand-written shaders. That is worth
   confronting before Phase 3 commits us to a GLSL → WGSL translation layer.

## Sources

GStreamer / GES —
[GES documentation](https://gstreamer.freedesktop.org/documentation/gst-editing-services/index.html) ·
[hardware-acceleration.md](https://github.com/GStreamer/gstreamer/blob/main/subprojects/gst-editing-services/docs/hardware-acceleration.md) ·
[ges-video-source.c](https://github.com/GStreamer/gstreamer/blob/main/subprojects/gst-editing-services/ges/ges-video-source.c) ·
[ges-smart-video-mixer.c](https://github.com/GStreamer/gstreamer/blob/main/subprojects/gst-editing-services/ges/ges-smart-video-mixer.c) ·
[gstreamer-rs](https://gitlab.freedesktop.org/gstreamer/gstreamer-rs) ·
[crates.io/gstreamer-editing-services](https://crates.io/crates/gstreamer-editing-services) ·
[GStreamer 1.26 release notes](https://gstreamer.freedesktop.org/releases/1.26/) ·
[Phoronix on 1.26](https://www.phoronix.com/news/GStreamer-1.26-Released) ·
[Pitivi](https://github.com/GNOME/pitivi) ·
[Pitivi manual, importing](https://www.pitivi.org/manual/importing.html) ·
[Pitivi 0.96 "Cogito Ergo Proxy"](https://blog.pitivi.org/2016/06/30/pitivi-0-96-cogito-ergo-proxy/) ·
[GStreamer 1.28 release notes](https://gstreamer.freedesktop.org/releases/1.28/) ·
[MR !11434 — registry-driven video element selection](https://gitlab.freedesktop.org/gstreamer/gstreamer/-/merge_requests/11434) ·
[MR !10198 — internal locking for multi-threaded GES](https://gitlab.freedesktop.org/gstreamer/gstreamer/-/merge_requests/10198) ·
[video-element-selection.md](https://github.com/GStreamer/gstreamer/blob/main/subprojects/gst-editing-services/docs/design/video-element-selection.md) ·
[GESAsset / proxies](https://gstreamer.freedesktop.org/documentation/gst-editing-services/gesasset.html) ·
[GESTrackElement / control sources](https://gstreamer.freedesktop.org/documentation/gst-editing-services/gestrackelement.html) ·
[compositor operators](https://gstreamer.freedesktop.org/documentation/compositor/) ·
[glshader](https://gstreamer.freedesktop.org/documentation/opengl/glshader.html) ·
[cerbero gstreamer-1.0-editing.package](https://gitlab.freedesktop.org/gstreamer/cerbero/-/blob/main/packages/gstreamer-1.0-editing.package) ·
[GStreamer legal information](https://github.com/GStreamer/gst-docs/blob/master/markdown/legal-information.md) ·
[Igalia multimedia in 2025](https://eocanha.org/blog/2026/01/26/igalia-multimedia-contributions-in-2025/) ·
[Pitivi drops the GPU GSoC idea](https://gitlab.gnome.org/GNOME/pitivi/-/commit/118cd60db013d0710a9fdad71a20a849fa500a18) ·
[ferricast — GTK4 + Rust + GES](https://github.com/AlexPiquard/ferricast) ·
[Bundling GStreamer with Tauri on macOS](https://altunenes.github.io/posts/taurigst/) ·
[gstreamer-full in a Rust project — static linking does not work](https://discourse.gstreamer.org/t/using-gstreamer-full-in-a-rust-project/3666) ·
[tauri#11992](https://github.com/tauri-apps/tauri/issues/11992) ·
[tauri#8075](https://github.com/tauri-apps/tauri/issues/8075)

MLT —
[mltframework/mlt](https://github.com/mltframework/mlt) ·
[framework docs](https://www.mltframework.org/docs/framework/) ·
[MLT XML](https://www.mltframework.org/docs/mltxml/) ·
[property animation](https://www.mltframework.org/docs/propertyanimation/) ·
[FAQ (real_time, readback bottleneck)](https://www.mltframework.org/faq/) ·
[copyright policy](https://www.mltframework.org/docs/copyrightpolicy/) ·
[discussion #1078 — maintainer on CPU performance](https://github.com/mltframework/mlt/discussions/1078) ·
[CVE-2026-45184 advisory](https://kde.org/info/security/advisory-20260508-1.txt) ·
[Kdenlive, State of 2026](https://kdenlive.org/news/2026/state-2026/) ·
[Kdenlive file format notes](https://github.com/KDE/kdenlive/blob/master/dev-docs/fileformat.md) ·
[Shotcut forum: choppy playback with proxies](https://forum.shotcut.org/t/choppy-playback-even-with-proxy-and-preview-scaling-on/51102) ·
[crates.io/mlt-sys](https://crates.io/crates/mlt-sys) ·
[Flowblade](https://github.com/jliljebl/flowblade) ·
[vean — Tauri + MLT via melt sidecar](https://github.com/Tshah-95/vean) ·
[Rook](https://github.com/xirtus/Rook) ·
[dalang](https://github.com/iyxan23/dalang) ·
plus first-party inspection of `/usr/include/mlt-7/framework/*.h` and
`melt -query` output from MLT 7.22.0 as installed on this machine.

Rust crates —
[ffmpeg-next](https://github.com/zmwangx/rust-ffmpeg) ·
[rsmpeg](https://github.com/larksuite/rsmpeg) ·
[rusty_ffmpeg](https://github.com/CCExtractor/rusty_ffmpeg) ·
[ez-ffmpeg](https://github.com/YeautyYE/ez-ffmpeg) ·
[video-rs](https://github.com/oddity-ai/video-rs) ·
[rave](https://github.com/oddity-ai/rave) ·
[ffmpeg-sidecar](https://github.com/nathanbabcock/ffmpeg-sidecar) ·
[Symphonia](https://github.com/pdeljanov/Symphonia) ·
[cpal](https://github.com/RustAudio/cpal) ·
[cpal#279, device clock for A/V sync](https://github.com/RustAudio/cpal/issues/279) ·
[rubato](https://github.com/HEnquist/rubato) ·
[oxivideo — Gyroflow's FFmpeg/HW layer, MIT/Apache](https://github.com/AdrianEddy/oxivideo) ·
[ffgpu — FFmpeg → wgpu zero-copy bridge](https://github.com/jazzfool/ffgpu) ·
[vidyut — a Sanskrit toolkit, not a media crate](https://github.com/ambuda-org/vidyut) ·
plus first-party inspection of `ffmpeg-next-6.1.1`, `ffmpeg-sys-next-6.1.0`'s
generated `bindings.rs`, and `wgpu-hal-30.0.0/src/vulkan/device.rs` in this
machine's Cargo registry.

Editors —
[Cap](https://github.com/CapSoftware/Cap) ·
[Gausian](https://github.com/gausian-AI/Gausian_native_editor) ·
[Gyroflow](https://github.com/gyroflow/gyroflow) ·
[Olive](https://github.com/olive-editor/olive) ·
[oak — the living Olive fork](https://github.com/OakVideoEditorCommunity/oak) ·
[Olive development update, March 2025](https://www.youtube.com/watch?v=invMlMRPUrM) ·
[lzw5399/video-editor](https://github.com/lzw5399/video-editor) ·
[cutlass](https://github.com/1mrnewton/cutlass) ·
[OpenCut](https://github.com/OpenCut-app/OpenCut) ·
[moviola](https://github.com/rntrtul/moviola) ·
[rs-wgpu-video-player](https://github.com/singh-ps/rs-wgpu-video-player) ·
[opentimelineio crate — a placeholder](https://crates.io/crates/opentimelineio)

Download counts and release dates come from the crates.io API; star counts,
push dates and commit counts from the GitHub API; both queried 2026-07-25.
Hardware capability figures come from `vainfo` on the target machine
(13th Gen Core i7-1355U, iHD 24.1.0, VA-API 1.20).
