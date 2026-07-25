# What the native engine exposes

CapCut's engine is five large C++ DLLs that between them export 41,840 named
symbols. Because the whole thing is built with MSVC and exports C++ names rather
than a flat C API, the mangled names carry full namespaces, class names, method
names and parameter types. Demangling them yields something close to a header
dump of the engine's public surface — without disassembling a single
instruction.

This matters for chukcut because it answers, from the horse's mouth, questions
that would otherwise be guesswork: how the timeline model is layered, where the
boundary between the editing model and the render graph sits, what a "filter"
actually is to this engine, how effects are fetched and cached, and which
subsystems are big enough that we should plan to *not* build them. It also gave
us the authoritative field lists used in
[`draft-format.md`](draft-format.md) for material categories that never appeared
in any sample project.

## Method

PowerShell has no `dumpbin`, and installing anything on the target machine was
out of scope. Instead a small PE parser was written locally, base64-encoded as
UTF-16LE and executed on the Windows box via
`powershell -NoProfile -EncodedCommand`, so nothing was written to that machine's
disk. It opens each DLL read-only, reads `e_lfanew` at offset `0x3C`, walks the
COFF and optional headers to data directory 0 (the export directory), maps that
RVA to a file offset through the section table, reads the containing section, and
walks `AddressOfNames` to print every exported name. Demangling was done locally
with `llvm-undname`.

| DLL | Machine | Export section | Exported names |
|---|---|---|---|
| `VECreator.dll` | PE32+ | `.rdata`, 100.7 MB | 1,808 |
| `videoeditor.dll` | PE32+ | `.rdata`, 14.6 MB | 31,602 |
| `EffectPlatform.dll` | PE32+ | `.rdata`, 316 KB | 765 |
| `cccreator.dll` | PE32+ | `.rdata`, 14.8 MB | 7,377 |
| `clipflow_sdk.dll` | PE32+ | `.rdata`, 344 KB | 288 |

Note the ratio: `VECreator.dll` is 236 MB but exports only 1,808 names, whereas
`videoeditor.dll` is a quarter of the size and exports 31,602. `VECreator` is
mostly *statically linked payload* — models, tables, embedded resources — behind
a narrow façade. `videoeditor` is a thin-but-wide model layer whose entire object
graph is exported so the UI process can talk to it.

---

## The five DLLs, by namespace

### `videoeditor.dll` — the editing model (`lvve`, `lyra`, `draft_store`)

31,602 exports, of which 25,000+ are in namespace **`lvve`** ("LV video editor";
LV = *Lightning Video*, the internal name of the CapCut/JianYing family). This is
not a renderer. It is the **document model**: the in-memory representation of
`draft_content.json`, plus undo, plus the command layer the UI drives.

**Timeline model.** `lvve::Draft`, `lvve::Track`, `lvve::Segment` and its
subclasses `SegmentVideo`, `SegmentAudio`, `SegmentText`, `SegmentSticker`,
`SegmentTextTemplate`, `SegmentShape`, plus `lvve::Clip`, `lvve::TimeRange`,
`lvve::UniformScale`, `lvve::Crop`, `lvve::HdrSettings`,
`lvve::ResponsiveLayout`. Seventy-eight distinct `lvve::Material*` classes —
roughly one per material category in the draft file, plus sub-objects such as
`MaterialEffectParam`, `MaterialFlowerStroke` and `MaterialColorConfig`. Their
`get_*` / `set_*` accessor names are exactly the JSON keys, which is how the
draft format could be documented completely.

**The node tree and undo.** `lvve::Node` alone accounts for 9,860 symbol
occurrences and `lvve::DraftTreeNode` for 565. Every model object derives from
`Node` and inherits `is_dirty`, `node_name`, `all_nodes`, `stash_copy`,
`escape_history_nodes`, `deep_copy_raw`, `restore_from` / `restore_to` /
`restore_by_diff`, and — the interesting part —
`to_patch_json` / `patch_from_json` / `dirty_node_to_patch_json`. The document is
a tree of dirty-tracked nodes, and edits are serialised as **JSON patches over
that tree**. Undo/redo, autosave and cloud sync are all the same mechanism.
`lvve::RuntimeTransfer` (288) and `lvve::PersistentTransfer` (277) are the two
serialisation modes: what goes to the engine versus what goes to disk. That split
— a runtime projection and a persistent projection of one model — is a design
chukcut should consider adopting rather than serialising the live model directly.

