# The CapCut draft format

CapCut stores a project as a directory of plain JSON. The only file that matters
for rendering is `draft_content.json`: it is the complete, self-contained
description of a timeline — every clip, every transform, every keyframe, every
reference to a downloaded effect. Everything else in the project directory is
editor bookkeeping.

This document is a specification of that file, written so that chukcut can (a)
import a CapCut project without guessing, and (b) borrow the structural
decisions that CapCut got right. It matters because it is a working, shipped
answer to the problems chukcut has to solve anyway: how to address media, how to
separate what a clip *is* from where it *sits*, how to attach an open-ended set
of per-clip effects without a combinatorial explosion of segment fields, and how
to keyframe arbitrary properties.

## Provenance and honesty about coverage

Everything below was derived from CapCut 9.0.0.3858 on Windows, from two
independent sources:

1. **38 real `draft_content.json` files** found under
   `%LOCALAPPDATA%\CapCut\User Data\Projects\com.lveditor.draft\` — 16 top-level
   projects, their 16 mirrored per-timeline copies, and 6 compound-clip
   sub-drafts. Sizes range from 19 KB to 588 KB. Between them they contain 414
   segments across 144 tracks.
2. **The export table of `videoeditor.dll`**, which exposes 31,602 mangled C++
   symbols. Once demangled these turn out to include the full accessor set of
   every `lvve::Material*`, `lvve::Segment*` and `lvve::Keyframe*` class, and the
   accessor names *are* the JSON field names, one for one. Where the sample
   projects were silent, the symbol table was not. Sections below that come from
   symbols rather than from observed JSON are marked as such.

The user's projects are mostly straight cuts with captions, so several material
categories were **empty in all 38 drafts**: `transitions`, `video_effects`,
`plugin_effects`, `text_templates`, `chromas`, `color_curves`, `hsl_curves`,
`primary_color_wheels`, `log_color_wheels`, `video_radius`, `video_shadows`,
`video_strokes`, `shapes`, `flowers`, `digital_humans`, `smart_crops`,
`green_screens`, `handwrites`, `images`. For those I document the field list
from the DLL symbols and say so; I do not invent example values. I also searched
`User Data\Cache\template`, `Cache\AETemplate` and `Cache\onlineMaterial` for
downloaded template drafts that might have contained them, and found none — that
cache holds only attachment stubs on this machine.

## Where a project lives

```
User Data\Projects\com.lveditor.draft\
├── root_meta_info.json          index of every draft on the machine
└── <project name>\              e.g. "0323", "0719", "0318 (4)"
    ├── draft_content.json       THE timeline  (see below)
    ├── draft_content.json.bak   previous save
    ├── draft_meta_info.json     media bin + cloud/ownership metadata
    ├── draft_virtual_store.json bin folder hierarchy
    ├── draft_settings           INI: create/edit timestamps, edit seconds
    ├── draft.extra              small opaque binary (26 bytes here)
    ├── draft_cover.jpg          thumbnail
    ├── timeline_layout.json     which timelines are docked in the UI
    ├── key_value.json           per-material analytics/attribution blobs
    ├── performance_opt_info.json pre-combine hints
    ├── draft_agency_config.json / draft_biz_config.json  B2B features
    ├── attachment_editing.json  "which features did this user touch" flags
    ├── attachment_pc_common.json desktop-only UI state
    ├── common_attachment\       attachment_action_scene / _gen_ai_info /
    │                            _pc_timeline / _plugin_draft / _script_video /
    │                            coperate_create
    ├── Timelines\
    │   ├── project.json         list of timelines, main_timeline_id
    │   └── <timeline uuid>\     a full copy of the above per timeline
    ├── subdraft\<uuid>\         compound clips — each a complete draft
    ├── Resources\               audioAlg, videoAlg, digitalHuman byproducts
    ├── matting\ adjust_mask\ smart_crop\ qr_upload\   algorithm caches
    └── template.tmp / template-2.tmp
```

Two structural points worth copying.

**Multi-timeline is done by nesting whole projects.** `Timelines\<uuid>\` is not
a fragment; it is a complete project directory with its own `draft_content.json`,
`draft_cover.jpg` and `common_attachment\`. The `draft_content.json` at the top
of the project directory is a **byte-identical copy** of the active timeline's
(verified by MD5 on project `0323`), and its `id` equals that timeline's UUID.
So the top-level file is a cache of "the timeline you had open", and
`Timelines\project.json` is the authority:

```json
{ "id": "FB07B276-…", "main_timeline_id": "01FCB960-D68D-4cad-8711-1835C27F8570",
  "timelines": [ { "id": "01FCB960-…", "name": "Zeitleiste 01",
                   "create_time": 1774228695215827, "is_marked_delete": false } ],
  "version": 0 }
```

**Compound clips are done the same way.** A `materials.drafts[]` entry embeds an
entire nested draft object under a `draft` key, and `subdraft\<uuid>\` holds the
same thing on disk as a standalone project. There is no separate "nested
sequence" concept — a compound clip is a project used as a material.

### `draft_meta_info.json` — the media bin

This is the project's asset register, and it is the only place the original media
paths are recorded with import metadata. `draft_materials` is a list of
`{ "type": <int>, "value": [ … ] }` buckets (types 0,1,2,3,6,7,8 are always
emitted; only bucket 0 was populated in the samples). An entry:

```json
{ "id": "621b2b51-95c8-4f33-9753-a1c97493b731",
  "file_Path": "Z:/editing/Input videos/Video by runifyyy [DTwyMcFkddz].mp4",
  "extra_info": "Video by runifyyy [DTwyMcFkddz].mp4",
  "metetype": "video", "type": 0, "item_source": 1,
  "width": 720, "height": 1280, "duration": 7800000,
  "create_time": 1774228718, "import_time": 1774228765,
  "import_time_ms": 1774228764190429,
  "roughcut_time_range": { "start": 0, "duration": 7800000 },
  "sub_time_range": { "start": -1, "duration": -1 },
  "md5": "", "ai_group_type": "", "enter_from": 0 }
