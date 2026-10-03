//! The bridge between decoding and compositing.
//!
//! The compositor asks "give me a texture for material X at source time T" and
//! knows nothing about files, codecs or FFmpeg. This module answers that
//! question: it resolves the material id to a path, keeps a decoder open per
//! material, decodes the requested frame, and uploads it to the GPU.
//!
//! ## Why it holds a snapshot rather than the live project
//!
//! A render walks the timeline while the user keeps editing. Borrowing the
//! live document for the duration would mean holding the project lock across
//! decode calls, which freezes the UI — exactly what the IPC contract
//! forbids. Instead the provider is built from a snapshot of the material
//! pool: a small map of id → path that is cheap to build and immutable
//! afterwards. An export started ten seconds ago renders the timeline as it
//! was ten seconds ago, which is also what the user expects.
//!
//! ## Caching
//!
//! Two layers, for two different costs:
//!
//! - **Decoders** are cached per material because opening a file and finding a
//!   keyframe is expensive, and because a decoder that stays open can answer a
//!   sequential request by decoding one frame forward instead of seeking.
//! - **Textures** are cached per (material, frame) because a paused preview,
//!   a resized window and a re-render after an unrelated edit all ask for the
//!   same frame repeatedly, and re-uploading 8 MB each time is pure waste.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use parking_lot::Mutex;

use ffmpeg_next as ffmpeg;

use super::decoder::{Acceleration, MappedFrame, VideoDecoder};
use crate::modules::project::document::TextMaterial;
use crate::modules::project::{Id, MaterialKind, Micros, Project};
use crate::modules::render::{self, RenderContext, SourceFrame, SourceProvider, SourceRequest};
use crate::modules::text::TextRenderer;

/// Where a material's pixels come from.
#[derive(Debug, Clone)]
enum MaterialSource {
    Video {
        path: PathBuf,
        /// Display dimensions, i.e. with container rotation applied. Kept so
        /// the decode resolution can be worked out before anything is opened —
        /// see [`fitted_height`].
        display: (u32, u32),
    },
    Image {
        path: PathBuf,
    },
    /// Text is rasterised rather than decoded, at the size of the frame being
    /// rendered, so the compositor's fit is the identity and the segment's own
    /// transform is what places the title. The material travels with it because
    /// rasterising needs the whole thing — string, font, size, colour, alignment
    /// — and there is nothing to open.
    Text(TextMaterial),
}

/// How close two requested times must be to reuse a cached texture.
///
/// Rendering at 30 fps asks for times 33 333 µs apart, so anything below half
/// a frame is safely "the same frame". Using an absolute value rather than the
/// project frame rate keeps the provider independent of the timeline.
const FRAME_EPSILON: Micros = 8_000;

/// A cached upload, keyed by material.
struct CachedTexture {
    source_time: Micros,
    frame: SourceFrame,
}

/// An open decoder plus the display height it was opened for.
///
/// The height matters because swscale can downscale during colour conversion,
/// which is far cheaper than converting a full 4K frame and then throwing most
/// of it away on the GPU. A preview at 540x960 that decodes at 1080x1920 pays
/// four times the conversion cost and four times the upload bandwidth for
/// pixels nobody will see.
struct OpenDecoder {
    /// Its own lock, so two clips decode in parallel; one decoder is still
    /// strictly one thread at a time, because it is a demuxer position.
    decoder: Mutex<VideoDecoder>,
    height: u32,
}

/// How tall this source needs to be decoded, given the area it will be drawn
/// into.
///
/// `max_size` is the canvas, not the space this particular clip occupies. A
/// 1920×1080 clip placed on a 540×960 vertical canvas is drawn as a 540×304
/// letterboxed strip — so decoding it 960 pixels tall means pushing 1706×960
/// through swscale to produce 540×304, five and a half times the pixels needed,
/// every single frame. That was measured: it is what made playback of landscape
/// footage on a vertical timeline drop 85 frames in a row.
///
/// The compositor fits a source into the canvas preserving aspect ratio, so the
/// same fit is computed here. A clip scaled above 100% by its transform will be
/// slightly soft as a result; that is a preview, and the export path asks for
/// full resolution.
fn fitted_height(display: (u32, u32), max_size: (u32, u32)) -> u32 {
    let (source_w, source_h) = (display.0.max(1) as f32, display.1.max(1) as f32);
    let (canvas_w, canvas_h) = (max_size.0.max(1) as f32, max_size.1.max(1) as f32);
    let scale = (canvas_w / source_w).min(canvas_h / source_h);
    // Never upscale during decode: enlarging is free on the GPU and expensive
    // in swscale.
    let scale = scale.min(1.0);
    ((source_h * scale).round() as u32).max(2)
}

/// How much larger a request has to be before the decoder is reopened.
///
/// Reopening costs a seek, so a preview that nudges its proxy size by a few
/// pixels must not trigger one; a jump from preview to export resolution must.
const REOPEN_RATIO: f32 = 1.5;

/// Which decoder this provider opens.
///
/// This is the one place in the codebase where hardware decode is turned on or
/// off, and it is now **on**, conditionally. The reason is measured, and so was
/// the reason it used to be off — `docs/research/hardware-decode.md` has the
/// table:
///
/// | 1920×1080 H.264, per frame | |
/// |---|---:|
/// | software decode → RGBA | ~27 ms |
/// | hardware decode → RGBA | ~37 ms |
/// | **hardware decode → DMA-BUF** | **1.7 ms** |
///
/// The middle row was what this provider got while [`upload_rgba`] was the only
/// way to feed the compositor: reaching RGBA from a VA surface means
/// `av_hwframe_transfer_data` out of tiled GPU memory followed by an swscale
/// pass, and both cost more than the decode did. Hardware was genuinely slower
/// and turning it on would have looked like a regression.
///
/// [`import_mapped`] below is the bottom row. It hands the compositor the
/// decoder's own surface as two textures, so nothing is copied and nothing is
/// converted on the CPU.
///
/// **The condition matters.** That path needs
/// [`RenderContext::can_import_dmabuf`], which is a Vulkan device with
/// `VK_EXT_external_memory_dma_buf`. On a GL fallback, an old driver, or
/// anything not Linux, asking for hardware would put us back on the middle row
/// — worse than where we started. So [`acceleration`] asks the device first.
const DEFAULT_ACCELERATION: Acceleration = Acceleration::Auto;

