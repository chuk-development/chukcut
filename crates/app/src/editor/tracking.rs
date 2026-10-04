//! Motion tracking in the player: drawing the box around an object, the live
//! box while a job runs, and the tracked box and path of the selected
//! follower. The inspector half is `inspector/tracking.rs`; everything that
//! changes the document goes through `modules::tracking::commands`.

use std::cell::Cell;

use chukcut_engine::modules::project::Segment;
use chukcut_engine::modules::tracking::commands::{self as tracking_commands, StartTracking};
use chukcut_engine::modules::tracking::edit as tracking_edit;
use chukcut_engine::modules::tracking::follow::{self, canvas_to_source, source_to_canvas};
use chukcut_engine::modules::tracking::job::Direction;
use chukcut_engine::modules::tracking::{
    FollowMode, TrackSample, TrackerKind, TrackingMaterial, LOW_CONFIDENCE,
};
use gpui::component::button::{Button, ButtonVariants as _};
use gpui::component::dialog::DialogFooter;
use gpui::component::WindowExt as _;
use gpui::{point, AnyElement, PathBuilder};

use super::*;

/// The player's tracking state, one field on the editor.
#[derive(Default)]
pub(crate) struct TrackingUi {
    /// Box selection on the player, while it is active.
    pub(crate) select: Option<BoxSelect>,
    /// The analysis that is running.
    pub(crate) job: Option<RunningJob>,
    /// How the next track is followed.
    pub(crate) mode: FollowMode,
    /// Which tracker the next track (or re-track) runs.
    pub(crate) tracker: TrackerKind,
    /// The clip chosen to track for the selected overlay; `None` is the
    /// default (the topmost video under it).
    pub(crate) target: Option<(String, String)>,
    /// Where the picture is on screen, for turning the pointer into canvas
    /// coordinates.
    pub(crate) picture: Rc<Cell<Bounds<Pixels>>>,
    /// The smoothing slider, made on first use.
    pub(crate) smoothing: Option<(
        gpui::Entity<gpui::component::slider::SliderState>,
        gpui::Subscription,
    )>,
    /// The smoothing slider is being dragged: the panel must not snap it
    /// back to the document's value until it is released.
    pub(crate) smoothing_drag: bool,
}

/// A box being drawn over the video.
pub(crate) struct BoxSelect {
    pub(crate) overlay_id: String,
    pub(crate) target_id: String,
    /// Re-track this track from the playhead instead of making a new one.
    pub(crate) retrack: Option<String>,
    /// Canvas-normalised corners (+y up), once the user has drawn one.
    pub(crate) rect: Option<([f32; 2], [f32; 2])>,
    /// The corner the drag started from, while the button is down.
    pub(crate) dragging: Option<[f32; 2]>,
}

/// A job in flight, and what the UI shows of it.
pub(crate) struct RunningJob {
    pub(crate) id: u64,
    pub(crate) target_id: String,
    pub(crate) done: u32,
    pub(crate) total: u32,
    pub(crate) latest: Option<TrackSample>,
    /// Where the playhead was when the box was drawn; it goes back there
    /// when the job ends, after following the frames being tracked.
    pub(crate) return_to: Micros,
}

impl Editor {
    // --- reading -------------------------------------------------------------------

    /// The selected clip when it can follow a track: a picture that is not
    /// itself the clip under it (text, stickers, images, overlay video).
    pub(crate) fn tracking_overlay(&self) -> Option<&Segment> {
        let (track, segment) = self.selected_segment()?;
        (track.kind != TrackKind::Audio).then_some(segment)
    }

    /// The clip tracking would analyse for `overlay`: the user's choice when
    /// it is still there, else the topmost video under the overlay.
    pub(crate) fn tracking_target(&self, overlay: &Segment) -> Option<String> {
        if let Some((_, target)) = self.followed(overlay) {
            return Some(target);
        }
        if let Some((overlay_id, target)) = &self.tracking.target {
            if overlay_id == &overlay.id && self.project.segment(target).is_some() {
                return Some(target.clone());
            }
        }
        tracking_edit::default_target(&self.project, &overlay.id, self.clock.position())
    }