```

Note the naming: `file_Path` with a capital P, `metetype` (sic), and
`import_time` in seconds sitting next to `import_time_ms` which is actually
**microseconds**. The rest of the file is cloud-sync, enterprise and purchase
metadata plus `draft_fold_path`, `draft_root_path`, `draft_id`, `draft_name`,
`draft_new_version`, `tm_draft_create` / `tm_draft_modified` (microseconds since
epoch) and `tm_duration`.

`root_meta_info.json` at the top of `com.lveditor.draft\` repeats a subset of
this per project in `all_draft_store[]`, which is what the launcher's project
list reads.

`draft_virtual_store.json` describes the *bin folder tree*: bucket `type: 1` is a
list of `{ "child_id", "parent_id" }` pairs over the media ids from
`draft_meta_info.json`, with `parent_id: ""` meaning root. Bucket `type: 0`
carries sort/filter state. It has nothing to do with rendering.

`draft_settings` is an old-fashioned INI:

```ini
[General]
draft_create_time=1774228694
draft_last_edit_time=1781303953
real_edit_seconds=353
real_edit_keys=2
cloud_last_modify_platform=windows
```

`attachment_editing.json` is a large flat record of booleans named
`is_use_chroma_key`, `is_use_curve_speed`, `is_use_digital_human`,
`is_use_subtitle_recognition`, … — a feature-usage ledger, presumably for
telemetry and for deciding which upsells to show. `key_value.json` maps material
ids and segment ids to their acquisition context (`searchKeyword`, `requestId`,
`is_vip`, `materialCategory`, …); it is analytics, not timeline data.

`draft.extra` is a 26-byte binary blob whose content did not decode as anything
recognisable. I did not pursue it; nothing appears to depend on it.

---

## `draft_content.json` — top level

Thirty-five keys in 9.0 (thirty-six once `mixed_track_mode_on` appears, see
below). Types and meanings:

| Key | Type | Meaning |
|---|---|---|
| `id` | string (UUID) | Timeline id. Equals the directory name under `Timelines\`. |
| `version` | int | Binary format generation. `360000` in every file seen. |
| `new_version` | string | Schema version, `"161.0.0"` … `"177.0.0"` across the samples. This is the field that actually moves; it tracks the app's draft-schema revision, not the app version. |
| `draft_type` | string | `"video"`. |
| `source` | string | `"default"`. Provenance of the draft (template, deeplink, …). |
| `name`, `path` | string | Empty in every sample; the real name lives in `draft_meta_info.json`. |
| `duration` | int | Total timeline length **in microseconds**. |
| `fps` | float | Timeline frame rate, `30.0` throughout. |
| `is_drop_frame_timecode` | bool | NTSC drop-frame display. |
| `canvas_config` | object | `{ width, height, ratio, background }`. `ratio` is a UI string (`"original"`), `background` was always `null`. |
| `color_space` | int | `0` = default/sRGB. |
| `config` | object | 25–26 editor settings, see below. |
| `materials` | object | The material pool, ~54 named categories. The core of the file. |
| `tracks` | array | Ordered lanes of segments. The other core. |
| `keyframes` | object | Eight buckets: `videos`, `texts`, `audios`, `effects`, `filters`, `stickers`, `adjusts`, `handwrites`. Empty in all samples — see *Keyframes*. |
| `keyframe_graph_list` | array | Reusable named easing curves referenced by `graphID`. Empty in all samples. |
| `relationships` | array | Empty in all samples. Symbol table gives no accessor beyond the raw list. |
| `platform`, `last_modified_platform` | object | `{ app_id, app_source, app_version, device_id, hard_disk_id, mac_address, os, os_version }`. Device ids are hashed. Note `app_version` said `"8.2.0"` in files last written by 9.0.0.3858 — it is the version that *created* the draft, and it is not reliably updated. |
| `create_time`, `update_time` | int | `0` in every file; the real timestamps live in `draft_meta_info.json`. |
| `cover`, `retouch_cover`, `static_cover_image_path` | null/string | Cover image overrides. |
| `extra_info` | null or object | Present (3 keys) only in the newest draft. |
| `group_container`, `mutable_config` | null | Unused here. |
| `free_render_index_mode_on` | bool | `false`. When true, `render_index` is authored freely instead of being derived. |
| `render_index_track_mode_on` | bool | `true`. Render order follows track order — see *Render order*. |
| `mixed_track_mode_on` | bool | Only present in the 9.0-era draft (`new_version` 177). Mixed video/audio lanes. |
| `lyrics_effects` | array | Lyric-effect bindings. |
| `time_marks` | null or object | `{ id, mark_items: [ { id, title, color, time_range } ] }` — timeline markers. Example: `{"title":"Marker 01","color":"#00c1cd","time_range":{"start":2900000,"duration":0}}`. |
| `smart_ads_info` | object | `{ draft_url, page_from, routine }`. |
| `uneven_animation_template_info` | object | `{ composition, content, order, sub_template_info_list }`. |
| `function_assistant_info` | object | 37 keys recording the "smart assistant" toggles (`auto_caption`, `enhance_quality`, `normalize_loudness`, …) and the segment-id lists they were applied to. |

`config` holds editor-scoped settings that do affect rendering in places:
`video_mute`, `use_float_render` (half/float framebuffers), `material_save_mode`,
`export_range`, plus counters used to generate default names
(`adjust_max_index`, `combination_max_index`, `sticker_max_index`,
`original_sound_last_index`, `extract_audio_last_index`,
`record_audio_last_index`), subtitle/lyrics recognition task state
(`subtitle_recognition_id`, `subtitle_taskinfo`, `lyrics_sync`, …), multi-language
mode fields, `maintrack_adsorb` (snapping), `system_font_list` and
`zoom_info_params`.

### Times are microseconds, quantised to frames

Every time-like integer in the file is microseconds. Two independent checks:

* Project `0323` has `duration: 7800000` and its source clip is a 7.8-second
  video (`draft_meta_info.json` gives `duration: 7800000` for a clip whose file
  is 7.8 s).
* Every duration seen is an exact frame count at the project's 30 fps, rounded to
  the nearest microsecond: `1333334` = 40 frames, `2433334` = 73 frames,
  `25533333` = 766 frames, `73500000` = 2205 frames, `19866666` = 596 frames.
  The alternating `…333` / `…334` endings are the giveaway — the writer computes
  `round(frames * 1e6 / fps)`.

Stills use a sentinel duration of `10800000000` µs = 3 hours, meaning "unbounded".

Timestamps outside the timeline are inconsistent: `draft_settings` uses seconds,
`draft_meta_info.json` uses both (`import_time` seconds, `import_time_ms` and
`tm_draft_create` microseconds). Do not assume.

---

## `materials` — the pool

`materials` is an object of ~54 arrays. Each array holds flat objects, each with
a unique `id` (upper-case UUID with a lower-case hex fourth group — CapCut's
signature id shape, e.g. `4144E7FB-68AD-46d2-9F62-32C4E384112E`). Segments refer
to materials only by id. Nothing is nested inside a segment.

The complete category list in 9.0.0.3858:

```
ai_text_effects        ai_translates          audio_balances       audio_effects
audio_fades            audio_pannings         audio_pitch_shifts   audio_track_indexes
audios                 beats                  canvases             chromas
color_curves           common_mask            digital_human_model_dressing
digital_humans         drafts                 effects              flowers
green_screens          handwrites             hsl                  hsl_curves
images                 log_color_wheels       loudnesses           manual_beautys
manual_deformations    material_animations    material_colors      multi_language_refs
placeholder_infos      placeholders           plugin_effects       primary_color_wheels
realtime_denoises      shapes                 smart_crops          smart_relights
sound_channel_mappings speeds                 stickers             tail_leaders
text_templates         texts                  time_marks           transitions
video_effects          video_radius           video_shadows        video_strokes
video_trackings        videos                 vocal_beautifys      vocal_separations
```

`ai_text_effects` appears only in the newest draft; the other 54 are always
present, emitted as empty arrays when unused. That list is a feature
specification: each entry is something the editor can do, named the way its
authors named it.

### A shape that repeats: the downloadable-resource reference

Every category whose content comes from ByteDance's CDN carries the same block of
fields, and recognising it collapses most of the format:

| Field | Meaning |
|---|---|
| `resource_id` | The catalogue id, e.g. `"7374021188315517456"`. Stable across machines. |
| `third_resource_id` | Same id in a second namespace, usually identical. |
| `effect_id` | Often the same number again; the id the effect runtime is asked for. |
| `path` | Local cache directory: `…\User Data\Cache\effect\<numeric id>\<md5>`. |
| `lumi_hub_path` | For colour-grading materials, a `lumi_hub_path` subdirectory of the above. |
| `name` | Localised display name (`"Kreis"`, `"Wort für Wort"`). |
| `category_id` / `category_name` | Panel grouping. |
| `panel` | Which UI panel it was picked from. |
| `platform` | `"all"`. |
| `source_platform` | `0` = built-in, `1` = downloaded from the online library. |
| `request_id` | The API request that fetched it — analytics, e.g. `"202607200458481F313D859E3A51E787B7"`. |
| `material_resource_id`, `material_source_platform` | Present on every material class per the symbol table; usually empty. |

`path` is an absolute local path, which means a draft is **not portable between
machines** without repointing every effect. `resource_id` is the portable
identifier. For chukcut the lesson is: store the catalogue id, derive the path.

### `videos` — visual media (video, photo, GIF)

The largest material type; ~100 fields. `type` is `"video"`, `"photo"` or
`"gif"`. Core fields:

| Field | Type | Meaning |
|---|---|---|
| `id` | UUID | Material id. |
| `type` | string | `"photo"` in the excerpt below. |
| `path` | string | Absolute path to the source file. |
| `material_name` | string | File name. |
| `duration` | int µs | Full source duration; `10800000000` for stills. |
| `width`, `height` | int px | Source pixel dimensions. |
| `has_audio` | bool | Whether an audio stream exists. |
| `crop` | object | Eight normalised corner coordinates `upper_left_x/y`, `upper_right_x/y`, `lower_left_x/y`, `lower_right_x/y`, each 0…1 in source space. Identity is `(0,0) (1,0) (0,1) (1,1)`. The quad form (rather than a rect) allows keystone/perspective crops. |
| `crop_ratio` | string | `"free"`, or a locked ratio name. |
| `crop_scale` | float | Scale applied on top of the crop. |
| `matting` | object | Background-removal state: `flag`, `path` to the generated matte, brush `strokes`, `expansion`, `feather`, `reverse`, `mask_video_path`. |
| `stable` | object | Stabilisation: `stable_level`, `matrix_path` to precomputed transforms, `time_range`. |
| `video_algorithm` | object | The ML pipeline attached to this clip: `algorithms[]`, `super_resolution`, `noise_reduction`, `deflicker`, `quality_enhance`, `motion_blur_config`, `complement_frame_config` (frame interpolation), `ai_in_painting_config`, `ai_background_configs`, `mouth_shape_driver`, `ai_expression_driven`, `ai_motion_driven`, `aigc_generate_list`, and a `path` to the cached result. |
| `reverse_path`, `intensifies_path`, `cartoon_path`, `intensifies_audio_path` | string | Paths to pre-rendered derivative media (reversed clip, speed-ramped clip, cartoonised clip). CapCut bakes these to disk rather than computing them live. |
| `video_mask_stroke`, `video_mask_shadow` | object | Outline and drop shadow applied to the masked cut-out. |
| `beauty_face_preset_infos`, `beauty_body_auto_preset`, `is_unified_beauty_mode` | | Retouching presets. |
| `corner_pin`, `surface_trackings`, `object_locked`, `smart_motion`, `multi_camera_info`, `freeze` | | Advanced features, `null` when unused. |
| `check_flag` | int | Bitfield of validation/capability flags (`62978047` observed). Opaque. |
| `aigc_type`, `is_ai_generate_content`, `aigc_history_id`, `is_copyright` | | Generative-AI provenance and rights tracking. |

Excerpt (trimmed):

```json
{ "id": "4144E7FB-68AD-46d2-9F62-32C4E384112E",
  "type": "photo",
  "path": "C:/Users/user/Downloads/Screenshot 2026-07-19 ….png",
  "duration": 10800000000, "width": 348, "height": 732, "has_audio": false,
  "crop": { "upper_left_x": 0.0, "upper_left_y": 0.0,
            "upper_right_x": 1.0, "upper_right_y": 0.0,
            "lower_left_x": 0.0,  "lower_left_y": 1.0,
            "lower_right_x": 1.0, "lower_right_y": 1.0 },
  "crop_ratio": "free", "crop_scale": 1.0, "source": 0, "source_platform": 0 }