/// What to use when the GPU cannot take a decoded surface directly.
const NO_IMPORT_ACCELERATION: Acceleration = Acceleration::Software;

/// `CHUKCUT_DECODE`, parsed once. `None` when it is unset or nonsense.
///
/// `software`, `auto` or `vaapi`. An unrecognised value is a typo rather than a
/// request, and silently ignoring it would leave somebody measuring the default
/// and believing they measured hardware — so it is logged.
fn acceleration_override() -> Option<Acceleration> {
    static CHOSEN: std::sync::OnceLock<Option<Acceleration>> = std::sync::OnceLock::new();
    *CHOSEN.get_or_init(|| {
        let chosen = match std::env::var("CHUKCUT_DECODE").as_deref() {
            Ok("software") => Some(Acceleration::Software),
            Ok("auto") => Some(Acceleration::Auto),
            Ok("vaapi") => Some(Acceleration::Vaapi),
            Ok("cuda") | Ok("nvdec") => Some(Acceleration::Cuda),
            Ok(other) => {
                tracing::warn!(
                    value = other,
                    "CHUKCUT_DECODE must be software, auto, vaapi or cuda; using the default"
                );
                None
            }
            Err(_) => None,
        };
        if let Some(chosen) = chosen {
            tracing::info!(?chosen, "decode acceleration overridden by CHUKCUT_DECODE");
        }
        chosen
    })
}

/// Which decoder to open, given what this GPU can accept.
///
/// The environment wins outright, including over the device check: somebody
/// measuring `CHUKCUT_DECODE=vaapi` on a machine that cannot import wants to see
/// the slow number, not a silent substitution.
fn acceleration(ctx: &RenderContext) -> Acceleration {
    if let Some(forced) = acceleration_override() {
        return forced;
    }
    if ctx.can_import_dmabuf() {
        DEFAULT_ACCELERATION
    } else {
        NO_IMPORT_ACCELERATION
    }
}

/// What a clip whose media is gone gets composited as: a flat dark-red field.
///
/// sRGB bytes, chosen to be unmistakable next to anything a camera produces —
/// the point of a placeholder is that nobody watches it and thinks the edit is
/// fine. Public so the tests that assert on the rendered pixel and the
/// frontend's missing tint have one value to agree on.
pub const MISSING_MEDIA_RGBA: [u8; 4] = [122, 26, 26, 255];

pub struct MediaSourceProvider {
    /// Material id → where its pixels live. Built once from a project snapshot.
    sources: HashMap<Id, MaterialSource>,
    /// The project's canvas, in document pixels.
    ///
    /// Only text needs it, and it needs it because font sizes are in document
    /// pixels while a preview renders at a fraction of them: a 72-pixel title
    /// in a 1080x1920 project is 36 device pixels tall in a 540x960 preview.
    /// Everything else in this file scales by fitting a picture, which needs no
    /// such reference.
    canvas: (u32, u32),
    /// Overrides [`acceleration`] for this provider only.
    ///
    /// The application never sets it: the environment and the device decide,
    /// once per process. It exists because comparing the two decode paths
    /// against each other has to happen *inside* one process — two runs of the
    /// same benchmark differ by more than the two paths do, as
    /// `docs/STATUS.md` keeps having to point out — and the process-wide choice
    /// cannot be changed twice.
    forced: Option<Acceleration>,
    /// The missing-media placeholder, built on first use and shared by every
    /// material that needs it.
    ///
    /// Deliberately *not* in the per-material texture cache: a placeholder
    /// cached under a material's id would keep serving after the file came
    /// back, and the existence check that decides between the two is one
    /// `stat` per frame, which is nothing next to a decode.
    placeholder: Mutex<Option<SourceFrame>>,
    /// Cached uploads and imports, keyed by material.
    ///
    /// Declared before `decoders` so it is dropped first: a mapped frame in
    /// here is a hold on one of that decoder's surfaces. See [`Self::clear`].
    textures: Mutex<HashMap<Id, CachedTexture>>,
    /// One decoder per material. `VideoDecoder` is `Send` but not `Sync`, so
    /// the whole map sits behind a mutex; two threads seeking one demuxer
    /// would fight over its read position anyway.
    decoders: Mutex<HashMap<Id, Arc<OpenDecoder>>>,
    /// Set once any frame came back as an imported decoder surface. See
    /// [`Self::hands_out_decoder_surfaces`].
    mapped: std::sync::atomic::AtomicBool,
}

impl MediaSourceProvider {
    /// Snapshot the material pool of `project`.
    pub fn from_project(project: &Project) -> Self {
        Self::from_project_with(project, None)
    }

