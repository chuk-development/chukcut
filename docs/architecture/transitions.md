# Transitions

The effect between two adjacent clips. Code lives in
`src-tauri/src/modules/transitions/`; the on-disk shape is `TransitionMaterial`
in `src-tauri/src/modules/project/document.rs`.

This document exists because two of the decisions below are cheap to make and
expensive to revisit, and because the compositor change they imply had not
landed when the rest of the feature did — the patch is spelled out at the bottom
so whoever picks up `render/` does not have to rederive it.

## What a transition is, in time

```text
           outgoing clip                 incoming clip
  ┌──────────────────────────┬────────────────────────────┐
  │                          │                            │
  └──────────────────────────┼────────────────────────────┘
                             cut
                   ├──── window ────┤
                 cut-d/2          cut+d/2
           progress 0 ────────────► 1
```

A transition is **centred on the cut** and **neither clip moves**.

The alternative — the two clips genuinely overlapping in time, which is CapCut's
`is_overlap` (see `docs/research/draft-format.md`) — is not available to us.
"Segments within a track never overlap" is an invariant the whole editing model
rests on (`timeline-editing.md`), and honouring an overlap would mean shortening
the timeline and shifting everything downstream every time a transition's length
changed. A user nudging a transition handle would watch the rest of their edit
slide.

Centring instead means each clip contributes frames from *beyond* its trimmed
boundary for half the duration: the outgoing clip is read past its out point and
the incoming one before its in point, out of the handles the trim left behind.
`resolve::extended_source_time` is `Segment::source_time_at` without the
containment check — the same arithmetic, including speed, minus the refusal.
Where there is no handle the borrowed frame is clamped into the material's real
extent, so it freezes on the boundary rather than asking a decoder for a
negative timestamp. That is what an editor does when it says "insufficient
media", and it is better than refusing to place the transition at all.

**The window start is not stored.** It is derived from the cut, which is
`incoming.target_range.start`. That is the single reason trimming and moving
either clip carry the transition correctly for free: there is no second copy of
the truth to fall out of date. The clamp is applied to each half independently
(`window_for`), so an over-long transition becomes asymmetric rather than
refusing to render — a legal trim can shorten a clip under a transition that was
legal when it was placed, and the frame still has to come out.

At any instant of the window, **exactly one of the two clips contains it**,
because the cut is the boundary between them. That is what makes the
compositor's hook a single `if`: the segment it was already about to draw either
takes part in a live transition or does not, and no frame can collect the same
transition twice.

## Where it lives, and why

Three decisions, in the order they constrain each other.

### It is a material, not an entity of its own

The obvious alternative is a `Vec<Transition>` on `Track`, each row naming the
two segments it joins. Rejected: a side table that names segments is a *second*
place segment ids appear, so every structural edit — remove, move, split, ripple
— has to remember to fix it up, and the one that forgets leaves a row pointing
at a segment that no longer exists. Hanging the reference on the segment means a
segment carries its transition with it through every edit, and `RemoveSegment`
— which already snapshots the whole `Segment` — undoes the removal of both
without knowing that transitions exist.

The parameters live in the pool rather than inline on the segment for the same
reason a video's do: the pool is where things that can be enumerated and shared
live, and it keeps the segment schema fixed.

### The incoming clip owns it, not the outgoing one

CapCut hangs its transition off the **left** (outgoing) segment. We hang it off
the **right** (incoming) one. The reason is splitting.

`split_at` trims the original in place and inserts a fresh clone for the
remainder. With left-ownership, splitting the outgoing clip leaves the
transition on the half that no longer touches the cut, so the split composite
grows an extra command to move it — and that command has to be invertible, and
has to not fire when the clip had no transition. With right-ownership the clone
is by construction a clip whose left edge is a brand new cut with nothing on it,
so the whole fix is one unconditional line in `split_at`:

```rust
right.extras.retain(|id| project.materials.transition(id).is_none());
```

and splitting the *outgoing* clip needs nothing at all, because the transition
sits on a segment the split never touched.

Read the ownership as a preposition: a transition describes how its segment is
*entered*.

### The id sits in `Segment::extras`