```

### `audios`

`type` distinguishes provenance: `"video_original_sound"` (audio extracted from a
video clip on the timeline), `"music"`, `"record"`, `"tts"`, `"sound"`. Fields of
substance are `path`, `duration` (µs), `name`, `local_material_id`,
`music_id`/`resource_id` for library music, `wave_points` (cached waveform), and
a very large block of text-to-speech parameters (`tone_speaker`, `tone_emotion_*`,
`tts_task_id`, `is_ai_clone_tone`, `moyin_emotion`, `tts_benefit_info`) that is
inert for ordinary audio. `copyright_limit_type` records licensing state.

### `texts`

The richest material after `videos`. Two representations coexist and both are
written:

* **Flat style fields** — `text_color`, `text_alpha`, `font_size`, `font_path`,
  `alignment`, `letter_spacing`, `line_spacing`, `border_color`/`border_width`/
  `border_alpha`/`border_mode`, `background_*` (11 fields incl.
  `background_round_radius`, `background_vertical_offset`), `single_char_bg_*`,
  `shadow_*` (`shadow_point` as an x/y pair *and* `shadow_angle`/`shadow_distance`),
  `bold_width`, `italic_degree`, `underline`/`underline_width`/`underline_offset`.
* **`content`, a JSON *string*** holding the rich-text model, with per-range
  styling. This is what the renderer actually consumes:

```json
{"text":"nach 1/2 Jahren nach Kriegsbeginn stellt sich die Frage",
 "styles":[{"fill":{"content":{"render_type":"solid",
                               "solid":{"color":[0.941176474094391,1,0]}}},
            "font":{"path":"…/Resources/Font/SystemFont/en.ttf","id":""},
            "strokes":[{"content":{"render_type":"solid",
                                   "solid":{"color":[0,0,0]}},
                        "width":0.0599999986588955,"mode":0}],
            "size":15,"useLetterColor":true,"range":[0,55]}]}