**The command layer.** A set of static `*Client` classes forms the RPC surface
the UI calls: `VideoClient` (292 methods), `TextTemplateClient` (202),
`CommonClient` (183), `PlayerClient` (116), `TemplateClient` (95), `TextClient`,
`AudioClient`, `AttachmentClient`, `MixedClient`, `CoverClient`,
`ScriptVideoClient`, `SingleResourcePlayerClient`, `TranscriptEditClient`,
`StickerClient`, `AsyncTaskClient`, `TextTemplateEditorClient`, `AiEditClient`,
`AdcubeClient`, `ProjectClient`, `GlobalAdjustClient`. This is the API the
Chromium/Lynx UI actually talks to. It is coarse-grained and verb-shaped
(`VideoClient::…`), not a generic property bag — consistent with an IPC boundary.

**The bridge to the renderer.** `lvve::NewVEWrapper` is where the editing model
meets the video engine, and its method list reads like a transaction log:
`begin` / `commit`, `addTrack`, `addClip` / `addClipCCModel`, `insertFilter` /
`insertFilterCCModel`, `insertVideoTransition`, `insertAudioTransition`,
`deleteKeyFrame`, `addSubSeq` / `createSubSeq`, `addWaterMark`,
`export_start` / `export_audio_start` / `multi_export_start` / `export_cancel`,
`GetSpecificTimeImageAsync`, `getPrerenderSegmentInfo`, `isPrerenderRunning`,
`CheckIsSupportGL`, `GetGpuInfo`, `DumpSequence`. Two things stand out. First,
edits are **batched between `begin` and `commit`** — the renderer is not
re-configured per property change. Second, the `…CCModel` variants show that
everything can be expressed either as an imperative call or as a declarative
"CCModel" object handed over wholesale (see `VECreator` below).

`PlayerClient` covers preview: `initPlayer` / `initPlayerPC` /
`initNetworkPlayer` / `initPcmPlayer`, `currentTime`, `getCurrentFramePts`,
`flushSeekCmd`, `getCurrDecodeImage(WithColor)`, `getSingleTrackProcessedImage`,
`isHdrSupported`, `getColorSpace` / `getEditorColorSpace`,
`getCurrentTimeKeyframeJson`, `getPerformanceInfo`. Note
`getSingleTrackProcessedImage` — the engine can render one track in isolation,
which is how the UI draws per-clip thumbnails without a second pipeline.

**Utilities worth knowing about**, because each names a problem we will also hit:
`CoordinateUtils`, `VideoCoordinateUtils`, `SmartCropCoordinateUtils` (three
separate coordinate-space converters — a warning about how many spaces this
format has), `RenderIndexUtils`, `KeyframeUtils`, `CurveSpeedUtils`,
`ColorCurvesUtils`, `VideoMaskUtils`, `SegmentUtils`, `DraftSizeUtils`,
`ConvertUtils`, `EncryptUtils`.

**`draft_store`** is the persistence layer proper: `DraftStore`, `StoreWrapper`,
`DraftStoreManager`, `DraftIO` / `DraftIOFactoryBase` / `IOManager`,
`Transaction`, `VirtualStore`, `MultiTimelineContext`, `TimelineData`,
`DraftUploadCloudStore` / `DraftRestoreCloudStore`, `MaterialType`, `kv_json`.
The names map one-to-one onto the files documented in `draft-format.md`
(`VirtualStore` → `draft_virtual_store.json`, `MultiTimelineContext` →
`Timelines\project.json`, `kv_json` → `key_value.json`).

