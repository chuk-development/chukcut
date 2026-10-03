# 0003 — Proxy media

Status: decided 2026-07-26. Implementation in `src-tauri/src/modules/proxy/`.

## The decision

Build proxy media ourselves, as `modules/proxy`: a decision rule, a background
transcode queue, an LRU cache under `paths::proxies_dir()`, and a type-level
switch that lets the preview use a proxy and makes it impossible for the export
to.

Decision 0001 named this as one of the four things neither GES nor MLT gives
away — Pitivi transcodes its own proxies on top of GES, Kdenlive and Shotcut
theirs on top of MLT — so there was never an option to adopt one.

## The rule for when a proxy is worth making

Resolution alone is the rule everybody writes first and it is wrong in both
directions. The rule is a pure function of **source resolution, codec, and a
measured decode cost when one exists**, in `proxy::decision`:

```
budget    = 1000 / fps                              one whole frame, in ms
allowance = budget × 0.7 × (2.0 if intra-only)      what decode may have of it
cost      = measured, or megapixels × per-codec cost

build a proxy  ⟺  the proxy is at least 1.4× smaller on the long side
                  AND cost > allowance
```

- **0.7** because decode is not the only thing in a preview frame, but the JPEG
  encode overlaps compositing on another thread, so decode legitimately gets
  most of the budget rather than half of it.
- **The intra-only bonus** is the "an intra-only codec may not need one at all"
  clause. It is a multiplier and not an exemption because ProRes at 4K is
  genuinely slow and does deserve a proxy — it is *scrubbing* that intra-only
  codecs get for free, not throughput.
- **The shrink clause** comes first, because it is true regardless of cost: a
  proxy that is not smaller than the source is a slower copy of it.

The per-codec costs are calibrated on this machine and keep HEVC at twice H.264
and AV1 at twice HEVC, which is the ratio those codecs have on any CPU decoder.
A measurement, when one is passed, replaces the model outright.

The rule is tested against a twenty-one row table in `decision.rs`. Two rows sit
deliberately close to the line and are the two arguments worth having: **HEVC at
1080p30** (≈21 ms against a 23 ms allowance) and **H.264 at 1080p60** (≈10
against 12). Both are judged playable.

## The codec

**All-intra H.264, 4:2:0 8-bit, long side capped at 1280, in MP4, no audio.**

Long-GOP codecs are cheap per frame *because* their neighbours did the work,
which is exactly why reaching a frame means decoding the ones before it.
All-intra deletes that, and cheap seeking is what a proxy is for. Resolve and
Premiere both ship all-intra proxy formats for the same reason.

H.264 rather than ProRes or DNxHR, given that property:

- the fastest decoder in any FFmpeg build (measured: 5 ms per source megapixel
  single-threaded, against 7 for ProRes);
- the only codec with a hardware encoder on every platform, so proxy generation
  gets the GPU nearly everywhere — there is no hardware DNxHR encoder at all;
- a fraction of ProRes Proxy's bitrate, which is the difference between a 20 GiB
  cache holding four hours of footage and forty minutes;
- FFmpeg's `dnxhd` encoder refuses arbitrary resolutions and frame rates, so a
  generator built on it fails on odd footage.

`tune=fastdecode` on the software path and `coder=cavlc` on the hardware one:
both trade CABAC and the deblocking filter — decode-side costs — for a larger
file, which is the right direction for a file whose only purpose is to be
decoded. **How the proxy is encoded changes what it costs to decode by 50%**, and
the hardware encoder's default output was the most expensive of the four
variants measured. Numbers in `docs/STATUS.md` under "Proxy media, measured".

Hardware encoding is still preferred when `hwaccel::detect` reports a usable
H.264 encoder, because it is what the user waits on — but the measurement says
it is a close call: it saved **8%** of the transcode (the 4K decode and the
scale dominate, exactly as with the export) and cost 50% of the decode before
`cavlc` clawed most of that back. `CHUKCUT_PROXY_ENCODER=software` takes the
other side of the trade without a rebuild.

## The switch, and why it is a type

A proxy reaching the export is a silent 720p deliverable. Nothing fails, nothing
warns, and it is found after the upload. Comments do not prevent that and a
boolean threaded through six call sites prevents it until somebody adds a
seventh.