```

`range` is `[startChar, endChar)`. Colours here are float triples in 0…1, while
the flat fields use `#rrggbb` strings — the two must be kept in sync by the
writer. `base_content` is the same structure before user styling.

Layout: `line_max_width` (0…1, fraction of canvas width), `typesetting`
(0 horizontal / 1 vertical), `line_feed`, `fixed_width`/`fixed_height` (−1 =
auto), `autoAdaptCanvasEnabled`, `inner_padding`. Path typesetting is supported:
`text_curve`, `enable_path_typesetting`, `text_typesetting_paths`,
`offset_on_path`, `text_loop_on_path`.

For auto-captions, `type` is `"subtitle"` and `words` carries per-word timing:

```json
"words": { "text":      ["nach"," ","1/2"," ","Jahren", …],
           "start_time":[0,0,600,1160,1200, …],
           "end_time":  [0,0,1160,1160,1400, …] }
```

Note the unit change: these are **milliseconds relative to the segment**, not
microseconds — the only place in the file where that is true. `group_id`
(`"de-DE_1784495269084"`) ties the caption run together, `language` names the
recognition locale, and `recognize_task_id` links back to the ASR job.

### `stickers`

A downloadable-resource reference (`resource_id`, `sticker_id`, `path` into
`Cache\artistEffect\<id>\<md5>`, `icon_url`, `preview_cover_url`) plus a shape
and fill model shared with `shapes`: `shape_param` (`shape_type`, `roundness[]`,
`custom_points[]`), `shape_fill_render_style` with `render_type` of
`"solid" | "gradient" | "texture"`, per-corner `radius`, border and shadow.
`cycle_setting` controls looping of animated stickers.

### `canvases` and `material_colors`

Two tiny materials attached to *every* visual segment. `canvases` sets what fills
the frame behind the clip:

```json
{ "id": "6E7FDFE6-…", "type": "canvas_color",
  "color": "", "blur": 0.0, "image": "", "album_image": "", "image_id": "" }
```

`type` is one of `canvas_color`, `canvas_blur`, `canvas_image`, `canvas_blend`
(the four strings are present in `videoeditor.dll`). `material_colors` carries a
solid or gradient colour used when the segment is a colour clip
(`is_color_clip`, `solid_color`, `is_gradient`, `gradient_colors[]`,
`gradient_percents[]`, `gradient_angle`).

### `speeds`

```json
{ "id": "AC7B21F3-…", "type": "speed", "mode": 0, "speed": 1.0, "curve_speed": null }
```

`mode` 0 = constant, non-zero = curve. `curve_speed` (class `lvve::CurveSpeed`)
has `name`, `source_platform` and `speed_points` — the control points of a speed
ramp. Not exercised in any sample.

### `material_animations`

One material holds *all* animations on a segment, as a list:

```json
{ "id": "21A2BEA7-…", "type": "sticker_animation",
  "animations": [ { "id": "7131927521469141505", "type": "caption",
                    "start": 0, "duration": 3433333,
                    "resource_id": "7131927521469141505",
                    "third_resource_id": "7131927521469141505",
                    "path": "…/Cache/effect/7131927521469141505/a0221e35…",
                    "name": "Wort für Wort",
                    "category_id": "caption_animation", "category_name": "Untertitel ",
                    "material_type": "sticker", "platform": "all",
                    "source_platform": 1, "anim_adjust_params": null,
                    "request_id": "…" } ],
  "multi_language_current": "none" }
```

The wrapper `type` is `sticker_animation` or `video_animation`. Each entry's own
`type` is `in`, `out`, `group` (loop) or `caption`. `start` is relative to the
segment, `duration` in µs; an "out" animation is stored with `start` = segment
duration − animation duration. `anim_adjust_params` carries per-animation user
tweaks. This is a clean design worth copying: one animation slot per segment
holding an ordered list, rather than three separate in/out/loop fields.

### `common_mask` (the shape-mask material)

```json
{ "id": "9D7E09D7-…", "type": "mask", "category": "video",
  "name": "Kreis", "resource_type": "circle",
  "resource_id": "7374021188315517456",
  "path": "…/Cache/effect/1068046525/3ab1c47350d987c8ad415497e020a38b",
  "constant_material_id": "18B867EA-CD22-48a3-823E-9A070F8120C8",
  "is_old_version": false, "contour_path": null, "track_segment": "",
  "config": { "centerX": 0.0055988086470495566,
              "centerY": 0.051740750688889725,
              "width": 3.547231341375073,
              "height": 1.631726417032533,
              "aspectRatio": 1.0,
              "rotation": 0.0, "roundCorner": 0.0,
              "feather": 0.0, "expansion": 0.0, "invert": false },
  "text_config": { … } }
```

`config` is the whole mask. `centerX`/`centerY` use the same normalised space as
segment `transform` (see below). `width`/`height` are normalised to the **canvas
width**, so a full-frame rectangle on a 9:16 canvas is `width: 1.0`,
`height ≈ 1.78`; values above 1 are normal and mean "larger than the frame".
`roundCorner`, `feather` and `expansion` are 0…1. `resource_type` names the
shape (`circle`, `rectangle`, `linear`, `mirror`, `heart`, `star` in the UI).

