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

Still to do on the TypeScript side: `TransitionMaterial`, `TransitionKind` and
`TransitionDirection` in `src/modules/project/types.ts`, plus `transitions:
TransitionMaterial[]` on `MaterialPool`; the three new `EditCommand` variants in
`src/modules/timeline/lib/api.ts` for anything that wants to send them
directly; a marker at the cut whose width is the transition duration and whose
drag calls `transitions_retime`; a picker fed by the catalog; and a gesture on a
clip's left edge that calls `transitions_add`.

---

## The compositor patch

Not yet applied. `render/compositor.rs` was owned by other work while this
landed, so the change is written out here rather than half-done in the file.
Line numbers will have drifted — the compositor has since grown planar sources,
chroma views and rotation — but nothing below depends on those.

Everything it needs already exists and is tested: `transitions::instant_for`
resolves the pair, and `TransitionPipeline` blends two views.

### 1. Imports

```rust
use crate::modules::transitions::{self, TransitionParams, TransitionPipeline};
```

### 2. `Compositor` gains one lazily built pipeline

Next to `nv12`:

```rust
    /// The transition blend pass, built on first use.
    ///
    /// Lazy for the reason `nv12` is: a project with no transitions should not
    /// pay five shader compiles.
    transitions: OnceLock<TransitionPipeline>,
```

`transitions: OnceLock::new()` in `with_config`'s `Self { .. }`, and an accessor
beside `nv12_converter`:

```rust
    fn transition_pipeline(&self) -> &TransitionPipeline {
        self.transitions
            .get_or_init(|| TransitionPipeline::new(&self.ctx, self.config.format))
    }
```

### 3. `Draw` becomes an enum

Replacing the struct at the bottom of the file:

```rust
/// One entry in the painter's-algorithm loop.
enum Draw {
    /// One segment as one textured quad.
    Quad {
        frame: SourceFrame,
        placement: QuadPlacement,
    },
    /// Two segments blended by a transition, as one full-canvas pass.
    ///
    /// Either side may be `None` — a provider with no frame for it, or an audio
    /// material — in which case that layer is transparent and the transition
    /// fades from or to nothing. That is what a missing clip should look like,
    /// rather than a hard error in the middle of a cut.
    Transition {
        from: Option<(SourceFrame, QuadPlacement)>,
        to: Option<(SourceFrame, QuadPlacement)>,
        params: TransitionParams,
    },
}
```

### 4. `collect_draws` — the hook

The loop header takes the track it was already given, and gains three lines:

```rust
-        for (_, segment) in layout::visible_segments(project, time) {
+        for (track, segment) in layout::visible_segments(project, time) {
+            // A segment inside a live transition is drawn as the pair, not on
+            // its own. Exactly one of the two clips contains any instant of the
+            // window — the cut is the boundary between them — so no frame ever
+            // collects the same transition twice.
+            if let Some(instant) =
+                transitions::instant_for(track, &project.materials, segment, time)
+            {
+                draws.push(self.collect_transition(project, &instant, time, size, sources)?);
+                continue;
+            }
```

and the tail of the loop body:

```rust
-            draws.push(Draw { frame, placement });
+            draws.push(Draw::Quad { frame, placement });
```

### 5. Two new methods beside `collect_draws`

```rust
    /// Both sides of a transition, each as its own would-be quad.
    fn collect_transition(
        &self,
        project: &Project,
        instant: &transitions::TransitionInstant<'_>,
        time: Micros,
        size: (u32, u32),
        sources: &dyn SourceProvider,
    ) -> Result<Draw> {
        Ok(Draw::Transition {
            from: self.collect_layer(project, &instant.from, time, size, sources)?,
            to: self.collect_layer(project, &instant.to, time, size, sources)?,
            params: TransitionParams::from(instant),
        })
    }

    /// One side of a transition.
    ///
    /// The same work as the per-segment body of `collect_draws`, with one
    /// difference that is the whole point: the source instant comes from the
    /// transition, not from `Segment::source_time_at`, because for half the
    /// window it lies outside the segment's own range.
    ///
    /// The transform is still sampled at the real timeline instant.
    /// `animated_transform` clamps to the first and last keyframe, so a
    /// borrowed frame holds the pose the clip was in at its boundary — which is
    /// what it should do, and what makes this correct without a special case.
    fn collect_layer(
        &self,
        project: &Project,
        layer: &transitions::TransitionLayer<'_>,
        time: Micros,
        size: (u32, u32),
        sources: &dyn SourceProvider,
    ) -> Result<Option<(SourceFrame, QuadPlacement)>> {
        if layer.kind == MaterialKind::Audio {
            return Ok(None);
        }
        let segment = layer.segment;
        let request = SourceRequest {
            material_id: &segment.material_id,
            kind: layer.kind,
            source_time: layer.source_time,
            segment_id: &segment.id,
            max_size: size,
        };
        let frame = match sources.frame(&self.ctx, &request) {
            Ok(Some(frame)) => frame,
            Ok(None) => return Ok(None),
            Err(e) => {
                if self.config.strict_sources {
                    return Err(RenderError::Source {
                        material_id: segment.material_id.clone(),
                        source_time: layer.source_time,
                        source: e,
                    });
                }
                tracing::warn!(
                    segment = %segment.id,
                    material = %segment.material_id,
                    error = %e,
                    "skipping a transition layer: source unavailable"
                );
                return Ok(None);
            }
        };
        let canvas = (project.canvas.width, project.canvas.height);
        let transform = layout::animated_transform(segment, time);
        Ok(layout::place_quad(canvas, frame.size(), &transform, segment.crop)
            .map(|placement| (frame, placement)))
    }
```

