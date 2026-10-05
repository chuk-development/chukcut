# 0034 — Colour in and out: tagged BT.709 exports, HDR sources tone-mapped to SDR in the source shader, 10-bit planes

Date: 2026-10-05. Status: accepted (colour-io agent).

## What was decided

### Export

- **The export converts with the matrix it tags.** `export::colour::OutputColour`
  picks the matrix: BT.709 when the frame is larger than standard definition
  (long side above 1024 or short side above 576), BT.601 at or below it.
  The user can force either (`color_matrix`), and can ask for full range
  (`color_range`); limited is the default. Both conversions honour it:
  swscale through `sws_setColorspaceDetails` on the RGBA→YUV context, and the
  GPU NV12 pass (`nv12.wgsl`, the zero-copy and readback tiers) through a
  `YuvEncoding` uniform.
- **Every video stream is tagged**: matrix, range, primaries BT.709, transfer
  BT.709. The tags go on the codec context before `avcodec_open2`, so
  libx264, libx265, NVENC, VAAPI and QSV write them into the bitstream VUI,
  and `avcodec_parameters_from_context` carries them to the container (`colr`
  in MP4/MOV, `Colour` in Matroska/WebM). A GIF is not tagged.
- **SD changes the matrix and nothing else.** The primaries stay BT.709,
  because the compositor works in BT.709 primaries and does not gamut-convert
  for SD. Tagging SMPTE 170M primaries would make a careful player convert a
  gamut nobody changed.
- **10-bit export** (`ten_bit`): HEVC Main 10 (`libx265`, `hevc_nvenc`,
  `hevc_vaapi`, `hevc_qsv`) and 10-bit AV1. The upload format becomes
  `yuv420p10le` (software) or `P010` (hardware), which takes the swscale tier;
  the GPU NV12 tiers are 8-bit only. Refused in prose for H.264, VP9 and GIF;
  ignored for ProRes, which is already 10-bit.
- **No HDR export.** The compositor's target is `Rgba8UnormSrgb`: there is no
  HDR picture to write, so PQ/HLG output tags would be a lie. 10-bit SDR is
  what the pipeline can honestly produce.

### HDR and 10-bit sources

- **The decoder reads transfer and primaries** from the frame, then the codec
  context; an untagged file is SDR (no guess from the size, unlike the
  matrix, because calling footage HDR on a hunch tone-maps it).
- **HDR, wide-gamut and deeper-than-8-bit 4:2:0 YUV sources reach the
  compositor as planes**, on every decode path: VAAPI (P010 surfaces import
  as `R16Unorm`/`Rg16Unorm`), NVDEC (the P010 download is uploaded as it is)
  and software (`VideoDecoder::wants_planar` → swscale to P010 or NV12 at the
  scaled size, matrix and range left alone). 8-bit SDR sources keep the RGBA
  path they always had, and so do deep SDR sources in 4:2:2 or 4:4:4
  (ProRes, FFV1): the planes are 4:2:0, and halving their chroma for two bits
  of depth measured 29 dB against ffmpeg's decode on hard colour edges. An
  HDR source goes planar whatever its chroma, because only the shader can
  tone-map at playback speed; a 4:2:2 HDR source (iPhone ProRes HLG) loses
  half its horizontal chroma resolution.
- **16-bit planes when the device has them.** `TEXTURE_FORMAT_16BIT_NORM` is
  requested, never required. Without it the planes are cut to their top byte
  (`Nv12Planes::to_eight_bit`) and the shader still tone-maps them.
- **The conversion is one function, `yuv.wgsl`'s `yuv_to_linear`**, which the
  quad shader calls for every planar source: matrix → R'G'B' → EOTF (PQ, or
  HLG inverse OETF + BT.2100 OOTF for a 1000-nit display) → nits / 203 (HDR
  reference white, BT.2408, becomes SDR 1.0) → BT.2020 to BT.709 → the
  BT.2390 EETF on the brightest channel with the others scaled by the same
  ratio, source peak 1000 nits, target peak reference white. Below the knee
  (about 0.45 of SDR white) nothing changes. Preview and export run the same
  shader, so they agree.
- **The colour word.** The transfer, primaries and sample depth ride in the
  quad uniform's existing `matrix` slot as a packed word
  (`SourceFrame::colour_word`), so the uniform layout did not change. A test
  asserts the Rust and WGSL constants agree.

## Why

- The BT.601 export was a correctness bug, not a preference: players read an
  untagged HD stream as BT.709. Measured on colour bars (1920x1080, H.264 CRF
  12, decoded as a player decodes it): **worst 39 code values, mean 6.9**
  before; **worst 2, mean 0.04** after with libx264, **0 and 0** with NVENC
  (whose input comes from the GPU NV12 pass). The Y'CbCr samples of the
  export against the source's are within 1 code value on every encoder here.
- Tone-mapping in the source shader, rather than in a separate pass or on the
  CPU, costs nothing extra (the shader was already converting) and is the only
  place every decode path meets. HDR fixtures built from the standards'
  formulas come back within 0.46 code values of a CPU reference of the same
  tone map, and greys below the knee within 0 of the SDR values they were made
  from, on software decode and NVDEC, on the RTX 3060 and lavapipe.
- BT.2390 rather than Hable or ACES: it leaves the midtones exactly where the
  SDR grade would put them, which is what makes an HDR phone clip cut against
  an SDR one look like the same scene. Hable and ACES re-curve the whole
  range.
- 203 nits for SDR white is BT.2408 and libplacebo's default; 1000 nits is
  what HLG is defined against and what phones master PQ to.

## What it costs

- **Fixed tone-map parameters.** The source peak is 1000 nits for every PQ
  clip; mastering-display metadata and MaxCLL are not read. A 4000-nit master
  has its top highlights clipped rather than compressed.
- **Saturated highlights stay saturated.** The max-RGB tone map preserves hue
  and saturation; a very bright saturated colour does not desaturate towards
  white the way a film-like curve would. Out-of-gamut BT.2020 colours are
  clipped per channel after the matrix.
- **Only the compositor sees HDR correctly.** Thumbnails, waveforms of
  pictures, ML analysis inputs and proxies still go through
  `seek_and_decode`'s RGBA path, where swscale flattens a PQ signal into
  sRGB-tagged bytes: thumbnails of HDR clips look grey, and a proxy of an HDR
  clip is a grey SDR file.
- **The planar path's chroma is the shader's.** Bilinear, centred, like the
  NVDEC and VAAPI frames have always been: against ffmpeg's decode of
  `testsrc` (nothing but hard colour edges) a 10-bit 4:2:0 source measures
  32.5 dB and 1.17 mean, where the RGBA path is exact. On real footage the
  hardware path measured 1.28 mean.
- **10-bit export carries 8-bit pictures.** The render target is 8-bit sRGB;
  10-bit output removes the YUV rounding and gives the encoder finer steps,
  not more precision than the composite has.
- **VAAPI P010 import is unverified.** No VAAPI device on the machine this
  was written on; the format table entries are from the DRM fourccs iHD
  documents (`R16 `, `GR32`) and a test asserts them.

## What would change our minds

- A float (or 16-bit) compositor target. Then HDR export (PQ/HLG, BT.2020)
  becomes honest, and the tone map moves from the source shader to the output
  stage for SDR exports.
- Footage that clips visibly because of the fixed 1000-nit peak: read
  `AV_FRAME_DATA_MASTERING_DISPLAY_METADATA` / content light level and pass
  the peak in the uniform.
- A user asking for HDR thumbnails or proxies: a CPU twin of `yuv_to_linear`
  in `media::decoder`, as `render/grade.rs` already has for the grade.
