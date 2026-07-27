/**
 * Audio fades, expressed as `Volume` keyframes.
 *
 * A fade is not its own concept in the document — it is the oldest trick in
 * the schema: a fade-in is a `Volume` keyframe pair `(0, 0) → (length, 1)`,
 * a fade-out the mirror of it against the clip's end. Both mixers already
 * sample the `Volume` track per frame (`audio/mixer.rs` for the preview,
 * `export/audio.rs` for the export, each with a test that a keyframed fade
 * shapes the samples), so drawing the handles and writing the keyframes is the
 * entire feature: playback and export follow without knowing fades exist.
 *
 * The keyframe value is a *multiplier* on the segment's own volume, so `1` is
 * neutral — a fade on a quietened clip fades to its set loudness, not past it.
 *
 * Everything here builds commands; nothing mutates the document. The fade
 * handles own exactly the keyframes the fade pattern implies and leave any
 * hand-authored volume automation between them alone.
 */

import { runEdit } from "@/modules/project/store";
import type { Keyframe, Micros, Segment } from "@/modules/project/types";
import { type EditCommand, timelineApply } from "@/modules/timeline/lib/api";

export interface Fades {
  /** Length of the fade-in, from the clip's start. Zero is "no fade". */
  fadeIn: Micros;
  /** Length of the fade-out, ending at the clip's end. Zero is "no fade". */
  fadeOut: Micros;
}

function volumeTrack(segment: Segment): Keyframe[] {
  return segment.keyframes.find((track) => track.property === "volume")?.keyframes ?? [];
}

/**
 * Read the fades a clip's volume keyframes spell.
 *
 * Recognised by pattern, not by side-channel state: a fade-in is a keyframe at
 * time 0 with value 0 ramping up to the next keyframe, a fade-out a keyframe
 * at the clip's end with value 0 ramped into from the one before. Anything
 * else on the track — hand-authored automation from the inspector — reads as
 * no fade on that side, and the handles then start from zero rather than
 * misrepresenting a curve they do not understand.
 */
export function fadesOf(segment: Segment): Fades {
  const keyframes = volumeTrack(segment);
  const duration = segment.target_range.duration;
  let fadeIn = 0;
  let fadeOut = 0;

  if (keyframes.length >= 2) {
    const first = keyframes[0];
    const second = keyframes[1];
    if (first.time === 0 && first.value === 0 && second.value > 0) {
      fadeIn = Math.min(second.time, duration);
    }
    const last = keyframes[keyframes.length - 1];
    const previous = keyframes[keyframes.length - 2];
    if (last.time === duration && last.value === 0 && previous.value > 0) {
      fadeOut = duration - previous.time;
    }
  }

  return { fadeIn, fadeOut };
}

/** Keep the two fades inside the clip and out of each other's way. */
export function clampFades(duration: Micros, fades: Fades): Fades {
  const fadeIn = Math.max(0, Math.min(Math.round(fades.fadeIn), duration));
  const fadeOut = Math.max(0, Math.min(Math.round(fades.fadeOut), duration - fadeIn));
  return { fadeIn, fadeOut };
}

/**
 * The volume keyframes a pair of fades spells, in time order.
 *
 * When the two ramps meet in the middle they share their apex — two keyframes
 * on one time is something the document refuses, and one apex is also what the
 * user drew.
 */
export function fadeKeyframes(duration: Micros, fades: Fades): Keyframe[] {
  const { fadeIn, fadeOut } = clampFades(duration, fades);
  const keyframes: Keyframe[] = [];
  if (fadeIn > 0) {
    keyframes.push(
      { time: 0, value: 0, easing: "linear" },
      { time: fadeIn, value: 1, easing: "linear" },
    );
  }
  if (fadeOut > 0) {
    const apex = duration - fadeOut;
    if (!(fadeIn > 0 && apex === fadeIn)) {
      keyframes.push({ time: apex, value: 1, easing: "linear" });
    }
    keyframes.push({ time: duration, value: 0, easing: "linear" });
  }
  return keyframes;
}

const sameKeyframe = (a: Keyframe, b: Keyframe) =>
  a.time === b.time && a.value === b.value && a.easing === b.easing;

/**
 * The edit that takes a clip from the fades it has to the fades it should
 * have, as one undo step.
 *
 * Built as a diff between the keyframes the current fades own and the ones the
 * next fades imply: removals first, then additions, so a retimed fade never
 * collides with its own old keyframe. Keyframes the fade pattern does not own
 * are left untouched. Dragging a fade back to zero length therefore removes
 * its keyframes outright — no zero-length ramp is left behind to show the
 * property as animated.
 *
 * Null when nothing changes, which is a released handle that never moved.
 */
export function fadeCommand(segment: Segment, next: Fades): EditCommand | null {
  const duration = segment.target_range.duration;
  const current = fadeKeyframes(duration, fadesOf(segment));
  const wanted = fadeKeyframes(duration, next);
  const existing = volumeTrack(segment);

  const removals: EditCommand[] = current
    .filter((keyframe) => !wanted.some((other) => sameKeyframe(other, keyframe)))
    // Only remove what is really on the track, exactly as it is there — the
    // command carries the whole keyframe so undo puts it back byte-exact.
    .flatMap((keyframe) => {
      const found = existing.find((other) => sameKeyframe(other, keyframe));
      return found
        ? [
            {
              type: "remove_keyframe",
              segment_id: segment.id,
              property: "volume",
              keyframe: found,
            } as const,
          ]
        : [];
    });

  const kept = existing.filter(
    (keyframe) =>
      !removals.some((r) => r.type === "remove_keyframe" && sameKeyframe(r.keyframe, keyframe)),
  );
  const additions: EditCommand[] = wanted
    .filter((keyframe) => !kept.some((other) => sameKeyframe(other, keyframe)))
    // A hand-authored keyframe already on the target time would collide, and
    // Rust refuses the whole composite — so the fade yields and keeps it.
    .filter((keyframe) => !kept.some((other) => other.time === keyframe.time))
    .map((keyframe) => ({
      type: "add_keyframe",
      segment_id: segment.id,
      property: "volume",
      keyframe,
    }));

  const commands = [...removals, ...additions];
  if (commands.length === 0) return null;
  if (commands.length === 1) return commands[0];
  return { type: "composite", label: "Fade", commands };
}

/** Write a clip's fades through the ordinary undoable edit path. */
export function applyFades(segment: Segment, next: Fades): Promise<boolean> {
  const command = fadeCommand(segment, next);
  if (!command) return Promise.resolve(false);
  return runEdit(() => timelineApply(command));
}
