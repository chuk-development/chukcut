# Hardware video decode on VAAPI

Written 2026-07-26, building the VAAPI decode path in
`src-tauri/src/modules/media/`. Everything here was measured or executed on the
Raptor Lake iGPU this project is developed on (`Intel(R) Graphics (RPL-U)`, iHD
driver 24.1.0, libva 1.20, FFmpeg 6.1.1 with `--enable-libdrm`). Where something
was not verified it says so.

`docs/research/zero-copy-encode.md` is the same problem pointing the other way,
and it ends by saying hardware *decode* should have been ranked higher than it
was. The numbers below agree with it, and then say something it did not expect:
hardware decode on its own is **not** faster. Only hardware decode that never
copies the frame to the CPU is faster, and it is faster by an order of
magnitude.

## The one-paragraph summary

VAAPI decode works here for H.264, HEVC, VP9 and AV1. Reaching RGBA from a
decoded surface costs *more* than decoding in software, because the download out
of tiled GPU memory plus an swscale pass costs more than the decode ever did.
Exporting the surface as DMA-BUF instead costs 1.7–2.4 ms per frame against
15–38 ms for the software path, and the exported buffers import into wgpu as two
textures that reconstruct the software decode's picture to a mean channel
difference of 0.3. The remaining work is in `render/`, and the patch is at the
bottom of this document.

## What this chip actually decodes

`export/hwaccel.rs` established the rule for encoders: being in the FFmpeg build
means nothing, so probe by encoding a frame. Decode has the same trap with the
sign reversed — this chip decodes AV1 and cannot encode it — so
`media/hwdecode.rs::capabilities` probes by decoding a frame, from a tiny
embedded bitstream per codec (`media/probe_streams/*.bin`, 3.3 KB in total,
Annex-B for H.264/HEVC and a bare IVF payload for VP9/AV1, so no demuxer is
involved).

| codec | in the build | declares VAAPI | decodes a real frame |
|---|---|---|---|
| H.264 | yes | yes | **yes** |
| HEVC | yes | yes | **yes** |
| VP9 | yes | yes | **yes** |
| AV1 | yes | yes | **yes** |

Encode, for contrast, is H.264 and HEVC only. The asymmetry the brief predicted
is real and it is in the direction it predicted.

### The trap that made AV1 look unsupported for an hour

**`avcodec_find_decoder(AV_CODEC_ID_AV1)` returns `libdav1d`,** which is an
excellent software decoder with no hardware support whatsoever. Attach a VAAPI
device to it and nothing happens: `get_format` is never offered
`AV_PIX_FMT_VAAPI`, every frame decodes on the CPU, and the code has no way of
knowing. The first version of the capability table above reported AV1 as
unavailable for exactly this reason, on a chip where `vainfo` plainly lists
`VAProfileAV1Profile0 : VAEntrypointVLD`.

The native `av1` decoder — which exists in the same build largely to drive
accelerators — is the one that works. FFmpeg's own CLI does this and says so:

```
Selecting decoder 'av1' because of requested hwaccel method vaapi
```

So `hwdecode::hardware_decoder` walks `av_codec_iterate` and picks the decoder
whose `avcodec_get_hw_config` advertises VAAPI, rather than the default one.
That is by capability rather than by a name table, so a future build that moves
the accelerator elsewhere keeps working.

The same shape of trap applies to VP9 (`libvpx-vp9` versus the native `vp9`) and
would apply to H.264 if anyone installed a competing decoder. It is worth
assuming this is true of every codec rather than remembering which.

## Measured: what a decoded frame costs

`cargo run --release --example decode_bench -- --dir target/bench-media`.
Sequential walk, first frame discarded, median of three runs of 90 frames. The
source clips are 20 seconds of the owner's own phone footage re-encoded at CRF
20 with a 60-frame GOP and B-frames, plus VP9/WebM transcodes of the same, which
is the shape YouTube delivers.

| clip | codec | sw → RGBA | hw → RGBA | hw → DMA-BUF | dmabuf vs sw |
|---|---|---:|---:|---:|---:|
| 1920×1080 | H.264 | 26.95 ms | 37.30 ms | **1.70 ms** | 15.8× |
| 1920×1080 | VP9 | 15.11 ms | 18.80 ms | **1.71 ms** | 8.8× |
| 1080×1920 | H.264 | 31.67 ms | 26.07 ms | **2.09 ms** | 15.1× |
| 1080×1920 | VP9 | 38.35 ms | 54.22 ms | **2.37 ms** | 16.2× |