`Segment::extras` is a list of material ids with no type tag; the kind of an id
is whichever pool category it resolves in. That trick is CapCut's, and
`draft-format.md` already recommends stealing it. Its payoff is visible here and
worth recording as evidence rather than as a claim: adding transitions to this
format changed **no existing segment, no existing constructor and no existing
test**. Thirteen files construct `Segment { .. }` literally, four of them in
modules owned by other work; a new required field would have touched all of
them.

The cost is that a corrupted id silently drops the transition, which is exactly
what `validate()` is built to catch.

`MaterialPool::kind_of` deliberately does **not** classify a transition id. A
`MaterialKind` is something that can be *placed* on a track, and a transition
cannot be; callers ask `MaterialPool::transition` or `transition_of`.

## The interface the renderer sees

```rust
// Asked once per segment the compositor was already going to draw.
transitions::instant_for(track, &project.materials, segment, time)
    -> Option<TransitionInstant>

pub struct TransitionInstant<'a> {
    pub material: &'a TransitionMaterial,
    pub window: TimeRange,
    pub linear: f32,    // position in the window, before easing
    pub progress: f32,  // what the shader gets: linear through Easing
    pub from: TransitionLayer<'a>,
    pub to: TransitionLayer<'a>,
}

pub struct TransitionLayer<'a> {
    pub segment: &'a Segment,
    pub kind: MaterialKind,
    pub source_time: Micros,  // extended past the segment's own range
}
```

`linear` is kept alongside `progress` because a test that only ever sees the
eased value cannot tell an easing bug from an arithmetic one, and because a UI
scrubbing a transition wants the raw position.

Progress is `(time - window.start) / window.duration`, and the window is
half-open like every other range in the document. So progress is exactly `0` at
the first instant and **never exactly `1`** — `1` is the limit, not a value any
frame is rendered at. `Easing::Hold` returning `0.0` for all inputs is a
legitimate choice and gives a hard cut at the far edge of the window.

`render::TransitionPipeline` takes **two `wgpu::TextureView`s and a progress
value** and knows nothing about segments, documents or time. Turning a segment
into pixels at a canvas position is the compositor's whole job, and a transition
that reached into it would have to duplicate transforms, crops, keyframes and
source lookup.

Two ways in:

- `draw(ctx, pass, slot, params, layers)` records into a render pass the caller
  already opened, so the transition lands at its own position in the painter's
  order, in the same pass, with the same attachment and the same blend state as
  every other layer. `slot` addresses a growable uniform buffer by dynamic
  offset — the same arrangement the quad pass uses for its per-draw blocks — so
  two transitions recorded into one pass do not overwrite each other's
  parameters.
- `blend_to_texture(...)` opens its own pass and submits, for tests and for
  anything that wants the blended result on its own.

All five pipelines are built up front from one shader module. Compiling a kind
the first time it is scrubbed over would put a shader compile inside a playback
frame, and a stutter on the first frame of a transition is exactly what a user
reads as "transitions are slow". Whether to build *any* of it is the
compositor's lazy decision.

## The shaders

`transitions/shaders/transition.wgsl`. A fullscreen triangle vertex shader and
five fragment entry points — `fs_dissolve`, `fs_dip`, `fs_wipe`, `fs_slide`,
`fs_zoom`. Direction is a uniform rather than four more entry points; a left
wipe and a right wipe are the same shader, and one expression covers all four:
`dot(uv - 0.5, travel()) + 0.5`.

Four things in there are not obvious and are the reason the file is commented at
length:

- **Every blend premultiplies, mixes, unpremultiplies.** A straight `mix()` of
  two straight-alpha colours is wrong wherever their alphas differ: a fully
  transparent pixel still carries a colour, and lerping towards it drags the
  visible pixel towards that colour instead of towards transparency. The
  letterbox bars around a 16:9 clip on a 9:16 canvas are exactly that case, so
  the bug shows up as a dark halo creeping in from the edges.
- **Dip is two halves, not a crossfade with a colour laid over it.** A crossfade
  would show both clips through the dip, which is the one thing a dip is for.
  The veil covers the whole canvas rather than just the clip, because "dip to
  black" means the frame goes black — a letterboxed clip dipping to black inside
  its own rectangle only looks like a bug.