The width/height normalisation is an inference from only two mask instances and
should be re-checked. The evidence is a default rectangle mask on a 1080×1920
canvas stored as `width: 1.0, height: 1.7857`; dividing both by the canvas width
makes a full-frame rectangle exactly `(1.0, 1.7778)`, which matches to 0.4 %.
Dividing height by the canvas height instead would give `1.0`, which it is not.
`aspectRatio` is a separate field, `1.0` in both samples, whose interaction with
`width`/`height` I could not determine.

`constant_material_id` is important: it is a *stable* id that keyframes point at,
while `id` changes when the mask material is rewritten. Mask keyframes carry
`material_id = constant_material_id`, not `id`.

### `hsl` — a representative colour-grading material

```json
{ "id": "6A5F444B-…", "type": "hsl", "version": "1",
  "hsl_color_type": 1, "custom_color": "#FFE64444",
  "hue": 0, "saturation": 0, "lightness": 0, "interacting": true,
  "constant_material_id": "2DD26E4A-…",
  "path": "…/Cache/effect/7501974767453474064/20cd8db6…",
  "lumi_hub_path": "…/Cache/effect/7501974767453474064/20cd8db6…/lumi_hub_path" }
```

Six of these appear per graded clip — one per colour band. The `path` /
`lumi_hub_path` pair is the tell that colour grading is not hard-coded in the
renderer: it is an *effect package* downloaded like any other, and `lumi_hub_path`
points at a Lumi render-graph sub-directory inside it.

### Audio-adjacent materials