Read the three columns as a decomposition, because they differ by exactly one
step each:

- `hw → DMA-BUF` is the decode and nothing else. **1.7–2.4 ms.**
- `hw → RGBA` adds `av_hwframe_transfer_data` and an swscale NV12→RGBA pass.
  That is **16–52 ms of copying and converting** on top of a 2 ms decode.
- `sw → RGBA` is libavcodec plus the same swscale pass, on the CPU.

Two conclusions follow and both matter.

**Hardware decode by itself is not a win.** In three of the four rows it is
*slower* than software. The download is not a memcpy: a VA surface is `Y_TILED`
(modifier `0x0100000000000002`), so reading it back is a detiling pass across
the whole frame, and on an integrated GPU it is a copy of DRAM to itself. This
is the same lesson `zero-copy-encode.md` learned on the encode side and it is
worth stating plainly: **on an iGPU, "move it to the GPU" is not the
optimisation. "Stop copying it" is.**

**Run-to-run spread is wide, as `docs/STATUS.md` warns.** These medians were
taken while other work was running on the machine; `sw → RGBA` for the same
1080p H.264 clip was measured anywhere between 15 and 32 ms across the session.
The ratios are stable because both sides move together; the absolute numbers are
not worth quoting to two significant figures.

### What is still on the CPU, per frame

On the `hw → RGBA` path, in descending order of cost:

1. **`av_hwframe_transfer_data`** — a detiling read of 3.1 MB out of the VA
   surface.
2. **swscale NV12 → RGBA** — a full-frame colour conversion.
3. **The tight-row copy in `decoder.rs::convert_software`** — swscale writes a
   padded stride and callers want a packed buffer, so every row is copied again
   into a freshly allocated 8 MB `Vec`.
4. **`rotate_rgba`**, for rotated files only — a third full-frame pass.
5. **`provider::upload_rgba`** — the same 8 MB written back into a wgpu texture.

On the `hw → DMA-BUF` path, items 1 to 5 are all gone. What remains on the CPU is
demuxing (`av_read_frame`), which is I/O and header parsing rather than pixels,
and `vaSyncSurface` inside the export, which is a wait rather than a copy.

## The DMA-BUF path, and the question the brief asked

`av_hwframe_map(dst, src, AV_HWFRAME_MAP_READ | AV_HWFRAME_MAP_DIRECT)` with
`dst->format = AV_PIX_FMT_DRM_PRIME` calls `vaExportSurfaceHandle` and gives
back an `AVDRMFrameDescriptor`. `AV_HWFRAME_MAP_READ` is not optional and is not
about permissions: it is what makes libavutil call `vaSyncSurface` first, and it
is the only synchronisation in the path.

What comes back, at 1920×1080:

```
DmabufFrame {
  size: (1920, 1080),
  objects: 1,
  layers: ["R8   1920x1080 @0+1920", "GR88 960x540 @2088960+1920"],
  modifier: 0x0100000000000002,        // I915_FORMAT_MOD_Y_TILED
}
```

**One buffer, two layers.** FFmpeg exports with
`VA_EXPORT_SURFACE_SEPARATE_LAYERS`, so NV12 arrives as an `R8` luma layer at
offset 0 and a `GR88` chroma layer at offset 2088960 into the *same* object.
`wgpu-hal`'s `texture_from_dmabuf_fd` is single-plane by construction — one fd,
one modifier, one offset, one stride — so the brief's question is exactly the
right one: two textures and a shader, or convert before exporting?

### Both routes were investigated. Import two textures.

**Route A — two textures, convert in the compositor's shader.** Import the `R8`
layer as `R8Unorm` at full size and the `GR88` layer as `Rg8Unorm` at half size,
from two duplicates of the same fd at the two offsets, and do BT.709 in the
fragment shader.

**Route B — VAAPI VPP.** Put `scale_vaapi=format=bgra` in front, let the
fixed-function video-processing engine produce a packed BGRA surface, and export
that as a single layer.

Route A wins, and it wins on evidence rather than on taste.

`cargo run --release --example dmabuf_import -- target/bench-media/bars_1920x1080.mp4`
does route A end to end — decode, export, import both planes, read them back,
reconstruct RGB and compare against a software decode of the same instant:

```
device: Intel(R) Graphics (RPL-U) (Vulkan), dma-buf import feature: true
plane 0 `R8  ` 1920x1080 as R8Unorm:  imported, 2073600 bytes read back
plane 1 `GR88` 960x540  as Rg8Unorm: imported, 1036800 bytes read back
  reconstructed RGB vs software decode: mean |Δ| 3.85 (R 4.89 G 4.25 B 2.42), worst 32
  chroma order control (U and V exchanged): mean |Δ| 41.88 — the planes are the right way round
  verdict: the imported surface is the same picture
```

