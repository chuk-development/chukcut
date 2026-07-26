/**
 * The keyframe edit commands.
 *
 * These are `EditCommand` variants that Rust does not have yet. They are
 * declared here, in the module that needs them, in the same shape the rest of
 * the enum crosses the boundary in — `#[serde(tag = "type", rename_all =
 * "snake_case")]`, both sides of the change in every variant so `invert()` is
 * mechanical. Once `src-tauri/src/modules/timeline/ops.rs` grows them, this
 * union folds into `EditCommand` in `@/modules/timeline/lib/api` and
 * `keyframeApply` loses its cast.
 *
 * What Rust has to do with each of them is written on the variant, because that
 * contract is the whole reason for defining them here rather than composing
 * something out of `SetTransform`: a composite of transform edits would apply
 * the same picture and then lose the animation on undo.
 *
 * The composite variant is repeated rather than reused because one *gesture* on
 * a row that drives two tracks — the scale control, which writes `scale_x` and
 * `scale_y` — still has to be one entry on the undo stack.
 */

import { runEdit } from "@/modules/project/store";
import type { AnimatableProperty, Easing, Id, Keyframe, Micros } from "@/modules/project/types";
import { type EditCommand, type EditResponse, timelineApply } from "@/modules/timeline/lib/api";

export type KeyframeEditCommand =
  /**
   * Insert `keyframe` into the segment's track for `property`, creating the
   * track when the property is not animated yet, keeping `keyframes` sorted by
   * time. Rejected when a keyframe already exists at that exact time — the UI
   * sends `move_keyframe` for that, so a collision here is a bug.
   * Inverts to `remove_keyframe` with the same keyframe.
   */
  | { type: "add_keyframe"; segment_id: Id; property: AnimatableProperty; keyframe: Keyframe }
  /**
   * Remove the keyframe at `keyframe.time`. Carries the whole keyframe — value
   * and easing included — because undo has to put it back exactly. Drops the
   * `KeyframeTrack` when it empties, so "is animated" stays "a track exists".
   * Inverts to `add_keyframe`.
   */
  | { type: "remove_keyframe"; segment_id: Id; property: AnimatableProperty; keyframe: Keyframe }
  /**
   * Retime and/or revalue the keyframe at `from_time`. Easing is untouched.
   * Rejected when another keyframe already sits at `to_time`. One of these is
   * emitted per completed drag, never per pointer move; `from_time == to_time`
   * is the ordinary case of editing a value while the playhead sits on a
   * keyframe. Inverts to itself with the two sides swapped.
   */
  | {
      type: "move_keyframe";
      segment_id: Id;
      property: AnimatableProperty;
      from_time: Micros;
      to_time: Micros;
      before_value: number;
      after_value: number;
    }
  /**
   * Change the easing the keyframe at `time` uses to reach the next one.
   * Inverts to itself with the two sides swapped.
   */
  | {
      type: "set_keyframe_easing";
      segment_id: Id;
      property: AnimatableProperty;
      time: Micros;
      before: Easing;
      after: Easing;
    }
  | { type: "composite"; label: string; commands: KeyframeEditCommand[] };

/**
 * Send a keyframe command.
 *
 * The cast is the one place this module admits that Rust cannot answer yet: the
 * payload is well-formed for the enum as it will be, and `timeline_apply` will
 * reject it as an unknown variant until the variants land. Deleting the cast is
 * the entire frontend side of that change.
 */
export function keyframeApply(command: KeyframeEditCommand): Promise<EditResponse> {
  return timelineApply(command as unknown as EditCommand);
}

/** Send a keyframe command and fold the new document into the store. */
export function runKeyframeEdit(command: KeyframeEditCommand): Promise<boolean> {
  return runEdit(() => keyframeApply(command));
}

/**
 * One command for a gesture that touches several tracks.
 *
 * The scale control is one slider over two properties. Two commands would be
 * two undo steps for one drag, and undoing half of a uniform scale leaves a
 * clip stretched.
 */
export function forProperties(
  label: string,
  properties: AnimatableProperty[],
  build: (property: AnimatableProperty) => KeyframeEditCommand,
): KeyframeEditCommand {
  const commands = properties.map(build);
  return commands.length === 1 ? commands[0] : { type: "composite", label, commands };
}