    /// The track `overlay` follows and the clip it is seen through.
    pub(crate) fn followed(&self, overlay: &Segment) -> Option<(String, String)> {
        let (track, link, target) =
            tracking_edit::followed_track(&self.project, overlay, self.clock.position())?;
        Some((
            track.id.clone(),
            target.map_or(link.target_segment_id.clone(), |s| s.id.clone()),
        ))
    }

    /// Video clips that overlap `overlay` in time, for the target menu.
    pub(crate) fn tracking_candidates(&self, overlay: &Segment) -> Vec<(String, String)> {
        self.project
            .tracks
            .iter()
            .filter(|t| t.kind == TrackKind::Video)
            .flat_map(|t| t.segments.iter())
            .filter(|s| {
                s.id != overlay.id
                    && self.project.materials.video(&s.material_id).is_some()
                    && s.target_range.overlaps(&overlay.target_range)
            })
            .map(|s| (s.id.clone(), self.material_name(&s.material_id)))
            .collect()
    }

    // --- actions -------------------------------------------------------------------

    /// Enter box selection for the selected overlay. With `retrack`, the box
    /// starts as the tracked one at the playhead.
    pub(crate) fn begin_box_select(&mut self, retrack: bool, cx: &mut Context<Self>) {
        let Some(overlay) = self.tracking_overlay().cloned() else {
            return;
        };
        let Some(target_id) = self.tracking_target(&overlay) else {
            self.report(Err("there is no video under this clip to track".into()), cx);
            return;
        };
        let Some((_, target)) = self.project.segment(&target_id) else {
            return;
        };
        let target = target.clone();
        self.pause();
        // The box is drawn on the frame at the playhead, so the playhead must
        // be on the video.
        let at = self.clock.position();
        if !target.target_range.contains(at) {
            self.seek(target.target_range.start.max(overlay.target_range.start));
        }
        let mut select = BoxSelect {
            overlay_id: overlay.id.clone(),
            target_id: target_id.clone(),
            retrack: None,
            rect: None,
            dragging: None,
        };
        if retrack {
            let Some((track_id, _)) = self.followed(&overlay) else {
                return;
            };
            let at = self.clock.position();
            if let Some(object) = self
                .project
                .materials
                .tracking(&track_id)
                .and_then(|track| follow::object_through(&self.project, track, &target, at))
            {
                let xs = object.corners.map(|c| c[0]);
                let ys = object.corners.map(|c| c[1]);
                let min = [
                    xs.iter().copied().fold(f32::MAX, f32::min),
                    ys.iter().copied().fold(f32::MAX, f32::min),
                ];
                let max = [
                    xs.iter().copied().fold(f32::MIN, f32::max),
                    ys.iter().copied().fold(f32::MIN, f32::max),
                ];
                select.rect = Some((min, max));
            }
            select.retrack = Some(track_id);
        }
        self.tracking.select = Some(select);
        self.status = Some("Drag a box around the object on the player".into());
        cx.notify();
    }

    pub(crate) fn cancel_box_select(&mut self, cx: &mut Context<Self>) {
        self.tracking.select = None;
        self.status = None;
        cx.notify();
    }