and on ordinary footage rather than saturated bars, mean |Δ| **0.32**. The
residual on the bars is chroma upsampling — the experiment reconstructs with
nearest-neighbour chroma and swscale interpolates — not a colour error. The
swapped-chroma control at 41.88 is there because a U/V swap leaves luma
untouched and is the classic way a YUV import ships broken; it is ten times the
matching score, so the plane order is established rather than assumed.

Three things that were feared and did not happen:

- **The `Y_TILED` modifier imported fine.** `zero-copy-encode.md` §3 warned at
  length about `VK_EXT_image_drm_format_modifier` and suggested forcing
  `LINEAR`. On the import direction that is unnecessary: ANV accepts the
  modifier the driver itself chose, for both `R8` and `R8G8`.
- **Two images over one buffer at two offsets is legal and works.** The
  `rust-crate-survey.md` §2b concern is about two *views* of one wgpu texture
  and WebGPU's copy-compatibility rules. Two separately imported textures, each
  with its own `VkDeviceMemory` imported from the same DMA-BUF, do not touch
  that rule at all.
- **The adapter advertises `VULKAN_EXTERNAL_MEMORY_DMA_BUF`** without any
  instance-level coaxing.

And the reasons route B is worse, now that route A is known to work:

- It adds a full-frame VPP pass and a second surface pool per clip to save one
  `texture_from_dmabuf_fd` call.
- Its RGBA output pool is fragile on this driver. `scale_vaapi=format=bgra`
  fails with `ENOMEM` after roughly ten frames when anything downstream holds
  frames, while `format=nv12` never does; it only survives a whole clip when the
  consumer frees each surface immediately. That is a pool-sizing problem which
  could be solved, but it is a problem route A does not have.
- Driving a filter graph over hardware frames needs
  `av_buffersrc_parameters_set` and a hand-built graph, i.e. more unsafe code
  for a slower result.

Route B remains the right answer for the *encode* direction, where the
conversion has to happen somewhere and the VPP engine is idle. That is a
different document.

## The patch `render/` needs — **applied, 2026-07-26**

All three parts below are in the tree now. Read them as the design rationale
rather than as work outstanding; what actually shipped, and what it measured,
is in `docs/STATUS.md` under "Hardware decode through the compositor".

Two changes, both small, neither of which this module was allowed to make.
`ash`, `wgpu-hal` and `libc` are already dependencies (the encode-side work
added them), and `render/dmabuf.rs` already does the mirror of this in the export
direction, so this is additive.

### 1. `render/context.rs` — ask for the feature

The device currently requests `Features::empty()`, and `texture_from_dmabuf_fd`
refuses without `VULKAN_EXTERNAL_MEMORY_DMA_BUF`. Ask for it when the adapter
has it, and never require it — a GL fallback or a driver without it must still
open a device.

```diff
@@ async fn try_backend(backends: wgpu::Backends, force_fallback: bool) -> Result<Self> {
-        // Ask for exactly what the adapter offers. We do not need any optional
-        // feature — a textured quad is the whole pipeline — but taking the
-        // adapter's limits rather than the defaults is what lets us render 4K
-        // and 8K frames on hardware that can.
+        // One optional feature, and only if the adapter has it: importing a
+        // decoded video frame as a texture instead of copying it through the
+        // CPU needs `VK_EXT_external_memory_dma_buf`. Requested rather than
+        // required, because a GL fallback or an older driver must still open a
+        // device — `media::dmabuf` produces the descriptors either way and the
+        // caller checks `RenderContext::can_import_dmabuf` before using them.
+        //
+        // Taking the adapter's limits rather than the defaults is what lets us
+        // render 4K and 8K frames on hardware that can.
+        let dmabuf = wgpu::Features::VULKAN_EXTERNAL_MEMORY_DMA_BUF;
+        let optional = adapter.features() & dmabuf;
         let (device, queue) = adapter
             .request_device(&wgpu::DeviceDescriptor {
                 label: Some("chukcut render device"),
-                required_features: wgpu::Features::empty(),
+                required_features: optional,
                 required_limits: adapter.limits(),
                 experimental_features: wgpu::ExperimentalFeatures::disabled(),
                 memory_hints: wgpu::MemoryHints::Performance,
                 trace: wgpu::Trace::Off,
             })
```