- **Slide is a push**, and it selects by whether the sample coordinate is still
  on canvas. The sampler is `ClampToEdge`, so without that check a translated
  layer smears its border pixels across everything it has already left.
- **The wipe edge starts one softness-width off canvas** and finishes on the far
  edge. Otherwise a soft wipe shows a band of the incoming clip at progress 0
  and never quite finishes.

Colour space: the layer textures are the compositor's format, so sampling
decodes to linear light and writing encodes back. All the arithmetic is in
linear light, which is where a crossfade belongs — an sRGB-space crossfade
darkens through the middle.

The file is validated against naga 30 (the version wgpu 30 links) as well as by
the GPU tests: 6 entry points, no diagnostics.

## Edit commands

Three variants on `EditCommand`, with the bodies in `transitions::edit` so the
enum in `timeline/ops.rs` stays a table of contents.

| command | undone by |
|---|---|
| `AddTransition { segment_id, transition }` | `RemoveTransition`, same payload |
| `RemoveTransition { segment_id, transition }` | `AddTransition`, same payload |
| `SetTransition { segment_id, before, after }` | itself, with the two swapped |

`segment_id` is always the **incoming** clip. Both add and remove carry the
whole `TransitionMaterial` rather than its id, for the reason `RemoveSegment`
carries the whole segment: undo has to put back exactly what was there, and an
id cannot rebuild a colour, an easing and a direction. `SetTransition` refuses
an id change — that would be an add and a remove wearing one label, which is how
an undo stack ends up pointing at a material that was never there.

Builders in the same module: `add_command` (a default duration is *shortened* to
fit rather than rejected, because a user dropping a transition onto a
half-second clip wants a shorter transition, not an error), `remove_command`,
`retime_command`, `set_command`. Plus `allowed_duration`, which the UI asks
before it permits a drop so the gesture can be refused with a cursor rather than
with a dialog, and `detach_around`, which a ripple-delete composite prepends so
deleting a clip does not leave a transition describing a join that is about to
stop existing.

### One deliberate leak

A raw `RemoveSegment` takes the transition id with it — the id lives in
`Segment::extras` — but leaves the `TransitionMaterial` in the pool with nothing
referring to it. That is on purpose. Collecting it would mean `RemoveSegment`
carrying the material too, which either changes that command's payload for every
existing caller or makes its undo lossy. An unreferenced transition is inert:
resolution only ever goes segment → material, so nothing reads it, and it costs
about a hundred bytes. `detach_around` is the tidy path for callers that want
one.

## Validation

`Project::validate()` delegates to `transitions::validate::issues`. A transition
is the only thing in the document whose correctness depends on two segments at
once, which makes it the only thing that can be broken by an edit to something
else.

Errors — the document is internally inconsistent, so an edit command has a bug:

- no clip before this one at all (a transition on the first segment of a track);
- the previous clip no longer reaches this one, reported with the gap in µs;
- more than one transition id on one segment;
- a non-positive duration.

Warning — the document is fine and the world is awkward:

- longer than the clips it joins, with the length it will actually play. Not an
  error, because a perfectly legal trim produces it and the renderer clamps the
  window rather than misbehaving. The UI wants to say so.

Nothing renders an orphan, so the *frame* is always correct; the reason the
orphan is an error rather than a shrug is that the timeline still draws a marker
for it, and a user cannot get rid of a thing they cannot see.

## What the frontend needs

Six commands, registered in `lib.rs`:

| command | purpose |
|---|---|
| `transitions_catalog` | every kind the renderer implements, with which controls each has |
| `transitions_max_duration` | the longest transition allowed at a clip's head; `0` means none |
| `transitions_add` | place one |
| `transitions_remove` | take one off |
| `transitions_retime` | change only the length |
| `transitions_set` | replace the parameters, keeping the identity |

The mutating four build an `EditCommand` and push it through the same history as
every other edit, then answer with the whole updated project — the shape
`timeline/commands.rs` established. They exist as commands rather than leaving
the webview to construct an `AddTransition` because constructing one means
minting a uuid, knowing the default duration, and knowing how short a clip
shortens it to. That is three pieces of policy, and policy in the webview is
policy in two places.