    /// [`Self::from_project`], with the decoder chosen rather than inferred.
    ///
    /// For benchmarks and for the pixel comparison. Production wants `None`.
    pub fn from_project_with(project: &Project, forced: Option<Acceleration>) -> Self {
        let mut sources = HashMap::new();

        for video in &project.materials.videos {
            // Rotation is a property of the container, and a 90°-rotated file
            // is taller than it is wide once displayed. Getting this backwards
            // would make the fit calculation pick the wrong axis.
            let display = if video.rotation % 180 == 0 {
                (video.width, video.height)
            } else {
                (video.height, video.width)
            };
            sources.insert(
                video.id.clone(),
                MaterialSource::Video {
                    path: PathBuf::from(&video.path),
                    display,
                },
            );
        }
        for image in &project.materials.images {
            sources.insert(
                image.id.clone(),
                MaterialSource::Image {
                    path: PathBuf::from(&image.path),
                },
            );
        }
        for text in &project.materials.texts {
            sources.insert(text.id.clone(), MaterialSource::Text(text.clone()));
        }

        Self {
            sources,
            canvas: (project.canvas.width.max(1), project.canvas.height.max(1)),
            forced,
            placeholder: Mutex::new(None),
            decoders: Mutex::new(HashMap::new()),
            mapped: std::sync::atomic::AtomicBool::new(false),
            textures: Mutex::new(HashMap::new()),
        }
    }

    /// Decode each video from its proxy, where the proxy cache has one.
    ///
    /// For the preview only. The export builds its provider without this, and
    /// `proxy::switch` refuses a proxy path on that side as well. The display
    /// size stays the original's, so a clip is framed the same either way —
    /// the proxy is only smaller, and the compositor scales it up.
    pub fn with_preview_proxies(mut self) -> Self {
        let queue = crate::modules::proxy::ProxyQueue::shared();
        for source in self.sources.values_mut() {
            if let MaterialSource::Video { path, .. } = source {
                if let Some(proxy) = queue.lookup(path) {
                    tracing::debug!(original = %path.display(), proxy = %proxy.display(), "previewing from a proxy");
                    *path = proxy;
                }
            }
        }
        self
    }

    /// The flat field a clip composites as when its media is gone — removed
    /// from the pool, or the file no longer on disk.
    ///
    /// Canvas-aspect so the compositor's fit stretches it over exactly the
    /// area the clip would have covered, at a sixteenth of the canvas per axis
    /// because every texel is the same colour anyway.
    fn missing_frame(&self, ctx: &RenderContext) -> SourceFrame {
        let mut cached = self.placeholder.lock();
        if let Some(frame) = cached.as_ref() {
            return frame.clone();
        }
        let width = (self.canvas.0 / 16).max(2);
        let height = (self.canvas.1 / 16).max(2);
        let data: Vec<u8> = MISSING_MEDIA_RGBA.repeat((width * height) as usize);
        let frame = upload_rgba(ctx, &data, width, height);
        *cached = Some(frame.clone());
        frame
    }

    /// Bring the titles up to date with `project` without touching the media.
    ///
    /// A provider is kept across edits so its decoders stay open, but it holds
    /// text materials *by value*: without this, a title added or retyped after
    /// the provider was built is missing (drawn as the offline placeholder) or
    /// stale in the preview until a file is imported. Changed titles also lose
    /// their cached upload, which is keyed by id, not by content.
    pub fn sync_texts(&mut self, project: &Project) {
        let mut textures = self.textures.lock();
        for text in &project.materials.texts {
            let unchanged = matches!(
                self.sources.get(&text.id),
                Some(MaterialSource::Text(known)) if known == text
            );
            if unchanged {
                continue;
            }
            let prefix = format!("{}\u{1}", text.id);
            textures.retain(|key, _| key != &text.id && !key.starts_with(&prefix));
            self.sources
                .insert(text.id.clone(), MaterialSource::Text(text.clone()));
        }
    }

    /// Drop every cached decoder and texture. Called when a render session
    /// ends, so a finished export does not pin a gigabyte of GPU memory.
    pub fn clear(&self) {
        // Textures first. On the mapped path a cached frame pins one of the
        // decoder's VA surfaces, and although libavutil's refcounting makes
        // either order safe — the surface holds the frames context, which holds
        // the device — releasing the holds before the thing they are holds on
        // is the order that stays obviously correct if any of that changes.
        self.textures.lock().clear();
        self.decoders.lock().clear();
        *self.placeholder.lock() = None;
    }

    /// Decode every video clip visible at `time` now, one thread per clip, so
    /// the render that asks for them next finds them in the texture cache.
    ///
    /// This is the player's decode-ahead. It runs while the previous frame is
    /// still on the GPU or being read back, so a frame costs the slowest clip's
    /// decode rather than the sum of every clip's decode plus the readback.
    /// Exactly the requests `Compositor::collect_draws` makes for an ordinary
    /// clip — same material, same source time, same size — so they hit. A clip
    /// inside a transition window is left to the render: its source times come
    /// from the transition, and guessing them wrong would cost a seek.
    pub fn prefetch_clips(
        &self,
        ctx: &RenderContext,
        project: &Project,
        time: Micros,
        size: (u32, u32),
    ) {
        let mut wanted: Vec<(&str, Micros)> = Vec::new();
        for (track, segment) in render::visible_segments(project, time) {
            if project.materials.kind_of(&segment.material_id) != Some(MaterialKind::Video) {
                continue;
            }
            if crate::modules::transitions::instant_for(track, &project.materials, segment, time)
                .is_some()
            {
                continue;
            }
            let Some(source_time) = segment.source_time_at(time) else {
                continue;
            };
            // One position per decoder: the same file twice on screen is
            // decoded twice by the render anyway, and prefetching both would
            // make its one demuxer seek back and forth.
            if wanted.iter().any(|(id, _)| *id == segment.material_id) {
                continue;
            }
            wanted.push((&segment.material_id, source_time));
        }
        let fetch = |&(material_id, source_time): &(&str, Micros)| {
            let request = SourceRequest {
                material_id,
                kind: MaterialKind::Video,
                source_time,
                segment_id: "",
                max_size: size,
            };
            if let Err(error) = self.frame(ctx, &request) {
                // The render will ask again and report it properly.
                tracing::debug!(material_id, %error, "decode-ahead failed");
            }
        };
        match wanted.as_slice() {
            [] => {}
            [one] => fetch(one),
            [first, rest @ ..] => std::thread::scope(|scope| {
                for clip in rest {
                    scope.spawn(move || fetch(clip));
                }
                fetch(first);
            }),
        }
    }

