# The CapCut effect package format

This is a reference description of the container that CapCut downloads and
executes whenever a user applies an effect, a filter, a text animation, a
sticker animation or a beauty adjustment. It matters for chukcut for two
reasons. First, it is the only place in the whole CapCut stack where the
render engine's internal data model is written down in a readable form — the
scene graph, the material system, the render-target descriptors and the shader
pass configuration are all serialised into these packages, so reading them
tells us how the engine that
[`capcut-stack.md`](capcut-stack.md) inventories is actually organised
internally. Second, if chukcut ever wants to consume third-party effect
packages — its own, or user-supplied ones from a URL — this is the shape the
ecosystem already expects.

Nothing here was decompiled. This is a file survey of the effect cache of a
CapCut 9.0.0.3858 installation on Windows: 209 downloaded packages, 11,054
files, of which roughly thirty packages spanning every family present were
unpacked and read. The SQLite catalogue that sits beside the cache was read for
its schema. No ByteDance asset is reproduced here beyond short excerpts needed
to show a schema.

An earlier reverse-engineering pass on an older CapCut version exists in
`python_renderer_specs/` in the `x` repo (May 2026). Where this document
contradicts or extends that pass, it says so explicitly; the
[**"What changed since the 2026-05 analysis"**](#what-changed-since-the-2026-05-analysis)
section at the end is the important part if you have already read those specs.

---

## 1. The cache on disk

Downloaded effects live under
`%LOCALAPPDATA%\CapCut\User Data\Cache\effect\`. The layout is two levels deep
and is completely flat — there is no hash-prefix fan-out, no sharding, no
lockfile, no index file of any kind inside the directory.

```
Cache\effect\
├── 7255573948300005889\                                  ← resource_id
│   ├── 729d5e76cb22236d447b602d48d31b91\                 ← extracted package
│   │   ├── config.json
│   │   └── ...
│   └── 729d5e76cb22236d447b602d48d31b91_tmp              ← the downloaded ZIP
├── 7501974767453474064\
│   └── ...
├── model\                                                ← ML model blobs, flat
└── script_segment_js\                                    ← shared JS runtime
```

The **outer directory name is the resource ID** — a ByteDance Snowflake-style
64-bit decimal (`7255573948300005889`). A handful of much shorter numeric IDs
(`1068046520` … `1068046527`, `1220399932`) are built-in effects that predate
the Snowflake scheme. Two outer directories are symbolic rather than numeric:
`model` and `script_segment_js`; see §7.5 and §6.4.

The **inner directory name is the MD5 of the downloaded ZIP**, and the sibling
`<md5>_tmp` file *is* that ZIP, kept in place after extraction. `_tmp` is a
misleading name: 208 of the 209 packages still had theirs. Confirming this
costs nothing — every `_tmp` file starts with `50 4b 03 04`. The MD5 also
appears as the `md5` column in the resource catalogue (§1.2), so the inner
directory doubles as a version key: when ByteDance republishes an effect under
the same resource ID, a second MD5 directory appears alongside the first rather
than replacing it. Three resource IDs in the sampled cache had two.

Two small details are worth knowing because they differ from the ZIP:

- The extractor **strips `__MACOSX/`**. Every ZIP that was authored on macOS
  still contains the resource forks, but the extracted tree has zero
  `__MACOSX` directories cache-wide. `.DS_Store` files, by contrast, survive.
- Some packages ship an `effect_platform_children.tag` file (12 cache-wide)
  that the extractor writes: a single line of comma-separated absolute-in-package
  paths listing every file the archive contained. It is a post-extraction
  inventory, useful as an integrity check, and it is the closest thing the cache
  has to a manifest.

### 1.1 How a project file reaches a cache directory

It does not need the catalogue at all. Every material in `draft_content.json`
that comes from the effect platform carries an absolute `path` pointing at the
extracted directory, next to the ID that names it:

```json
{
  "id": "7255573948300005889",
  "resource_id": "7255573948300005889",
  "third_resource_id": "7255573948300005889",
  "name": "Retro Typer",
  "type": "in",
  "material_type": "sticker",
  "duration": 1166666,
  "path": "C:/Users/user/AppData/Local/CapCut/User Data/Cache/effect/7255573948300005889/729d5e76cb22236d447b602d48d31b91"
}
```

So the mapping is `path = Cache/effect/<resource_id>/<md5>`, and
`id == resource_id == third_resource_id` for platform effects. Sibling caches
follow the same convention with a different root — stickers resolve to
`Cache/artistEffect/<resource_id>/<md5>`, music to `Cache/music/<md5>.mp3`.

Built-in adjustments are the interesting exception. The colour-adjustment
bundle (§2.4) is referenced by a `materials.hsl` entry with an **empty**
`resource_id` and only a `path`:

```json
{"id": "6A5F444B-…", "type": "hsl", "resource_id": "", "source_platform": 0,
 "path": ".../Cache/effect/7501974767453474064/20cd8db6531c21bf7e4053026d20e395"}
```

The engine treats "an effect the user picked from the catalogue" and "the
built-in HSL/curves/wheels panel" as the same kind of object; the only
difference is that the latter's package is downloaded silently on first use.

### 1.2 The resource catalogue (`ressdk_db`)

`Cache\ressdk_db\` holds the effect-platform catalogue as SQLite:

```
ressdk_db\
├── rp_master.db                        12 KB
├── 7618250755584643066\rp.db          134 MB   (+ -wal, -shm)
├── 14926150683140686826\rp.db          73 MB
├── 2900233024813344641\rp.db          3.9 MB
└── 17748792570547766607\rp.db         1.3 MB
```

`rp_master.db` has a single table that explains the numeric directory names:

```sql
CREATE TABLE ressdk_db_info(
  hash_path TEXT PRIMARY KEY, did TEXT, uid TEXT,
  region TEXT, language TEXT, last_access_time INTEGER);
```

so each `rp.db` is a per-(device, user, region, language) partition of the same
catalogue. The catalogue schema is large — the `effect` table alone has around
170 columns — but the ones that matter are `id`, `effect_id`, `resource_id`,
`md5`, `title`, `effect_type`, `panel_name`, `item_urls`, `sdk_extra`,
`requirements` and `model_names`. There is also a parallel `loki_effect` table
with a leaner shape (`effect_id`, `md5`, `file_uri`, `composer_params`,
`resource_id`, `_model_names`) that appears to serve the beauty/composer path.
Category membership lives in `category_effect(category_id, effect_id,
effect_index)` and `loki_category_effect`.

I could not read any catalogue *rows*. The three smaller partitions are
schema-only (zero rows in `effect`, `loki_effect` and `category_effect`, even
after replaying their WAL), and the two large partitions were not copied
because doing so meant moving 200 MB of ByteDance catalogue data for a
confirmation the `path` field in the project file already gives us. So: the
schema below is certain, the exact `effect_id` ↔ `resource_id` relationship in
practice is inferred from the project file only.

---

## 2. Package layouts

`config.json` at the package root is the only file that is always present
(209/209). Everything else is per-family. The `effect.Link[]` array in it is
what selects the runtime and names the subdirectory.

### 2.1 AmazingFeature — the general case

The dominant family (about 90% of the cache). The Link path is usually
`AmazingFeature/`, sometimes lowercase `amazingfeature/`, sometimes a custom
name.

```
<package>/
├── config.json                     manifest (§3.1)
├── extra.json                      user-tunable parameters (§3.2) — optional
├── algorithmConfig.json            ML/CV dataflow graph (§3.6) — optional
├── sticker.info                    encrypted blob, ignorable (§4.6)
├── effect_platform_children.tag    post-extraction file list
└── AmazingFeature/
    ├── content.json                per-feature metadata (§3.3)
    ├── sticker.config              required ECS systems (§3.4)
    ├── scene.config                same list, scene-side; binary or YAML
    ├── main.scene                  the ECS scene graph (§4.2)
    ├── lua/                        *.lua, plus lua/LumiFamily/ framework
    ├── js/                         *.js — newer alternative to Lua (§6.3)
    ├── xshader/                    *.vert *.frag *.xshader *.ausl
    ├── material/                   *.material
    ├── mesh/                       *.mesh (almost always a unit quad)
    ├── rt/                         *.rt render-target descriptors
    ├── image/                      *.png + *.png.meta
    ├── texture/                    *.texture — self-contained texture assets
    ├── resource/{images,seq,video}/  newer asset bucket
    ├── effects/<SubEffect>/        Lumi sub-effect bundles (§2.5)
    ├── prefabs/*.prefab            reusable scene fragments
    ├── lua-meta.json, js-meta.json  parameter manifests (§8.2)
    ├── LuaRTTI.MarkGen.lua         type registry for the editor
    ├── ImageBusinessSlider.json    slider → uniform binding
    └── graph.dat                   opaque AE-exporter artifact
```

Cache-wide counts give a sense of the weighting: 1,308 `.material`, 1,206
`.frag`, 1,142 `.vert`, 1,054 `.xshader`, 800 `.mesh`, 766 `.lua`, 601 `.rt`,
192 `main.scene`, 144 `.prefab`, 140 `.ausl`, 14 `.texture`, 14 `.js`.

### 2.2 Prefab-rooted packages

Newer and simpler packages skip `main.scene` and declare a root prefab instead,
via a `filemap` key in `content.json`:

```json
{"ae_tool": "AmazingEditor", "content": {},
 "filemap": {"prefab": "prefabs/heartMaskProcess.prefab"},
 "needblend": false, "tag": "AMG_1169…", "version": "1.0"}
```

Both forms coexist — some packages have `main.scene` *and* a `filemap` prefab,
some have only one. A loader must handle both: if `content.json.filemap.prefab`
is present, that prefab is the scene root; otherwise look for `main.scene`.

### 2.3 InfoSticker

The lightweight family — animated stickers, text animations, entrance/exit
animations. `config.json` is minimal (`{"effect":{"Link":[{"type":"InfoSticker"}]},
"version":"3.1.1"}`) and the payload sits at the package root rather than in a
subdirectory. In this cache the InfoSticker packages are *not* the PNG/MP4
sprite variants the 2026-05 survey found; they are full Amaz scenes with
`xshader/`, `filter.material`, `anim.prefab` and a `Transform.lua` or
`TextAnim.lua` controller. One InfoSticker package contains nothing but
`config.json` and `font.ttf`.

### 2.4 Multi-link colour pipelines (`AmazingFilter`)

The two largest structural packages in the cache (542 and 314 files) are the
built-in colour-adjustment chains. Their `config.json` declares fifteen-plus
links, each pointing at its own feature directory, each with its own
`sticker.config`, `main.scene` and shaders, ordered by `zorder`:

```json
{"effect": {"Link": [
  {"path": "AmazingFeature_blit/",      "type": "AmazingFeature", "zorder": 1000},
  {"path": "AmazingFilter_lut/",        "type": "AmazingFilter",  "zorder": 2000,
   "defaultEnable": true},
  {"path": "AmazingFeature_colorCorrection/", "type": "AmazingFeature",
   "zorder": 5000, "preRenderAlgorithmTex": true},
  {"path": "AmazingFeature_hsl/",       "type": "AmazingFeature", "zorder": 5016},
  {"path": "AmazingFeature_curves/",    "type": "AmazingFeature", "zorder": 5017},
  {"path": "AmazingFeature_primaryWheel/", …}, {"path": "AmazingFeature_logWheel/", …},
  {"path": "AmazingFeature_adjustColor/", …}, {"path": "AmazingFeature_sharpen/", …},
  {"path": "AmazingFeature_vignetting/", …}, {"path": "AmazingFeature_particle/", …},
  {"path": "AmazingFeature_blend/", …},  …]}}
```

Observed sub-feature names: `blit`, `blit_color`, `clear`, `blend`,
`adjustColor`, `colorCorrection`, `colorMigration`, `smartColorAdjustment`,
`hsl`, `curves`, `primaryWheel`, `logWheel`, `sharpen`, `desharpe`, `dehazing`,
`backlight`, `splendor`, `vignetting`, `particle`, plus `AF_DL` (a deep-learning
enhancement group) and `lumi_hub_path`. This is CapCut's entire colour panel,
shipped as one downloadable package — which is a useful architectural data
point on its own: the "adjust" sliders in the UI are not engine built-ins, they
are an effect package like any other.

`AmazingFilter_lut/` is the odd one out: its payload is a **plain Adobe
`.cube` 3D LUT** (`LUT_3D_SIZE 33`, 550 KB) plus a tiny per-feature
`config.json` and a `featureAlgorithmConfig/algorithmConfig.json`. No shaders,
no scene — the engine applies it natively.

### 2.5 Lumi sub-effects

`effects/<Name>/` holds self-contained mini-effects driven by the `LumiFamily`
Lua framework. 110 such directories cache-wide. The inventory is much larger
than the 2026-05 list:

`Lumi3DShape`, `Lumi3DShapeDeprecated`, `LumiAnimSeqLoadAndCrop`,
`LumiBokehBlur`, `LumiBoxBlur`, `LumiBulge`, `LumiChromaticAberration`,
`LumiColorAdjustBundle`, `LumiCornerPin`, `LumiDeepGlow`,
`LumiDirectionalBlurs`, `LumiDropShadow`, `LumiExposure`, `LumiFill`,
`LumiFlicker`, `LumiGaussianBlur`, `LumiGlitchSignal`, `LumiGrain`, `LumiHub`,
`LumiKineScope`, `LumiLayer`, `LumiLinearWipe`, `LumiMotionBlur2D`,
`LumiPageTurn`, `LumiRadialBlur`, `LumiSaturation`, `LumiSShake`,
`LumiStrongSharpen`, `LumiSurfaceBlur`, `LumiTurbulenceDisplacement`,
`LumiUsmSharpen`, `LumiVignette`, plus a lowercase mask family
(`lumi_circle_mask`, `lumi_custom_mask`, `lumi_heart_mask`,
`lumi_linear_mask`, `lumi_mirror_mask`, `lumi_rectangle_mask`,
`lumi_star_mask`) and the `lumi_hub_path` root.

A sub-effect directory has the same internal shape as a feature directory,
scoped down: `lua/`, `material/`, `mesh/`, `xshader/`, `rt/`, sometimes
`image/`, and a `xshader/shaderLib/` with per-backend generated source (§5.3).

### 2.6 The Lynx-Studio text-animation family (new)

Two packages in the cache belong to a format that has no `effect` block at all:

```json
{"encrypt": 0, "script_type": "js",
 "studio_animation_path": "/studioAnim.lsanim", "version": "19.6.0"}
```

```
<package>/
├── config.json
├── textAnim.lsproj          project file
├── studioAnim.lsanim        animation data
└── res/
    ├── video_…_output.mp4
    └── <EffectName>/
        ├── AEInfo.dat
        └── AmazingFeature/    ← a normal, complete AmazingFeature tree
```

`.lsproj` and `.lsanim` are normally encrypted — both start with the same
`9d d7 8a` prefix and are high-entropy. But one of the two packages sets
`"encrypt": 0` in `config.json`, and its `studioAnim.lsanim` is then **plain
JSON**, which gives away the whole schema:

```json
{"studio_anim_params": {"effectAnimators": [{"effects": [{
  "caption": "线性擦除", "name": "LinearWipe", "isRelativePath": true,
  "path": "/res/LinearWipe", "prefabName": "Lumi…",
  "params": {
    "feather":  {"value": 0.02},
    "rotation": {"value": 270},
    "progress": {"value": 0.3,
      "motionKeyFrameInfo": [
        {"t": 0,   "v": 0.74, "it": "cubic", "vi": 0, "vo": 0.585, "vti": 0, "vto": 0.693},
        {"t": 2.1, "v": 0.27, "it": "cubic", "vi": 0.43, "vo": 0.178, "vti": -0.714, "vto": 0.297},
        {"t": 3,   "v": 0,    "it": "cubic", "vi": 0.095, "vo": 0, "vti": -0.306, "vto": 0}]}}}]}]}}
```

That is a keyframe track: `t` time in seconds, `v` value, `it` interpolation
kind, and `vi`/`vo`/`vti`/`vto` the incoming and outgoing Bézier tangent
components. Each `effects[]` entry names a directory under `res/` and a prefab
inside it. So this family is a thin animation-driver layer wrapping ordinary
AmazingFeature effects — exactly the composition model chukcut's own effect
layer would want.

The `encrypt` flag is a plain integer in the manifest, and the engine
presumably honours `0` by skipping decryption. Whether it can be flipped on an
already-encrypted package is not something this survey can answer, and it was
not tested.

### 2.7 Degenerate packages

Five packages contain only `config.json` + `algorithmConfig.json` and declare
`"Link": []`. They exist purely to pull ML models:

```json
{"effect": {"Link": [], "model_names": {"alg_model":
   ["lock_obj_det", "tt_body_detection_lockon"]}},
 "bALG_BACH_CONFIG": true, "version": "14.1.0"}
```

One package (`"type": "matting"`) contains five empty directories
(`ai_matting/`, `ai_matting_gru/`, `custom_matting/`, `interactive/`,
`saliency_matting/`) and nothing else — the directories are name-only slots
that select which matting model the engine loads.

---

## 3. The JSON manifests

### 3.1 `config.json`

Always present, always valid JSON, UTF-8. Indentation and colon spacing vary
between the two authoring tools (`AmazingEditor` uses tabs and `"key" : value`;
`AEExporter` uses four spaces and `"key": value`) — parse it, never compare it
textually.

```json
{
  "ae_tool": "AEExporter:1.4.27",
  "bALG_BACH_CONFIG": false,
  "effect": {
    "Link": [{"path": "AmazingFeature/", "type": "AmazingFeature", "zorder": 8029}]
  },
  "name": "AE2Effect_20251021005254856",
  "version": "14.8.0"
}
```

| Key | Type | Meaning |
|---|---|---|
| `effect` | object | Required. Wraps the runtime descriptor. |
| `effect.Link` | array | Required. Ordered list of features to apply. May be empty (§2.7). |
| `effect.requirement` | object | Capability gates. Mostly booleans, but see below. |
| `effect.model_names` | object of arrays | ML models to fetch, bucketed by consumer. |
| `effect.exclusiveScene` | array | Mutual-exclusion rules; **no longer always empty**. |
| `effect.forceRender` | bool | Render even when the engine would skip the frame. |
| `effect.forceUseAlgCache` | bool | Reuse cached algorithm output instead of recomputing. |
| `name` | string | Internal name. Free-form. |
| `version` | string | Minimum engine version, *not* a format version. |
| `ae_tool` | string | `"AmazingEditor"` or `"AEExporter:<x.y.z>"`. |
| `bALG_BACH_CONFIG` | bool | An algorithm-graph config is bundled. |
| `encrypt` | int | Lynx-Studio family only (§2.6). |
| `script_type` | string | Lynx-Studio family only; observed `"js"`. |
| `studio_animation_path` | string | Lynx-Studio family only. |

`effect.requirement` is not purely boolean any more. One package declares a
structured requirement:

```json
"requirement": {"faceDetect": true, "blit": {"width": 720, "height": 1280}}
```

`effect.exclusiveScene` now carries real content, declaring which other effect
families this one must not coexist with:

```json
"exclusiveScene": [
  {"priority": 9999, "sceneKey": "FaceMakeup*",       "tagName": []},
  {"priority": 9999, "sceneKey": "Filter*",           "tagName": []},
  {"priority": 9999, "sceneKey": "FaceDeformation*",  "tagName": []},
  {"priority": 9999, "sceneKey": "FaceBeauty*",       "tagName": []}]
```

`effect.model_names` buckets observed: `alg_model`, `matting`, `script`,
`bytenn`, `skinseg`, and one literal `xxx` (someone's placeholder that shipped).

#### `Link[]` entry

| Key | Type | Meaning |
|---|---|---|
| `type` | string | `AmazingFeature`, `InfoSticker`, `AmazingFilter`, `TouchGes`, `matting`. |
| `path` | string | Directory inside the package, trailing slash. May be `""`. |
| `zorder` | number | Ordering key. Integer or float (`8011.0` observed). Ranges 1000–10000. |
| `extra.composer_param` | array | Slider declarations for beauty/makeup effects (§8.3). |
| `defaultEnable` | bool | The link is on unless the user turns it off. |
| `preRenderAlgorithmTex` | bool | Run the algorithm graph before this pass. |
| `rtShare` | bool | Share the render target with the previous link. |
| `needBlend` | bool | This link needs its own blend step. |

### 3.2 `extra.json`

Optional (31 cache-wide). Three distinct shapes now coexist under `setting`:

```json
{"setting": {"effect_adjust_params": [
  {"effect_key": "effects_adjust_intensity", "default": 0.9, "max": 1.0, "min": 0.0},
  {"effect_key": "effects_colormigration_target_path", "default": 0.85, "max": 1.0, "min": 0.0}]}}
```

```json
{"setting": {"animation_duration": 1.3}}
```

```json
{"setting": {"bloom_adjust_params": [
  {"effect_key": "bloom_adjust_strength", "default": 0.50, "min": 0.01, "max": 1.0},
  {"effect_key": "bloom_adjust_range",    "default": 0.4,  "min": 0.01, "max": 1.0}]}}
```

and one package uses a bare scalar at top level: `{"manual_beauty_face": 3}`.

`animation_duration` (seconds, float) is the most common form in this cache and
is not a slider at all — it tells the host how long an entrance/exit animation
runs, which the timeline needs before the effect is instantiated.

### 3.3 `content.json`

Per-feature metadata, inside the Link directory.

```json
{"ae_tool": "AmazingEditor", "content": {}, "needblend": false,
 "requirement": {"faceDetect": true},
 "filemap": {"prefab": "anim.prefab"},
 "tag": "AMG_1803366472594868808713239347891717198482", "version": "1.0"}
```

`content` is always `{}`. `tag` is a stable `AMG_`-prefixed 39–40 digit ID.
`needblend` was `false` everywhere it appeared. `filemap` is the new key
described in §2.2. `requirement` here is the per-feature version of the
top-level one and, unlike in 2026-05, is now usually **omitted entirely or
reduced to the flags that are actually true** rather than spelling out thirty
`false` entries — though the exhaustive form still appears in older packages.

### 3.4 `sticker.config`

Declares which Amaz ECS systems the feature needs. `"type": "amazing"` and
`"version": "0.1"` in all 183 instances.

```json
{"dev_version": "17.8.0", "min_version": "16.4.0", "editor_info": {},
 "systemList": ["BehaviorSystem", "Brush2DRendererSystem", "CameraSystem",
   "EventSystem", "MeshRendererSystem", "ScriptSystem", "TransformSystem"],
 "type": "amazing", "version": "0.1"}
```

Systems observed across the sample: `AnimSeqSystem`, `BehaviorSystem`,
`Brush2DRendererSystem`, `BuiltinObjectSystem`, `CameraSystem`, `EventSystem`,
`FaceMakeupV2System`, `FaceReshapeSystem`, `LightSystem`, `MeshRendererSystem`,
`MorpherSystem`, `NsSystem`, `RenderSystem`, `ScriptSystem`,
`SDFTextSystem`, `ShadowMapSystem`, `Sprite2DRendererSystem`, `TextSystem`,
`TransformSystem`, `TweenSystem`, and a lowercase `v6_*` group
(`v6_camera`, `v6_directionalLight`, `v6_meshrender`, `v6_morpher`) used by the
3D face path.

Some packages instead (or additionally) carry `app_params`:

```json
"app_params": {"innerfilter": true, "facetips": 1,
               "detectFlags": "face_240_detect", "faceModelName": "perfect2"}
```

### 3.5 `scene.config`

The same system list from the scene side. Comes in three encodings depending on
the exporter: `%SerializedFormat%@` binary (12), JSON (55), and YAML (3). The
YAML form is legible and confirms the binary's contents exactly:

```yaml
%YAML 1.1
--- !SystemList &1
name: ""
guid: {a: 8088712477189810352, b: 17230777776182555827}
systemList:
  - EventSystem
  - TransformSystem
  - BehaviorSystem
  - CameraSystem
  …
```

### 3.6 `algorithmConfig.json`

The ML/CV dataflow graph, present in 183 packages — far more common than the
earlier survey suggested. Same shape as before: `version`, `mode` (always `2`),
optional `name`, `nodes[]` and `links[]`.

```json
{"version": "1.0", "mode": 2, "name": "colormigration_…",
 "nodes": [
   {"name": "blit_0",        "type": "blit",         "config": {"keyMaps": {…}}},
   {"name": "general_lens_0","type": "general_lens", "config": {"keyMaps": {…}}}],
 "links": [{"fromNode": "blit_0", "fromIndex": 0, "toNode": "general_lens_0", "toIndex": 0}]}
```

`keyMaps` has `intParam` / `floatParam` / `stringParam` / `pathParam` buckets,
empty in everything sampled. Node types observed: `blit`, `texture_blit`,
`ext_texture_producer`, `general_lens`, `script`, `face`, `face_fitting`,
`freid`, `nh_face_align`, `skin_seg`, `skeleton`, `object_detection2`,
`interactive_matting`. Feature directories can carry their own
`featureAlgorithmConfig/algorithmConfig.json` scoped to that link.

### 3.7 `ImageBusinessSlider.json`

Present 35 times. Every instance sampled was the degenerate
`{"ImageBusinessSlider": null}`. The richer form documented in the 2026-05
spec — a `LV` map binding slider names to entity GUIDs and uniform ranges — was
not found in this cache. Either it moved, or those effects are simply not
installed here.

---

## 4. `%SerializedFormat%@` and its readable twin

This is the most useful thing in the whole survey.

### 4.1 The two encodings

All of the structural assets — `.scene`, `.prefab`, `.material`, `.xshader`,
`.mesh`, `.rt`, `.seq`, `.texture`, `.png.meta`, sometimes `scene.config` — use
one serialiser with two interchangeable encodings. The binary one starts with
the literal ASCII `%SerializedFormat%@\n`, followed by a 4-byte little-endian
format version (always `2`, occasionally `1` in old files) and a 4-byte object
count. The text one is **YAML 1.1 with typed document tags**, and it is not a
debug artifact — the engine ships and loads both.

Distribution in the 30-package sample:

| Extension | Binary | YAML | JSON |
|---|---:|---:|---:|
| `.material` | 259 | 4 | — |
| `.xshader` | 209 | 4 | — |
| `.rt` | 186 | 24 | — |
| `.mesh` | 103 | 1 | — |
| `.scene` | 56 | 2 | — |
| `.prefab` | 15 | 3 | — |
| `.png.meta` | 36 | 2 | — |
| `.texture` | 7 | — | — |
| `scene.config` | 12 | 3 | 55 |

Because the two encode the same object model, the YAML files document the
binary format's semantics for free. A renderer that wants to consume real
packages still has to decode the binary — but it now knows exactly what fields
it is looking for, which is the difference between guessing and parsing.

Every object carries a `guid: {a, b}` pair of unsigned 64-bit integers; this is
the identity used for cross-file references. Asset references are
`{localId: N, path: "relative/path"}` — `localId` indexes an object inside the
same document, `path` names another file in the package.

### 4.2 `main.scene` / `.prefab` — the ECS scene graph

```yaml
%YAML 1.1
--- !Scene &1
name: Sticker_empty
guid: {a: 7728202298759107630, b: 4591771596645522361}
calibrateVer: V4
entities:
  - __class: Entity
    name: Camera_entity
    guid: {…}
    scene: {localId: 1}
    selfvisible: true
    tag: 0
    components:
      - {localId: 2}                       # the Transform, defined below
      - __class: Camera
        name: Camera_camera
        enabled: true
        layerVisibleMask: {__class: DynamicBitset, numBits: 1, values: [1]}
        renderOrder: 1
        type:       {__class: CameraType,       value: ORTHO}
        clearColor: {r: 0, g: 0, b: 0, a: 0}
        clearType:  {__class: CameraClearType,  value: COLOR}
        alwaysClear: true
        viewport: {x: 0, y: 0, w: 1, h: 1}
        fovy: 60
        orthoScale: 1
        zNear: 0.1
        zFar: 1000
        renderTexture: {localId: 1, path: rt/outputTex.rt}
        isRootCamera: true
    layer: 0
  - __class: Entity
    name: SeekModeScript
    components:
      - {localId: 3}
      - __class: MeshRenderer
        sharedMaterials:
          - {localId: 1, path: material/entity.material}
        sortingOrder: 0
        autoSortingOrder: true
        useFrustumCulling: true
        mesh: {localId: 1, path: mesh/quad.mesh}
        castShadow: true
      - __class: ScriptComponent
        enabled: true
        path: lua/SeekModeScript.lua
        properties: {__class: Map, curTime: 0}
        className: Script
    layer: 0
visible: true
config: {__class: Map}
msaa: {__class: MSAAMode, value: NONE}
--- !Transform &2
localPosition: {x: 0, y: 0, z: 10}
localScale:    {x: 1, y: 1, z: 1}
localOrientation: {w: 1, x: 0, y: 0, z: 0}
--- !Transform &3
…
```

Everything the 2026-05 render-graph spec inferred from string extraction is
confirmed and made precise: cameras carry an explicit integer `renderOrder`, an
explicit `layerVisibleMask` bitset, an explicit `renderTexture` asset
reference; renderer entities carry `sharedMaterials[]` (a list, not a single
material) and a `mesh`; script components carry a `path`, a `className`, and a
`properties` map holding the *scene-side* default values of the script's
declared parameters. Transforms live as separate documents referenced by
`localId`, which is why the binary form interleaves them.

`.prefab` uses the same object model under a `!Prefab` root, with `entities[]`
and a `standaloneResources` map. A prefab-rooted package (§2.2) can be as small
as four strings:

```yaml
--- !Prefab &1
entities:
  - __class: Entity
    name: LumiRoot
    components:
      - __class: ScriptComponent
        path: lua/LumiFamily/LumiHub.lua
        className: LumiHub
```

### 4.3 `.material`

```yaml
--- !Material &1
name: noFilter_material
guid: {a: …, b: …}
xshader: {localId: 1, path: xshader/noface.xshader}
properties:
  __class: PropertySheet
  name: noFilter_propertySheet
  floatmap: {__class: Map, blurRadius: 25.0, u_BlurScale: 4., u_Intensity: -22, …}
  vec2map:  {__class: Map, u_Center: {x: 0.5, y: 0.5}, …}
  vec3map:  {__class: Map, u_BgColor: {x: 1, y: 1, z: 1}, …}
  vec4map:  {__class: Map}
  mat4map:  {__class: Map}
  intmap:   {__class: Map}
  texmap:
    __class: Map
    u_BloomTex1:    {localId: 1, path: rt/midRT4.rt}
    screen_Texture: {localId: 1, path: rt/midRT1.rt}
    u_X1InputTex:   {localId: 1, path: rt/gaussianBlurMidRT.rt}
renderQueue: 3090
enabledMacros:
  __class: Map
  AE_AreaLightNum:  !<str> 0
  AE_DirLightNum:   !<str> 0
  AE_PointLightNum: !<str> 0
  AE_SpotLightNum:  !<str> 0
  SAMPLETIIMES1:    !<str> 40
mshaderPath: ""
instanceCount: 0
```

So a material is: one xshader reference, seven typed uniform maps, a texture
binding map, an integer render queue, and a preprocessor-macro map. The
`AE_*LightNum` macros appear in every material (an `AE_AreaLightNum` slot has
been added since the 2026-05 notes, which listed only Spot/Point/Dir).

Texture bindings resolve to one of three things: a `.rt` path (another
camera's framebuffer), a `.texture` / `.png` path, or the special URI
`share://input.texture` — the source video frame. A second share URI now
exists, `share://skinsegmask.texture`, appearing once.

### 4.4 `.xshader`

The shader-program manifest, and the piece the earlier spec called "safe to
ignore". It is not — it holds the render state, the vertex semantics, and the
per-pass render target.

```yaml
--- !XShader &1
name: noface/xshader
renderQueue: 3090
passes:
  - __class: Pass
    name: GaussianBlurX1
    shaders:
      __class: Map
      gles2: [{localId: 4}, {localId: 5}]     # vertex, fragment
    angleBinaryPrograms: {__class: Map}
    semantics:
      __class: Map
      position:     {__class: VertexAttribType, value: POSITION}
      texcoord0:    {__class: VertexAttribType, value: USER_DEFINE1}
      a_bloomPara:  {__class: VertexAttribType, value: COLOR1}
    renderTexture: {localId: 1, path: rt/gaussianBlurMidRT.rt}
    clearColor: {r: 0, g: 0, b: 0, a: 0}
    clearDepth: 1
    clearType: {__class: CameraClearType, value: COLOR}
    renderState:
      __class: RenderState
      depthstencil:
        depthTestEnable: false
        depthCompareOp: {__class: CompareOp, value: LESS}
        depthWriteEnable: false
        stencilTestEnable: false
      colorBlend:
        blendConstants: {x: 0, y: 0, z: 0, w: 0}
        attachments:
          - blendEnable: false
            srcColorBlendFactor: {__class: BlendFactor, value: ONE}
            dstColorBlendFactor: {__class: BlendFactor, value: ONE_MINUS_SRC_ALPHA}
            srcAlphaBlendFactor: {__class: BlendFactor, value: ONE}
            dstAlphaBlendFactor: {__class: BlendFactor, value: ONE_MINUS_SRC_ALPHA}
            colorWriteMask: 15
            ColorBlendOp: {__class: BlendOp, value: ADD}
            AlphaBlendOp: {__class: BlendOp, value: ADD}
    useFBOTexture: false
    useCameraRT: false
    useFBOFetch: false
    isFullScreenShading: false
    macrosMap: {__class: Map}
    preprocess: false
    passType:  {__class: PassType,  value: NORMAL}
    lightMode: {__class: LightMode, value: NONE}
--- !Shader &4
type: {__class: ShaderType, value: VERTEX}
sourcePath: xshader/GaussianBlur.vert
--- !Shader &5
type: {__class: ShaderType, value: FRAGMENT}
sourcePath: xshader/GaussianBlurX1.frag
macros: [SAMPLETIIMES1]
```

Several things fall out of this. A single `.xshader` can declare **multiple
passes**, each with its own render target and render state — so multi-pass
effects are not always expressed as multiple camera entities. `angleBinaryPrograms`
is an empty slot for cached ANGLE binaries, which fits the ANGLE dependency
noted in `capcut-stack.md`. Vertex `semantics` map attribute *names* to typed
slots, including generic `USER_DEFINE1` / `COLOR1` channels, so a loader can
bind by semantic rather than guessing from the attribute name. And `!Shader`
objects can carry a `macros` list, which is how the same `.frag` gets compiled
into several variants.

Blend factors and compare ops use Vulkan-style enum names.

### 4.5 `.rt`, `.mesh`, `.png.meta`

```yaml
--- !ScreenRenderTexture &1
name: RTFilterOut
width: 720
height: 1280
depth: 1
internalFormat: {__class: InternalFormat, value: RGBA8}
dataType:       {__class: DataType,       value: U8norm}
builtinType:    {__class: BuiltInTextureType, value: NORAML}   # sic
enableMipmap: false
filterMin: {__class: FilterMode, value: LINEAR}
filterMag: {__class: FilterMode, value: LINEAR}
filterMipmap: {__class: FilterMipmapMode, value: NONE}
wrapModeS/T/R: {__class: WrapMode, value: CLAMP}
maxAnisotropy: 1
attachment: {__class: RenderTextureAttachment, value: NONE}
massMode:   {__class: MSAAMode, value: NONE}
shared: true
colorFormat: {__class: PixelFormat, value: RGBA8Unorm}
pecentX: 1     # sic
pecentY: 1
```

`pecentX`/`pecentY` are the resolution scale factors relative to the input
frame; `shared: true` marks a render target that can be aliased with another
pass's. The 2026-05 guess of "RGBA8, linear, clamp, one sample" is confirmed as
the universal default.

```yaml
--- !Mesh &1
name: quad
boundingBox: {min: {…}, max: {…}}
vertices: [-1,-1,0, 0,0,  1,-1,0, 1,0,  1,1,0, 1,1,  -1,1,0, 0,1]
vertexAttribs:
  - semantic: {__class: VertexAttribType, value: POSITION},  bindingIndex: 0
  - semantic: {__class: VertexAttribType, value: TEXCOORD0}, bindingIndex: 0
submeshes:
  - indices16: [0, 1, 2, 2, 3, 0]
    indicesCount: 6
    primitive: {__class: Primitive, value: TRIANGLES}
clearAfterUpload: true
```

Interleaved position(vec3) + uv(vec2), four vertices, six indices. Every mesh
in the sample except one face mesh was this quad.

```yaml
--- !PngMeta &1
innerAlphaPremul: true
outerAlphaPremul: false
enableMipmap: false
filterMin/filterMag: LINEAR
filterMipmap: NONE
wrapModeS/T/R: CLAMP
maxAnisotropy: 1
isColorTexture: false
maxTextureSize: {__class: TextureResizeLevel, value: OFF}
needFlipY: false
```

`innerAlphaPremul` / `outerAlphaPremul` are the useful ones: they say whether
the PNG's alpha is premultiplied, which a compositor must know.

### 4.6 Opaque blobs

| File | Count | State |
|---|---:|---|
| `sticker.info` | 10 | High-entropy (7.83–7.89 bits/byte), no magic, no readable strings. Encrypted server-issued metadata. Not needed for rendering. |
| `graph.dat` | 8 | Magic `37 82 20 76`, entropy ~7.9. The AE-exporter's source project. `main.scene` already carries everything needed. |
| `*.ausl` | 140 | Magic `ASLE\x00\x00\x00\x00`, then a 4-byte LE size. Compiled shader; the generated GLSL/Metal sits alongside. |
| `*.lsproj`, `*.lsanim` | 2 + 2 | Prefix `9d d7 8a`, encrypted unless `config.json` says `"encrypt": 0`. |
| `AEInfo.dat` | 3 | Small (346–672 bytes), high-entropy, inside `res/<Effect>/` of Lynx-Studio packages. Purpose unknown. |
| `*.bin` / `*.raw` | 2 | `lens_vhdr_image_lut_rgb.{bin,raw}`, 20,577 bytes each, inside `featureAlgorithmConfig/`. Almost certainly a raw LUT for the HDR lens algorithm; not decoded. |

---

## 5. Shaders

### 5.1 Dialect

Unchanged from the 2026-05 analysis, and that is worth stating plainly: this is
still **GLSL ES 1.0**. Of 228 fragment shaders in the sample, 224 have no
`#version` directive at all and open with `precision highp float;`; four use
`#version 300 es`. 222 use `texture2D(...)`, 152 write `gl_FragColor`, 72 write
`gl_FragData[…]`, and only the four ES 3.0 files declare `out vec4`. There are
zero `#extension` directives and zero `#include` directives across the whole
sample — every shader is self-contained.

So the GLSL-ES-1.0 → desktop-GL-330 rewrite recipe from `SHADER_SPEC.md` still
applies verbatim: strip precision qualifiers, `attribute`/`varying` →
`in`/`out`, `texture2D` → `texture`, synthesise the fragment output
declaration, prepend `#version`.

### 5.2 How shaders are referenced

Three layers, in this order:

1. A `MeshRenderer` component names a `.material` file.
2. The `.material` names an `.xshader` file and supplies uniform values and
   texture bindings.
3. The `.xshader` declares one or more passes; each pass's `shaders` map holds
   per-backend `!Shader` objects whose `sourcePath` points at the `.vert` /
   `.frag` file.

A pass can also declare its own `renderTexture`, `renderState` and `macros`, so
the shader files alone never fully describe a pass.

### 5.3 Backends

The `shaders` map inside `.xshader` is keyed by backend tag. Scanning all
`.xshader` files in the sample for those tags gives:

| Tag | Occurrences |
|---|---:|
| `gles2` | 210 |
| `glsl20` | 73 |
| `glsl30` | 73 |
| `glsl31` | 73 |
| `glsl32` | 73 |
| `metal` | 73 |

The 73-count group belongs to the AUSL-based packages (§5.4): those declare six
backend profiles at once. Older hand-authored shaders declare only `gles2`.

On disk, generated sources live under `xshader/shaderLib/<backend>/<md5>.{vert,frag}`:

| Directory | Count cache-wide | Contents |
|---|---:|---|
| `shaderGLES` | 69 | GLSL ES source — what we consume |
| `shaderMetal` | 69 | Metal source |
| `shaderHLSL5` | 20 | **empty in every case** |
| `shaderVulkan` | 20 | **empty in every case** |

The HLSL5 and Vulkan directories exist as directory entries in the ZIPs but
contain no files anywhere in this cache. Two readings are possible — the
toolchain emits the slots unconditionally, or these backends are staged for a
future release — and this survey cannot distinguish them. What it can say is
that no shipped package currently provides HLSL or SPIR-V.

### 5.4 AUSL — the actual shader source

140 `.ausl` files, each with a sibling `.ausl.compile_hash` (a 31–32 hex-char
text file). The `.xshader` names the `.ausl` as its authoritative source and
lists the generated per-backend files as outputs. The generated GLSL is
recognisably machine-produced:

```glsl
precision highp float;
precision highp int;

uniform float u_frqW;
uniform mediump sampler2D u_inputTexture;
uniform float u_time;
varying vec2 v_uv;

vec3 _f0(vec4 _p0)
{
    if (_p0.w <= 0.001000000047497451305389404296875)
    …
```

Mangled function and parameter names (`_f0`, `_p0`) and full float
round-tripping are the give-aways. So Amaz Shader Language is the source of
truth for modern effects and GLSL/Metal are compiled artifacts. The `.ausl`
bytecode itself was not decoded, and does not need to be while the GLSL is
shipped alongside.

### 5.5 Uniform vocabulary

Top fragment-shader uniforms in the sample, largely matching the earlier survey:
`u_ScreenParams` (63), `u_inputTexture` (42), `_MainTex` (24), `u_gamma` (23),
`inputImageTexture` (22), `u_radius` (17), `inputTex` (14), `intensity` (13),
`u_inputTex` (12), `u_intensity` (10), `u_albedo` (10), `u_borderType` (9),
`u_blendMode` (9), `u_Angle` (9), `inputWidth`/`inputHeight` (9),
`u_rotation` (8), `u_InvModel` (8), `u_FBOTexture` (8), `u_BlurScale` (8),
`u_AngleX`/`u_AngleY` (8), `blurRadius` (8), `u_mirrorEdge` (7), `u_bgTex` (7),
`u_videoTex` (7), `_alpha` (7). The role-based sampler binding table from
`SHADER_SPEC.md` §10.3 is still the right approach.

---

## 6. Scripting

### 6.1 Lua inventory

766 `.lua` files cache-wide, 237 in the sample. The recurring names:

| Script | Count in sample | Role |
|---|---:|---|
| `LuaRTTI.MarkGen.lua` | 53 | Type registry — declares `__typename`/`__supername`, no behaviour |
| `SeekModeScript.lua` | 28 | The canonical per-effect controller |
| `ImageBusinessSlider.lua` | 20 | Slider plumbing |
| `LumiObjectExtension.lua` | 14 | Lumi framework: texture re-pointing |
| `LumiManager.lua` | 9 | Lumi framework: pass ordering and ping-pong |
| `LumiParamsSetter.lua` | 9 | Lumi framework: parameter fan-out |
| `AETools.lua` | 9 | After-Effects easing/keyframe helpers |
| `LumiEffect.lua` / `LumiExportData.lua` | 7 each | Lumi base classes |
| `LumiManagerLite.lua` | 3 | **New** — reduced Lumi manager |
| `videoAnimationBaseScript.lua` | 5 | **New** — video-animation base controller |
| `LumiHub.lua` | 2 | **New** — root of the `lumi_hub_path` feature |
| `BachAlgorithm.lua`, `graphBuild.lua` | 2 each | Algorithm-graph glue |

`LumiFamily/` now contains `AETools.lua`, `LumiEffect.lua`, `LumiExportData.lua`,
`LumiManager.lua`, `LumiObjectExtension.lua`, `LumiParamsSetter.lua`, plus
`LumiHub.lua` and `LumiUtils.lua` in the hub variant.

### 6.2 Binding and lifecycle

Unchanged. A script is attached by a `ScriptComponent` in the scene or prefab,
which carries `path` (the `.lua` file, package-relative), `className` (the
global table name the file exports), and a `properties` map of scene-side
defaults. Module composition still uses the host-provided
`includeRelativePath(name)`; there are zero `require(` calls in the sample.

Hook definition counts in the sample: `onStart` 123, `onUpdate` 115,
`onEvent` 82, `constructor` 53, `onDestroy` 22, `seekToTime` 18. Component
types passed to `getComponent`: `MeshRenderer` 145, `Camera` 128,
`Transform` 108, `ScriptComponent` 70, `Renderer` 29, and a long tail of new
ones — `Text`, `SDFText`, `Sprite2DRenderer`, `Layer2DRenderer`,
`TableComponent`, `VideoAnimSeq`, `AnimSeqComponent`, `MorpherComponent`,
`NsComponent`, `FaceReshapeLiquefy`, `EffectFaceMakeup`,
`EffectFaceMakeupFaceU`, and a `Brush2D*` group (`Brush2DComponent`,
`Brush2DCanvas`, `Brush2DMeshGenerator`, `Brush2DInputProcessor`).

The `Amaz.*` API surface is broadly what `LUA_API_SPEC.md` described, with
additions listed in §9.6.

### 6.3 JavaScript inside AmazingFeature

New and significant: JS is now a peer of Lua inside AmazingFeature packages,
not just the text-template runtime. `Feature.js` sits at the package root of
the two colour-adjustment bundles and drives them:

```js
const Amaz = effect.Amaz;

class Feature
{
    onInit()
    {
        this.scenesMap = {};
        this.colorMigrationMaterial = null;
        this.lutTexture = null;
        this.m_maskPreviewColor = new Amaz.Vector4f(0.0, 0.0, 0.0, 1.0);
        this.m_maskBlendMode = 0;
        this.enableAdjustmentOptimizeSmartColor = false;
        if (Amaz.hasOwnProperty("SwingTemplateUtils")) { … }
```

The API surface mirrors the Lua one (`Amaz.Vector4f`, etc.), reached through an
`effect` global rather than a bare `Amaz`. Feature directories can also carry a
`js/` folder (`MainSystem.js`, `bach.js`, plus effect-specific files), and
`js-meta.json` describes those classes exactly as `lua-meta.json` describes Lua
ones:

```json
[{"ClassName": "MainSystem", "Super": "BaseSystem",
  "FilePath": "alg/MainSystem.js",
  "FileAbsPath": "/Volumes/D/EffectCapacity/…/AmazingFeature/alg/MainSystem.js",
  "Properties": [], "Comment": "", "ExportFiles": []}]
```

91 `js-meta.json` files exist cache-wide against 109 `lua-meta.json`, so the JS
path is not a curiosity.

### 6.4 The shared JS runtime (`script_segment_js`)

The cache entry named `script_segment_js` is not an effect. It is the
JavaScript engine bundle, downloaded once and shared, with two MD5 versions
present. Its `current_revision` file is unusually candid:

```
revision:89c23bb4075054a5f36b99fd8431e85784e714bb
version:js-20.2.0-202511071321-834-89c23bb4
pub date:2025-11-07 13:21:55
arch:x86_64
region:cn
repo name:ies/effect/segmentJS
```

and `version.json` carries the git branch (`release/20.2.0`), commit ID and
merge-commit subject. The bundle itself is `main.js` (a minified module system
with an `EventHandler` class at its head) plus `template/template.js`. So the
`ScriptInfoSticker` text-template runtime documented in 2026-05 as living
*inside* each text-effect ZIP has been factored out into one shared, versioned
package.

---

## 7. Resources

### 7.1 Static textures

Two mechanisms coexist.

The old one is `image/<name>.png` with a sibling `<name>.png.meta` holding the
import settings (§4.5). 94 PNGs and 35 `.png.meta` files in the sample, so
roughly a third of PNGs carry explicit metadata and the rest take defaults.

The new one is a **`.texture` file**: a `%SerializedFormat%@` container that
embeds the PNG bytes *and* the import settings in one asset. A hex dump shows
the PNG's `IHDR`/`IDAT`/`IEND` chunks inline, and the container also records the
original filename (`texture/fade_max.png`). 14 exist cache-wide, in
`texture/` directories inside the newer colour-adjustment features. Materials
reference them by path exactly like PNGs:

```
texture_trans  → texture/transmission.texture
texture_source → share://input.texture
texture_skin   → texture/skinMask.texture
texture_color  → texture/color.texture
```

### 7.2 Image sequences

`resource/seq/<name>/` holding `<name>_NNN.png` frames plus a `<name>.seq`
index (binary `%SerializedFormat%@`). Only two `.seq` files exist cache-wide,
so PNG-sequence effects are rare in this installation — the largest sampled
package had a 21 KB `.seq` indexing a `bg11` sequence.

### 7.3 Video

`resource/video/*.mp4`, six files cache-wide, H.264 in an ISO Media container.
Used as texture sources by materials.

### 7.4 Fonts

One `font.ttf` in the whole cache, in a package that contains nothing but that
file and a two-line `config.json`. Fonts are otherwise delivered through a
separate cache (`Cache\fontImage\`) and referenced by resource ID.

### 7.5 ML models

`Cache\effect\model\` is a flat directory of `.model` blobs named
`<name>_v<version>_size<N>_md5<md5>.model`, e.g.
`cc_yunfeng_v1.0_size0_md5b9e28561f63c8549c3a724434bfa025d.model` (4.7 MB) or
`aed40_10m_v1.0_size0_md52adca481f477a315f27b684f6a7140a6.model` (9.8 MB). The
`model_names` arrays in `config.json` reference the bare `<name>` — the
resolver adds the version and hash. Sizes in this cache range from 1 KB to
about 10 MB; the much larger SmartCrop models live in their own cache
directory.

### 7.6 3D LUTs

`.cube` files in Adobe's standard text format, shipped by the `AmazingFilter`
link type. Four cache-wide, all 550 KB, `LUT_3D_SIZE 33`, no header comments —
just the size line and 35,937 RGB triples. These are directly usable by any
renderer; no reverse engineering required.

---

## 8. Parameter declaration

There are four distinct mechanisms, layered.

### 8.1 Source annotations in Lua

The `---@field` form dominates: 630 occurrences in the sample against 18 for
the older `--@input` form. The grammar is LuaLS-style with an Amaz-specific
`[UI(...)]` attribute block:

```lua
---@class makeup : ScriptComponent [UI(Display="makeup")]
---@field ampW      double  [UI(Display="Wave Amp", Range={0, 100}, Drag)]
---@field cycle     int     [UI(Range={1, 100}, Slider)]
---@field color     Color   [UI(NoAlpha)]
---@field bgTexture Texture [UI(Type="Texture2D")]
---@field blendMode string  [UI(Option={"Add", "Multiply", "Difference", …})]
---@field blitMesh  Mesh    [UI(Order=0, Display="blit Mesh")]
```

Field type vocabulary by frequency: `number` 196, `Texture` 97, `double` 80,
`boolean` 56, `string` 54, `Vector2f` 31, `Vector3f` 28, `int` 28, `Bool` 12,
`Transform` 10, `Vector` 8, `Vector4f` 2, `Color` 2, `Material` 1, `AnimSeq` 3.

`[UI(...)]` attributes: `Range={lo,hi}`, `Drag`, `Slider`, `Option={…}`,
`Type="…"`, `NoAlpha`, and three that are new since 2026-05 — `Display="…"`
(the human label), `Button` (renders an action button rather than a value
widget), and `Order=N` (widget ordering).

The older `--@input` form still appears:

```
--@input float slim_body_intensity = 0.50 {"widget":"slider","min":0.0,"max":1.0}
--@input meshSize or rect, Amaz.Vector3f
--@input breverseUV, boolean
```

### 8.2 `lua-meta.json` / `js-meta.json` — the compiled form

This is the important one, and the 2026-05 spec dismissed it as "often `[]`".
It is not: 109 `lua-meta.json` and 91 `js-meta.json` exist cache-wide, the
largest 72 KB, and they are the **machine-readable form of the annotations
above**. A loader should read these rather than parse Lua comments.

```json
[{
  "ClassName": "AnimScript",
  "Super": "ScriptComponent",
  "FilePath": "lua/AnimScript.lua",
  "FileAbsPath": "/Users/apple/Documents/…/lua/AnimScript.lua",
  "Properties": [
    {"VarName": "duration", "VarType": "Double", "Comment": ""},
    {"VarName": "progress", "VarType": "Double", "Comment": "",
     "AnnoItems": [{"ItemType": "UI", "Attributes": [
       {"AttrType": "Range",  "RawValue": "", "Values": [0.0, 1.0]},
       {"AttrType": "Slider", "RawValue": "", "Values": []}]}]},
    {"VarName": "auto_play",        "VarType": "Bool",     "Comment": ""},
    {"VarName": "effect_material",  "VarType": "Material", "Comment": ""}],
  "Methods": [{"FuncName": "new", "Params": [], "Comment": ""}, …],
  "Comment": "", "ExportFiles": []
}]
```

`VarType` vocabulary observed: `Double` 131, `Texture` 45, `Bool` 33,
`String` 18, `Int64` 18, `Vector3f` 12, `Vector2f` 9, `Transform` 6,
`Vector4f` 4, `Vector` 3, `Material` 2, `Mesh` 1, `Color` 1.
`AttrType` vocabulary: `Range` 99, `Drag` 74, `Slider` 27, `Option` 19,
`Button` 6, `Type` 5, `Order` 3, `Display` 3, `NoAlpha` 1.

Note the leaked author paths in `FileAbsPath` — harmless, but a reminder that
these files are build output, not curated.

### 8.3 Host-level knobs

Two host-facing declarations, both outside the script layer.

`extra.json`'s `effect_adjust_params[]` (§3.2) declares the sliders CapCut's UI
shows for a whole effect. Their `effect_key` values are stable IDs that the
host broadcasts as events; the script receives them in `onEvent` as
`event.args:get(0)` (the key) and `event.args:get(1)` (the value). Alongside
the familiar `effects_adjust_*` family, this cache adds
`effects_colormigration_target_path`, `effects_colormigration_source_path`,
`effects_colormigration_reset`, `bloom_adjust_strength` and
`bloom_adjust_range`.

`config.json`'s `Link[].extra.composer_param[]` declares beauty/makeup sliders
with an explicit range instead of a normalised one:

```json
"composer_param": [
  {"name": "intensity", "key": "face_adjust_eyeshadow_test",
   "default_value": 0.8, "min_value": 0.0, "max_value": 1.0, "allow_negative": false},
  {"name": "proportion_correct_intensity", "key": "proportion_correct_intensity",
   "default_value": 0.0, "allow_negative": false}]
```

### 8.4 Scene-side defaults

The `ScriptComponent.properties` map inside `main.scene` / `.prefab` (§4.2)
holds the values a specific instance of a script starts with, overriding the
defaults in `.new()`. In the YAML form these are plainly visible
(`properties: {__class: Map, curTime: 0}`).

So the full chain for one tunable value is:
**declaration** in the Lua/JS annotation → **compiled** into `*-meta.json` →
**instance default** in the scene's `properties` map → **user override** via an
`effect_adjust_params` slider broadcast as an event → **written to a uniform**
by the script calling `material:setFloat(...)` → **consumed** by the GLSL.

---

## What changed since the 2026-05 analysis

This section compares against `EFFECT_FORMAT_SPEC.md`, `LUA_API_SPEC.md`,
`SHADER_SPEC.md` and `RENDER_GRAPH_SPEC.md` in `x/python_renderer_specs/`.
Those were built from a corpus of loose effect ZIPs; this survey is of a live
CapCut 9.0.0.3858 cache, so some differences are "different sample", not
"different version". Where that ambiguity exists it is called out.

### The big one: the binary formats have a readable YAML twin

`EFFECT_FORMAT_SPEC.md` §7.1 and `RENDER_GRAPH_SPEC.md` §3 treated
`%SerializedFormat%@` as a proprietary binary to be attacked with string
extraction and a state machine, and explicitly advised *"do not attempt full
serialization"*. That advice is now obsolete in the useful direction: a
minority of every asset type ships as **YAML 1.1 with typed document tags**
encoding the identical object model (§4). 24 of 210 `.rt` files, 4 of 263
`.material`, 4 of 213 `.xshader`, 2 of 58 `.scene`, 3 of 18 `.prefab`, 1 of 104
`.mesh` and 2 of 38 `.png.meta` in the sample are plain text.

This turns several open questions from `SHADER_SPEC.md` §14 into settled facts:

- **`u_ScreenParams` layout** — still not answered directly, but render-target
  dimensions are: `.rt` carries explicit `width`, `height`, and `pecentX`/
  `pecentY` scale factors relative to the input frame.
- **Texture filter and wrap defaults** — answered. `LINEAR`/`LINEAR`,
  `CLAMP` on S/T/R, `maxAnisotropy: 1`, mipmap off, `RGBA8` / `U8norm` /
  `RGBA8Unorm`.
- **Whether `.xshader` holds uniform defaults** — it does not hold values, but
  it holds far more than the earlier spec assumed: multiple passes, per-pass
  render targets, full Vulkan-style blend and depth-stencil state, vertex
  semantic mapping, and per-shader macro variant lists (§4.4).

### `alphaOutput/` is gone

`RENDER_GRAPH_SPEC.md` §11 called the `alphaOutput_<hash>/` sub-pass "the
universal compositing rule for every effect that ships one", and
`EFFECT_FORMAT_SPEC.md` §5.14 documented its six-file structure. **There are
zero `alphaOutput` files or directories in the entire 11,054-file cache.** The
equivalent job is now done by a named `blend` pass inside
`effects/videoAnimationBase/`, which the scene wires as the last camera. Any
loader that keys off `alphaOutput` will find nothing in 9.0-era packages.

### `atlas/` and `.imageatlas` are gone

`EFFECT_FORMAT_SPEC.md` §5.15 documented an older `atlas/*.imageatlas` layout.
Zero `.imageatlas` files and zero `atlas/` directories cache-wide.

### New file types

| File | Count | What it is |
|---|---:|---|
| `*.texture` | 14 | `%SerializedFormat%@` container embedding a PNG *and* its import settings, replacing the `.png` + `.png.meta` pair (§7.1). |
| `*.cube` | 4 | Plain Adobe 3D LUT text, shipped by `AmazingFilter` links (§7.6). |
| `*.lsproj`, `*.lsanim` | 2 + 2 | Lynx-Studio text-animation project and keyframe data (§2.6). |
| `effect_platform_children.tag` | 12 | Post-extraction file inventory written by the extractor (§1). |
| `Feature.js`, `js/*.js` | 14 | JavaScript effect controllers inside AmazingFeature packages (§6.3). |
| `AEInfo.dat` | 3 | Small encrypted blob per Lynx-Studio sub-effect. Purpose unknown. |
| `material.svg` | 2 | An SVG next to `config.json` in two 6.6.0-era packages. Not investigated. |
| `*.bin` / `*.raw` | 2 | Raw LUT for the HDR lens algorithm, in `featureAlgorithmConfig/`. Not decoded. |

### New `config.json` keys

New at `effect` level: `forceRender`, `forceUseAlgCache`. `exclusiveScene` was
documented as "reserved, always `[]`" — it now carries real
`{priority, sceneKey, tagName}` entries (§3.1). `requirement` was documented as
`object<string,bool>` — it now also takes structured values such as
`"blit": {"width": 720, "height": 1280}`.

New at `Link[]` level: `defaultEnable`, `preRenderAlgorithmTex`, `rtShare`.
`zorder` can be a float (`8011.0`).

New at top level (Lynx-Studio family only): `encrypt`, `script_type`,
`studio_animation_path`; those packages have **no `effect` block at all**,
which will break any loader that assumes `config.json → effect.Link[]`.

New in `content.json`: `filemap`, naming a root `.prefab` in place of
`main.scene` (§2.2).

New in `extra.json`: `setting.animation_duration` (a scalar, not a slider list),
`setting.bloom_adjust_params`, and a top-level `manual_beauty_face`.

### New Link types

`AmazingFeature`, `InfoSticker` and `ScriptInfoSticker` were the documented
set. Three more exist here: **`AmazingFilter`** (a native 3D-LUT applier),
**`TouchGes`** (a touch-gesture link with no path, paired with an
`amazingfeature/` link), and **`matting`** (model-selection slots only). Note
that **`ScriptInfoSticker` did not appear once** in this cache — its runtime
has been factored out into the shared `script_segment_js` package (§6.4),
which suggests the family was restructured rather than removed, but this survey
cannot confirm that from the cache alone.

### `.xshader` backends: six declared, two shipped

The earlier spec recorded a single backend tag, `gles2`. Modern AUSL-derived
shaders declare six: `gles2`, `glsl20`, `glsl30`, `glsl31`, `glsl32`, `metal`.
On disk, `shaderLib/` now has four subdirectory slots — `shaderGLES`,
`shaderMetal`, `shaderHLSL5`, `shaderVulkan` — but **the HLSL5 and Vulkan
directories are empty in all 20 instances cache-wide**. The `.xshader` also
carries an empty `angleBinaryPrograms` map, a cache slot for compiled ANGLE
binaries.

### AUSL is now the primary shader source

`SHADER_SPEC.md` §1.1 described `.ausl` as rare, "only in some particle
effects", and advised ignoring it. There are now 140 `.ausl` files with 140
matching `.compile_hash` siblings, and the `.xshader` manifests name the
`.ausl` as the source with the GLSL and Metal listed as generated outputs. The
generated GLSL has mangled identifiers (`_f0`, `_p0`) confirming it is compiler
output. Practically this changes nothing for a consumer — the GLSL is still
shipped — but it means the GLSL should be treated as artifact, not authored
code, and reformatting or reasoning about its variable names is pointless.

### The GLSL dialect has not changed

Worth stating because it is load-bearing for any renderer plan: still GLSL ES
1.0 in 224 of 228 sampled fragment shaders, still no `#version`, still
`precision highp float;`, still `texture2D` and `gl_FragColor`/`gl_FragData`,
still zero `#extension` and zero `#include`. The four ES 3.0 outliers behave as
before. The preprocessing recipe in `SHADER_SPEC.md` §9.2 needs no changes.

### Lua: same core, wider surface

The lifecycle (`new` → `constructor` → `onStart` → `onUpdate` / `seekToTime` →
`onEvent` → `onDestroy`), the `includeRelativePath` module system, the material
`setFloat`/`setInt`/`setTex` plus bracket-assignment shortcut, and the
`Amaz.Vector*` / `Amaz.LOG*` / `Amaz.BuiltinObject` core are all unchanged.
Zero `require(` calls, as before.

New `Amaz.*` symbols not in `LUA_API_SPEC.md`:
`Amaz.LOGD`; `Amaz.BuiltInTextureType.{INPUT0, OUTPUT, NORMAL}`;
`Amaz.PixelFormat.RGBA8Unorm`, `Amaz.InternalFormat.RGBA8`,
`Amaz.DataType.U8norm`; `Amaz.CameraClearType.COLOR_DEPTH`,
`Amaz.CameraEvent.{BEFORE_RENDER, AFTER_RENDER}`;
`Amaz.PrefabManager.loadPrefab`; `Amaz.MaterialPropertyBlock`;
`Amaz.ScreenRenderTexture`; `Amaz.AmazingUtil.{guidToPointer, SWIGToAMGObj}`
(which incidentally confirms the bindings are SWIG-generated);
`Amaz.Guid`; `Amaz.Ease.{linearFunc, quadIn, quadOut, ElasticOut}`;
`Amaz.Algorithm.{setAlgorithmParamFloat, setAlgorithmParamInt}`;
`Amaz.AMGFaceMeshUtils`, `Amaz.AMGBeautyMeshType.FACE145`;
`Amaz.AppEventType.COMPAT_BEF`, `Amaz.BEFEventType.BET_RECORD_VIDEO`;
`Amaz.EventType.TOUCH_MANIPULATE`; a full typed-vector family
(`Amaz.{Int8,Int16,Int32,Int64,UInt8,UInt16,UInt32,Double,String,Quat,Vec4}Vector`);
short aliases `Amaz.{Vec2,Vec3,Vec4,Quat,Mat3,Mat4}`; geometry types
`Amaz.{Rect, Ray, AABB}`; the `Amaz.Brush2D*` group
(`Brush2DUtils.generateBBoxMesh`, `Brush2DUtils.alignBBoxToResolution`,
`Brush2DDirtyFlag.{Unchanged, Cache, Increase, Undo, Redo}`); and the
body-reshape group `Amaz.Ns*`
(`NsItemType.{SLIM_BODY, SMALL_HEAD, STRETCH_LEG, SWAN_NECK}`,
`NsInputType.{IMAGE, VIDEO, NOCHANGE}`, `NsAutoBodyStrategy.ORIGIN_JY`).

Notably, `Amaz.Ease.*` **does** exist — `LUA_API_SPEC.md` §7 concluded it was a
Rust-side invention with no real counterpart. It is real, though rare (four
call sites).

`Amaz.Algorithm.getAEAlgorithmResult` went from 1 call site to 27, so the
depth/algorithm pipeline is no longer a curiosity.

New component types reachable via `getComponent`: `Text`, `SDFText`,
`Sprite2DRenderer`, `Layer2DRenderer`, `MorpherComponent`, `NsComponent`,
`FaceReshapeLiquefy`, and the four `Brush2D*` components.

New ECS systems in `sticker.config`: `Brush2DRendererSystem`,
`FaceReshapeSystem`, `LightSystem`, `MorpherSystem`, `NsSystem`,
`SDFTextSystem`, `ShadowMapSystem`, `TextSystem`, and the `v6_*` group.

### `lua-meta.json` went from noise to the primary parameter manifest

`EFFECT_FORMAT_SPEC.md` §5.1 listed `lua-meta.json` and `js-meta.json` as
"often `[]`, editor metadata". They are now the compiled, machine-readable
parameter declaration for every script — 109 and 91 files respectively, up to
72 KB — and reading them is strictly better than parsing `---@field` comments
(§8.2). New `[UI(...)]` attributes since the earlier grammar: `Display`,
`Button`, `Order`.

### `ImageBusinessSlider.json` went the other way

Documented in 2026-05 with a rich `LV → slider → materialValue → entity GUID`
mapping. All 35 instances in this cache are the empty
`{"ImageBusinessSlider": null}`. Whether the richer form was retired or simply
absent from this installation's effect selection cannot be determined here.

### Cache layout details that were not previously documented

The earlier work was done on loose ZIPs, so none of §1 had a counterpart: the
`<resource_id>/<md5>/` two-level layout, the retained `<md5>_tmp` ZIP, the
`ressdk_db` SQLite catalogue, the `path`-based mapping from `draft_content.json`
into the cache, `__MACOSX` stripping on extraction, and the `model` and
`script_segment_js` pseudo-resources.

### `%SerializedFormat%@` version: unchanged

The format version field is still `2` (with a handful of legacy `1` files), and
the second 4-byte field is an object count that varies per file. No new binary
format version has appeared.

---

## What this survey could not determine

- **Catalogue rows.** The `ressdk_db` schema is fully known; no actual
  `effect` / `loki_effect` rows were read (§1.2). The `effect_id` ↔
  `resource_id` relationship is therefore inferred from `draft_content.json`,
  not verified against the catalogue.
- **`.ausl` bytecode.** Not decoded. Not needed while GLSL ships alongside.
- **`sticker.info`, `graph.dat`, `AEInfo.dat`, `.lsproj`/`.lsanim` (encrypted).**
  All high-entropy with no recoverable structure. `graph.dat` and `sticker.info`
  were already established as unnecessary for rendering; the other two are
  unknown.
- **`lens_vhdr_image_lut_rgb.bin/.raw`.** Size and context strongly suggest a
  raw LUT; not confirmed.
- **`material.svg`.** Two instances, not investigated.
- **Whether `shaderHLSL5`/`shaderVulkan` are staged or vestigial.** The
  directories exist and are empty everywhere; no shipped package resolves the
  question.
- **Whether `ScriptInfoSticker` still exists as a Link type.** Absent from this
  cache; the shared `script_segment_js` runtime implies the family was
  restructured, but that is an inference.
- **The `encrypt: 0` flag's scope.** It plainly produces plaintext `.lsanim` in
  one package. Nothing was tested, and nothing on the Windows machine was
  modified.