The catalog is server-side for the same reason: a kind added to `TransitionKind`
appears in the picker without the frontend changing. Each descriptor carries
`directional`, `has_color`, `has_softness` and `has_zoom`, which are properties
of the shader — a dissolve has no direction and never will.

The TypeScript side lives in `src/modules/transitions/`. `lib/geometry.ts` is
the mirror of `resolve.rs` — window, clamping, `max_duration` — duplicated on
purpose, because the renderer must not ask the webview where to draw and the
webview must not ask Rust where to put a handle it is dragging at sixty frames a
second; the two are kept honest by being pinned to the same worked examples.
`components/TransitionLane.tsx` draws a marker over every cut on a lane — a `+`
button where there is no transition yet, a bow-tie badge over the window where
there is — and `components/TransitionInspector.tsx` is the parameter panel,
with its controls chosen by the catalogue's `has_*` flags rather than by a
second table on this side.

One seam is still open, and it is one element: `TransitionLane` has to be
mounted inside the timeline's lane, which owns the zoom and the client-x to
instant mapping it takes as props. `src/modules/timeline/` was owned by other
work when this landed. The same applies to `edits.ts::applyOverlap`, which is
the drag-a-clip-over-its-neighbour gesture: the arithmetic and the command are
here and tested, and the timeline's drag handler is where it has to be called
from — a move whose proposed start would overlap the left-hand neighbour is a
transition of that length, not a move, and the clip stays where it was.

---

## How the compositor draws one

**Applied.** `render/compositor.rs` renders transitions in both the preview and
the export; what follows is the shape of it and the two decisions that are not
obvious from reading the code. An earlier version of this section carried the
patch as a diff, written out because the file was owned by other work at the
time. That diff went stale — `QuadUniform` grew `planar`, `matrix`, `range` and
`turns`, and the source bind group grew a third entry for the chroma plane —
and a stale patch is worse than none, so it has been replaced by this.

### The shape

- **`Draw` is an enum.** `Draw::Quad(QuadDraw)` is what it always was;
  `Draw::Transition { params, from, to }` carries two of the same `QuadDraw`
  and the blend parameters. Either side may be `None` — a provider with no
  frame, an audio material, a clip faded fully out — and that side is then a
  transparent layer, so a transition with a missing clip fades from or to
  nothing instead of losing the frame.
- **A `QuadDraw` owns its uniform slot.** It used to be positional: the *n*th
  draw read the *n*th uniform block. A transition is one draw and two quads, so
  the two numbers stopped agreeing and the slot is now explicit
  (`DrawList::slots` hands them out, `DrawList::quads()` walks them in order).
  Getting this wrong shifts every dynamic offset after the first transition,
  which draws the right clips with each other's transforms.
- **`collect_draws` asks `transitions::instant_for` once per visible segment.**
  Exactly one of the two clips contains any instant of the window — the cut is
  the boundary between them — so no frame collects the same transition twice,
  and the far-side clip is not separately visible. A transition whose cut no
  longer exists resolves to `None` and the pair renders as an ordinary cut;
  the compositor does not depend on `detach_broken_transitions` having run.
- **Each side is drawn into a canvas-sized pooled target before the composite
  pass**, and the blend is then one fullscreen draw *inside* that pass, at the
  place in the painter's order the segment would have had. So a transition
  composites onto the tracks beneath it exactly as a clip would, and the
  single-pass structure is unchanged.

### Hardware-decoded sources come free, and that is the point

The layers are drawn with **the same quad pipeline and the same `QuadUniform`**
as any other clip, so `planar`, `matrix`, `range` and `turns` travel through
untouched and an NV12 surface imported from the decoder blends exactly like a
software-decoded RGBA one. Nothing in the transition path branches on how a
frame was decoded.

That is not a detail to take on trust: hardware decode is the default on any
machine that can import a decoded surface as a texture, so a transition that
only worked on the software path would fail on most real footage, and it would
fail *plausibly* — as a wrong-looking mix rather than as an error.
`a_crossfade_is_the_same_however_each_side_was_decoded` renders the same
crossfade with all four combinations of the two paths and asserts one number for
all of them.