    /// Whether a frame this provider handed out may still be a decoder's own
    /// surface (VAAPI, imported as DMA-BUF) rather than a copy.
    ///
    /// Such a frame goes back to the decoder's pool when it is dropped, while
    /// the GPU may still be sampling it. A caller that decodes the next frame
    /// before the GPU has finished the last one must wait for the GPU first
    /// when this is true.
    pub fn hands_out_decoder_surfaces(&self) -> bool {
        self.mapped.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Number of materials this provider can serve, for diagnostics.
    pub fn len(&self) -> usize {
        self.sources.len()
    }

    pub fn is_empty(&self) -> bool {
        self.sources.is_empty()
    }

    /// A cached upload for this material, if it is both the right frame and
    /// large enough. The size check is what stops an export reusing the
    /// preview's proxy texture and silently producing a soft picture.
    fn cached(
        &self,
        material_id: &str,
        source_time: Micros,
        want_height: u32,
    ) -> Option<SourceFrame> {
        let textures = self.textures.lock();
        let cached = textures.get(material_id)?;
        let close_enough = (cached.source_time - source_time).abs() <= FRAME_EPSILON;
        let big_enough = cached.frame.height as f32 * REOPEN_RATIO >= want_height as f32;
        (close_enough && big_enough).then(|| cached.frame.clone())
    }

    fn store(&self, material_id: &str, source_time: Micros, frame: &SourceFrame) {
        self.textures.lock().insert(
            material_id.to_string(),
            CachedTexture {
                source_time,
                frame: frame.clone(),
            },
        );
    }

    fn video_frame(
        &self,
        ctx: &RenderContext,
        material_id: &str,
        path: &PathBuf,
        source_time: Micros,
        want_height: u32,
    ) -> anyhow::Result<SourceFrame> {
        // Reopen when the caller now wants meaningfully more resolution than
        // the open decoder produces — going from a preview to an export, in
        // practice. Shrinking is not worth a reopen: downscaling on the GPU is
        // nearly free next to a seek.
        //
        // The map lock is held only to find the decoder, never while it
        // decodes, so the decode-ahead (`prefetch`) can run every clip's
        // decoder at once.
        let found = {
            let mut decoders = self.decoders.lock();
            let stale = decoders
                .get(material_id)
                .is_some_and(|open| want_height as f32 > open.height as f32 * REOPEN_RATIO);
            if stale {
                decoders.remove(material_id);
            }
            decoders.get(material_id).cloned()
        };
        let open = match found {
            Some(open) => open,
            None => {
                let wanted = self.forced.unwrap_or_else(|| acceleration(ctx));
                let decoder = VideoDecoder::open_scaled_with(path, want_height, wanted)?;
                let height = decoder.output_size().1;
                let opened = Arc::new(OpenDecoder {
                    decoder: Mutex::new(decoder),
                    height,
                });
                Arc::clone(
                    self.decoders
                        .lock()
                        .entry(material_id.to_string())
                        .or_insert(opened),
                )
            }
        };
        let mut decoder = open.decoder.lock();

        // The mapped path first, when the decoder is actually on the GPU. Each
        // mapped frame pins a VA surface out of a fixed pool, so the *previous*
        // frame for this material is released before the next one is asked for
        // — otherwise every material holds two surfaces at the moment of
        // greatest pressure and a busy timeline exhausts the pool. Nothing is
        // lost by dropping it: `cached` has already missed.
        //
        // The condition is `acceleration()` rather than `is_hardware()`, and
        // the difference is one frame per file that is easy to miss.
        // `is_hardware` is only true once a frame has actually come back as a
        // surface, so on the very first request it is false even for a decoder
        // that is about to produce nothing but surfaces — and that first frame
        // would then be downloaded, converted, uploaded and *cached*, so the
        // next request for the same instant is answered from the copy. Before
        // the first frame `acceleration()` reports what was asked for, which is
        // exactly the "might be hardware" this needs; after it, it reports the
        // truth. `seek_and_map` refuses cheaply when the answer turns out to be
        // no, leaving the frame decoded and in hand for the fall-through.
        // NVDEC frames have no DMA-BUF export, so a CUDA decoder goes straight
        // to the copying path below rather than failing a map per frame.
        if matches!(
            decoder.acceleration(),
            Acceleration::Auto | Acceleration::Vaapi
        ) {
            self.textures.lock().remove(material_id);
            let started = std::time::Instant::now();
            match decoder.seek_and_map(source_time) {
                Ok(mapped) => {
                    let decode_micros = started.elapsed().as_micros();
                    let import_started = std::time::Instant::now();
                    if let Some(frame) = import_mapped(ctx, mapped) {
                        tracing::debug!(
                            file = %file_name(path),
                            at_ms = source_time / 1000,
                            decoded = format_args!("{}x{}", frame.width, frame.height),
                            decode_ms = decode_micros as f64 / 1000.0,
                            import_ms = import_started.elapsed().as_micros() as f64 / 1000.0,
                            path = ?decoder.acceleration(),
                            "mapped source frame",
                        );
                        drop(decoder);
                        self.mapped
                            .store(true, std::sync::atomic::Ordering::Relaxed);
                        self.store(material_id, source_time, &frame);
                        return Ok(frame);
                    }
                    // The surface came back in a layout this driver will not
                    // import. Not an error: fall through and copy it, which is
                    // slower and always works.
                    tracing::debug!(
                        file = %file_name(path),
                        "the decoded surface could not be imported; copying it instead"
                    );
                }
                Err(error) => {
                    tracing::debug!(
                        file = %file_name(path),
                        %error,
                        "cannot export the decoded surface; copying it instead"
                    );
                }
            }
        }

        // NVDEC: download the NV12 planes and upload them as they are. The
        // shader converts them, as it does for an imported VA surface, so the
        // swscale pass that made a downloaded frame slower than a software one
        // never runs. Before the first frame `Auto` might still turn out to be
        // NVDEC, so it is tried then too; a software frame refuses cheaply.
        if matches!(
            decoder.acceleration(),
            Acceleration::Auto | Acceleration::Cuda
        ) {
            let started = std::time::Instant::now();
            match decoder.seek_and_download_nv12(source_time) {
                Ok(planes) => {
                    let decode_micros = started.elapsed().as_micros();
                    let upload_started = std::time::Instant::now();
                    let frame = upload_nv12(ctx, &planes);
                    tracing::debug!(
                        file = %file_name(path),
                        at_ms = source_time / 1000,
                        decoded = format_args!("{}x{}", planes.width, planes.height),
                        decode_ms = decode_micros as f64 / 1000.0,
                        upload_ms = upload_started.elapsed().as_micros() as f64 / 1000.0,
                        path = ?decoder.acceleration(),
                        "downloaded NV12 source frame",
                    );
                    drop(decoder);
                    self.store(material_id, source_time, &frame);
                    return Ok(frame);
                }
                Err(error) => {
                    tracing::trace!(
                        file = %file_name(path),
                        %error,
                        "no NV12 download; converting to RGBA instead"
                    );
                }
            }
        }

        let started = std::time::Instant::now();
        let decoded = decoder.seek_and_decode(source_time)?;
        let decode_micros = started.elapsed().as_micros();

        let upload_started = std::time::Instant::now();
        let frame = upload_rgba(ctx, &decoded.data, decoded.width, decoded.height);
        let upload_micros = upload_started.elapsed().as_micros();

        // Per-frame, at debug: this is the line that tells you whether a stutter
        // is decode, upload, or something further down the pipeline — and which
        // file caused it, which matters the moment a timeline has more than one.
        tracing::debug!(
            file = %file_name(path),
            at_ms = source_time / 1000,
            decoded = format_args!("{}x{}", decoded.width, decoded.height),
            decode_ms = decode_micros as f64 / 1000.0,
            upload_ms = upload_micros as f64 / 1000.0,
            // Which decoder actually ran, not which one was asked for. Without
            // this a session where the hardware quietly refused looks identical
            // in the log to one where it worked, and the only symptom is the
            // millisecond count.
            path = ?decoder.acceleration(),
            "decoded source frame"
        );
        // Drop the decoder lock before touching the texture cache; the two are
        // independent and holding both invites a lock-order bug later.
        drop(decoder);

        self.store(material_id, source_time, &frame);
        Ok(frame)
    }

    fn image_frame(
        &self,
        ctx: &RenderContext,
        material_id: &str,
        path: &PathBuf,
    ) -> anyhow::Result<SourceFrame> {
        // A still never changes, so any cached upload is valid regardless of
        // the requested time.
        if let Some(cached) = self.textures.lock().get(material_id) {
            return Ok(cached.frame.clone());
        }

        let image = image::open(path)?.to_rgba8();
        let (width, height) = image.dimensions();
        let frame = upload_rgba(ctx, image.as_raw(), width, height);
        self.store(material_id, 0, &frame);
        Ok(frame)
    }

    /// Rasterise a text layer and upload it.
    ///
    /// The layer is the size of the frame being rendered rather than of the
    /// text, because `render::layout::fit_size` scales a source to fit the
    /// canvas: a tightly cropped title would be stretched to fill the frame.
    /// At frame size the fit is the identity and the segment's own transform
    /// positions it, exactly as for a clip.
    ///
    /// `TextRenderer` caches by content hash, so the call this makes on every
    /// frame costs a hash lookup rather than a rasterisation — half a
    /// microsecond against five milliseconds, measured in
    /// `docs/research/text-rendering.md`. `shared()` rather than a fresh
    /// renderer, because constructing one scans the system's fonts.
    fn text_frame(
        &self,
        ctx: &RenderContext,
        material_id: &str,
        material: &TextMaterial,
        source_time: Micros,
        size: (u32, u32),
    ) -> anyhow::Result<SourceFrame> {
        let size = (size.0.max(1), size.1.max(1));
        if crate::modules::captions::karaoke::is_animated(material) {
            return self.karaoke_frame(ctx, material_id, material, source_time, size);
        }
        // A title does not vary with time, so any cached upload at the right
        // size is valid whatever instant was asked for. The size check is what
        // stops an export reusing the preview's smaller raster, which would be
        // a visibly soft title in the delivered file.
        if let Some(cached) = self.textures.lock().get(material_id) {
            if (cached.frame.width, cached.frame.height) == size {
                return Ok(cached.frame.clone());
            }
        }

        let scale = size.0 as f32 / self.canvas.0 as f32;
        let rastered = TextRenderer::shared().rasterize_material(material, size, scale);
        let frame = upload_rgba(ctx, &rastered.pixels, rastered.width, rastered.height);
        self.store(material_id, 0, &frame);
        Ok(frame)
    }

    /// A karaoke caption: the same title with the spoken word lit, which makes
    /// it the one kind of text that changes with time.
    ///
    /// It changes only when the lit word does, so the upload is cached per
    /// word under a key of its own. The plain material id is never stored for
    /// it, which keeps the time-blind cache check at the top of `frame` from
    /// answering with whichever word happened to be drawn first.
    fn karaoke_frame(
        &self,
        ctx: &RenderContext,
        material_id: &str,
        material: &TextMaterial,
        source_time: Micros,
        size: (u32, u32),
    ) -> anyhow::Result<SourceFrame> {
        let (request, lit) = crate::modules::captions::karaoke::request_at(material, source_time);
        let key = match lit {
            Some(index) => format!("{material_id}\u{1}karaoke-{index}"),
            None => format!("{material_id}\u{1}karaoke"),
        };
        if let Some(cached) = self.textures.lock().get(&key) {
            if (cached.frame.width, cached.frame.height) == size {
                return Ok(cached.frame.clone());
            }
        }
        let scale = size.0 as f32 / self.canvas.0 as f32;
        let rastered = TextRenderer::shared().rasterize(
            &request,
            &crate::modules::text::RasterOptions::canvas(size.0, size.1).with_scale(scale),
        );
        let frame = upload_rgba(ctx, &rastered.pixels, rastered.width, rastered.height);
        self.store(&key, 0, &frame);
        Ok(frame)
    }
}

impl SourceProvider for MediaSourceProvider {
    fn prefetch(&self, ctx: &RenderContext, project: &Project, time: Micros, size: (u32, u32)) {
        self.prefetch_clips(ctx, project, time, size);
    }