### 6. `render_to_texture`

The uniform-writing loop tolerates both variants. A transition needs no quad
block, but the block index has to stay in step with the draw index or every
dynamic offset after the first transition is wrong:

```rust
        for (i, draw) in draws.iter().enumerate() {
            let block = match draw {
                Draw::Quad { frame, placement } => QuadUniform { /* as today */ },
                Draw::Transition { .. } => QuadUniform::zeroed(),
            };
            self.ctx.queue().write_buffer(
                uniform_buffer,
                i as u64 * stride,
                bytemuck::bytes_of(&block),
            );
        }
```

`source_groups` becomes `Vec<Option<wgpu::BindGroup>>`, `None` for transitions.

Immediately before `let mut encoder = ...`:

```rust
        // A transition needs each of its two sides as a whole layer before it
        // can blend them, because a wipe, a slide and a zoom are defined
        // against the *frame* and not against the clip: a wipe across a clip
        // scaled to a third of the canvas and rotated is not a wipe, and nobody
        // would recognise it as one.
        //
        // Two extra canvas-sized targets per transition per frame, out of the
        // same pool everything else comes from.
        let mut layers: Vec<Option<(PooledTexture, PooledTexture)>> =
            Vec::with_capacity(draws.len());
        for draw in &draws {
            match draw {
                Draw::Quad { .. } => layers.push(None),
                Draw::Transition { from, to, .. } => layers.push(Some((
                    self.render_layer(size, from.as_ref())?,
                    self.render_layer(size, to.as_ref())?,
                ))),
            }
        }

        let blend = draws
            .iter()
            .any(|d| matches!(d, Draw::Transition { .. }))
            .then(|| self.transition_pipeline());
        let layer_groups: Vec<Option<wgpu::BindGroup>> = layers
            .iter()
            .map(|pair| {
                pair.as_ref().map(|(a, b)| {
                    blend
                        .expect("a layer pair exists only for a transition draw")
                        .bind_layers(&self.ctx, a.view(), b.view())
                })
            })
            .collect();
```

The pass loop branches. The three `set_*` calls move inside the `Quad` arm
because a transition draw changes the pipeline; re-setting them per quad costs
nothing measurable and is clearer than tracking which state is still bound:

```rust
            for (i, draw) in draws.iter().enumerate() {
                match draw {
                    Draw::Quad { .. } => {
                        pass.set_pipeline(&self.pipeline);
                        pass.set_vertex_buffer(0, self.vertices.slice(..));
                        pass.set_index_buffer(
                            self.indices.slice(..),
                            wgpu::IndexFormat::Uint16,
                        );
                        pass.set_bind_group(
                            0,
                            &uniform_group,
                            &[i as u32 * self.uniform_stride],
                        );
                        pass.set_bind_group(
                            1,
                            source_groups[i].as_ref().expect("a quad has a source"),
                            &[],
                        );
                        pass.draw_indexed(0..QUAD_INDICES.len() as u32, 0, 0..1);
                    }
                    Draw::Transition { params, .. } => {
                        // Same pass, same attachment, same blend state, at its
                        // own place in the painter's order — so a transition
                        // composites onto the tracks beneath it exactly as a
                        // clip would, and nothing about this module's
                        // single-pass structure has to change.
                        blend.expect("a transition draw exists").draw(
                            &self.ctx,
                            &mut pass,
                            i as u32,
                            params,
                            layer_groups[i].as_ref().expect("built above"),
                        );
                    }
                }
            }
```

After `self.ctx.queue().submit(...)`, give the layer textures back:

```rust
        for (a, b) in layers.into_iter().flatten() {
            self.pool.release(a);
            self.pool.release(b);
        }
```

### 7. `render_layer`

```rust
    /// Composite one side of a transition into a canvas-sized target of its own.
    ///
    /// Cleared to **transparent**, not to the project background. The two
    /// layers are blended and only then composited over whatever is beneath, so
    /// a background in each of them would be composited twice and the letterbox
    /// bars would come out opaque.
    ///
    /// `None` draws nothing and the clear is the whole result.
    fn render_layer(
        &self,
        size: (u32, u32),
        layer: Option<&(SourceFrame, QuadPlacement)>,
    ) -> Result<PooledTexture> {
        // One pooled target from `self.target_key(size)`, one render pass with
        // `LoadOp::Clear(wgpu::Color::TRANSPARENT)`, one quad draw with the
        // existing pipeline. Use a second `Mutex<Scratch>` (`layer_uniforms`)
        // rather than `self.uniforms`, so a layer's uniform block cannot
        // collide with the frame's own.
        …
    }
```

### Cost, and the alternative that was rejected

Two extra canvas-sized pooled targets per transition per frame, plus one
fullscreen pass. Transitions are rare in a frame — at most one per track — and
the textures come out of the pool, so the steady-state allocation is zero.

Blending inside the quad pass, in quad space, would avoid both. It was rejected
because a wipe, a slide and a zoom are defined against the frame rather than
against the clip: performed in the local space of a clip that has been scaled
and rotated, a wipe is not a wipe. Doing it in canvas space is what makes the
five shaders mean what their names say.