    /// Start the analysis with the box that was drawn.
    pub(crate) fn start_tracking(&mut self, cx: &mut Context<Self>) {
        let Some(select) = self.tracking.select.take() else {
            return;
        };
        let Some((a, b)) = select.rect else {
            self.tracking.select = Some(select);
            self.report(Err("drag a box around the object first".into()), cx);
            return;
        };
        let Some((_, target)) = self.project.segment(&select.target_id) else {
            self.report(Err("the clip to track is gone".into()), cx);
            return;
        };
        let at = self.clock.position();
        // The box's centre and size in the video's own frame. Two opposite
        // corners are enough: a drawn box is never rotated, and a rotated
        // clip is unusual enough to accept the bounding box of the mapping.
        let corners = [[a[0], a[1]], [b[0], a[1]], [b[0], b[1]], [a[0], b[1]]];
        let mapped: Option<Vec<[f32; 2]>> = corners
            .iter()
            .map(|c| canvas_to_source(&self.project, target, at, *c))
            .collect();
        let Some(mapped) = mapped else {
            self.report(Err("the video is not visible at the playhead".into()), cx);
            return;
        };
        let (mut min, mut max) = ([f32::MAX; 2], [f32::MIN; 2]);
        for p in &mapped {
            for k in 0..2 {
                min[k] = min[k].min(p[k]);
                max[k] = max[k].max(p[k]);
            }
        }
        let rect = [
            0.5 * (min[0] + max[0]),
            0.5 * (min[1] + max[1]),
            max[0] - min[0],
            max[1] - min[1],
        ];
        let request = StartTracking {
            target_segment_id: select.target_id.clone(),
            at,
            rect,
            direction: if select.retrack.is_some() {
                Direction::Forward
            } else {
                Direction::Both
            },
            overlay_id: Some(select.overlay_id.clone()),
            mode: self.tracking.mode,
            retrack: select.retrack.clone(),
            tracker: Some(self.tracking.tracker),
        };
        match tracking_commands::tracking_start(&self.state, request, None) {
            Ok(id) => {
                self.tracking.job = Some(RunningJob {
                    id,
                    target_id: select.target_id,
                    done: 0,
                    total: 0,
                    latest: None,
                    return_to: at,
                });
                self.status = Some("Tracking…".into());
            }
            Err(error) => self.status = Some(error.into()),
        }
        cx.notify();
    }

    pub(crate) fn cancel_tracking(&mut self, cx: &mut Context<Self>) {
        if let Some(job) = &self.tracking.job {
            tracking_commands::tracking_cancel(job.id);
            self.status = Some("Stopping — the frames tracked so far are kept".into());
            cx.notify();
        }
    }

    /// Called from the editor's tick: progress, the frame being tracked, and
    /// the result when the job ends. Returns whether anything changed.
    pub(crate) fn poll_tracking(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(job) = &mut self.tracking.job else {
            return false;
        };
        let Some(status) = tracking_commands::tracking_status(job.id) else {
            self.tracking.job = None;
            return true;
        };
        if let Some(finished) = status.finished {
            tracking_commands::tracking_forget(job.id);
            let return_to = job.return_to;
            self.tracking.job = None;
            self.refresh(cx);
            self.seek(return_to);
            self.status = Some(match finished {
                Ok(outcome) => {
                    let mut text = format!(
                        "{} {} frames in {:.1} s ({:.0} frames/s)",
                        if outcome.cancelled {
                            "Stopped after"
                        } else {
                            "Tracked"
                        },
                        outcome.frames,
                        outcome.seconds,
                        outcome.frames as f64 / outcome.seconds.max(1e-3),
                    );
                    if let Some(note) = &outcome.note {
                        text.push_str(" · ");
                        text.push_str(note);
                    }
                    text.into()
                }
                Err(error) => error.into(),
            });
            return true;
        }
        let changed = status.done != job.done;
        job.done = status.done;
        job.total = status.total;
        job.latest = status.latest;
        // Show the frame being tracked, so a box that wanders off the object
        // is seen at once and can be cancelled.
        if changed {
            if let (Some(sample), Some((_, target))) =
                (job.latest, self.project.segment(&job.target_id))
            {
                let time = target.target_range.start
                    + self.project.materials.time_map(target).offset_of(sample.t);
                if target.target_range.contains(time) {
                    self.clock.seek(time);
                }
            }
        }
        changed
    }

    /// The tracker the next track or re-track runs. Changing it does not
    /// touch an existing track; "Re-track from here" applies it.
    pub(crate) fn set_tracker(&mut self, tracker: TrackerKind, cx: &mut Context<Self>) {
        self.tracking.tracker = tracker;
        cx.notify();
    }

    pub(crate) fn set_follow_mode(&mut self, mode: FollowMode, cx: &mut Context<Self>) {
        self.tracking.mode = mode;
        let Some(overlay) = self.tracking_overlay().cloned() else {
            cx.notify();
            return;
        };
        if self.followed(&overlay).is_some() {
            let result =
                tracking_commands::tracking_set_mode(&self.state, overlay.id, mode).map(|_| ());
            self.refresh(cx);
            self.report(result, cx);
        } else {
            cx.notify();
        }
    }