    fn frame(
        &self,
        ctx: &RenderContext,
        request: &SourceRequest<'_>,
    ) -> anyhow::Result<Option<SourceFrame>> {
        let Some(source) = self.sources.get(request.material_id) else {
            // The material is not in the pool — normally because
            // `RemoveMaterial` took it out and the clip was deliberately left
            // behind. `Project::validate` warns about it; the frame's job is
            // to show the clip as unmistakably offline rather than to leave a
            // silent hole in the composite.
            tracing::debug!(
                material_id = request.material_id,
                "no source for material; compositing the missing-media placeholder"
            );
            return Ok(Some(self.missing_frame(ctx)));
        };

        // Audio contributes nothing to a video frame.
        if request.kind == MaterialKind::Audio {
            return Ok(None);
        }

        if let Some(cached) =
            self.cached(request.material_id, request.source_time, request.max_size.1)
        {
            return Ok(Some(cached));
        }

        match source {
            MaterialSource::Video { path, display } => {
                // A file gone from disk is the same honest picture as a
                // material gone from the pool. Checked here rather than left
                // to the decoder's open error, because the placeholder is a
                // *result*, not a failure: strict consumers (the export)
                // refuse missing media by name before rendering anything.
                if !path.exists() {
                    return Ok(Some(self.missing_frame(ctx)));
                }
                self.video_frame(
                    ctx,
                    request.material_id,
                    path,
                    request.source_time,
                    fitted_height(*display, request.max_size),
                )
                .map(Some)
            }
            MaterialSource::Image { path } => {
                if !path.exists() {
                    return Ok(Some(self.missing_frame(ctx)));
                }
                self.image_frame(ctx, request.material_id, path).map(Some)
            }
            MaterialSource::Text(material) => self
                .text_frame(
                    ctx,
                    request.material_id,
                    material,
                    request.source_time,
                    request.max_size,
                )
                .map(Some),
        }
    }
}

/// A file's own name, for a log line.
fn file_name(path: &PathBuf) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default()
}