**`lyra`** is the text and AI-editing layer: `RichTextUtils`, `RichTextLetter`,
`RichTextStyle`, `RichTextSelectRange`, `TextTemplateUtils`, `TextClipUtils`,
`CaptionAnimUtils`, `CaptionKeyWordStyles`, `CaptionCapital`, `FlowerUtils`
("flower" is CapCut's word for decorative text fills), `MaskUtils`, `AIEdit`,
`AlgorithmUtils`, `ScriptStandaloneMgr`, `SyncToAllManager`, `AbtestConfig`,
`Session` / `Server` / `RespStruct` (534 occurrences — a local HTTP/RPC server
surface).

Finally, `videoeditor.dll` exports 51 plain-C symbols. Nine are miniz/zip
(`mz_zip_reader_init`, `tinfl_decompress`, `zip_extract`) — it opens effect
packages itself. The other 42 are the `VKFF*` keyframe-field name constants
(`VKFFPosition`, `VKFFScale`, `VKFFCommonMaskSize`, `VKFFEffectAdjustParam1`, …,
`VKFFEnd`), the internal enum-to-string table behind the `KFType*` strings in the
draft file.

### `VECreator.dll` — the video engine façade (`vesdk`, `davinci::effectplatform`, `gecko_cpp`)

236 MB, 1,808 exports. Three namespaces.

**`vesdk::ccmodel`** (949 occurrences) is the render graph, declared as a model
rather than built imperatively. `CCModel` is the root; `CCClip`, `CCAVClip`,
`CCStreamClip`, `CCEffectClip`, `CCAlgorithmClip` are the sources; and then a
long list of filters, which is effectively the engine's node catalogue:

*Geometry and composition* — `CCTransformFilter`, `CCImageTransformFilter`,
`CCPreviewTransformFilter`, `CCVideoCropFilter`, `CCVideoCanvasFilter`,
`CCVideoBackgroundFilter`, `CCVideoBlurFilter`, `CCRadiusCornerBorderFilter`,
`CCPreviewMagnifierFilter`, `CCReverseFilter`.

*Effects and stickers* — `CCAmazingFilter` (the AmazingEngine effect host),
`CCBefEffectFilter`, `CCInfoStickerFilter`, `CCLayerStickerFilter`,
`CCScriptStickerFilter`, `CCBrushStickerFilter`, `CCStickerTemplateFilter`,
`CCStickerAnimationFilter`, `CCTextStickerFilter` (119–121 occurrences — the
single largest filter, matching how much of the draft format is text),
`CCVideoAnimationFilter`, `CCVideoTransitionFilter`, `CCPluginFilter`,
`CCCustomFilter` / `CCCustomPreFilter` / `CCCustomPostFilter` /
`CCCustomBaseFilter`.

*Colour* — `CCColorHslFilter`, `CCFilter`, `CCGreenEditingFilter`.

*ML-backed* — `CCAiCutOutClipFilter` (matting), `CCDigitalManFilter`,
`CCMakeupFilter`, `CCSmartLightingFilter`, `CCObjectTrackerFilter`,
`CCMotionBlurFilter`, `CCVideoStableFilter` / `CCVideoStableFilterEX`,
`CCLensVFIFilter` (frame interpolation), `CCLensVideoDenoiseFilter`,
`CCLensDeflickerFilter`, `CCSwingPostProcessFilter`, `CCAlgorithm`,
`CCLeaderSecurityDetectFilter`.

*Audio* — `CCAudioVolumeFilter`, `CCAudioFadeFilter`, `CCAudioCrossFadingFilter`,
`CCAudioSpeedFilter`, `CCAudioInvertFilter`, `CCAudioNoiseFilter`,
`CCAudioLoudnessBalanceFilter`, `CCAudioPreprocessFilter`, `CCAudioSamiFilter` /
`CCAudioSamiProcessorFilter` / `CCAudioSamiAECFilter` (SAMI is ByteDance's speech
stack), `CCAudioMdspSingleProcessorFilter`.

`vesdk::pub` holds the public enums, and reading them is instructive:
`FilterType`, `FilterSubType`, `FilterCoveringRelation`, `ClipType`,
`ClipStreamType`, `EffectClipDecodeType`, `AmazingEffectMode`,
`AmazingEffectSubType`, `AmazingEffectAlgorithmType`,
`AmazingEffectPostProcessType`, `AmazingBeautifulBodyType`, `HslParam`,
`AudioFadingCurveType`, `MotionBlurAlgorithmType`, `MotionBlurMergeType`,
`LensVFIType`, `LensDeflickerType`, `SamiType`, `SamiAECType`,
`VEBlurRadiusCalculationMode`, `ColorBlock`, `AnimationInfo`,
`VideoCaptureDeviceInfo` / `AudioCaptureDeviceInfo`. `FilterCoveringRelation` is
the one to note: filters declare how they *cover* each other, which is how the
engine resolves ordering conflicts between effects added by different features.

**`davinci::effectplatform`** (600+ occurrences, and the whole of
`EffectPlatform.dll`) is the effect *delivery* system — see below.

**`gecko_cpp`** (`GeckoClient`, `GeckoUpdateParams`, `GeckoDownloadParams`,
`GeckoPostParams`, `GeckoUpdateResult`) is ByteDance's generic resource
hot-update channel, also shipped as `TTPGeckoCppSDK.dll`. It is how packages
land on disk before `EffectPlatform` indexes them.

The single plain-C export from this 236 MB library is `entryPoint`.

### `cccreator.dll` — the effect runtime (`AmazingEngine`, `Bach`, `bef_*`)

150 MB, 7,377 exports, and the most revealing of the five. The name suggests
"colour correction creator", but the contents say otherwise: this is the
**AmazingEngine effect runtime plus its ML stack plus a Vulkan backend plus a
video encoder**, all statically linked into one object.

**`AmazingEngine`** (1,926 occurrences) is a small game engine. It has a scene
graph (`Scene`, `Segment` and subclasses `VideoSegment`, `TextSegment`,
`StickerSegment`, `StickerCommonSegment`, `StickerBrushSegment`, `EmojiSegment`,
`ScriptSegment`, `FeatureSegment`, `TransitionSegment`, `TemplateSegment`,
`CustomSegment`), a maths library (`Vector2f/3f/4f`, `Matrix3x3f`, `Matrix4x4f`,
`Quaternionf`, `Rect`, `Color`, `ColorRGBA32`, `PerlinNoise`,
`TimeInterpolateType`), its own RTTI (`RTTI`, `_RTTIOf`, 530 occurrences), its
own memory manager (`memory::MemoryManager`, `MemoryPool`, `MemoryStream`,
`MemoryReader` / `MemoryWriter`), its own file layer (`File`, `FileHandle`,
`FileReader` / `FileWriter`, `FileUtils`, `Archive`), a thread pool, a font
rasteriser (`stbtt_fontinfo`, `stbtt_pack_context` — stb_truetype), a networking
client (`NetworkClient`, `NetworkClientWS`, `NetworkRequest`, `NetworkCall`,
`AllowListManager`), and a JavaScript task system (`JsTaskStatus`,
`JsAlgorithmInputType`). `AMGPixelFormat`, `AMGDataType`, `AMGAlphaStateType` and
`RendererType` are the graphics abstraction.

The `Swing*` family (`SwingManager`, `SwingObjectTracker`, `SwingSurfaceTracker`,
`SwingSurfaceTrackingController`, `SwingTextureManager`, `SwingBaseBlender`,
`SwingSegmentType`) is the compositor that sits between the draft's segments and
the effect engine — `CCSwingPostProcessFilter` in `VECreator` is its entry point.

**`Bach`** (658 occurrences) is the ML runtime: `BachAlgorithmSystem`,
`BachAlgorithmFactory`, `BachAlgorithmModel`, `BachAlgorithmInput`,
`BachMultiInput`, `BachImageBuffer` / `BachGPUBuffer` / `BachCacheBuffer`,
`BachTextureInfo`, `BachResourceFinder` (+ `File`- and `Composer`-flavoured
finders and a `BachDownloadableResourceFinder`), `BachCacheSerializationV2`,
`MattingResult`, `FaceBaseMask`, `FaceBuffer`, `AETrackable`, `AEPlaneAnchor`.
The presence of `BachDownloadableResourceFinder` and
`BachAlgorithmSystemWithDevice` says models are fetched on demand and dispatched
per accelerator — matching the 771 MB of SmartCrop models in the user cache and
the `openvino.dll` / `openvino_intel_npu_plugin.dll` pair in the install.

**The `bef_*` C API** (1,286 exports) is the stable ABI other ByteDance products
consume, and it is the clearest statement of what an "effect" is:

* `bef_effect_set_*` / `get_*` (214) — parameter plumbing, including
  `bef_effect_set_render_cache_int_value`, `bef_effect_set_resolution`,
  `bef_effect_set_algorithm_force_detect`, `bef_effect_set_assigned_model_names`.
* `bef_effect_composer_*` (31) — the **node composer**: `set_nodes`,
  `append_nodes`, `replace_nodes`, `remove_nodes`, `update_node`,
  `update_node_with_json`, `get_node_paths`, `get_node_value`,
  `check_node_exclusion`, `set_states`, plus a 2D brush sub-API. This is how
  beauty/retouch effects are assembled from a path-addressed node graph at
  runtime, and `update_node_with_json` confirms the parameter interface is JSON.
* `bef_swing_*` (230) — the segment/keyframe API of the effect engine:
  `bef_swing_manager_create`, `add_segment`, `process_touchDownEvent`,
  `bef_swing_key_frame_create` / `set_config` / `get_value_static` /
  `remove_attribute`, `bef_swing_interpolation_cubic_bezier_static`,
  `bef_swing_caption_split_line_page`, `bef_swing_convert_text_case`.
  Effects have their **own** keyframe system, independent of the draft's.
* `bef_ae_*` (146) — an After-Effects-compatible layer including a 3D "brick"
  engine with cameras and world transforms (`bef_ae_brick_engine_create`,
  `bef_ae_brick_add_camera`, `set_camera_world_position`, `set_msaa_mode`).
* `bef_effect_javascript_*` (11) and `bef_effect_slam_*` (10),
  `bef_effect_facewarp_*` (21), `bef_effect_mv_*` (30),
  `bef_effect_enigma_*` (QR/dynamic-code generation),
  `bef_info_*` (156, InfoSticker), `bef_portrait_*`, `bef_bingo_*`,
  `bef_moment_*`, `bef_smash_*`, `bef_fs_*`.

Also linked in: **314 `tl_*` tensor operations** with `_cuda` and `_d2d`
suffixes (`tl_tensor_softmax_prepare_cuda`, `tl_tensor_pad_prepare_cuda`,
`tl_clone_h2d`) — a small in-house tensor library with a CUDA path; **449
`vk*` Vulkan entry points**, so the effect engine has a Vulkan backend in
addition to the GLES/ANGLE path used for the editor preview; **`ByteVC1*` and
`bytevc0*`** — ByteDance's proprietary codec encoder, in 8-bit and 10-bit
variants; **`unqlite_*`** (38) — an embedded key/value database, which is what
backs the effect caches; **`BEF::BEFContext` / `BEFController`**;
**`LENS::FRAMEWORK`**; **`mammonengine::AudioBackend`**; and **`NvCVImage`**
(NVIDIA Video Effects SDK interop).

The `AmazingEngine::AllowList` / `AllowListManager` and
`CCLeaderSecurityDetectFilter` classes are worth flagging: the runtime gates
which effects and which algorithms may run, so a package is not simply
self-describing.

### `EffectPlatform.dll` — effect discovery, fetch and cache (`davinci::effectplatform`)

Only 765 exports and 1.5 MB, but it defines the entire online-materials story.
Two parallel platform implementations:

* **`loki`** — the older/internal one: `LokiPlatform`, `LokiPlatformConfig`,
  `LokiPlatformUtils`, `Effect`, `CategoryEffectModel`, `CategoryInfoModel`,
  `CategoryPageResponseModel`, `EffectChannelResponseModel`,
  `EffectListResponseModel`, `PanelInfoResponseModel`,
  `ResourceListResponseModel`, `ResourceListBean`, `CheckUpdateResponseModel`,
  `AlgorithmModelRecord` / `AlgorithmModelRecordList`,
  `LokiRequirementsPeeker`, `LokiAlgorithmEventReport`, `UrlModel`.
* **`heycan`** — the current consumer one: `HeyCanPlatform`,
  `HeyCanPlatformConfig`, `Effect`, `EffectListResponse`, `StickerModel`,
  `StickerPackage`, `TextTemplateModel`, `AudioModel`, `VideoModel`,
  `BeatsModel`, `RecipeModel`, `SpecialEffectModel`, `CollectionModel`,
  `ArtistCoverModel`, `AuthorModel`, `ReviewInfoModel`, `BusinessInfo`,
  `DependResource` / `DependResConfig`, favourites
  (`AddToFavoriteListResponse`, `FetchFavoriteListData`, …), search
  (`SearchEffectsResponseModel`, `SearchOptional`, `SuggestWordsResponseModel`,
  `RecommendSearchWordsResponseModel`, `FilterOption`), `TabIcon`,
  `CategoryInfoResponseModel`, `PanelInfoResponseModel`.

Three structural facts fall out. `LokiRequirementsPeeker` and
`AlgorithmModelRecord` mean an effect package **declares which ML models it
requires**, and the platform resolves and downloads those separately.
`DependResource` / `DependResConfig` mean effects can depend on other effects.
And `PanelInfoResponseModel` means the *UI panel layout* — which categories,
which tabs, which icons — is served from the network, not compiled into the app;
this is the seam between the effect system and the UI described in
[`ui-inventory.md`](ui-inventory.md).

Everything is delivered through `ResourceFetchCallback<T>` /
`StdFunctionResourceFetchCallback<T>` templates (115 + 30 instantiations), with
`PlatformHttpClientDelegate` and `DAVUnZipper` doing transport and unpacking, and
`DAVResourceManagerWrapper` owning the on-disk cache.

### `clipflow_sdk.dll` — the AI task graph (`clipflow`)

288 exports, and the newest-feeling code in the install. `clipflow` is a
**directed task graph executor**, not a video component:
`ClipFlowClient::createClipFlowTask`, `createClipFlowNode`, `createDecisionNode`,
`createNodeCluster`, `createTaskGrapher`, `copySubGrapher`, `joinNode`,
`joinTask`, `exec`, `abortNode`, `retryTask`, `resume`,
`removeDependNodesRecursively`, `getAllNodesOutputs`, `generateValidInputJson`,
plus a full listener/hook system (`addNodeStageHooker`, `addTaskStageHooker`,
`addTaskProgressListener`, `postNodeCustomEvent`) and a two-level cache
(`IClipflowGlobalCache`, `IClipflowNodeCache`, `readClipflowCache`,
`restoreClipflowTaskFromCache`).

The give-away symbols are `runAgentToolsExecutor`,
`queryAgentToolsExecutorTaskInCache`, `getNodeIdToToolCallId` and
`getToolCallIdToNodeId`. `clipflow` is the orchestration layer for LLM-driven
editing: nodes are tool calls, the graph is a plan, and results are cached and
resumable. `ClipflowScene` and `LyraClipflowNodeDesc` bind it back to the `lyra`
text/AI layer in `videoeditor.dll`. `releaseBindDraftTasks` ties task lifetimes
to a draft.

---

## What the symbol table says about the architecture

**The model layer is enormous and the render layer is narrow.** 31,602 exports
describe the document; 1,808 describe the engine façade. Almost all of CapCut's
complexity is in *representing an edit*, not in *drawing a frame*. That is a
useful calibration: the hard part of a video editor is the model.

**Edits are transactions of JSON patches over a dirty-tracked node tree.**
`Node::to_patch_json`, `dirty_node_to_patch_json`, `patch_from_json`,
`restore_by_diff`, `stash_copy`, `escape_history_nodes`, plus
`NewVEWrapper::begin`/`commit` and `draft_store::Transaction`. One mechanism
serves undo, autosave, cloud sync and engine reconfiguration. chukcut currently
has no equivalent and would benefit from one.

**There are two model projections, not one.** `RuntimeTransfer` versus
`PersistentTransfer`. What the engine sees is not what is written to disk.

**A "filter" is a first-class graph node with an ordering policy.** The
`CC*Filter` catalogue plus `FilterCoveringRelation` and
`MaterialVideoEffect::covering_relation_change` show that ordering conflicts
between independently added effects are resolved by declared policy rather than
by insertion order. Any editor that lets multiple features attach effects to the
same clip needs this and will otherwise accumulate ad-hoc ordering hacks.

**Effects carry their own scene graph, their own keyframes and their own script
runtime.** `AmazingEngine::Scene`/`Segment`, `bef_swing_key_frame_*`, and a
JavaScript task system inside `cccreator.dll`. The draft's `KFTypeEffectAdjustParam1/2/3`
are only the three slots by which the *host* timeline can animate an effect
whose internals animate themselves. This is the correct division of labour and
chukcut should copy it: do not try to expose an effect's internal parameters as
timeline keyframes.

**Effects declare their ML dependencies and are gated.** `LokiRequirementsPeeker`,
`AlgorithmModelRecord`, `BachDownloadableResourceFinder`,
`AmazingEngine::AllowListManager`. Packages are not self-contained.

**Three graphics backends coexist.** GLES via ANGLE (`VEAngle\libGLESv2.dll`) for
the editor preview, Vulkan (449 `vk*` symbols in `cccreator.dll`) inside the
effect runtime, and D3D11 underneath ANGLE. Plus SwiftShader in the CEF
directory for the browser. A wgpu-based compositor sits at the same abstraction
level as ANGLE and is a defensible choice.

**The AI layer is a separate, cache-first task graph.** `clipflow_sdk.dll` is
architecturally independent of the timeline and communicates through node inputs
and outputs. Whatever chukcut does with LLM-assisted editing should be a
sidecar, not woven into the model.

---

## Every DLL in the install directory

`C:\Users\user\AppData\Local\CapCut\Apps\9.0.0.3858\`. 164 DLLs at the top level
plus 78 in subdirectories, and 17 EXEs. Sizes are on-disk bytes.

### Engine and media

| File | Size | Purpose |
|---|---|---|
| `VECreator.dll` | 236.1 MB | Video engine façade: `vesdk::ccmodel` render-graph model, effect-platform client, Gecko resource updater. |
| `cccreator.dll` | 150.0 MB | AmazingEngine effect runtime, Bach ML runtime, `bef_*` C API, Vulkan backend, ByteVC1 encoder, tensor library. |
| `lens.dll` | 84.4 MB | Real-time ML video effects (denoise, deflicker, frame interpolation) — the `CCLens*Filter` implementations. |
| `videoeditor.dll` | 62.6 MB | Editing document model (`lvve`), draft store, command clients, engine wrapper. |
| `audioeffect.dll` | 13.4 MB | Audio DSP effect chain. |
| `speechsdk.dll` | 7.9 MB | Speech recognition / TTS (SAMI). |
| `res_pool.dll` | 7.8 MB | Shared resource pool / texture cache. |
| `SvtAv1Enc.dll` | 7.1 MB | SVT-AV1 encoder. |
| `nvjpeg64_12.dll` | 7.3 MB | NVIDIA GPU JPEG codec. |
| `avcodec-61.dll` | 18.8 MB | FFmpeg 6.1+ codec library (custom build). |
| `avfilter-10.dll` | 5.4 MB | FFmpeg filters. |
| `avformat-61.dll` | 3.2 MB | FFmpeg containers. |
| `avutil-59.dll` | 1.4 MB | FFmpeg utilities. |
| `avdevice-61.dll` | 172 KB | FFmpeg capture devices. |
| `swscale-8.dll` | 834 KB | FFmpeg scaling / pixel-format conversion. |
| `swresample-5.dll` | 195 KB | FFmpeg audio resampling. |
| `ffmpeg.dll` | 458 KB | Thin wrapper over the above. |
| `dav1d.dll` | 1.0 MB | AV1 decoder. |
| `ByteVC1_dec.dll` | 925 KB | ByteVC1 decoder (the encoder is inside `cccreator.dll`). |
| `codec_extend.dll` | 71 KB | Codec registration shims. |
| `libvpl.dll` | 485 KB | Intel oneVPL hardware video acceleration. |
| `libmp3lame-0.dll` | 597 KB | MP3 encoder. |
| `jxl.dll` / `jxl_cms.dll` / `jxl_threads.dll` | 3.1 MB / 155 KB / 41 KB | JPEG XL. |
| `ttheif_dec.dll` | 1.4 MB | HEIF decoder. |
| `libraw.dll` | 1.1 MB | Camera RAW decoder. |
| `lcms2.dll` | 560 KB | ICC colour management. |
| `libvecrptor.dll` | 103 KB | Video-engine crypto helper (encrypted media/assets). |

### ML inference

| File | Size | Purpose |
|---|---|---|
| `openvino.dll` | 13.6 MB | Intel OpenVINO inference runtime. |
| `openvino_intel_npu_plugin.dll` | 2.7 MB | OpenVINO NPU device plugin. |
| `openvino_ir_frontend.dll` | 427 KB | OpenVINO IR model loader. |
| `bytenn_openvinowrapper.dll` | 72 KB | ByteNN → OpenVINO adapter. |
| `metasecml.dll` | 10.6 MB | ByteDance anti-abuse / device-attestation ML. |
| `fastcv.dll` | 683 KB | Qualcomm FastCV computer-vision primitives. |
| `Tracking.dll` | 1.6 MB | Object/motion tracking. |
| `nuro.dll` | 826 KB | ByteDance NN runtime component. |
| `tbb12.dll`, `vcomp140.dll` | 189 KB, 180 KB | Intel TBB and OpenMP, used by the above. |

### Graphics

| File | Size | Purpose |
|---|---|---|
| `VEAngle\libGLESv2.dll` | 5.4 MB | ANGLE — GLES 2/3 translated to Direct3D 11. The engine's graphics target. |
| `VEAngle\libEGL.dll` | 214 KB | ANGLE EGL. |
| `opengl32sw.dll` | 19.7 MB | Mesa llvmpipe software GL fallback for Qt Quick. |
| `d3dcompiler_47.dll` | 4.7 MB | HLSL compiler used by ANGLE. |

### UI

| File | Size | Purpose |
|---|---|---|
| `cef\libcef.dll` | 207.5 MB | Chromium Embedded Framework — the web panels. |
| `cef\libGLESv2.dll`, `cef\libEGL.dll` | 7.3 MB, 475 KB | CEF's own ANGLE. |
| `cef\vk_swiftshader.dll`, `cef\vulkan-1.dll` | 4.9 MB, 935 KB | SwiftShader software Vulkan for CEF. |
| `cef\dxcompiler.dll`, `cef\dxil.dll` | 20.9 MB, 1.4 MB | DirectX shader compiler for CEF. |
| `cef\chrome_elf.dll` | 1.4 MB | Chromium early-loader / crash hooks. |
| `CefCreator.dll` | 2.9 MB | CapCut's CEF host and JS bridge. |
| `PlatinumWebView.dll` | 945 KB | Lightweight embedded web view. |
| `lynx.dll` | 22.0 MB | Lynx — ByteDance's React-Native equivalent. |
| `FusionUI.dll` | 2.2 MB | CapCut's own widget/design-system layer. |
| `Qt6Core.dll` … `Qt6Widgets.dll` | 25 MB total | Qt 6: Core, Gui, Widgets, Quick, Qml, QmlModels, QuickControls2 (+Impl, Templates, Layouts, Shapes, Particles, Dialogs), OpenGL, Network, Svg, Sql, Concurrent, Core5Compat, QmlWorkerScript, QmlXmlListModel, QmlLocalStorage, LabsSettings, LabsQmlModels. |
| `Qt6Bodymovin.dll` | 227 KB | Lottie/Bodymovin animation playback in native Qt UI. |
| `plugins\*` (39 DLLs) | ~5 MB | Qt platform (`qwindows`), image formats (jpeg, png, webp, tiff, svg, gif, icns, ico, tga, wbmp), TLS backends (openssl, schannel, cert-only), SQL drivers (sqlite, odbc, psql), icon engines, network information, `qmltooling` debug plugins. |
| `QtQuick\*`, `QtQml\*`, `Qt\labs\*`, `Qt5Compat\*` (36 DLLs) | ~5 MB | QML module plugins, including all five Controls styles (Basic, Fusion, Imagine, Material, Universal, Windows), `lottieqtplugin`, and `Qt5Compat.GraphicalEffects`. |
| `gettext.dll` | 165 KB | Message catalogue lookup. |

### Networking, telemetry, platform

| File | Size | Purpose |
|---|---|---|
| `pc_push.dll` | 23.0 MB | Push-notification client. |
| `sscronet.dll` | 8.9 MB | Cronet — Chromium's network stack, used standalone. |
| `ever_cloud_sdk.dll` | 8.7 MB | Cloud-drafts storage SDK. |
| `tt_c2pa_sdk.dll` | 7.6 MB | C2PA content-provenance signing. |
| `uploader.dll` | 1.5 MB | Chunked media upload. |
| `boringssl.dll` | 1.5 MB | TLS. |
| `bd_mojo.dll` | 1.1 MB | Mojo IPC (Chromium's IPC layer) — the process bridge. |
| `TTNetDownloaderCrossPlatform.dll` | 322 KB | Resource downloader. |
| `NetworkStateMachineSDK.dll` | 89 KB | Connectivity state machine. |
| `TTPGeckoCppSDK.dll` | 414 KB | Gecko hot-update resource channel. |
| `bytebench.dll`, `bytebenchsdk.dll` | 1.0 MB, 930 KB | Device benchmarking (chooses codec/ML paths). |
| `parfait.dll`, `parfait_wer.dll` | 1.3 MB, 23 KB | Crash/ANR reporting. |
| `deviceregister_shared.dll` | 147 KB | Device registration. |
| `mmkv.dll` | 196 KB | Tencent MMKV mmap key-value store — the settings backend. |
| `Settings.dll`, `VEConfig.dll` | 346 KB, 305 KB | Settings and engine configuration. |
| `UETSDKWrapper.dll` | 227 KB | Marketing attribution SDK. |
| `jazz.dll` | 1.4 MB | ByteDance internal service component. |
| `ttwinrt.dll` | 83 KB | WinRT interop. |
| `Microsoft.WindowsAppRuntime.Bootstrap.dll` | 191 KB | Windows App SDK bootstrapper. |
| `VESafeGuard.dll`, `VEExceptMonitor.dll` | 372 KB, 190 KB | Engine watchdog and exception monitor. |
| `Dbghelp.dll`, `minidump_stackwalk.dll` | 1.8 MB, 2.2 MB | Crash dump capture and symbolisation. |
| `7z.dll` | 1.4 MB | Archive extraction (effect packages, updates). |
| `zlib.dll`, `brotli{enc,dec,common}.dll`, `libiconv.dll` | ~2 MB | Compression and text encoding. |
| `jsoncpp.dll` | 210 KB | JSON. |
| `boost-di.dll` | 122 KB | Dependency injection. |
| `system_wrappers.dll`, `pthreadVC2.dll` | 228 KB, 94 KB | Platform abstraction and pthreads. |
| `base.dll` | 2.0 MB | Chromium `//base` utilities. |
| MSVC runtime: `msvcp140*.dll`, `vcruntime140*.dll`, `concrt140.dll`, `ucrtbase.dll`, `msvcrt.dll` | ~2.7 MB | C++ runtime. |
| 47 × `api-ms-win-*.dll` | ~1.2 MB | Universal CRT forwarder stubs. |

### Executables

`CapCut.exe` (86 KB — a launcher stub), `VEHelper.exe` (3.4 MB, the engine
helper process), `VEDetector.exe` (4.2 MB, GPU/codec capability probing),
`VECrashHandler.exe`, `parfait_crash_handler.exe`, `minidump_stackwalk.exe`,
`taskcontainer.exe` (43 KB, sandboxed task host), `ttdaemon.exe`,
`push_detect.exe`, `feedbacktool.exe`, `ffmpeg.exe`,
`CapCut-DiffUpgrade.exe` + `courgette64.exe` + `hpatchz.exe` (binary-diff
updater), `uninstshell.exe`, and `Service\CapCutService.exe` +
`Service\ShellRegSvrX64.exe` + `Service\CapCutShellExtX64.dll` (Explorer shell
integration).

---

## Limitations

The export table gives names and parameter *types*, not semantics: a
`set_covering_relation_change(int)` tells us the field exists and is an integer,
not what the integers mean. Nothing here was disassembled or decompiled, so any
statement about behaviour is inference from naming, and is flagged as such above.

Symbols that are internal (not exported) are invisible. In particular
`VECreator.dll`'s 236 MB contains a great deal that never reaches the export
table, and the actual shader compilation and render-pass scheduling live there.

`lens.dll`, `audioeffect.dll`, `res_pool.dll` and `Tracking.dll` were not dumped;
only the five DLLs named in the brief were. They would be the obvious next
targets if the ML filter parameters matter.