So `proxy::switch` has two types. `PreviewSource` is the only one that can hold
a proxy path, and the only accessor that returns it is called `decode_path`.
`ExportSource` has no field a proxy could live in, no `From<PreviewSource>`, and
a constructor that **refuses any path underneath `paths::proxies_dir()`**. The
second half is the belt under the braces and is the one that can be tested at
runtime; it is.

`MediaSourceProvider::from_project` keeps its name and its behaviour — no
proxies — and the preview gets a second constructor. The export path therefore
did not change at all.

## What it costs

- An RGBA round trip per frame in the transcode, because the loop is built out
  of `media::VideoDecoder` and `export::MediaWriter` rather than a second
  FFmpeg pipeline. It buys the seek policy, the VAAPI frame pool, the
  rate-control ladder and a correct encoder flush, all already tested. At proxy
  resolution it is a couple of milliseconds against a transcode dominated by
  decoding the 4K source.
- Disk, bounded by a 20 GiB cap with LRU eviction.
- The cache key is path + size + mtime + 8 KB from each end of the file. Size
  and mtime alone are **not** enough — the kernel stamps mtime at clock-tick
  granularity, so a file rewritten in place without changing length keeps its
  key forever, which is the same hole `media::thumbnails::fingerprint` was
  changed to close and there is a unit test for it here too. What 16 KB still
  cannot catch is a same-length change confined to the middle of a file whose
  mtime also did not move; content-hashing a 40 GB source at import is not a
  trade worth making.

## What would change our minds

- **Hardware decode landing in the compositor.** Once `render/` can import a
  decoded VA surface as a texture (`docs/research/hardware-decode.md`), 4K HEVC
  decode drops towards 2 ms a frame and most of this rule's output flips to "not
  needed". The rule would then be re-calibrated against the hardware path rather
  than deleted — the machines without a usable VAAPI decoder are still there.
- **A measured cost at import.** The model exists because measuring every file
  at import costs more than it saves. If a cheap measurement appears — decoding
  ten frames during the probe, say — it should be passed to `decide` and the
  table becomes a fallback.

## The two seams still open, with the exact patches

`modules/proxy` is complete and tested, and it does nothing until these land.
Both are in files other agents owned while this was written — `media/` and
`preview/` — so they are recorded here rather than applied. Neither is more
than a few lines of real change; the bulk below is the comments that explain
why the two constructors are two constructors.