and an accessor beside `limits()`:

```rust
/// Whether a decoded video surface can be imported as a texture rather than
/// copied through system memory. See `docs/research/hardware-decode.md`.
pub fn can_import_dmabuf(&self) -> bool {
    self.device
        .features()
        .contains(wgpu::Features::VULKAN_EXTERNAL_MEMORY_DMA_BUF)
}
```

### 2. `render/dmabuf.rs` — the import direction

The file already exports a buffer for the encoder; this is the mirror. It takes
a `media::dmabuf::Plane` — fourcc, modifier, offset, pitch, size, and a `dup_fd`
that hands over an owned duplicate, because Vulkan closes what it is given while
libavutil closes its own.

```rust
use chukcut_engine::modules::media::dmabuf::Plane;   // or `crate::modules::media::...`

/// Import one plane of a decoded video surface as a texture.
///
/// `None` when this device cannot import DMA-BUF, when the plane carries no
/// usable modifier, or when the driver refuses the layout — all of which mean
/// "use the copying path", not "fail".
pub fn import_plane(ctx: &RenderContext, plane: &Plane) -> Option<wgpu::Texture> {
    if !ctx.can_import_dmabuf() || !plane.is_importable() {
        return None;
    }
    let format = match plane.fourcc_name().as_str() {
        // NV12 exported with SEPARATE_LAYERS: luma is a single-channel plane…
        "R8  " => wgpu::TextureFormat::R8Unorm,
        // …and chroma is interleaved U and V at half resolution, which is
        // exactly a two-channel texture of half the size.
        "GR88" => wgpu::TextureFormat::Rg8Unorm,
        "AR24" | "XR24" => wgpu::TextureFormat::Bgra8Unorm,
        "AB24" | "XB24" => wgpu::TextureFormat::Rgba8Unorm,
        other => {
            tracing::debug!(fourcc = other, "no wgpu format for this DRM fourcc");
            return None;
        }
    };
    let fd = plane.dup_fd().ok()?;
    let size = wgpu::Extent3d {
        width: plane.width,
        height: plane.height,
        depth_or_array_layers: 1,
    };
    let hal_descriptor = wgpu_hal::TextureDescriptor {
        label: Some("decoded video plane"),
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUses::RESOURCE | wgpu::TextureUses::COPY_SRC,
        memory_flags: wgpu_hal::MemoryFlags::empty(),
        view_formats: Vec::new(),
    };

    // SAFETY: `texture_from_dmabuf_fd` requires a descriptor that describes the
    // buffer truthfully and takes ownership of the descriptor. Both hold: the
    // modifier, offset and pitch come straight from the driver's own
    // `AVDRMFrameDescriptor`, and `fd` is a duplicate made for this call —
    // libavutil keeps and closes its own. On failure wgpu-hal closes the
    // duplicate itself, which is why there is no cleanup here.
    let hal_texture = unsafe {
        let hal = ctx.device().as_hal::<wgpu_hal::api::Vulkan>()?;
        hal.texture_from_dmabuf_fd(fd, &hal_descriptor, plane.modifier, plane.pitch, plane.offset)
            .ok()?
    };

    // SAFETY: the wgpu descriptor agrees with the hal one field for field. The
    // initial state is `RESOURCE` and not `UNINITIALIZED`, which matters: the
    // texture already holds the decoded picture, and telling wgpu it is
    // uninitialised invites it to clear the frame before anything samples it.
    Some(unsafe {
        ctx.device().create_texture_from_hal::<wgpu_hal::api::Vulkan>(
            hal_texture,
            &wgpu::TextureDescriptor {
                label: Some("decoded video plane"),
                size,
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            },
            wgpu::TextureUses::RESOURCE,
        )
    })
}
```

That code is not speculative: it is `examples/dmabuf_import.rs` with the device
handling replaced by `RenderContext`, and that example is what produced the
verification output above.

### 3. What the compositor then needs, which is the larger piece

`SourceFrame` today is one RGBA texture and the shader samples it directly. A
decoded surface is two textures, so `render/source.rs` and the compositor's
fragment shader have to grow a second, YUV-shaped case. The shape that fits the
existing code with the least disturbance:

- `SourceFrame` gains an optional chroma texture and view, plus the colour
  matrix and range to use. `SourceFrame::from_texture` keeps meaning "RGBA",
  so nothing existing changes.