/// A mapped surface, kept alive for as long as the textures imported from it.
///
/// The textures are Vulkan images over memory the *decoder* owns. The
/// `MappedFrame` holds the `AVFrame` that holds the VA surface, so while it is
/// alive the decoder cannot recycle that surface and write the next picture
/// into it. Drop it early and the symptom is not a crash — it is an occasional
/// wrong frame under motion, which reads as a decoder bug and is hunted for in
/// the wrong module.
struct PinnedSurface(#[allow(dead_code)] MappedFrame);

/// # Safety
///
/// `MappedFrame` is already `Send` — `media::dmabuf` argues that case, and it
/// is the same argument `export::hwframes` makes: libavutil refcounts an
/// `AVFrame` atomically and the buffer carries no thread affinity.
///
/// `Sync` needs one thing more, and it holds here because this wrapper exposes
/// *nothing*. It is a keep-alive: no method reads the frame, no `&self` escapes,
/// and the only operation anything performs on it is `Drop`, which `&mut self`
/// makes exclusive. A shared reference to it therefore permits no operation at
/// all, which is trivially thread-safe.
///
/// The reason it is needed is that [`SourceFrame`] is cached behind a mutex and
/// handed to whichever thread renders, and `SourceProvider` is `Sync`.
unsafe impl Sync for PinnedSurface {}

/// Turn a mapped decoder surface into a [`SourceFrame`] the compositor can draw.
///
/// `None` whenever this is not going to work — a driver that will not import
/// the layout, a surface that came back as something other than the two NV12
/// planes, a device without the extension. Every one of those means "copy it
/// instead", not "fail": the software path is always there and is what the
/// caller falls back to.
///
/// Nothing here touches a pixel. The two `wgpu::Texture`s are views onto the
/// memory the decoder wrote, which is the entire point — 1.7 ms against 26.9,
/// per `docs/research/hardware-decode.md`.
///
/// Linux only, because DMA-BUF is a DRM concept and `render::dmabuf` is
/// compiled nowhere else. Everything above this is portable; the decoder simply
/// never reports hardware frames on a platform where it cannot open VAAPI.
#[cfg(target_os = "linux")]
fn import_mapped(ctx: &RenderContext, mapped: MappedFrame) -> Option<SourceFrame> {
    let planes = mapped.dmabuf.planes();
    // Two layers over one buffer is what iHD exports for NV12 with
    // `SEPARATE_LAYERS`. Anything else — a packed BGRA from a VPP pass, a
    // three-plane YUV420 — would need its own case in the shader, and showing
    // the luma plane alone is a plausible-looking greyscale picture, which is
    // the worst way to be wrong.
    if planes.len() != 2 {
        tracing::debug!(
            planes = planes.len(),
            "a decoded surface has to arrive as two NV12 planes to be imported"
        );
        return None;
    }

    let luma = render::dmabuf::import_plane(ctx, &planes[0])?;
    let chroma = render::dmabuf::import_plane(ctx, &planes[1])?;

    // Rotation is deliberately not applied: applying it means touching pixels,
    // which is the thing being avoided. The compositor folds the angle into the
    // matrix it computes per quad anyway.
    let turns = mapped.rotation.rem_euclid(360) / 90;
    let matrix = yuv_matrix(mapped.color_space);
    let range = yuv_range(mapped.color_range);

    Some(SourceFrame::from_planes(
        Arc::new(luma),
        Arc::new(chroma),
        matrix,
        range,
        turns as u32,
        Some(Arc::new(PinnedSurface(mapped)) as render::FrameGuard),
    ))
}

#[cfg(not(target_os = "linux"))]
fn import_mapped(_ctx: &RenderContext, _mapped: MappedFrame) -> Option<SourceFrame> {
    None
}

/// FFmpeg's colour space to the three matrices a shader can apply.
///
/// Everything that is BT.601 by another name collapses onto `Bt601`: SMPTE
/// 170M, BT.470BG and FCC differ in primaries and transfer, not in the luma
/// weights the conversion uses. Anything genuinely unhandled falls back to
/// BT.709 and says so, because the alternative is refusing to draw a frame over
/// a colour tag.
fn yuv_matrix(space: ffmpeg::color::Space) -> render::YuvMatrix {
    use ffmpeg::color::Space;
    match space {
        Space::BT470BG | Space::SMPTE170M | Space::FCC | Space::SMPTE240M => {
            render::YuvMatrix::Bt601
        }
        Space::BT2020NCL | Space::BT2020CL => render::YuvMatrix::Bt2020,
        Space::BT709 => render::YuvMatrix::Bt709,
        other => {
            tracing::debug!(?other, "unhandled colour matrix; treating it as BT.709");
            render::YuvMatrix::Bt709
        }
    }
}

fn yuv_range(range: ffmpeg::color::Range) -> render::YuvRange {
    match range {
        ffmpeg::color::Range::JPEG => render::YuvRange::Full,
        // `Unspecified` included: limited is what a camera writes and what
        // every decoder assumes when nothing says otherwise.
        _ => render::YuvRange::Limited,
    }
}

/// Upload tightly packed RGBA8 to a fresh GPU texture.
///
/// `Rgba8UnormSrgb` rather than `Rgba8Unorm`: decoded video is sRGB-encoded, and
/// tagging it as such is what makes the sampler linearize before the compositor
/// blends. Blending sRGB values directly is the standard way to get muddy
/// cross-fades.
/// Upload NV12 planes as the two textures the compositor's YUV path samples:
/// `R8Unorm` luma and `Rg8Unorm` interleaved chroma at half resolution — the
/// same layout an imported VA surface has.
fn upload_nv12(ctx: &RenderContext, planes: &crate::modules::media::Nv12Planes) -> SourceFrame {
    let plane = |format, width: u32, height: u32, bytes_per_texel: u32, data: &[u8]| {
        let texture = ctx.device().create_texture(&wgpu::TextureDescriptor {
            label: Some("media source nv12 plane"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        ctx.queue().write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            data,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(bytes_per_texel * width),
                rows_per_image: Some(height),
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        Arc::new(texture)
    };

    let (width, height) = (planes.width, planes.height);
    let luma = plane(wgpu::TextureFormat::R8Unorm, width, height, 1, &planes.luma);
    let chroma = plane(
        wgpu::TextureFormat::Rg8Unorm,
        width.div_ceil(2),
        height.div_ceil(2),
        2,
        &planes.chroma,
    );
    let turns = planes.rotation.rem_euclid(360) / 90;
    SourceFrame::from_planes(
        luma,
        chroma,
        yuv_matrix(planes.color_space),
        yuv_range(planes.color_range),
        turns as u32,
        None,
    )
}

fn upload_rgba(ctx: &RenderContext, data: &[u8], width: u32, height: u32) -> SourceFrame {
    let texture = ctx.device().create_texture(&wgpu::TextureDescriptor {
        label: Some("media source"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });

    ctx.queue().write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        data,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(4 * width),
            rows_per_image: Some(height),
        },
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );

    SourceFrame::from_texture(Arc::new(texture))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::project::{CanvasConfig, ImageMaterial, VideoMaterial};

    /// A title added or retyped after the provider was built must reach it:
    /// the preview keeps one provider across edits for its open decoders.
    #[test]
    fn titles_added_or_changed_later_are_synced_in() {
        let mut project = Project::new("t", CanvasConfig::default(), 30.0);
        let mut provider = MediaSourceProvider::from_project(&project);
        assert_eq!(provider.len(), 0);

        let mut title = crate::modules::text::edit::default_material(&project, Some("a".into()));
        project.materials.texts.push(title.clone());
        provider.sync_texts(&project);
        assert_eq!(provider.len(), 1);

        title.content = "b".into();
        project.materials.texts[0] = title.clone();
        provider.sync_texts(&project);
        assert!(matches!(
            provider.sources.get(&title.id),
            Some(MaterialSource::Text(t)) if t.content == "b"
        ));
    }

    fn project_with_materials() -> Project {
        let mut project = Project::new("t", CanvasConfig::default(), 30.0);
        project.materials.videos.push(VideoMaterial {
            id: "v1".into(),
            path: "/nonexistent/clip.mp4".into(),
            width: 1920,
            height: 1080,
            duration: 5_000_000,
            fps: 30.0,
            has_audio: true,
            rotation: 0,
        });
        project.materials.images.push(ImageMaterial {
            id: "i1".into(),
            path: "/nonexistent/still.png".into(),
            width: 800,
            height: 600,
        });
        project
    }

    #[test]
    fn snapshot_covers_every_material_kind() {
        let provider = MediaSourceProvider::from_project(&project_with_materials());
        assert_eq!(provider.len(), 2);
        assert!(provider.sources.contains_key("v1"));
        assert!(provider.sources.contains_key("i1"));
    }

    #[test]
    fn an_empty_project_yields_an_empty_provider() {
        let project = Project::new("t", CanvasConfig::default(), 30.0);
        assert!(MediaSourceProvider::from_project(&project).is_empty());
    }

    /// A title has to arrive as a texture the size of the frame, not the size of
    /// the glyphs. `render::layout::fit_size` stretches a source to fill the
    /// canvas, so a tightly cropped title would come out as a wall of letters —
    /// and the segment's transform, which is what the user drags, would have
    /// nothing left to position.
    #[test]
    fn a_text_material_rasterises_at_the_requested_frame_size() {
        let Some(ctx) = crate::modules::render::test_context() else {
            eprintln!("skipping: no GPU adapter");
            return;
        };
        let mut project = Project::new(
            "t",
            CanvasConfig {
                width: 1080,
                height: 1920,
                background: [0.0, 0.0, 0.0, 1.0],
            },
            30.0,
        );
        project.materials.texts.push(TextMaterial {
            id: "t1".into(),
            content: "Hello".into(),
            font_family: String::new(),
            font_size: 72.0,
            color: [1.0, 1.0, 1.0, 1.0],
            bold: false,
            italic: false,
            align: Default::default(),
            stroke_width: 0.0,
            stroke_color: [0.0, 0.0, 0.0, 1.0],
            shadow: None,
            background: None,
            caption: None,
        });
        let provider = MediaSourceProvider::from_project(&project);

        // Half resolution, as the preview renders it.
        let frame = provider
            .frame(
                &ctx,
                &SourceRequest {
                    material_id: "t1",
                    kind: MaterialKind::Text,
                    source_time: 0,
                    segment_id: "seg",
                    max_size: (540, 960),
                },
            )
            .expect("rasterise")
            .expect("a text material draws something");
        assert_eq!(frame.size(), (540, 960));
        assert!(!frame.is_planar(), "a title is RGBA, not a video surface");

        // Full resolution afterwards, as the export asks for it. The cached
        // half-size raster must not be reused: that is a soft title in the
        // delivered file, and it is invisible in the preview that produced it.
        let full = provider
            .frame(
                &ctx,
                &SourceRequest {
                    material_id: "t1",
                    kind: MaterialKind::Text,
                    source_time: 0,
                    segment_id: "seg",
                    max_size: (1080, 1920),
                },
            )
            .expect("rasterise")
            .expect("a text material draws something");
        assert_eq!(full.size(), (1080, 1920));
    }

    /// The two ways a clip loses its media — the material removed from the
    /// pool, and the file removed from disk — both answer the placeholder
    /// rather than `None` or an error: the compositor draws it where the clip
    /// would be, so the missing state is visible instead of being a hole.
    /// The *pixels* of the placeholder are asserted through the real
    /// compositor in `tests/missing_media.rs`.
    #[test]
    fn missing_media_answers_the_placeholder_frame() {
        let Some(ctx) = crate::modules::render::test_context() else {
            eprintln!("skipping: no GPU adapter");
            return;
        };
        // Every path in this fixture points at /nonexistent.
        let provider = MediaSourceProvider::from_project(&project_with_materials());

        let request = |id: &'static str, kind: MaterialKind| SourceRequest {
            material_id: id,
            kind,
            source_time: 0,
            segment_id: "seg",
            max_size: (640, 360),
        };

        let ghost = provider
            .frame(&ctx, &request("not-in-the-pool", MaterialKind::Video))
            .expect("a dangling reference is not an error")
            .expect("and it draws something");
        let gone_file = provider
            .frame(&ctx, &request("v1", MaterialKind::Video))
            .expect("a file gone from disk is not an error")
            .expect("and it draws something");
        let gone_image = provider
            .frame(&ctx, &request("i1", MaterialKind::Image))
            .expect("a still gone from disk is not an error")
            .expect("and it draws something");

        // One shared canvas-aspect field, not three: the placeholder is built
        // once per provider and stretched by the compositor's fit.
        for frame in [&ghost, &gone_file, &gone_image] {
            assert!(!frame.is_planar(), "the placeholder is a plain RGBA field");
            assert_eq!(frame.size(), ghost.size());
        }
        let (w, h) = ghost.size();
        assert!(w >= 2 && h >= 2);
    }

    #[test]
    fn frame_epsilon_treats_one_render_step_as_distinct() {
        // Two adjacent frames at 60 fps are 16 666 µs apart, which must not
        // collapse to the same cache entry.
        assert!(FRAME_EPSILON < 16_666);
        // Repeated requests for the same nominal frame must hit, even with
        // rounding noise in the microsecond conversion.
        assert!(FRAME_EPSILON > 2);
    }
}