`audio_fades` — `{ fade_in_duration, fade_out_duration, fade_type }`, µs.
`loudnesses` — `{ enable, target_loudness, file_id, loudness_param, time_range }`
for LUFS normalisation. `sound_channel_mappings` —
`{ audio_channel_mapping, is_config_open, type }`. `vocal_separations` —
`{ choice, removed_sounds[], production_path, final_algorithm, time_range }`,
where `production_path` points at the stem file once the model has run. `beats` —
beat-detection results with `ai_beats.beats_path` (a `.beat` file in
`Cache\music\`), `beats_url`, `melody_percents[]`, plus `user_beats[]` for
manually tapped markers and `gear`/`mode` for the detection preset.

### Housekeeping materials

`placeholder_infos` records what a missing clip *was*
(`{ meta_type, res_path, res_text, error_path, error_text }`) so the timeline
survives offline media. `sound_channel_mappings`, `speeds`, `vocal_separations`
and `placeholder_infos` are emitted for **every** segment whether used or not —
in the 38 drafts each of those had exactly 292 objects, one per segment. That is
CapCut trading file size for a uniform segment shape.

### Categories not present in the samples

Field lists below are taken from the accessor names exported by
`videoeditor.dll`. They are the real JSON keys, but I have no example values.

**`transitions`** (`lvve::MaterialTransition`) — `duration`, `is_overlap`,
`is_ai_transition`, `video_path`, `task_id`, plus the standard resource block
(`resource_id`, `third_resource_id`, `effect_id`, `path`, `name`, `category_id`,
`category_name`, `platform`, `source_platform`, `request_id`). A transition is
stored on the *left* segment of the pair (`SegmentVideo` has a `transition`
slot); `is_overlap` selects whether the transition eats into both clips or
extends the timeline.

**`video_effects`** (`lvve::MaterialVideoEffect`) — `adjust_params`, `value`,
`time_range`, `apply_time_range`, `render_index`, `track_render_index`,
`item_effect_type`, `sub_type`, `bind_segment_id`, `common_keyframes`,
`effect_mask`, `enable_mask`, `enable_video_mask_stroke`,
`enable_video_mask_shadow`, `disable_effect_faces`, `transparent_params`,
`covering_relation_change`, `formula_id`, `version`, `algorithm_artifact_path`,
`aigc_current_artifact_path`/`_cnt`, plus the resource block. Two things stand
out: a video effect carries **its own `common_keyframes`** (so effect parameters
animate independently of the clip), and it carries its own `render_index`, so an
effect can be ordered relative to other effects rather than to clips.

**`chromas`** (`lvve::MaterialChroma`) — `color`, `intensity_value`,
`shadow_value`, `spill_value`, `edge_smooth_value`, `should_transfer_color`,
`version`, `path`, `resource_id`.

**`text_templates`** (`lvve::MaterialTextTemplate`) — `resources`,
`text_info_resources`, `non_text_info_resources`, `material_text_ranges`,
`merge_content`, `render_mode`, `is_3d`, `is_dynamic_build`, `is_pre_rendered`,
`is_uneven_animation`, `is_lyric_effect`, `lyric_group_id`, `preview_time`,
`text_template_command`, `text_template_resource_type`,
`text_template_preset_resource_id`, `current_word_info`, `origin_word_info`,
`ai_emoji_config`, `is_ai_emoji`, `ai_emoji_changed_seq`, `aigc_config`,
`aigc_type`, `check_flag`, `text_to_audio_ids`, plus the resource block. The
`resources` / `text_info_resources` split is how a template with N editable text
slots binds each slot to its own sub-resource.

**`primary_color_wheels`** / **`log_color_wheels`**
(`lvve::MaterialPrimaryColorWheels`) — `lift`, `gamma`, `gain`, `offset`,
`intensity`, `visible`, `panel`, `path`, `lumi_hub_path`, `resource_id`.

**`color_curves`** (`lvve::MaterialColorCurves`) — `luma`, `red`, `green`,
`blue`, `link_status`, `select_channel`, `visible`, `panel`, `path`,
`lumi_hub_path`. Each channel is a list of `lvve::ColorCurvesPoint`.

**`video_shadows`** (`lvve::MaterialVideoShadow`) — `enable_video_shadow`,
`color`, `alpha`, `angle`, `distance`, `ambiguity`, `path`, `resource_id`,
`resource_name`, `source_platform`.

**`video_strokes`** (`lvve::MaterialVideoStroke`) — `enable_video_stroke`,
`color`, `adjust_params`, `path`, `resource_id`, `resource_name`.

**`effects`** (`lvve::MaterialEffect`, i.e. filters and adjustment presets) —
`adjust_params`, `value`, `intensity_key`, `time_range`, `visible`, `panel_id`,
`category_key`, `sub_category_id`/`_name`, `item_effect_type`, `sub_type`,
`exclusion_group`, `lumi_hub_path`, `color_match_info`, `smart_color_mode`,
`face_adjust_params`, `bloom_params`, `beauty_face_auto_preset_id`,
`enable_skin_tone_correction`, `report_name`, `formula_id`, `version`, plus the
resource block.

---

## `tracks` and `segments`

```json
{ "id": "E9A9289E-FB0F-4e60-961C-BA4AFF0B91B5",
  "type": "text", "attribute": 0, "flag": 1,
  "name": "", "is_default_name": true,
  "segments": [ … ] }
```

A track has exactly seven fields. `type` is `video`, `audio`, `text`, `sticker`,
`effect`, `filter` or `adjust` (only the first four occurred). `attribute` was
`0` except for one audio track at `1` (that track's segments also carried
`track_attribute: 1`; it appears to mark a muted/locked or "attached" lane).
`flag` is a small bitfield. Across all 38 drafts the distribution is exact and
unambiguous: `video/flag 0` occurs **38 times, once per draft, always as
`tracks[0]`** — it is the main track; `video/flag 2` occurs 50 times and is
always an overlay (picture-in-picture) lane; `text/flag 0` (20) is a
manually-added text track and `text/flag 1` (4) is the auto-generated subtitle
track; `audio` and `sticker` tracks are always `flag 0`, with `attribute 1`
appearing on 2 of the 28 audio tracks. Segments within a track are stored in timeline
order and must not overlap.

### The segment

Fifty-one keys, uniform across every track type — even audio segments carry
`clip` (as `null`) and text segments carry `volume`. The ones that matter:

| Field | Meaning |
|---|---|
| `id` | Segment UUID. |
| `material_id` | The **primary** material. Its category is implied by the track type: a video track's segment points at `videos` (or `drafts` for a compound clip), a text track's at `texts`, an audio track's at `audios`, a sticker track's at `stickers`. |
| `extra_material_refs` | Flat array of *other* material ids attached to this segment. See below. |
| `target_timerange` | `{ start, duration }` in µs **on the timeline**. |
| `source_timerange` | `{ start, duration }` in µs **inside the source media**. `null` for text and sticker segments, which have no source. |
| `render_timerange` | `{ start, duration }`; `{0,0}` in every sample. Reserved for pre-render/cache bookkeeping. |
| `speed` | Playback rate; duplicated from the attached `speeds` material for fast access. |
| `render_index` | Compositing order, see below. |
| `track_render_index` | Index of the owning track in `tracks[]`. Redundant but always consistent. |
| `clip` | Transform, see below. `null` on audio segments. |
| `uniform_scale` | `{ on, value }`. When `on`, the UI links scale.x and scale.y; the renderer still reads `clip.scale`. |
| `volume`, `last_nonzero_volume` | Linear gain (values above 1 occur — `1.81` observed). |
| `visible` | Per-segment mute for video. |
| `reverse`, `is_loop`, `is_tone_modify`, `intensifies_audio`, `cartoon` | Booleans selecting the pre-baked derivative paths on the `videos` material. `is_tone_modify` = pitch correction when speed-shifted. |
| `enable_lut`, `enable_adjust`, `enable_hsl`, `enable_color_curves`, `enable_hsl_curves`, `enable_color_wheels`, `enable_video_mask`, `enable_mask_stroke`, `enable_mask_shadow`, `enable_smart_color_adjust`, `enable_color_match_adjust`, `enable_color_correct_adjust`, `enable_adjust_mask`, `enable_color_adjust_pro` | Per-stage bypass switches. The attached materials stay in the file; these turn the corresponding render stages on and off. |
| `hdr_settings` | `{ mode, intensity, nits }` — `nits: 1000` default. |
| `group_id` | Non-empty when segments are grouped in the UI. |
| `common_keyframes` | See *Keyframes*. |
| `keyframe_refs` | Ids into the top-level `keyframes` buckets. Empty in every sample. |
| `responsive_layout` | `{ enable, target_follow, size_layout, horizontal_pos_layout, vertical_pos_layout }` — auto-relayout when the canvas ratio changes. |
| `source` | Provenance string. `"segmentsourcenormal"` throughout; the DLL also contains `segmentsourceimage`, `segmentsourceaivoice`, `segmentsourcesmarteditaudio`, `segmentsourcesmartedittext`, `segmentsourcesmarteditquickclip`, `segmentsourcequalitycompare`, `segmentsourceimageeditorsticker`, `segmentsourceintelligentfunctionrecommendautocaption`, `segmentsourceintelligentfunctionrecommendcaptionopt` — a record of which feature created the segment. |
| `state`, `template_id`, `template_scene`, `raw_segment_id`, `is_placeholder`, `caption_info`, `lyric_keyframes`, `segment_color_tag`, `desc`, `digital_human_template_group_id`, `color_correct_alg_result` | Editor bookkeeping. |

### `target_timerange` vs `source_timerange`

`target_timerange.start` is where the segment begins on the timeline;
`source_timerange.start` is the in-point inside the media. Trimming the head of a
clip advances `source_timerange.start` while `target_timerange.start` stays put;
moving the clip does the reverse. When `speed != 1`,
`target.duration ≈ source.duration / speed` — the two are stored independently
and the writer keeps them consistent rather than deriving one from the other.

A real pair from an audio segment:

```json
"target_timerange": { "start": 6600000, "duration": 866666 },
"source_timerange": { "start": 0,       "duration": 866666 }
```

This is the single most important structural decision to copy. It makes trim,
slip, ripple and speed all local edits to one segment.

### `render_index` and render order

Observed values:

* Main video track (`flag: 0`): `render_index` 0.
* Overlay video tracks (`flag: 2`): small positive integers, roughly increasing
  with track index but **not** unique and **not** monotonic — in one project a
  single overlay track held segments with `render_index` 14, 7 and 2.
* Text and sticker segments: a base of `14000`, incrementing per segment within
  a caption run (`14000, 14001, 14002, …`) or shared across a track (`14002`
  five times).

The top-level flags explain this. `render_index_track_mode_on: true` and
`free_render_index_mode_on: false` mean **track order is authoritative**:
`track_render_index` (the segment's position in `tracks[]`) decides compositing,
and `render_index` is a legacy/secondary key retained for compatibility and for
the "free" mode where the user reorders layers independently of tracks. The
14000 base separates the overlay-graphics band from the video band so that text
always draws above video regardless of track position.

For chukcut: keep an explicit per-segment render index (CapCut is right that
implicit track order eventually fails), but do not replicate the dual mode.

### `clip` — the transform

```json
"clip": { "scale":     { "x": 0.29831, "y": 0.29831 },
          "transform": { "x": -0.51995, "y": 0.0 },
          "rotation": 0.0,
          "flip": { "horizontal": false, "vertical": false },
          "alpha": 1.0 }
```

**`transform` is normalised device coordinates over the canvas: `x` and `y` run
−1 … +1 from edge to edge, origin at the centre, +y up.** Determined as follows.
In project `0719` (canvas 1080×1920) the 31 auto-generated subtitle segments all
sit at `transform.y = -0.56`. If the divisor were the half-height (960 px) the
caption centre lands 538 px below the middle, i.e. 1498 px from the top — the
lower third, which is where CapCut puts captions. If the divisor were the full
height (1920 px) it would land 2035 px from the top, off the bottom of the frame.
The same test on a 960×146 lower-third graphic in the 1920×1080 project gives
744…898 px under the half-height reading and off-canvas under the full-height
one. Across all 414 segments the observed range is `x ∈ [−0.586, +0.554]`,
`y ∈ [−0.749, +0.872]`, consistent with a [−1, 1] space and inconsistent with a
[−0.5, 0.5] one. The sign convention follows from the captions being at negative
`y`.

**`scale` is relative to a canvas-fitted base size, not to source pixels.** In
the 1920×1080 project, photos of 250×250, 330×330 and 1024×1024 all carry
`scale: 0.2536` — three different source resolutions rendered at one on-screen
size. So the pipeline first fits the material to the canvas and then applies
`scale`. The fit is *contain* rather than *cover*: a 960×146 banner at
`scale 0.527` works out to 1011×154 px under contain (a plausible lower third)
and to 3741×569 px under cover (twice the canvas width). Text scale is likewise
relative to the layout box implied by `font_size`, not to pixels — text segments
carry `scale` values of 0.505, 1.0, 1.10, 1.64.

`rotation` is degrees, clockwise, about the segment centre. `alpha` is 0…1.
`flip` is applied before rotation.

`uniform_scale.on` only records that the UI has the aspect lock engaged; the
renderer reads `scale.x` and `scale.y`.

Note that `crop` lives on the **material**, not the segment. Two segments sharing
a material share its crop — which is why CapCut duplicates the `videos` entry
whenever the same file is placed twice with different crops. There were 70
`videos` objects for 45 main-track segments in one project. chukcut should put
crop on the segment.

### `extra_material_refs`

A flat, unordered array of material ids:

```json
"extra_material_refs": [
  "10DC1541-…",   →  speeds            (speed)
  "88DC75CD-…",   →  placeholder_infos (placeholder_info)
  "549B3675-…",   →  canvases          (canvas_color)
  "68589D9C-…",   →  sound_channel_mappings
  "3AA3F533-…",   →  material_colors
  "787F673C-…"    →  vocal_separations (vocal_separation)
]
```

There is no type tag. The loader resolves each id in the material pool, reads the
category it was found in, and assigns it to the corresponding typed slot on the
segment. The slots are visible in `videoeditor.dll` as the accessors of
`lvve::SegmentVideo`:

```
material  speed  canvas  background  material_color  sound_channel_mapping
vocal_separation  placeholder_info  transition  filter  chroma  video_mask
video_effects  plugin_effects  animations  anim  audio_fade  audio_panning
audio_pitch_shift  audio_track_index  beat  loudness  adjust  beauty  reshape
makeup_root  manual_beautys  manual_deformation  realtime_denoise  green_screen
smart_crop  smart_relight  digital_human  digital_human_model_dressing
video_tracking  surface_trackings  matting  ai_matting  ai_translate  crop
stable  stretch_leg  radius  shadow  stroke  video_mask_shadow  video_mask_stroke
corner_pin  figures  material_draft  vocal_beautify  voice_change  multi_camera_info
```

`SegmentText` has a narrower set (`material`, `effect`, `effects`, `flowers`,
`shape`, `bloom`, `animations`, `caption_info`, `video_tracking`,
`multi_language_material_refs`), `SegmentAudio` narrower still (`material`,
`fade`, `beat`, `speed`, `loudness`, `balance`, `audio_panning`,
`audio_pitch_shift`, `vocal_separation`, `realtime_denoise`, `voice_change`,
`ai_translate`).

Note `video_effects` and `plugin_effects` are *plural* slots — a segment can
carry a list of effects, ordered by their own `render_index`.

This is an elegant trick and worth stealing: the segment schema never changes
when a new effect type is added, because the segment only ever holds ids. The
cost is that a reader must build the whole material index before it can interpret
a single segment, and that a corrupted id silently drops an effect.

---

## Keyframes

CapCut has **two** keyframe mechanisms, and only the newer one was exercised in
the sample projects.

### `common_keyframes` — the current mechanism

Each segment holds a list of `CommonKeyframes` objects, one per animated
property:

```json
{ "id": "05E1E524-8F0E-45bb-A8C0-19F50475FB74",
  "property_type": "KFTypeCommonMaskSizeWidth",
  "material_id": "18B867EA-CD22-48a3-823E-9A070F8120C8",
  "keyframe_list": [
    { "id": "CD2C9F00-…", "time_offset": 0,      "values": [0.14167388167388173],
      "curveType": "Line", "graphID": "", "string_value": "",
      "left_control": {"x":0.0,"y":0.0}, "right_control": {"x":0.0,"y":0.0} },
    { "id": "B49CE079-…", "time_offset": 566667, "values": [2.6712426754892524], … },
    { "id": "C3B6DCE4-…", "time_offset": 600000, "values": [1.397274631418363], … }
  ] }
```

* `property_type` names the animated property (vocabulary below).
* `material_id` is `""` when the property belongs to the segment itself
  (position, scale, rotation, alpha, volume) and is set when the property belongs
  to an attached material — for mask keyframes it holds the mask's
  `constant_material_id`, **not** its `id`.
* `time_offset` is microseconds **from the start of the segment**, not from the
  start of the timeline. Frame-quantised like everything else.
* `values` is an array, so a single keyframe can carry a vector (a colour, a
  point). Scalars use a one-element array. `string_value` carries non-numeric
  values.
* `curveType` is `"Line"` in every observed keyframe; `left_control` and
  `right_control` are Bézier handles for the eased case, and `graphID` refers
  into the top-level `keyframe_graph_list` for a shared named curve.

Crucially, the *current* value of an animated property is also written back into
the material — the mask's `config.centerX` exactly equalled the last
`KFTypeCommonMaskPositionX` value. The material holds the state at the playhead;
the keyframe list holds the animation. A reader that ignores keyframes still gets
a sensible still frame.

### Property vocabulary

The complete set of `property_type` strings is in `videoeditor.dll` — 184 of
them. Grouped:

* **Geometry** — `KFTypePositionX/Y`, `KFTypeScaleX/Y`, `KFTypeScale`,
  `KFTypeRotation`, `KFTypeAlpha`, `KFTypeGlobalAlpha`, `KFTypeZoom`,
  `KFTypeCornerPinUpLeft/UpRight/DownLeft/DownRight`, `KFTypeSmartMotion`,
  `KFTypeObjectLocked`.
* **Audio** — `KFTypeVolume`, `KFTypeLastVolume`, `KFTypeFade`,
  `KFTypeAudioPanning`.
* **Basic colour** — `KFTypeBrightness`, `KFTypeContrast`, `KFTypeSaturation`,
  `KFTypeNaturalSaturation`, `KFTypeHue`, `KFTypeTemperature`, `KFTypeTone`,
  `KFTypeHightLight` (sic), `KFTypeShadow`, `KFTypeWhite`, `KFTypeBlack`,
  `KFTypeSharpen`, `KFTypeClear`, `KFTypeVignetting`, `KFTypeParticle`,
  `KFTypeLightSensatione` (sic), `KFTypeLUT`, `KFTypeLUTSkin`, `KFTypeFilter`.
* **Advanced colour** — `KFTypeColorCorrect`, `KFTypeColorDehaze`,
  `KFTypeColorLevel`, `KFTypeColorMatch`, `KFTypeColorSoften`,
  `KFTypeColorRestoreDetail`, `KFTypeColorBacklightCorrection`,
  `KFTypeSmartColorAdjust`, `KFTypeSkinToneCorrection`,
  `KFTypePrimaryColorWheelIntensity`, `KFTypeLogColorWheelIntensity`,
  `KFTypeHslHueIntensity`, `KFTypeHslSaturationIntensity`,
  `KFTypeHslLightnessIntensity`.
* **Chroma key** — `KFTypeChromaIntensity`, `KFTypeChromaShadow`,
  `KFTypeChromaSpill`, `KFTypeChromaEdgeSmooth`.
* **Masks** — two families. `KFTypeMask*` (`MaskPostionX/Y` — note the
  misspelling — `MaskSizeX/Y`, `MaskRotation`, `MaskFeather`, `MaskRoundCorner`,
  `MaskStroke*` ×7, `MaskShadow*` ×5) for the legacy mask, and
  `KFTypeCommonMask*` (`PositionX/Y`, `SizeWidth/Height`, `Rotation`, `Feather`,
  `Expansion`, `RoundCorner`, `Scale`) for the current one.
  `KFTypeAdjustColorMask*` is the mask on an adjustment layer. `KFTypeContourPath`
  animates a freehand mask outline.
* **Text and shapes** — `KFTypeTextColor`, `KFTypeTextAlpha`,
  `KFTypeTextGradient{Color,Angle,Alpha,Percent}`, `KFTypeTextCurveAngle`,
  `KFTypeTextInstanceOpacity`, `KFTypeBorder{Color,Width,Alpha,Mode}`,
  `KFTypeBackground{Color,Alpha,Width,Height,OffsetX,OffsetY,RoundRadius,Stroke,Style}`,
  `KFTypeShadow{Color,Alpha,Angle,Distance,Point,Smoothing}`, `KFTypeBloom*` ×6,
  and 30-odd `KFTypeShape*` for the vector-shape tool.
* **Body/face retouch** — `KFTypeSlim`, `KFTypeStretch`.
* **Video decoration** — `KFTypeVideoRoundness`, `KFTypeVideoShadow*` ×5.
* **Light source** — seven `KFTypeLightSource*` properties: `Color`,
  `Intensity`, `Radius`, `PostionX`, `PostionY`, `DistanceZ`,
  `SpecularIntensity`.
* **Effect parameters** — `KFTypeEffectAdjustParam1/2/3`. Only three generic
  slots, which is the mechanism by which a downloaded effect's sliders get
  keyframed.
* **Sentinels** — `KFTypeNone`, `KFTypeGraph`, `KFTypeVoH`.

The engine also exports these as C data symbols from `videoeditor.dll` under a
`VKFF` prefix (`VKFFPosition`, `VKFFScale`, `VKFFCommonMaskSize`,
`VKFFEffectAdjustParam1`, …, terminated by `VKFFEnd`), which is the internal
enum-to-string table.

### `keyframe_refs` and the top-level `keyframes` — the older mechanism

The draft's top-level `keyframes` object has eight buckets — `videos`, `texts`,
`audios`, `effects`, `filters`, `stickers`, `adjusts`, `handwrites` — and
segments carry a `keyframe_refs` array of ids into them. All were empty in all 38
drafts, so I have no example. The class behind the `videos` bucket,
`lvve::KeyframeVideo`, has one **named field per property** rather than a
`property_type` discriminator:

```
position scale rotation alpha volume last_volume fade graph
brightness_value contrast_value saturation_value naturalSaturation_value
temperature_value tone_value highlight_value shadow_value white_value black_value
sharpen_value clear_value vignetting_value particle_value light_sensation_value
lut_value filter_value color_correct_value color_dehaze_value color_levels_value
color_match_value color_soften_value restore_detail_value backlight_correction_value
skin_tone_correction_value smart_color_adjust_value
primary_color_wheels_intensity log_color_wheels_intensity
chroma_intensity chroma_shadow mask_config
figure_slim figure_stretch figure_zoom
effect_adjust_param_1 effect_adjust_param_2 effect_adjust_param_3
```

So the older format was a fixed struct of optional per-property tracks, and the
newer one is a generic list keyed by an enum string. `KFTypeGraph` and the
`graph` field are the shared-easing-curve mechanism in both. A CapCut importer
must handle both; a writer should emit `common_keyframes` only.

---

## What chukcut should take, and what it should not

Take: microsecond times quantised to frames; the material pool addressed by id;
`target_timerange` / `source_timerange` as independent ranges; one animation
material per segment holding an ordered list; the generic keyframe list keyed by
a property enum with a `values` array; writing the current value back into the
material so a non-animating reader still renders correctly; storing the
catalogue id of a downloaded resource alongside (not instead of) its local path.

Do not take: crop on the material instead of the segment; the dual
`render_index` / `track_render_index` scheme; emitting a `speeds`,
`placeholder_infos`, `sound_channel_mappings` and `vocal_separations` object for
every segment whether used or not; three magic `EffectAdjustParam` keyframe slots
instead of parameters addressed by name; absolute local paths in the project
file; and the two coexisting keyframe representations.
