/**
 * Typed wrappers around the `timeline_*` commands, and the `EditCommand` union.
 *
 * `EditCommand` mirrors the Rust enum in
 * `src-tauri/src/modules/timeline/ops.rs`, which is serialized with
 * `#[serde(tag = "type", rename_all = "snake_case")]`: the variant name lands
 * in a `type` field in snake_case, the variant's own fields sit alongside it.
 * Every variant carries *both* sides of the change so Rust can invert it — a
 * move sends where it came from as well as where it went, even though applying
 * the move only needs the destination.
 */

import { invoke } from "@tauri-apps/api/core";

import type {
  Id,
  Micros,
  Project,
  Segment,
  TimeRange,
  Track,
  Transform,
} from "@/modules/project/types";

/**
 * The per-track switches, snapshotted together.
 *
 * They travel as one payload because the UI toggles them from one row of
 * controls, so one undo step per click is the honest granularity.
 */
export interface TrackFlags {
  muted: boolean;
  locked: boolean;
  hidden: boolean;
  volume: number;
}

export type EditCommand =
  | { type: "add_track"; track: Track; index: number }
  | { type: "remove_track"; track: Track; index: number }
  | { type: "insert_segment"; track_id: Id; segment: Segment; index: number }
  | { type: "remove_segment"; track_id: Id; segment: Segment; index: number }
  | {
      type: "move_segment";
      segment_id: Id;
      from_track: Id;
      to_track: Id;
      from_start: Micros;
      to_start: Micros;
    }
  | {
      type: "trim_segment";
      segment_id: Id;
      before_target: TimeRange;
      before_source: TimeRange;
      after_target: TimeRange;
      after_source: TimeRange;
    }
  | { type: "set_transform"; segment_id: Id; before: Transform; after: Transform }
  | { type: "set_speed"; segment_id: Id; before: number; after: number }
  | { type: "set_volume"; segment_id: Id; before: number; after: number }
  | { type: "set_track_flags"; track_id: Id; before: TrackFlags; after: TrackFlags }
  /**
   * Put a clip into a link group, or take it out of one. `null` on both sides
   * is how Rust spells `Option<Id>`; `before` is checked against the document,
   * so a stale panel is refused rather than obeyed.
   */
  | { type: "set_link_group"; segment_id: Id; before: Id | null; after: Id | null }
  | { type: "composite"; label: string; commands: EditCommand[] };

/** What every mutating command answers with: the whole document plus history state. */
export interface EditResponse {
  project: Project;
  can_undo: boolean;
  can_redo: boolean;
  undo_label: string | null;
  redo_label: string | null;
}

export function timelineApply(command: EditCommand): Promise<EditResponse> {
  return invoke<EditResponse>("timeline_apply", { command });
}

/**
 * One edit per clip, applied as a single undo step.
 *
 * What a multi-selection sends. Deliberately *not* a `composite` through
 * `timeline_apply`: Rust has to expand the link partners of every part and put
 * the parts in an order no intermediate state rejects, and it will not do
 * either of those things to a composite the webview built — see `compose_edits`
 * in `timeline/ops.rs`. The label is ours because only this side knows whether
 * four removals were a delete or the tail of a cut.
 */
export function timelineApplyMany(commands: EditCommand[], label: string): Promise<EditResponse> {
  return invoke<EditResponse>("timeline_apply_many", { commands, label });
}

export function timelineSplit(segmentId: Id, at: Micros): Promise<EditResponse> {
  return invoke<EditResponse>("timeline_split", { segmentId, at });
}

/**
 * Break the group a clip belongs to, releasing every member.
 *
 * Rust builds the composite, because which clips are in the group is a fact
 * about the document rather than about what the user could see.
 */
export function timelineUnlink(segmentId: Id): Promise<EditResponse> {
  return invoke<EditResponse>("timeline_unlink", { segmentId });
}

/** Make several clips move, trim and delete as one. */
export function timelineLink(segmentIds: Id[]): Promise<EditResponse> {
  return invoke<EditResponse>("timeline_link", { segmentIds });
}

export function timelineUndo(): Promise<EditResponse> {
  return invoke<EditResponse>("timeline_undo");
}

export function timelineRedo(): Promise<EditResponse> {
  return invoke<EditResponse>("timeline_redo");
}