```diff
Patch for src-tauri/src/modules/media/provider.rs
Written against the file as of commit 4da5cff + working tree, 2026-07-26.
Three hunks. Nothing else in the file changes, and both existing public
constructors keep their signatures and their behaviour.

--- HUNK 1: a private purpose marker, next to `enum MaterialSource` (~line 40)

/// Whether this provider is serving a preview or producing a deliverable.
///
/// Two constructors rather than a parameter on the public API, because the
/// distinction is not a tuning knob: a proxy reaching the export is a silent
/// low-resolution master. See `modules::proxy::switch`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Purpose {
    /// May decode a proxy in place of the original.
    Preview,
    /// Must decode the original. Always.
    Deliverable,
}


--- HUNK 2: replace lines 225-252 (`impl MediaSourceProvider` down to the end of
--- the video loop). Only the marked lines are new.

 impl MediaSourceProvider {
     /// Snapshot the material pool of `project`.
+    ///
+    /// For **rendering a deliverable**: never consults the proxy cache. An
+    /// export always reads the original, and the way to be sure of that is for
+    /// the path that produces a deliverable to have no way of asking.
     pub fn from_project(project: &Project) -> Self {
-        Self::from_project_with(project, None)
+        Self::snapshot(project, None, Purpose::Deliverable)
     }

     /// [`Self::from_project`], with the decoder chosen rather than inferred.
     ///
     /// For benchmarks and for the pixel comparison. Production wants `None`.
     pub fn from_project_with(project: &Project, forced: Option<Acceleration>) -> Self {
+        Self::snapshot(project, forced, Purpose::Deliverable)
+    }
+
+    /// Snapshot the material pool for the **preview**, substituting a proxy
+    /// wherever `proxy::ProxyCache` has a valid one.
+    ///
+    /// A caller that caches the result must invalidate it when
+    /// `proxy::ProxyQueue::shared().generation()` changes: a proxy finishing
+    /// does not change the material pool, so a fingerprint over the pool alone
+    /// will not notice it and the preview would keep decoding the original
+    /// until the next edit.
+    pub fn from_project_for_preview(project: &Project) -> Self {
+        Self::snapshot(project, None, Purpose::Preview)
+    }
+
+    fn snapshot(
+        project: &Project,
+        forced: Option<Acceleration>,
+        purpose: Purpose,
+    ) -> Self {
         let mut sources = HashMap::new();
+        let proxies = (purpose == Purpose::Preview)
+            .then(crate::modules::proxy::ProxyQueue::shared);

         for video in &project.materials.videos {
             // Rotation is a property of the container, and a 90°-rotated file
             // is taller than it is wide once displayed. Getting this backwards
             // would make the fit calculation pick the wrong axis.
             let display = if video.rotation % 180 == 0 {
                 (video.width, video.height)
             } else {
                 (video.height, video.width)
             };
+
+            let original = PathBuf::from(&video.path);
+            let decode = match proxies {
+                // The preview. `decode_path` is the only accessor in the
+                // codebase that can return a proxy, and this is the only place
+                // that calls it.
+                Some(queue) => {
+                    let source = queue.preview_source(&original);
+                    if source.is_proxied() {
+                        tracing::debug!(
+                            material = %video.id,
+                            proxy = %source.decode_path().display(),
+                            "previewing from a proxy"
+                        );
+                    }
+                    source.decode_path().to_path_buf()
+                }
+                // The export. `deliverable` refuses anything inside the proxy
+                // cache, and a material it refuses is skipped rather than
+                // exported soft: a missing clip is noticed, a low-resolution
+                // one is not.
+                None => match crate::modules::proxy::ExportSource::deliverable(&original) {
+                    Ok(export) => export.path().to_path_buf(),
+                    Err(error) => {
+                        tracing::error!(
+                            %error,
+                            material = %video.id,
+                            "skipping a material that points into the proxy cache"
+                        );
+                        continue;
+                    }
+                },
+            };
+
             sources.insert(
                 video.id.clone(),
-                MaterialSource::Video {
-                    path: PathBuf::from(&video.path),
-                    display,
-                },
+                MaterialSource::Video { decode, display },
             );
         }

--- HUNK 3: the `MaterialSource::Video` variant (~line 44) and its one use in
--- `SourceProvider::frame` (~line 526). A rename, so that a reader of
--- `video_frame`'s call site cannot assume it is looking at the original.

 enum MaterialSource {
     Video {
-        path: PathBuf,
+        /// The file the decoder opens: the proxy when this provider was built
+        /// by `from_project_for_preview` and one exists, the original
+        /// otherwise. Never the proxy on any path that produces a deliverable
+        /// — see `modules::proxy::switch`.
+        decode: PathBuf,
         /// Display dimensions, i.e. with container rotation applied. …
         display: (u32, u32),
     },

 …

         match source {
-            MaterialSource::Video { path, display } => self
+            MaterialSource::Video { decode, display } => self
                 .video_frame(
                     ctx,
                     request.material_id,
-                    path,
+                    decode,
                     request.source_time,
                     fitted_height(*display, request.max_size),
                 )
                 .map(Some),


--- AND, in preview/commands.rs (owned by the preview agent), two lines:

 fn source_provider_for(project: &Project) -> Arc<dyn SourceProvider> {
-    let fingerprint = material_fingerprint(project);
+    // A proxy finishing does not change the material pool, so the pool's own
+    // fingerprint cannot see it. Without this the preview keeps decoding the
+    // original until the next edit.
+    let fingerprint = material_fingerprint(project)
+        ^ crate::modules::proxy::ProxyQueue::shared().generation();
     …
-    let provider = Arc::new(MediaSourceProvider::from_project(project));
+    let provider = Arc::new(MediaSourceProvider::from_project_for_preview(project));
```

## Addendum 2026-10-03: the user's policy

Settings → Proxy media (Off / Automatic / Always) is applied in
`proxy::policy::decide_with_policy`, on top of `decide` rather than inside it,
so the measured rule stays a pure function of the file. Always keeps the shrink
clause and drops only the cost clause. The policy is a field of the queue, not
a global, and the shared queue starts Off until `workspace_settings_apply` runs.
`ProxyQueue::preview_source` returns the original under Off, so the provider
seam above inherits the policy without knowing about it. `docs/STATUS.md`,
"Proxy policy and cache limit take effect".