- The compositor binds a second texture in the same bind group and passes a
  `u32` flag in its uniform block; the fragment shader branches on it and
  applies BT.709 or BT.601, limited or full range. `render/shaders/nv12.wgsl`
  already contains the RGB→YUV constants for the encode direction, so the
  inverse belongs beside them rather than in a new file.
- The colour matrix must come from the *file*, not be assumed.
  `frame::Video::color_space` and `color_range` carry it and
  `media::MappedFrame` should be extended to pass it through — a 1080p clip is
  usually BT.709 and an SD one usually BT.601, and getting it wrong is a subtle
  saturation error nobody catches until delivery.

Rotation is deliberately *not* applied on the mapped path — applying it means
touching pixels, which is the entire thing being avoided — so `MappedFrame`
hands back the angle and the compositor folds it into the transform matrix it is
already computing.

## What is built, and what is not

Built and tested, in `src-tauri/src/modules/media/`:

- `hwdecode.rs` — the VAAPI device (shared process-wide, atomically refcounted),
  the `get_format` callback, the capability-by-decoding probe, the
  hardware-capable decoder selection, and `av_hwframe_transfer_data`.
- `dmabuf.rs` — `av_hwframe_map` to `AV_PIX_FMT_DRM_PRIME` and a flattened view
  of the descriptor's objects/layers/planes, with `dup_fd` for handing a
  descriptor to something that closes it.
- `decoder.rs` — `Acceleration::{Software, Auto, Vaapi}`, `seek_and_map`
  alongside `seek_and_decode`, and the automatic fallback.
- `tests/decode.rs` — 34 tests, of which 22 run against both decoders.

Built since, 2026-07-26, and measured in `docs/STATUS.md`:

- **The `render/` import above**, as `render::dmabuf::import_plane`, plus the
  compositor's two-texture case and `render::source::SourceFrame::from_planes`.
- **`provider.rs` handing mapped frames to the compositor.**
  `provider::DEFAULT_ACCELERATION` is now `Auto`, conditional on
  `RenderContext::can_import_dmabuf()`; `CHUKCUT_DECODE` still overrides it, and
  `MediaSourceProvider::from_project_with` forces it per provider so both paths
  can be measured in one process.
- **The colour matrix and range come from the file**, through
  `MappedFrame::{color_space, color_range}`, and rotation is folded into the
  compositor's texture coordinate rather than applied to pixels.

Not built, in rough order of what it is worth:

1. **A decoded-surface cache keyed by document identity.** Each mapped frame
   pins a VA surface out of a fixed pool, so the existing texture cache becomes
   a *surface* cache with a hard budget. `hwdecode::EXTRA_HW_FRAMES` is 6 today,
   which is enough for the decoder's own `last` and `pending` plus a handful of
   held frames, and a timeline with many clips on screen will need thinking
   about.
2. **Hardware decode for thumbnails.** Currently deliberately software:
   the surface comes back at full resolution whatever height was asked for, so a
   hardware decode downloads every pixel of a 4K frame to make an 80-pixel
   strip. It becomes worth doing only via a VPP downscale, which is route B
   above and is a separate piece of work.
3. **10-bit and HDR.** Every measurement here is 8-bit 4:2:0. A `P010` surface
   exports as `R16`/`GR32` layers and the import needs `R16Unorm`/`Rg16Unorm`;
   `dmabuf.rs::plane_size` already handles the sizing, the format table does
   not.

## Unverified, stated as unknown

- **Whether the two imported textures need an external memory barrier.** The
  export path's only synchronisation is `vaSyncSurface` inside
  `av_hwframe_map`, which is a CPU-side wait for the decode to finish before the
  fds are handed over. That is sufficient for correctness *as measured* — the
  readback matches — but it is a wait per frame, and whether a fenced path would
  be faster has not been tested. `zero-copy-encode.md` §4 reaches the same "start
  with the blunt instrument" conclusion from the other side.
- **Whether holding many mapped frames at once starves the decoder.** The suite
  holds four and walks a whole file mapping every frame, and neither exhausts
  the pool. A timeline with ten clips on screen has not been tried.
- **Anything about AMD or NVIDIA.** The modifier, the layer split and the
  `SEPARATE_LAYERS` behaviour are all Intel/iHD observations. The code reads
  what the driver reports rather than assuming, but it has only ever been told
  one thing.
- **Multi-threaded use of the shared `VADisplay`.** One device is shared across
  decoders, and access is serialised by `provider::MediaSourceProvider`'s mutex
  exactly as `VideoDecoder`'s `Send` impl already requires. Thumbnail workers run
  in parallel and are therefore left on the software path.