    /// Delete pressed on a clip that overlays follow: offer to keep their
    /// motion as keyframes before the clip goes. Returns whether the prompt
    /// took over; when it did not, Delete goes ahead as usual.
    pub(crate) fn offer_bake_before_delete(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let deleted = self.clips_to_delete();
        let followers = tracking_commands::tracking_dependent_followers(&self.project, &deleted);
        if followers.is_empty() {
            return false;
        }
        self.pause();
        let editor = cx.entity().downgrade();
        let count = followers.len();
        let message = if count == 1 {
            "A clip follows an object in this video. Bake its motion to keyframes so it keeps \
             moving after the video is deleted?"
                .to_string()
        } else {
            format!(
                "{count} clips follow an object in this video. Bake their motion to keyframes so \
                 they keep moving after the video is deleted?"
            )
        };
        window.open_dialog(cx, move |dialog, _, _| {
            let (bake, plain) = (editor.clone(), editor.clone());
            dialog
                .w(px(460.0))
                .title("Delete a tracked clip?")
                .child(
                    div()
                        .text_sm()
                        .text_color(rgb(TEXT_DIM))
                        .child(message.clone()),
                )
                .footer(
                    DialogFooter::new()
                        .child(
                            Button::new("tracked-delete-cancel")
                                .label("Cancel")
                                .on_click(|_, window, cx| window.close_dialog(cx)),
                        )
                        .child(
                            Button::new("tracked-delete-plain")
                                .label("Delete")
                                .on_click(move |_, window, cx| {
                                    window.close_dialog(cx);
                                    let _ =
                                        plain.update(cx, |editor, cx| editor.delete_selection(cx));
                                }),
                        )
                        .child(
                            Button::new("tracked-delete-bake")
                                .primary()
                                .label("Bake and delete")
                                .on_click(move |_, window, cx| {
                                    window.close_dialog(cx);
                                    let _ = bake.update(cx, |editor, cx| {
                                        editor.delete_selection_baking(cx)
                                    });
                                }),
                        ),
                )
        });
        true
    }

    pub(crate) fn bake_track(&mut self, cx: &mut Context<Self>) {
        let Some(overlay) = self.tracking_overlay().cloned() else {
            return;
        };
        let result = tracking_commands::tracking_bake(&self.state, overlay.id).map(|_| ());
        self.refresh(cx);
        self.report(result, cx);
    }

    pub(crate) fn stop_following(&mut self, cx: &mut Context<Self>) {
        let Some(overlay) = self.tracking_overlay().cloned() else {
            return;
        };
        let at = self.clock.position();
        let result = tracking_commands::tracking_detach(&self.state, overlay.id, at).map(|_| ());
        self.refresh(cx);
        self.report(result, cx);
    }

    pub(crate) fn remove_track(&mut self, cx: &mut Context<Self>) {
        let Some(overlay) = self.tracking_overlay().cloned() else {
            return;
        };
        let Some((track_id, _)) = self.followed(&overlay) else {
            return;
        };
        let at = self.clock.position();
        let result = tracking_commands::tracking_remove(&self.state, track_id, at).map(|_| ());
        self.refresh(cx);
        self.report(result, cx);
    }

    pub(crate) fn set_track_smoothing(&mut self, smoothing: f32, cx: &mut Context<Self>) {
        let Some(overlay) = self.tracking_overlay().cloned() else {
            return;
        };
        let Some((track_id, _)) = self.followed(&overlay) else {
            return;
        };
        let result =
            tracking_commands::tracking_set_smoothing(&self.state, track_id, smoothing).map(|_| ());
        self.refresh(cx);
        if let Err(error) = result {
            if error != "nothing to change" {
                self.report(Err(error), cx);
            }
        }
    }

    // --- the player overlay ------------------------------------------------------------