### The layer pipeline has blending switched off, and it must

A layer holds exactly one clip over a transparent clear, so there is nothing to
blend with — and blending is not merely unnecessary here, it is wrong.
`ALPHA_BLENDING` over a transparent destination leaves **premultiplied** colour
behind, while `transition.wgsl` samples its layers as straight alpha and
premultiplies them itself. Anything not fully opaque is then multiplied by its
own coverage twice.

The symptom is specific and easy to miss: two opaque clips look perfect, and a
clip at 50% opacity *darkens the instant the transition window opens* and
brightens again as it closes. `Compositor::layer_pipeline` is the same pipeline
as `pipeline` with `blend: None`, which writes the fragment through untouched,
and `a_half_transparent_clip_does_not_change_brightness_when_the_window_opens`
fails by exactly a factor of two if it is switched back.

### Preview and export agree because the work is below both of them

The hook is inside `render_to_texture`, which `render`, `render_frame`,
`render_nv12` and `render_nv12_into` all call. The preview reads the composited
target back as RGBA; a hardware export runs the RGBA→NV12 compute pass over the
same target and never touches system memory in between. Neither knows
transitions exist.
`the_preview_and_the_export_agree_on_a_frame_mid_transition` renders three
instants of one crossfade — one side hardware-decoded, one side software —
through both entry points and compares the pixels, inverting the NV12 by hand.

### Cost, and the alternative that was rejected

Two extra canvas-sized pooled targets per transition per frame, plus one
fullscreen pass. Transitions are rare in a frame — at most one per track — and
the textures come out of the same pool everything else does, so the steady-state
allocation is zero.

Blending inside the quad pass, in quad space, would avoid both. It was rejected
because a wipe, a slide and a zoom are defined against the frame rather than
against the clip: performed in the local space of a clip that has been scaled
and rotated, a wipe is not a wipe. Doing it in canvas space is what makes the
five shaders mean what their names say.

---

## The library: gl-transitions and the seamless set

Beyond the five built-in kinds, `TransitionKind::Library` names a preset in
`transitions/library/` through `TransitionMaterial::preset` (`gl:<id>` or
`seamless:<id>`), with `params` holding only values that differ from the
preset's defaults. A preset this build does not know draws as a dissolve.

- **gl-transitions** (120 of 125; MIT, two BSD) are translated to WGSL by
  naga through the harness in `port.py`. The output is committed, so a build
  needs neither the network nor naga-cli. The harness turns each `uniform`
  into a private global set from a slot of one uniform block (so a function
  argument with a parameter's name still means what it meant), hands the
  shader premultiplied samples and unpremultiplies its result (straight
  alpha layers, the reason given above), flips GL's bottom-left texture
  origin, measures `ratio` from the layer textures, and clamps alpha before
  unpremultiplying (some transitions add to it). Each file keeps its header;
  `LICENSE-gl-transitions.md` lists authors and licences and why five are
  left out.
- **Seamless** (`seamless.wgsl`, ours): zoom in, zoom out, spin, whip pan
  and push. Both clips move as one camera move; the outgoing clip carries
  the first half and the incoming one the second, so the cut lands at peak
  speed. Motion blur is a sixteen-tap shutter whose width follows an
  ease-in-out speed curve, so the first and last frames are sharp.
  Off-frame samples mirror back in. Whip and push use the material's
  `direction`.

Every preset shares one uniform layout (`state` = progress, unused,
direction, blur; twelve `vec4` parameter slots) and the transition
pipeline's own layer bind group layout, so the compositor's transition path
is unchanged: `TransitionPipeline::draw` dispatches to a per-preset pipeline,
compiled the first time that preset is drawn. `library/gpu_tests.rs` checks
every preset compiles, starts on the outgoing layer and ends on the incoming
one; `fx/render_tests.rs` checks a library transition through the compositor
against the export.

A transition side also carries its clip's built-in effects
(`modules/fx`): each side's layer has them applied before the blend.