    /// Canvas-normalised coordinates (+y up) of a window point over the
    /// picture.
    fn picture_point(&self, position: Point<Pixels>) -> [f32; 2] {
        let b = self.tracking.picture.get();
        let (x, y) = (
            f32::from(position.x - b.origin.x),
            f32::from(position.y - b.origin.y),
        );
        let (w, h) = (
            f32::from(b.size.width).max(1.0),
            f32::from(b.size.height).max(1.0),
        );
        [
            (x / w * 2.0 - 1.0).clamp(-1.0, 1.0),
            (1.0 - y / h * 2.0).clamp(-1.0, 1.0),
        ]
    }

    fn on_box_down(&mut self, event: &MouseDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let p = self.picture_point(event.position);
        if let Some(select) = &mut self.tracking.select {
            select.dragging = Some(p);
            select.rect = Some((p, p));
            cx.stop_propagation();
            cx.notify();
        }
    }

    fn on_box_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        let p = self.picture_point(event.position);
        if let Some(select) = &mut self.tracking.select {
            if let Some(start) = select.dragging {
                if event.pressed_button != Some(MouseButton::Left) {
                    select.dragging = None;
                } else {
                    let min = [start[0].min(p[0]), start[1].min(p[1])];
                    let max = [start[0].max(p[0]), start[1].max(p[1])];
                    select.rect = Some((min, max));
                }
                cx.notify();
            }
        }
    }

    fn on_box_up(&mut self, _: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(select) = &mut self.tracking.select {
            select.dragging = None;
            // A click without a drag is not a box.
            if let Some((a, b)) = select.rect {
                if (b[0] - a[0]).abs() < 0.01 || (b[1] - a[1]).abs() < 0.01 {
                    select.rect = None;
                }
            }
            cx.notify();
        }
    }

    /// What the player draws over the picture for tracking: the box being
    /// drawn, the live box of a running job, or the tracked box and path of
    /// the selected follower. Empty otherwise.
    pub(super) fn render_tracking_overlay(&self, cx: &mut Context<Self>) -> AnyElement {
        let picture = Rc::clone(&self.tracking.picture);
        let shapes = self.tracking_shapes();
        let selecting = self.tracking.select.is_some();
        let layer = div().absolute().top_0().left_0().size_full().child(
            canvas(
                move |bounds, _, _| picture.set(bounds),
                move |bounds, _, window, _| paint_shapes(bounds, &shapes, window),
            )
            .size_full(),
        );
        if !selecting {
            return layer.into_any_element();
        }
        layer
            .id("tracking-box-select")
            .cursor_crosshair()
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_box_down))
            .on_mouse_move(cx.listener(Self::on_box_move))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_box_up))
            .into_any_element()
    }

    /// The shapes to draw, in canvas-normalised coordinates.
    fn tracking_shapes(&self) -> Vec<Shape> {
        let mut shapes = Vec::new();
        if let Some(select) = &self.tracking.select {
            if let Some((a, b)) = select.rect {
                shapes.push(Shape::Polygon {
                    points: vec![[a[0], b[1]], [b[0], b[1]], [b[0], a[1]], [a[0], a[1]]],
                    colour: ACCENT,
                    width: 2.0,
                });
            }
            return shapes;
        }
        let at = self.clock.position();
        if let Some(job) = &self.tracking.job {
            let Some((_, target)) = self.project.segment(&job.target_id) else {
                return shapes;
            };
            if let Some(sample) = job.latest {
                let corners = [
                    [sample.x - 0.5 * sample.w, sample.y - 0.5 * sample.h],
                    [sample.x + 0.5 * sample.w, sample.y - 0.5 * sample.h],
                    [sample.x + 0.5 * sample.w, sample.y + 0.5 * sample.h],
                    [sample.x - 0.5 * sample.w, sample.y + 0.5 * sample.h],
                ];
                let points: Option<Vec<[f32; 2]>> = corners
                    .iter()
                    .map(|c| source_to_canvas(&self.project, target, at, *c))
                    .collect();
                if let Some(points) = points {
                    shapes.push(Shape::Polygon {
                        points,
                        colour: if sample.is_lost() { DANGER } else { ACCENT },
                        width: 2.0,
                    });
                }
            }
            return shapes;
        }
        // The follower is not on screen outside its own time, so neither is
        // the path it follows.
        let Some(overlay) = self
            .tracking_overlay()
            .filter(|overlay| overlay.target_range.contains(at))
        else {
            return shapes;
        };
        let Some((track, _, Some(target))) =
            tracking_edit::followed_track(&self.project, overlay, at)
        else {
            return shapes;
        };
        shapes.extend(path_shapes(&self.project, track, target));
        if let Some(object) = follow::object_through(&self.project, track, target, at) {
            let doubtful = object.pose.confidence < LOW_CONFIDENCE;
            shapes.push(Shape::Polygon {
                points: object.corners.to_vec(),
                colour: if doubtful { WARNING } else { ACCENT },
                width: 1.5,
            });
        }
        shapes
    }
}

/// The track's path through `target`: a line through the centres, with the
/// frames the tracker was unsure of or lost marked.
fn path_shapes(
    project: &chukcut_engine::modules::project::Project,
    track: &TrackingMaterial,
    target: &Segment,
) -> Vec<Shape> {
    let mut line = Vec::new();
    let mut marks = Vec::new();
    let source = target.source_range;
    let map = project.materials.time_map(target);
    for sample in &track.samples {
        if sample.t < source.start || sample.t >= source.end() {
            continue;
        }
        let time = target.target_range.start + map.offset_of(sample.t);
        // The path the follower takes, so smoothing shows on it.
        let (x, y) = track
            .pose_at(sample.t)
            .map_or((sample.x, sample.y), |pose| (pose.x, pose.y));
        let Some(p) = source_to_canvas(project, target, time, [x, y]) else {
            continue;
        };
        if sample.is_lost() {
            marks.push(Shape::Dot {
                at: p,
                colour: DANGER,
            });
        } else {
            if sample.c < LOW_CONFIDENCE {
                marks.push(Shape::Dot {
                    at: p,
                    colour: WARNING,
                });
            }
            line.push(p);
        }
    }
    let mut shapes = Vec::new();
    if line.len() >= 2 {
        shapes.push(Shape::Line {
            points: line,
            colour: ACCENT,
            width: 1.0,
        });
    }
    shapes.extend(marks);
    shapes
}

/// Something drawn over the picture, in canvas-normalised coordinates.
#[derive(Clone)]
enum Shape {
    Polygon {
        points: Vec<[f32; 2]>,
        colour: u32,
        width: f32,
    },
    Line {
        points: Vec<[f32; 2]>,
        colour: u32,
        width: f32,
    },
    Dot {
        at: [f32; 2],
        colour: u32,
    },
}

fn paint_shapes(bounds: Bounds<Pixels>, shapes: &[Shape], window: &mut Window) {
    let to_screen = |p: [f32; 2]| {
        point(
            bounds.origin.x + bounds.size.width * ((p[0] + 1.0) * 0.5),
            bounds.origin.y + bounds.size.height * ((1.0 - p[1]) * 0.5),
        )
    };
    for shape in shapes {
        match shape {
            Shape::Polygon {
                points,
                colour,
                width,
            }
            | Shape::Line {
                points,
                colour,
                width,
            } => {
                let closed = matches!(shape, Shape::Polygon { .. });
                let mut builder = PathBuilder::stroke(px(*width));
                let mut iter = points.iter();
                let Some(first) = iter.next() else {
                    continue;
                };
                builder.move_to(to_screen(*first));
                for p in iter {
                    builder.line_to(to_screen(*p));
                }
                if closed {
                    builder.line_to(to_screen(*first));
                }
                if let Ok(path) = builder.build() {
                    window.paint_path(path, rgb(*colour));
                }
            }
            Shape::Dot { at, colour } => {
                let c = to_screen(*at);
                let r = px(2.5);
                window.paint_quad(
                    gpui::fill(
                        Bounds::new(point(c.x - r, c.y - r), gpui::size(r * 2.0, r * 2.0)),
                        rgb(*colour),
                    )
                    .corner_radii(r),
                );
            }
        }
    }
}
