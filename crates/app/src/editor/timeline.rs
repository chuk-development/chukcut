//! The timeline: toolbar, track headers, ruler, lanes, clips, playhead and
//! everything the pointer does to them.
//!
//! Laid out like CapCut's desktop timeline. From left to right: a column of
//! track headers (kind, lock, eye, mute), a narrow column holding the "Cover"
//! box in front of the main track, then the lanes, which scroll and zoom.
//! Overlay video lanes stack above the main track (the first video lane),
//! audio lanes below it; the stack sits low in the panel so there is room to
//! grow upwards, as in CapCut.
//!
//! All timeline state lives in [`TimelineState`], one field on the editor.
//! Every change to the document still goes through an `EditCommand`; the
//! multi-clip gestures (ripple delete, magnetic reorder) are built in
//! [`ripple`].

mod media_cache;
mod ripple;

use chukcut_engine::modules::project::{Marker, MarkerColor, Segment, TimeRange};
use chukcut_engine::modules::timeline::ops::TrackFlags;
use gpui::assets::IconName;
use gpui::component::button::{Button, ButtonVariants};
use gpui::component::menu::DropdownMenu;
use gpui::component::slider::{Slider, SliderEvent, SliderState};
use gpui::component::{Disableable, Icon, Selectable, Sizable};
use gpui::{fill, point, size, Corners, CursorStyle, Entity, KeyBinding, Subscription};

use super::*;
use media_cache::{MediaCache, Picture};
use ripple::Edge;

actions!(
    chukcut,
    [
        DeleteLeft,
        DeleteRight,
        ToggleMarker,
        ToggleMagnet,
        ToggleSnapping,
        ZoomToFit,
        SelectTool,
        BladeTool
    ]
);

/// The timeline's own shortcuts, CapCut's keys.
pub(crate) fn key_bindings() -> Vec<KeyBinding> {
    vec![
        KeyBinding::new("q", DeleteLeft, None),
        KeyBinding::new("w", DeleteRight, None),
        KeyBinding::new("m", ToggleMarker, None),
        KeyBinding::new("p", ToggleMagnet, None),
        KeyBinding::new("n", ToggleSnapping, None),
        KeyBinding::new("shift-z", ZoomToFit, None),
        KeyBinding::new("a", SelectTool, None),
        KeyBinding::new("b", BladeTool, None),
        KeyBinding::new("ctrl-+", ZoomIn, None),
        KeyBinding::new("ctrl-shift-=", ZoomIn, None),
    ]
}

// --- geometry -------------------------------------------------------------------

const TOOLBAR_H: f32 = 36.0;
/// The track header column.
const HEADER_W: f32 = 132.0;
/// The column between the headers and time zero, where the Cover box sits.
const LEAD_W: f32 = 52.0;
const RULER_H: f32 = 28.0;
const SCROLLBAR_H: f32 = 12.0;
const ROW_GAP: f32 = 4.0;
/// The gap between the panels above and the timeline, which is also the
/// handle that resizes it.
const SPLITTER_H: f32 = 6.0;
/// How far inside a clip's edge a press grabs the edge instead of the clip.
const EDGE_GRAB: f32 = 7.0;
/// How close, in pixels, an edge has to come to something to snap to it.
const SNAP_PX: f32 = 8.0;
/// How far the pointer has to travel before a press on a clip becomes a drag.
const DRAG_SLOP: f32 = 3.0;
const ZOOM_MIN: f32 = 0.5;
const ZOOM_MAX: f32 = 2400.0;
const TITLE_H: f32 = 16.0;

fn row_height(kind: TrackKind, main: bool) -> f32 {
    match kind {
        TrackKind::Video if main => 72.0,
        TrackKind::Video => 56.0,
        TrackKind::Audio => 48.0,
        _ => 32.0,
    }
}

// Colours only the timeline uses. Shared ones come from `theme`.
const LANES_BG: u32 = 0x1c1c1c;
const ROW_BG: u32 = 0x222222;
const AUDIO_TITLE: u32 = 0x0c2547;
const SNAP_LINE: u32 = 0xf2c94c;
const SCROLL_THUMB: u32 = 0x4a4a4a;

/// The slider runs 0..=1000 over a logarithmic zoom range, so each step of it
/// is the same relative change at any zoom.
fn zoom_to_slider(zoom: f32) -> f32 {
    ((zoom / ZOOM_MIN).ln() / (ZOOM_MAX / ZOOM_MIN).ln() * 1000.0).clamp(0.0, 1000.0)
}

fn slider_to_zoom(value: f32) -> f32 {
    ZOOM_MIN * (ZOOM_MAX / ZOOM_MIN).powf(value / 1000.0)
}

// --- state ----------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Tool {
    Select,
    Blade,
}

pub(crate) enum Drag {
    Scrub,
    Clip {
        segment_id: String,
        kind: TrackKind,
        grab: Micros,
        down_x: f32,
        moved: bool,
        origin_track: String,
        origin_start: Micros,
        track: String,
        start: Micros,
    },
    Trim {
        segment_id: String,
        edge: Edge,
        /// Pointer distance from the edge at the press, in time.
        grab: Micros,
        ripple: bool,
        target: TimeRange,
        source: TimeRange,
    },
    Scrollbar {
        grab: f32,
    },
    Resize {
        window_h: f32,
    },
}

/// One lane as drawn: where it is, in lanes-local pixels.
#[derive(Clone)]
struct Row {
    track: usize,
    top: f32,
    height: f32,
    main: bool,
}

/// Where a press on the lanes landed.
enum Hit {
    Clip { segment_id: String, zone: Zone },
    Lane,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Zone {
    Head,
    Body,
    Tail,
}

#[derive(Clone, Copy)]
enum Flag {
    Lock,
    Hide,
    Mute,
}

pub(crate) struct TimelineState {
    /// Pixels per second.
    zoom: f32,
    scroll_x: f32,
    scroll_y: f32,
    pub(crate) drag: Option<Drag>,
    tool: Tool,
    /// CapCut's "main track magnet": the main lane stays gapless.
    magnet: bool,
    snapping: bool,
    /// The share of the window height the timeline takes.
    share: f32,
    /// The lanes' bounds in window coordinates, measured while painting.
    lanes: Rc<Cell<Bounds<Pixels>>>,
    /// The pointer over the lanes, for the blade's cut line.
    hover: Option<Point<Pixels>>,
    /// Where an edge snapped during the current drag, drawn as a guide.
    snap: Option<Micros>,
    zoom_slider: Entity<SliderState>,
    media: MediaCache,
    _subscriptions: Vec<Subscription>,
}

impl TimelineState {
    pub(crate) fn new(cx: &mut Context<Editor>) -> Self {
        let zoom = 60.0;
        let zoom_slider = cx.new(|_| {
            SliderState::new()
                .min(0.0)
                .max(1000.0)
                .step(1.0)
                .default_value(zoom_to_slider(zoom))
        });
        let subscription = cx.subscribe(
            &zoom_slider,
            |editor: &mut Editor, _, event: &SliderEvent, cx| {
                if let SliderEvent::Change(value) = event {
                    let anchor = editor.playhead_anchor();
                    editor.set_zoom(slider_to_zoom(value.start()), anchor);
                    cx.notify();
                }
            },
        );
        Self {
            zoom,
            scroll_x: 0.0,
            scroll_y: 0.0,
            drag: None,
            tool: Tool::Select,
            magnet: true,
            snapping: true,
            share: 0.42,
            lanes: Rc::new(Cell::new(Bounds::default())),
            hover: None,
            snap: None,
            zoom_slider,
            media: MediaCache::default(),
            _subscriptions: vec![subscription],
        }
    }
}

// --- our own icons ----------------------------------------------------------------

// Drawn for chukcut in Lucide's grid and stroke so they sit with the Lucide
// icons around them. Only the alpha is used; the colour comes from the text.
const SVG_HEAD: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="black" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">"#;
const SPLIT_ICON: &str = r#"<path d="M4 4h4v16H4"/><path d="M20 4h-4v16h4"/>"#;
const DELETE_LEFT_ICON: &str = r#"<path d="M4 4h4v16H4" stroke-dasharray="2 3"/><path d="M20 4h-4v16h4"/><path d="M12 2v20"/>"#;
const DELETE_RIGHT_ICON: &str = r#"<path d="M4 4h4v16H4"/><path d="M20 4h-4v16h4" stroke-dasharray="2 3"/><path d="M12 2v20"/>"#;
const SNAP_ICON: &str = r#"<path d="M12 2v20"/><rect x="2" y="7" width="7" height="10" rx="1.5"/><rect x="15" y="7" width="7" height="10" rx="1.5"/>"#;
const ZOOM_FIT_ICON: &str = r#"<path d="M3 8V5a2 2 0 0 1 2-2h3"/><path d="M16 3h3a2 2 0 0 1 2 2v3"/><path d="M21 16v3a2 2 0 0 1-2 2h-3"/><path d="M8 21H5a2 2 0 0 1-2-2v-3"/><path d="M7 12h10"/><path d="m10 9-3 3 3 3"/><path d="m14 9 3 3-3 3"/>"#;

fn own_icon(body: &str) -> Icon {
    Icon::default().data(format!("{SVG_HEAD}{body}</svg>").as_bytes())
}

fn tool_button(id: &'static str, icon: impl Into<Icon>, tooltip: &'static str) -> Button {
    Button::new(id).ghost().small().icon(icon).tooltip(tooltip)
}

/// A switch in the toolbar: the icon turns the accent colour while it is on,
/// as CapCut draws its magnet and snapping buttons.
fn toggle_button(
    id: &'static str,
    icon: impl Into<Icon>,
    tooltip: &'static str,
    on: bool,
) -> Button {
    let icon: Icon = icon.into();
    let icon = if on {
        icon.text_color(rgb(ACCENT))
    } else {
        icon
    };
    tool_button(id, icon, tooltip).selected(on)
}

fn separator() -> impl IntoElement {
    div().w(px(1.0)).h(px(18.0)).mx(px(4.0)).bg(rgb(BORDER))
}

fn marker_color(color: MarkerColor) -> u32 {
    match color {
        MarkerColor::Blue => 0x3d8bfd,
        MarkerColor::Green => 0x3ccf6b,
        MarkerColor::Yellow => 0xf2c94c,
        MarkerColor::Orange => 0xf2994a,
        MarkerColor::Red => 0xeb5757,
        MarkerColor::Purple => 0xa77bf3,
    }
}

// --- the ruler ------------------------------------------------------------------

/// The spacing of labelled ticks, in seconds, and how many minor ticks divide
/// it. The smallest step whose labels stay `min_px` apart wins; below a second
/// the steps are whole frames.
fn ruler_step(zoom: f32, fps: f64, min_px: f32) -> (f64, u32) {
    let fps = if fps > 0.0 { fps } else { 30.0 };
    let frame = 1.0 / fps;
    let mut ladder: Vec<(f64, u32)> = vec![(frame, 1), (2.0 * frame, 2), (5.0 * frame, 5)];
    if fps >= 20.0 {
        ladder.push((10.0 * frame, 5));
    }
    if fps >= 30.0 {
        ladder.push(((fps / 2.0).floor() * frame, 3));
    }
    ladder.extend([
        (1.0, 5),
        (2.0, 4),
        (5.0, 5),
        (10.0, 5),
        (15.0, 3),
        (30.0, 5),
        (60.0, 4),
        (120.0, 4),
        (300.0, 5),
        (600.0, 5),
        (1800.0, 3),
        (3600.0, 4),
    ]);
    ladder
        .into_iter()
        .find(|(step, _)| *step as f32 * zoom >= min_px)
        .unwrap_or((7200.0, 4))
}

/// A ruler label: `mm:ss` on whole seconds (`h:mm:ss` past an hour), and the
/// frame number with an `f` in between, as CapCut writes them.
fn ruler_label(seconds: f64, fps: f64) -> String {
    let fps = if fps > 0.0 { fps } else { 30.0 };
    let whole = seconds.floor();
    let frame = ((seconds - whole) * fps).round() as i64;
    if frame > 0 && frame < fps.round() as i64 {
        return format!("{frame}f");
    }
    let whole = (seconds + 0.5 / fps).floor() as i64;
    if whole >= 3600 {
        format!("{}:{:02}:{:02}", whole / 3600, whole / 60 % 60, whole % 60)
    } else {
        format!("{:02}:{:02}", whole / 60, whole % 60)
    }
}

// --- the editor's timeline ------------------------------------------------------------

impl Editor {
    // --- geometry ------------------------------------------------------------------

    fn time_to_x(&self, time: Micros) -> f32 {
        time as f32 / 1_000_000.0 * self.timeline.zoom - self.timeline.scroll_x
    }

    fn x_to_time(&self, x: f32) -> Micros {
        (((x + self.timeline.scroll_x) / self.timeline.zoom) * 1_000_000.0).max(0.0) as Micros
    }

    fn lanes_size(&self) -> (f32, f32) {
        let bounds = self.timeline.lanes.get();
        (
            f32::from(bounds.size.width).max(1.0),
            f32::from(bounds.size.height).max(1.0),
        )
    }

    /// Lanes-local coordinates of a window position.
    fn lanes_local(&self, position: Point<Pixels>) -> (f32, f32) {
        let bounds = self.timeline.lanes.get();
        (
            f32::from(position.x - bounds.origin.x),
            f32::from(position.y - bounds.origin.y),
        )
    }

    fn main_track_index(&self) -> Option<usize> {
        self.project
            .tracks
            .iter()
            .position(|t| t.kind == TrackKind::Video)
    }

    fn is_main_track(&self, track_id: &str) -> bool {
        self.main_track_index()
            .is_some_and(|i| self.project.tracks[i].id == track_id)
    }

    /// The lanes top to bottom, CapCut's order: text and other overlays, then
    /// video lanes with the main one last, then audio.
    fn rows(&self) -> (Vec<Row>, f32) {
        let tracks = &self.project.tracks;
        let main = self.main_track_index();
        let mut order: Vec<usize> = Vec::with_capacity(tracks.len());
        order.extend(
            (0..tracks.len())
                .rev()
                .filter(|&i| !matches!(tracks[i].kind, TrackKind::Video | TrackKind::Audio)),
        );
        order.extend(
            (0..tracks.len())
                .rev()
                .filter(|&i| tracks[i].kind == TrackKind::Video),
        );
        order.extend((0..tracks.len()).filter(|&i| tracks[i].kind == TrackKind::Audio));

        let content: f32 = order
            .iter()
            .map(|&i| row_height(tracks[i].kind, Some(i) == main) + ROW_GAP)
            .sum();
        let (_, lanes_h) = self.lanes_size();
        let area = lanes_h - RULER_H - SCROLLBAR_H;
        // Sit low with room above, like CapCut; scroll once it does not fit.
        let pad = if content < area {
            ((area - content) * 0.72).max(8.0)
        } else {
            8.0
        };
        let mut top = RULER_H + pad - self.timeline.scroll_y;
        let rows = order
            .into_iter()
            .map(|i| {
                let height = row_height(tracks[i].kind, Some(i) == main);
                let row = Row {
                    track: i,
                    top,
                    height,
                    main: Some(i) == main,
                };
                top += height + ROW_GAP;
                row
            })
            .collect();
        (rows, content)
    }

    fn row_of(&self, track_id: &str) -> Option<Row> {
        let (rows, _) = self.rows();
        rows.into_iter()
            .find(|row| self.project.tracks[row.track].id == track_id)
    }

    /// The lane under a lanes-local y; the gap under a lane belongs to it.
    fn row_at(&self, y: f32) -> Option<Row> {
        let (rows, _) = self.rows();
        rows.into_iter()
            .find(|row| y >= row.top && y < row.top + row.height + ROW_GAP)
    }

    fn hit(&self, x: f32, y: f32) -> Option<(Row, Hit)> {
        let row = self.row_at(y)?;
        let track = &self.project.tracks[row.track];
        let time = self.x_to_time(x);
        let Some(segment) = track
            .segments
            .iter()
            .find(|s| time >= s.target_range.start && time < s.target_range.end())
        else {
            return Some((row, Hit::Lane));
        };
        let left = self.time_to_x(segment.target_range.start);
        let right = self.time_to_x(segment.target_range.end());
        // Narrow clips keep a middle to grab.
        let grab = EDGE_GRAB.min((right - left) / 3.0);
        let zone = if x < left + grab {
            Zone::Head
        } else if x > right - grab {
            Zone::Tail
        } else {
            Zone::Body
        };
        Some((
            row,
            Hit::Clip {
                segment_id: segment.id.clone(),
                zone,
            },
        ))
    }

    /// The x the playhead is drawn at, when it is on screen; zooming keeps it
    /// in place. Otherwise the left edge stays.
    fn playhead_anchor(&self) -> f32 {
        let (width, _) = self.lanes_size();
        let x = self.time_to_x(self.clock.position());
        if (0.0..=width).contains(&x) {
            x
        } else {
            0.0
        }
    }

    fn content_width(&self) -> f32 {
        let (width, _) = self.lanes_size();
        (self.project.duration() as f32 / 1_000_000.0 * self.timeline.zoom + width * 0.6).max(width)
    }

    fn clamp_scroll(&mut self) {
        let (width, height) = self.lanes_size();
        let max_x = (self.content_width() - width).max(0.0);
        self.timeline.scroll_x = self.timeline.scroll_x.clamp(0.0, max_x);
        let (_, content) = self.rows();
        let area = height - RULER_H - SCROLLBAR_H;
        let max_y = (content + 16.0 - area).max(0.0);
        self.timeline.scroll_y = self.timeline.scroll_y.clamp(0.0, max_y);
    }

    /// Zoom to `zoom`, keeping the time under the lanes-local `anchor` there.
    fn set_zoom(&mut self, zoom: f32, anchor: f32) {
        let time = (anchor + self.timeline.scroll_x) / self.timeline.zoom;
        self.timeline.zoom = zoom.clamp(ZOOM_MIN, ZOOM_MAX);
        self.timeline.scroll_x = time * self.timeline.zoom - anchor;
        self.clamp_scroll();
    }

    pub(super) fn zoom_by(&mut self, factor: f32, cx: &mut Context<Self>) {
        let anchor = self.playhead_anchor();
        self.set_zoom(self.timeline.zoom * factor, anchor);
        cx.notify();
    }

    fn zoom_to_fit(&mut self, cx: &mut Context<Self>) {
        let duration = self.project.duration();
        if duration > 0 {
            let (width, _) = self.lanes_size();
            self.timeline.zoom =
                (width * 0.92 / (duration as f32 / 1_000_000.0)).clamp(ZOOM_MIN, ZOOM_MAX);
            self.timeline.scroll_x = 0.0;
        }
        cx.notify();
    }

    // --- snapping ------------------------------------------------------------------

    /// Everything an edge can snap to: time zero, the playhead, markers, and
    /// the edges of every clip but `exclude`.
    fn snap_points(&self, exclude: &str) -> Vec<Micros> {
        let mut points = vec![0, self.clock.position()];
        points.extend(self.project.markers.iter().map(|m| m.time));
        for track in &self.project.tracks {
            for segment in &track.segments {
                if segment.id != exclude {
                    points.push(segment.target_range.start);
                    points.push(segment.target_range.end());
                }
            }
        }
        points
    }

    /// The shift that brings the nearest of `edges` onto a snap point, when
    /// one is within reach, and the point it lands on.
    fn snap(&self, edges: &[Micros], exclude: &str) -> Option<(Micros, Micros)> {
        if !self.timeline.snapping {
            return None;
        }
        let reach = (SNAP_PX / self.timeline.zoom * 1_000_000.0) as Micros;
        self.snap_points(exclude)
            .into_iter()
            .flat_map(|point| edges.iter().map(move |edge| (point - edge, point)))
            .filter(|(shift, _)| shift.abs() <= reach)
            .min_by_key(|(shift, _)| shift.abs())
    }

    // --- pointer -------------------------------------------------------------------

    fn on_lanes_down(&mut self, event: &MouseDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let (x, y) = self.lanes_local(event.position);
        let (width, height) = self.lanes_size();
        let time = self.x_to_time(x);

        if y >= height - SCROLLBAR_H {
            let content = self.content_width();
            let thumb_w = (width * width / content).max(30.0);
            let thumb_x = self.timeline.scroll_x / content * width;
            let grab = if x >= thumb_x && x <= thumb_x + thumb_w {
                x - thumb_x
            } else {
                thumb_w / 2.0
            };
            self.timeline.drag = Some(Drag::Scrollbar { grab });
            self.scroll_to_thumb(x, grab);
            cx.notify();
            return;
        }

        if y < RULER_H {
            self.pause();
            self.seek(time);
            self.timeline.drag = Some(Drag::Scrub);
            cx.notify();
            return;
        }

        let hit = self.hit(x, y);
        let clip = match &hit {
            Some((row, Hit::Clip { segment_id, zone }))
                if !self.project.tracks[row.track].locked =>
            {
                Some((row.clone(), segment_id.clone(), *zone))
            }
            _ => None,
        };
        let Some((row, segment_id, zone)) = clip else {
            self.selected = None;
            self.seek(time);
            self.timeline.drag = Some(Drag::Scrub);
            cx.notify();
            return;
        };

        if self.timeline.tool == Tool::Blade {
            let at = self
                .snap(&[time], "")
                .map(|(shift, _)| time + shift)
                .unwrap_or(time);
            let result = timeline_commands::timeline_split(&self.state, segment_id, at).map(|_| ());
            self.refresh(cx);
            self.report(result, cx);
            return;
        }

        self.selected = Some(segment_id.clone());
        let track = &self.project.tracks[row.track];
        let Some(segment) = track.segments.iter().find(|s| s.id == segment_id) else {
            return;
        };
        self.timeline.drag = Some(match zone {
            Zone::Head | Zone::Tail => {
                let edge = if zone == Zone::Head {
                    Edge::Head
                } else {
                    Edge::Tail
                };
                let at = if edge == Edge::Head {
                    segment.target_range.start
                } else {
                    segment.target_range.end()
                };
                Drag::Trim {
                    segment_id,
                    edge,
                    grab: time - at,
                    ripple: self.timeline.magnet && row.main,
                    target: segment.target_range,
                    source: segment.source_range,
                }
            }
            Zone::Body => Drag::Clip {
                segment_id,
                kind: track.kind,
                grab: time - segment.target_range.start,
                down_x: x,
                moved: false,
                origin_track: track.id.clone(),
                origin_start: segment.target_range.start,
                track: track.id.clone(),
                start: segment.target_range.start,
            },
        });
        cx.notify();
    }

    fn scroll_to_thumb(&mut self, x: f32, grab: f32) {
        let (width, _) = self.lanes_size();
        self.timeline.scroll_x = (x - grab) / width * self.content_width();
        self.clamp_scroll();
    }

    fn on_splitter_down(
        &mut self,
        _: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.timeline.drag = Some(Drag::Resize {
            window_h: f32::from(window.viewport_size().height),
        });
        cx.notify();
    }

    pub(super) fn on_mouse_move(
        &mut self,
        event: &MouseMoveEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.timeline.drag.is_none() || event.pressed_button != Some(MouseButton::Left) {
            if self.timeline.tool == Tool::Blade {
                let inside = self.timeline.lanes.get().contains(&event.position);
                let hover = inside.then_some(event.position);
                if hover != self.timeline.hover {
                    self.timeline.hover = hover;
                    cx.notify();
                }
            }
            return;
        }
        let (x, y) = self.lanes_local(event.position);
        let time = self.x_to_time(x);
        match self.timeline.drag.take() {
            Some(Drag::Scrub) => {
                self.seek(time);
                self.timeline.drag = Some(Drag::Scrub);
            }
            Some(Drag::Clip {
                segment_id,
                kind,
                grab,
                down_x,
                mut moved,
                origin_track,
                origin_start,
                mut track,
                mut start,
            }) => {
                moved |= (x - down_x).abs() >= DRAG_SLOP;
                self.timeline.snap = None;
                if moved {
                    let duration = self
                        .project
                        .segment(&segment_id)
                        .map(|(_, s)| s.target_range.duration)
                        .unwrap_or(0);
                    start = (time - grab).max(0);
                    if let Some((shift, point)) = self.snap(&[start, start + duration], &segment_id)
                    {
                        start = (start + shift).max(0);
                        self.timeline.snap = Some(point);
                    }
                    if let Some(row) = self.row_at(y) {
                        let lane = &self.project.tracks[row.track];
                        if lane.kind == kind && !lane.locked {
                            track = lane.id.clone();
                        }
                    }
                }
                self.timeline.drag = Some(Drag::Clip {
                    segment_id,
                    kind,
                    grab,
                    down_x,
                    moved,
                    origin_track,
                    origin_start,
                    track,
                    start,
                });
            }
            Some(Drag::Trim {
                segment_id,
                edge,
                grab,
                ripple,
                ..
            }) => {
                self.timeline.snap = None;
                let Some((track, segment)) = self.project.segment(&segment_id) else {
                    return;
                };
                let mut to = time - grab;
                if let Some((shift, point)) = self.snap(&[to], &segment_id) {
                    to += shift;
                    self.timeline.snap = Some(point);
                }
                // A free lane keeps its neighbours; a rippled one pushes them.
                if !ripple {
                    let index = track
                        .segments
                        .iter()
                        .position(|s| s.id == segment_id)
                        .unwrap_or(0);
                    to = match edge {
                        Edge::Head => to.max(
                            index
                                .checked_sub(1)
                                .map(|i| track.segments[i].target_range.end())
                                .unwrap_or(0),
                        ),
                        Edge::Tail => to.min(
                            track
                                .segments
                                .get(index + 1)
                                .map(|s| s.target_range.start)
                                .unwrap_or(Micros::MAX),
                        ),
                    };
                }
                let limit = ripple::source_limit(&self.project, &segment.material_id);
                let anchored = ripple && edge == Edge::Head;
                let (target, source) = ripple::trimmed(segment, edge, to, limit, anchored);
                self.timeline.drag = Some(Drag::Trim {
                    segment_id,
                    edge,
                    grab,
                    ripple,
                    target,
                    source,
                });
            }
            Some(Drag::Scrollbar { grab }) => {
                self.scroll_to_thumb(x, grab);
                self.timeline.drag = Some(Drag::Scrollbar { grab });
            }
            Some(Drag::Resize { window_h }) => {
                let y = f32::from(event.position.y);
                self.timeline.share = ((window_h - y) / window_h).clamp(0.2, 0.75);
                self.timeline.drag = Some(Drag::Resize { window_h });
            }
            None => {}
        }
        cx.notify();
    }

    pub(super) fn on_mouse_up(&mut self, _: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.timeline.snap = None;
        match self.timeline.drag.take() {
            Some(Drag::Clip {
                segment_id,
                moved: true,
                origin_track,
                origin_start,
                track,
                start,
                ..
            }) if track != origin_track || start != origin_start => {
                let commands = self.move_commands(&segment_id, &origin_track, &track, start);
                self.apply_many(commands, "Move clip", cx);
            }
            Some(Drag::Trim {
                segment_id,
                ripple,
                target,
                source,
                ..
            }) => {
                let commands = ripple::trim(&self.project, &segment_id, target, source, ripple);
                self.apply_many(commands, "Trim clip", cx);
            }
            _ => {}
        }
        cx.notify();
    }

    /// The commands a dropped clip turns into. With the magnet on, the main
    /// lane reorders instead of taking the clip where it was let go, and a
    /// clip lifted off it leaves no hole.
    fn move_commands(
        &self,
        segment_id: &str,
        from: &str,
        to: &str,
        start: Micros,
    ) -> Result<Vec<EditCommand>, String> {
        let magnet = self.timeline.magnet;
        if magnet && self.is_main_track(to) {
            let track = self.project.track(to).ok_or("the lane is gone")?;
            let duration = self
                .project
                .segment(segment_id)
                .map(|(_, s)| s.target_range.duration)
                .unwrap_or(0);
            let index = ripple::insertion_index(track, segment_id, start + duration / 2);
            return ripple::drop_into_gapless(&self.project, to, segment_id, index);
        }
        if magnet && self.is_main_track(from) {
            let commands = ripple::lift_out_of_gapless(&self.project, segment_id, to, start)?;
            // The lane it lands on has to have room; the main lane always
            // has room for its own clips closing up.
            edits::move_to(&self.project, segment_id, to, start)?;
            return Ok(commands);
        }
        edits::move_to(&self.project, segment_id, to, start).map(|command| vec![command])
    }

    fn apply_many(
        &mut self,
        commands: Result<Vec<EditCommand>, String>,
        label: &str,
        cx: &mut Context<Self>,
    ) {
        let result = commands.and_then(|commands| {
            if commands.is_empty() {
                return Ok(());
            }
            timeline_commands::timeline_apply_many(&self.state, commands, label.into()).map(|_| ())
        });
        self.refresh(cx);
        self.report(result, cx);
    }

    pub(super) fn on_timeline_scroll(
        &mut self,
        event: &ScrollWheelEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let delta = event.delta.pixel_delta(px(20.0));
        let (dx, dy) = (f32::from(delta.x), f32::from(delta.y));
        if event.modifiers.control {
            let (x, _) = self.lanes_local(event.position);
            let factor = if dy > 0.0 { 1.15 } else { 1.0 / 1.15 };
            if dy != 0.0 {
                self.set_zoom(self.timeline.zoom * factor, x.max(0.0));
            }
        } else if event.modifiers.shift {
            self.timeline.scroll_y -= dy;
        } else if dx.abs() > dy.abs() {
            self.timeline.scroll_x -= dx;
        } else {
            self.timeline.scroll_x -= dy;
        }
        self.clamp_scroll();
        cx.notify();
    }

    // --- commands ------------------------------------------------------------------

    /// Delete a clip; on the main lane with the magnet on, close the hole.
    pub(super) fn remove_clip(&mut self, segment_id: &str, cx: &mut Context<Self>) {
        let ripple = self.timeline.magnet
            && self
                .project
                .segment(segment_id)
                .is_some_and(|(track, _)| self.is_main_track(&track.id));
        let commands = ripple::remove(&self.project, segment_id, ripple);
        self.apply_many(commands, "Delete clip", cx);
    }

    /// The clip Q and W act on: the selected one when the playhead is inside
    /// it, otherwise the main lane's clip under the playhead.
    fn trim_target(&self) -> Option<String> {
        let at = self.clock.position();
        let inside = |s: &Segment| s.target_range.start < at && at < s.target_range.end();
        if let Some((_, segment)) = self
            .selected
            .as_deref()
            .and_then(|id| self.project.segment(id))
        {
            if inside(segment) {
                return Some(segment.id.clone());
            }
        }
        let main = &self.project.tracks[self.main_track_index()?];
        if main.locked {
            return None;
        }
        main.segments
            .iter()
            .find(|s| inside(s))
            .map(|s| s.id.clone())
    }

    /// Delete left (Q) and delete right (W): cut the clip at the playhead and
    /// drop the part on one side.
    fn trim_to_playhead(&mut self, edge: Edge, cx: &mut Context<Self>) {
        let Some(id) = self.trim_target() else {
            self.status = Some("Put the playhead inside a clip first".into());
            cx.notify();
            return;
        };
        let Some((track, segment)) = self.project.segment(&id) else {
            return;
        };
        let at = self.clock.position();
        let ripple = self.timeline.magnet && self.is_main_track(&track.id);
        let limit = ripple::source_limit(&self.project, &segment.material_id);
        let start = segment.target_range.start;
        let (target, source) =
            ripple::trimmed(segment, edge, at, limit, ripple && edge == Edge::Head);
        let commands = ripple::trim(&self.project, &id, target, source, ripple);
        let label = match edge {
            Edge::Head => "Delete left",
            Edge::Tail => "Delete right",
        };
        self.apply_many(commands, label, cx);
        if ripple && edge == Edge::Head {
            // What was at the playhead now starts where the clip started.
            self.seek(start);
        }
    }

    fn toggle_marker(&mut self, cx: &mut Context<Self>) {
        let at = self.clock.position();
        let half_frame = (500_000.0 / self.project.fps.max(1.0)) as Micros;
        let existing = self
            .project
            .markers
            .iter()
            .find(|m| (m.time - at).abs() <= half_frame)
            .cloned();
        let command = match existing {
            Some(marker) => EditCommand::RemoveMarker { marker },
            None => EditCommand::AddMarker {
                marker: Marker::new(at),
            },
        };
        self.apply(Ok(command), cx);
    }

    fn toggle_track(&mut self, track_id: &str, flag: Flag, cx: &mut Context<Self>) {
        let Some(track) = self.project.track(track_id) else {
            return;
        };
        let before = TrackFlags::of(track);
        let mut after = before;
        match flag {
            Flag::Lock => after.locked = !after.locked,
            Flag::Hide => after.hidden = !after.hidden,
            Flag::Mute => after.muted = !after.muted,
        }
        if after.locked
            && self
                .selected
                .as_deref()
                .is_some_and(|id| track.segments.iter().any(|s| s.id == id))
        {
            self.selected = None;
        }
        self.apply(
            Ok(EditCommand::SetTrackFlags {
                track_id: track_id.to_string(),
                before,
                after,
            }),
            cx,
        );
    }

    fn set_tool(&mut self, tool: Tool, cx: &mut Context<Self>) {
        self.timeline.tool = tool;
        self.timeline.hover = None;
        cx.notify();
    }

    /// Register the timeline's actions on the editor's root element.
    pub(super) fn timeline_actions(&self, root: gpui::Div, cx: &mut Context<Self>) -> gpui::Div {
        root.on_action(
            cx.listener(|this, _: &DeleteLeft, _, cx| this.trim_to_playhead(Edge::Head, cx)),
        )
        .on_action(
            cx.listener(|this, _: &DeleteRight, _, cx| this.trim_to_playhead(Edge::Tail, cx)),
        )
        .on_action(cx.listener(|this, _: &ToggleMarker, _, cx| this.toggle_marker(cx)))
        .on_action(cx.listener(|this, _: &ToggleMagnet, _, cx| {
            this.timeline.magnet = !this.timeline.magnet;
            cx.notify();
        }))
        .on_action(cx.listener(|this, _: &ToggleSnapping, _, cx| {
            this.timeline.snapping = !this.timeline.snapping;
            cx.notify();
        }))
        .on_action(cx.listener(|this, _: &ZoomToFit, _, cx| this.zoom_to_fit(cx)))
        .on_action(cx.listener(|this, _: &SelectTool, _, cx| this.set_tool(Tool::Select, cx)))
        .on_action(cx.listener(|this, _: &BladeTool, _, cx| this.set_tool(Tool::Blade, cx)))
    }

    // --- media -----------------------------------------------------------------------

    fn picture(&self, material_id: &str) -> Option<Picture> {
        let pool = &self.project.materials;
        if let Some(video) = pool.videos.iter().find(|m| m.id == material_id) {
            let (w, h) = if video.rotation.rem_euclid(180) == 90 {
                (video.height, video.width)
            } else {
                (video.width, video.height)
            };
            return Some(Picture {
                path: video.path.clone(),
                still: false,
                aspect: if h > 0 {
                    w as f32 / h as f32
                } else {
                    16.0 / 9.0
                },
                duration: video.duration,
            });
        }
        pool.images
            .iter()
            .find(|m| m.id == material_id)
            .map(|image| Picture {
                path: image.path.clone(),
                still: true,
                aspect: if image.height > 0 {
                    image.width as f32 / image.height as f32
                } else {
                    1.0
                },
                duration: 0,
            })
    }

    fn request_strip(
        &mut self,
        material_id: &str,
        picture: Picture,
        count: usize,
        cx: &mut Context<Self>,
    ) {
        let Some(key) = self.timeline.media.wanted_strip(material_id, count) else {
            return;
        };
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { media_cache::load_strip(&picture, count) })
                .await;
            let _ = this.update(cx, |editor, cx| {
                for image in editor.timeline.media.finish_strip(key, result) {
                    cx.drop_image(image, None);
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn request_wave(&mut self, material_id: &str, cx: &mut Context<Self>) {
        let Some(audio) = self
            .project
            .materials
            .audios
            .iter()
            .find(|m| m.id == material_id)
        else {
            return;
        };
        if !self.timeline.media.wanted_wave(material_id) {
            return;
        }
        let (path, duration, id) = (audio.path.clone(), audio.duration, audio.id.clone());
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { media_cache::load_wave(&path, duration) })
                .await;
            let _ = this.update(cx, |editor, cx| {
                editor.timeline.media.finish_wave(id, result);
                cx.notify();
            });
        })
        .detach();
    }

    // --- drawing ---------------------------------------------------------------------

    pub(super) fn render_timeline(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        // Keep the slider on the zoom, however it was changed.
        let slider_value = zoom_to_slider(self.timeline.zoom);
        let slider = self.timeline.zoom_slider.clone();
        if (slider.read(cx).value().start() - slider_value).abs() > 0.5 {
            slider.update(cx, |state, cx| state.set_value(slider_value, window, cx));
        }

        // Follow the playhead a page at a time while playing.
        let (lanes_w, lanes_h) = self.lanes_size();
        if self.clock.is_playing() && self.timeline.drag.is_none() {
            let x = self.time_to_x(self.clock.position());
            if x > lanes_w - 24.0 || x < 0.0 {
                self.timeline.scroll_x += x - lanes_w * 0.1;
                self.clamp_scroll();
            }
        }

        let height = (f32::from(window.viewport_size().height) * self.timeline.share).max(200.0);
        let (rows, _) = self.rows();

        div()
            .h(px(height))
            .flex_none()
            .flex()
            .flex_col()
            .child(
                div()
                    .id("timeline-splitter")
                    .h(px(SPLITTER_H))
                    .flex_none()
                    .cursor_row_resize()
                    .on_mouse_down(MouseButton::Left, cx.listener(Self::on_splitter_down)),
            )
            .child(
                div()
                    .flex_1()
                    .min_h(px(0.0))
                    .mx(px(6.0))
                    .mb(px(6.0))
                    .rounded(px(6.0))
                    .overflow_hidden()
                    .flex()
                    .flex_col()
                    .bg(rgb(PANEL))
                    .child(self.render_toolbar_row(cx))
                    .child(
                        div()
                            .flex_1()
                            .min_h(px(0.0))
                            .flex()
                            .flex_row()
                            .border_t_1()
                            .border_color(rgb(BORDER))
                            .child(self.render_headers(&rows, cx))
                            .child(self.render_cover_column(&rows))
                            .child(self.render_lanes(&rows, lanes_w, lanes_h, cx)),
                    ),
            )
    }

    fn render_toolbar_row(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let at = self.clock.position();
        let inside = |s: &Segment| s.target_range.start < at && at < s.target_range.end();
        let selected = self
            .selected
            .as_deref()
            .and_then(|id| self.project.segment(id));
        let can_split = match selected {
            Some((_, segment)) => inside(segment),
            None => self
                .project
                .tracks
                .iter()
                .any(|t| !t.locked && t.segments.iter().any(inside)),
        };
        let can_trim = self.trim_target().is_some();
        let (can_undo, can_redo) = {
            let history = self.state.history.read();
            (history.can_undo(), history.can_redo())
        };
        let tool = self.timeline.tool;
        let focus = self.focus.clone();

        let left = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(2.0))
            .child(
                tool_button(
                    "tool",
                    match tool {
                        Tool::Select => IconName::MousePointer2,
                        Tool::Blade => IconName::Scissors,
                    },
                    "Tool: select (A) or split (B)",
                )
                .dropdown_caret(true)
                .dropdown_menu(move |menu, _, _| {
                    menu.action_context(focus.clone())
                        .menu_with_check("Select", tool == Tool::Select, Box::new(SelectTool))
                        .menu_with_check("Split", tool == Tool::Blade, Box::new(BladeTool))
                }),
            )
            .child(separator())
            .child(
                tool_button("undo", IconName::Undo2, "Undo (Ctrl+Z)")
                    .disabled(!can_undo)
                    .on_click(cx.listener(|this, _, w, cx| this.on_undo(&Undo, w, cx))),
            )
            .child(
                tool_button("redo", IconName::Redo2, "Redo (Ctrl+Shift+Z)")
                    .disabled(!can_redo)
                    .on_click(cx.listener(|this, _, w, cx| this.on_redo(&Redo, w, cx))),
            )
            .child(separator())
            .child(
                tool_button("split", own_icon(SPLIT_ICON), "Split (Ctrl+B)")
                    .disabled(!can_split)
                    .on_click(cx.listener(|this, _, w, cx| this.on_split(&Split, w, cx))),
            )
            .child(
                tool_button("delete-left", own_icon(DELETE_LEFT_ICON), "Delete left (Q)")
                    .disabled(!can_trim)
                    .on_click(cx.listener(|this, _, _, cx| this.trim_to_playhead(Edge::Head, cx))),
            )
            .child(
                tool_button(
                    "delete-right",
                    own_icon(DELETE_RIGHT_ICON),
                    "Delete right (W)",
                )
                .disabled(!can_trim)
                .on_click(cx.listener(|this, _, _, cx| this.trim_to_playhead(Edge::Tail, cx))),
            )
            .child(
                tool_button("delete", IconName::Trash, "Delete (Del)")
                    .disabled(selected.is_none())
                    .on_click(cx.listener(|this, _, w, cx| this.on_delete(&DeleteSelected, w, cx))),
            )
            .child(
                tool_button("marker", IconName::Bookmark, "Marker (M)")
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_marker(cx))),
            );

        let right = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(2.0))
            .child(
                toggle_button(
                    "magnet",
                    IconName::Magnet,
                    "Main track magnet (P)",
                    self.timeline.magnet,
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.timeline.magnet = !this.timeline.magnet;
                    cx.notify();
                })),
            )
            .child(
                toggle_button(
                    "snapping",
                    own_icon(SNAP_ICON),
                    "Snapping (N)",
                    self.timeline.snapping,
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.timeline.snapping = !this.timeline.snapping;
                    cx.notify();
                })),
            )
            .child(separator())
            .child(
                tool_button("zoom-fit", own_icon(ZOOM_FIT_ICON), "Zoom to fit (Shift+Z)")
                    .disabled(self.project.duration() <= 0)
                    .on_click(cx.listener(|this, _, _, cx| this.zoom_to_fit(cx))),
            )
            .child(
                tool_button("zoom-out", IconName::ZoomOut, "Zoom out (Ctrl+-)")
                    .on_click(cx.listener(|this, _, _, cx| this.zoom_by(1.0 / 1.4, cx))),
            )
            .child(
                div()
                    .w(px(110.0))
                    .px(px(4.0))
                    .child(Slider::new(&self.timeline.zoom_slider).horizontal()),
            )
            .child(
                tool_button("zoom-in", IconName::ZoomIn, "Zoom in (Ctrl+=)")
                    .on_click(cx.listener(|this, _, _, cx| this.zoom_by(1.4, cx))),
            );

        div()
            .h(px(TOOLBAR_H))
            .flex_none()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .px(px(8.0))
            .child(left)
            .child(right)
    }

    fn render_headers(&self, rows: &[Row], cx: &mut Context<Self>) -> impl IntoElement {
        let headers: Vec<_> = rows
            .iter()
            .map(|row| {
                let track = &self.project.tracks[row.track];
                let id = track.id.clone();
                let kind_icon = match track.kind {
                    TrackKind::Video => IconName::Film,
                    TrackKind::Audio => IconName::Music2,
                    TrackKind::Text => IconName::Type,
                    TrackKind::Sticker => IconName::Sticker,
                    TrackKind::Effect => IconName::Sparkles,
                };
                let toggle = |name: &str,
                              icon: IconName,
                              on: bool,
                              tooltip: &'static str,
                              flag: Flag,
                              cx: &mut Context<Self>| {
                    let id = id.clone();
                    Button::new(SharedString::from(format!("{name}-{id}")))
                        .ghost()
                        .small()
                        .icon(Icon::new(icon).text_color(rgb(if on { ACCENT } else { TEXT_DIM })))
                        .tooltip(tooltip)
                        .on_click(
                            cx.listener(move |this, _, _, cx| this.toggle_track(&id, flag, cx)),
                        )
                        .into_any_element()
                };
                let spacer = || div().w(px(22.0)).into_any_element();
                let lock = toggle(
                    "lock",
                    if track.locked {
                        IconName::Lock
                    } else {
                        IconName::LockOpen
                    },
                    track.locked,
                    "Lock track",
                    Flag::Lock,
                    cx,
                );
                let eye = if track.kind == TrackKind::Audio {
                    spacer()
                } else {
                    toggle(
                        "hide",
                        if track.hidden {
                            IconName::EyeOff
                        } else {
                            IconName::Eye
                        },
                        track.hidden,
                        "Hide track",
                        Flag::Hide,
                        cx,
                    )
                };
                let mute = if matches!(track.kind, TrackKind::Video | TrackKind::Audio) {
                    toggle(
                        "mute",
                        if track.muted {
                            IconName::VolumeX
                        } else {
                            IconName::Volume2
                        },
                        track.muted,
                        "Mute track",
                        Flag::Mute,
                        cx,
                    )
                } else {
                    spacer()
                };
                div()
                    .absolute()
                    .left(px(0.0))
                    .top(px(row.top))
                    .w_full()
                    .h(px(row.height))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(2.0))
                    .pl(px(8.0))
                    .child(
                        div()
                            .w(px(22.0))
                            .flex()
                            .justify_center()
                            .child(Icon::new(kind_icon).small().text_color(rgb(TEXT_DIM))),
                    )
                    .child(lock)
                    .child(eye)
                    .child(mute)
            })
            .collect();
        div()
            .id("track-headers")
            .w(px(HEADER_W))
            .flex_none()
            .h_full()
            .relative()
            .overflow_hidden()
            .bg(rgb(LANES_BG))
            .border_r_1()
            .border_color(rgb(BORDER))
            .on_scroll_wheel(cx.listener(Self::on_timeline_scroll))
            .children(headers)
    }

    fn render_cover_column(&self, rows: &[Row]) -> impl IntoElement {
        let cover = rows.iter().find(|row| row.main).map(|row| {
            let size = (row.height - 20.0).clamp(30.0, 40.0);
            div()
                .absolute()
                .left(px((LEAD_W - size) / 2.0))
                .top(px(row.top + (row.height - size) / 2.0))
                .w(px(size))
                .h(px(size))
                .rounded(px(4.0))
                .bg(rgb(PANEL_RAISED))
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(px(1.0))
                .child(
                    Icon::new(IconName::PencilLine)
                        .small()
                        .text_color(rgb(TEXT)),
                )
                .child(
                    div()
                        .text_size(px(10.0))
                        .text_color(rgb(TEXT))
                        .child("Cover"),
                )
        });
        div()
            .w(px(LEAD_W))
            .flex_none()
            .h_full()
            .relative()
            .overflow_hidden()
            .bg(rgb(LANES_BG))
            .children(cover)
    }

    fn render_lanes(
        &mut self,
        rows: &[Row],
        lanes_w: f32,
        lanes_h: f32,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let probe = Rc::clone(&self.timeline.lanes);
        let zoom = self.timeline.zoom;

        // --- ruler
        let (step, minor) = ruler_step(zoom, self.project.fps, 96.0);
        let first = ((self.timeline.scroll_x / zoom) as f64 / step)
            .floor()
            .max(0.0) as i64;
        let last = (((self.timeline.scroll_x + lanes_w) / zoom) as f64 / step).ceil() as i64;
        let mut ruler = Vec::new();
        for i in first..=last {
            let seconds = i as f64 * step;
            let x = self.time_to_x((seconds * 1_000_000.0).round() as Micros);
            ruler.push(
                div()
                    .absolute()
                    .left(px(x))
                    .top(px(6.0))
                    .h(px(12.0))
                    .border_l_1()
                    .border_color(rgb(TEXT_DIM))
                    .pl(px(3.0))
                    .text_size(px(10.0))
                    .line_height(px(12.0))
                    .text_color(rgb(TEXT_DIM))
                    .child(ruler_label(seconds, self.project.fps))
                    .into_any_element(),
            );
            for m in 1..minor {
                let offset = (step * m as f64 / minor as f64) as f32 * zoom;
                // A tick under the label would cross out its last digit.
                if offset < 40.0 {
                    continue;
                }
                let x = x + offset;
                ruler.push(
                    div()
                        .absolute()
                        .left(px(x))
                        .top(px(10.0))
                        .w(px(1.0))
                        .h(px(4.0))
                        .bg(rgb(BORDER))
                        .into_any_element(),
                );
            }
        }
        for marker in &self.project.markers {
            let x = self.time_to_x(marker.time);
            ruler.push(
                div()
                    .absolute()
                    .left(px(x - 4.0))
                    .top(px(RULER_H - 11.0))
                    .w(px(8.0))
                    .h(px(9.0))
                    .rounded_t(px(2.0))
                    .bg(rgb(marker_color(marker.color)))
                    .into_any_element(),
            );
        }

        // --- lanes and clips
        // A handle of its own, so drawing a clip can ask for its filmstrip
        // while the lanes are being walked.
        let project = Arc::clone(&self.project);
        let mut lanes = Vec::new();
        let mut clips = Vec::new();
        let mut overlay = Vec::new();
        let trim_shift = match &self.timeline.drag {
            Some(Drag::Trim {
                segment_id,
                ripple: true,
                target,
                ..
            }) => self.project.segment(segment_id).map(|(track, segment)| {
                (
                    track.id.clone(),
                    segment.target_range.start,
                    target.end() - segment.target_range.end(),
                )
            }),
            _ => None,
        };
        for row in rows {
            let track = &project.tracks[row.track];
            lanes.push(
                div()
                    .absolute()
                    .left(px(0.0))
                    .top(px(row.top))
                    .w_full()
                    .h(px(row.height))
                    .bg(rgb(if row.main { TRACK_HEADER } else { ROW_BG })),
            );
            for segment in &track.segments {
                let mut target = segment.target_range;
                let mut source = segment.source_range;
                let mut top = row.top;
                let mut height = row.height;
                let mut dragged = false;
                match &self.timeline.drag {
                    Some(Drag::Clip {
                        segment_id,
                        moved: true,
                        track: to,
                        start,
                        ..
                    }) if *segment_id == segment.id => {
                        if let Some(to_row) = self.row_of(to) {
                            top = to_row.top;
                            height = to_row.height;
                        }
                        target.start = *start;
                        dragged = true;
                    }
                    Some(Drag::Trim {
                        segment_id,
                        target: live_target,
                        source: live_source,
                        ..
                    }) if *segment_id == segment.id => {
                        target = *live_target;
                        source = *live_source;
                    }
                    _ => {}
                }
                if let Some((lane, after, shift)) = &trim_shift {
                    if *lane == track.id && segment.target_range.start > *after {
                        target.start += shift;
                    }
                }
                let x0 = self.time_to_x(target.start);
                let x1 = self.time_to_x(target.end());
                if x1 < 0.0 || x0 > lanes_w {
                    continue;
                }
                let element = self.render_clip(
                    track, segment, target, source, x0, x1, top, height, row.main, lanes_w, cx,
                );
                if dragged {
                    overlay.push(element);
                } else {
                    clips.push(element);
                }
            }
        }

        // Where a clip dragged onto the magnetic main lane will land.
        if let Some(Drag::Clip {
            segment_id,
            moved: true,
            track,
            start,
            ..
        }) = &self.timeline.drag
        {
            if self.timeline.magnet && self.is_main_track(track) {
                if let (Some(lane), Some(row)) = (self.project.track(track), self.row_of(track)) {
                    let duration = self
                        .project
                        .segment(segment_id)
                        .map(|(_, s)| s.target_range.duration)
                        .unwrap_or(0);
                    let index = ripple::insertion_index(lane, segment_id, start + duration / 2);
                    let at: Micros = lane
                        .segments
                        .iter()
                        .filter(|s| s.id != *segment_id)
                        .take(index)
                        .map(|s| s.target_range.duration)
                        .sum();
                    overlay.push(
                        div()
                            .absolute()
                            .left(px(self.time_to_x(at) - 1.0))
                            .top(px(row.top - 2.0))
                            .w(px(3.0))
                            .h(px(row.height + 4.0))
                            .rounded(px(1.0))
                            .bg(rgb(ACCENT))
                            .into_any_element(),
                    );
                }
            }
        }

        // --- guides: snap line, blade line, playhead
        let tracks_bottom = lanes_h - SCROLLBAR_H;
        let snap_line = self.timeline.snap.map(|time| {
            div()
                .absolute()
                .left(px(self.time_to_x(time)))
                .top(px(RULER_H))
                .w(px(1.0))
                .h(px((tracks_bottom - RULER_H).max(0.0)))
                .bg(rgb(SNAP_LINE))
        });
        let blade_line = match (self.timeline.tool, self.timeline.hover) {
            (Tool::Blade, Some(position)) if self.timeline.drag.is_none() => {
                let (x, _) = self.lanes_local(position);
                Some(
                    div()
                        .absolute()
                        .left(px(x))
                        .top(px(RULER_H))
                        .w(px(1.0))
                        .h(px((tracks_bottom - RULER_H).max(0.0)))
                        .bg(gpui::white().opacity(0.6)),
                )
            }
            _ => None,
        };
        let playhead_x = self.time_to_x(self.clock.position());
        let playhead = (playhead_x >= -6.0 && playhead_x <= lanes_w + 6.0).then(|| {
            div()
                .absolute()
                .left(px(playhead_x - 5.0))
                .top(px(2.0))
                .w(px(11.0))
                .h(px(tracks_bottom - 2.0))
                .child(
                    div()
                        .absolute()
                        .left(px(5.0))
                        .top(px(10.0))
                        .w(px(1.0))
                        .h(px((tracks_bottom - 12.0).max(0.0)))
                        .bg(rgb(PLAYHEAD)),
                )
                .child(
                    div()
                        .absolute()
                        .left(px(0.0))
                        .top(px(0.0))
                        .w(px(11.0))
                        .h(px(14.0))
                        .rounded(px(3.0))
                        .border_1()
                        .border_color(rgb(PLAYHEAD))
                        .bg(rgb(0x3a3a3a)),
                )
        });

        // --- scrollbar
        let content = self.content_width();
        let scrollbar = (content > lanes_w + 1.0).then(|| {
            let thumb_w = (lanes_w * lanes_w / content).max(30.0);
            let thumb_x = self.timeline.scroll_x / content * lanes_w;
            div()
                .absolute()
                .left(px(thumb_x))
                .top(px(lanes_h - SCROLLBAR_H + 3.0))
                .w(px(thumb_w))
                .h(px(6.0))
                .rounded_full()
                .bg(rgb(SCROLL_THUMB))
        });

        div()
            .id("timeline-lanes")
            .flex_1()
            .min_w(px(0.0))
            .h_full()
            .relative()
            .overflow_hidden()
            .bg(rgb(LANES_BG))
            .when(self.timeline.tool == Tool::Blade, |this| {
                this.cursor(CursorStyle::Crosshair)
            })
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_lanes_down))
            .on_scroll_wheel(cx.listener(Self::on_timeline_scroll))
            .child(
                canvas(move |bounds, _, _| probe.set(bounds), |_, _, _, _| {})
                    .absolute()
                    .size_full(),
            )
            .child(
                div()
                    .absolute()
                    .left(px(0.0))
                    .top(px(RULER_H))
                    .w_full()
                    .h(px((tracks_bottom - RULER_H).max(0.0)))
                    .overflow_hidden()
                    // Children are positioned in lanes coordinates; this box
                    // only clips them below the ruler.
                    .child(
                        div()
                            .absolute()
                            .left(px(0.0))
                            .top(px(-RULER_H))
                            .size_full()
                            .children(lanes)
                            .children(clips)
                            .children(overlay),
                    ),
            )
            .child(
                div()
                    .absolute()
                    .left(px(0.0))
                    .top(px(0.0))
                    .w_full()
                    .h(px(RULER_H))
                    .children(ruler),
            )
            .children(snap_line)
            .children(blade_line)
            .children(playhead)
            .children(scrollbar)
    }

    #[allow(clippy::too_many_arguments)]
    fn render_clip(
        &mut self,
        track: &Track,
        segment: &Segment,
        target: TimeRange,
        source: TimeRange,
        x0: f32,
        x1: f32,
        top: f32,
        height: f32,
        main: bool,
        lanes_w: f32,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let _ = target;
        let selected = self.selected.as_deref() == Some(segment.id.as_str());
        let width = (x1 - x0 - 1.0).max(2.0);
        let name = self.material_name(&segment.material_id);
        let kind = track.kind;
        let color = self.clip_color(kind, &segment.material_id);
        let zoom = self.timeline.zoom;
        let speed = if segment.speed.is_finite() && segment.speed > 0.0 {
            segment.speed
        } else {
            1.0
        };

        let mut body = div()
            .absolute()
            .left(px(x0))
            .top(px(top))
            .w(px(width))
            .h(px(height))
            .rounded(px(4.0))
            .overflow_hidden()
            .bg(rgb(color));
        if track.hidden {
            body = body.opacity(0.45);
        }

        // The name stays in view while the clip's head is scrolled off.
        let label_left = (-x0).max(0.0) + 2.0;
        let label = |bg: u32| {
            div()
                .absolute()
                .left(px(label_left))
                .top(px(1.0))
                .max_w(px((width - label_left - 2.0).max(0.0)))
                .h(px(TITLE_H - 2.0))
                .px(px(4.0))
                .rounded(px(2.0))
                .bg(rgb(bg))
                .overflow_hidden()
                .whitespace_nowrap()
                .text_size(px(11.0))
                .line_height(px(TITLE_H - 2.0))
                .text_color(rgb(TEXT))
                .child(name.clone())
        };

        match kind {
            TrackKind::Video => {
                let thumbs_h = if main {
                    height - TITLE_H - 14.0
                } else {
                    height - TITLE_H
                };
                body = body.child(
                    div()
                        .absolute()
                        .left(px(0.0))
                        .top(px(0.0))
                        .w_full()
                        .h(px(TITLE_H))
                        .bg(rgb(CLIP_VIDEO_TITLE)),
                );
                if let Some(picture) = self.picture(&segment.material_id) {
                    let tile_w = (thumbs_h * picture.aspect).max(8.0);
                    let count = if picture.still {
                        1
                    } else {
                        media_cache::strip_count(picture.duration, zoom / speed, tile_w)
                    };
                    self.request_strip(&segment.material_id, picture.clone(), count, cx);
                    if let Some(strip) = self.timeline.media.strip(&segment.material_id, count) {
                        let tiles = strip.tiles.clone();
                        let duration = picture.duration.max(1);
                        let src_start = source.start;
                        body = body.child(
                            canvas(
                                |_, _, _| {},
                                move |bounds, _, window, _| {
                                    let n = tiles.len();
                                    if n == 0 {
                                        return;
                                    }
                                    let clip_w = f32::from(bounds.size.width);
                                    let first = ((-x0) / tile_w).floor().max(0.0) as i64;
                                    let last = ((lanes_w - x0) / tile_w).ceil() as i64;
                                    for k in first..=last {
                                        let slot = k as f32 * tile_w;
                                        if slot >= clip_w {
                                            break;
                                        }
                                        let at = src_start
                                            + (slot as f64 / zoom as f64 * speed as f64 * 1e6)
                                                as Micros;
                                        let index = ((at as f64 / duration as f64 * n as f64)
                                            as usize)
                                            .min(n - 1);
                                        let image = Bounds::new(
                                            point(bounds.origin.x + px(slot), bounds.origin.y),
                                            size(px(tile_w), bounds.size.height),
                                        );
                                        let _ = window.paint_image(
                                            bounds,
                                            image,
                                            Corners::default(),
                                            Arc::clone(&tiles[index]),
                                            0,
                                            false,
                                        );
                                    }
                                },
                            )
                            .absolute()
                            .left(px(0.0))
                            .top(px(TITLE_H))
                            .w_full()
                            .h(px(thumbs_h)),
                        );
                    }
                }
                body = body.child(label(CLIP_VIDEO));
            }
            TrackKind::Audio => {
                self.request_wave(&segment.material_id, cx);
                if let Some(wave) = self.timeline.media.wave(&segment.material_id) {
                    let wave = Arc::clone(wave);
                    let muted = track.muted;
                    let src_start = source.start;
                    body = body.child(
                        canvas(
                            |_, _, _| {},
                            move |bounds, _, window, _| {
                                paint_wave(
                                    window, bounds, &wave, src_start, speed, zoom, x0, lanes_w,
                                    muted,
                                )
                            },
                        )
                        .absolute()
                        .left(px(0.0))
                        .top(px(TITLE_H + 2.0))
                        .w_full()
                        .h(px((height - TITLE_H - 3.0).max(1.0))),
                    );
                }
                body = body.child(label(AUDIO_TITLE));
            }
            _ => {
                body = body.child(label(color));
            }
        }

        // The edges: a resize cursor where a press trims.
        if !track.locked && self.timeline.tool == Tool::Select && width > 3.0 * EDGE_GRAB {
            body = body
                .child(
                    div()
                        .absolute()
                        .left(px(0.0))
                        .top(px(0.0))
                        .w(px(EDGE_GRAB))
                        .h_full()
                        .cursor_ew_resize(),
                )
                .child(
                    div()
                        .absolute()
                        .right(px(0.0))
                        .top(px(0.0))
                        .w(px(EDGE_GRAB))
                        .h_full()
                        .cursor_ew_resize(),
                );
        }
        if selected {
            body = body.child(
                div()
                    .absolute()
                    .left(px(0.0))
                    .top(px(0.0))
                    .size_full()
                    .rounded(px(4.0))
                    .border_2()
                    .border_color(rgb(PLAYHEAD)),
            );
        }
        if track.locked {
            body = body.child(
                div()
                    .absolute()
                    .left(px(0.0))
                    .top(px(0.0))
                    .size_full()
                    .bg(gpui::black().opacity(0.35)),
            );
        }
        body.into_any_element()
    }
}

/// Bars rising from the bottom of an audio clip, one per two pixels, each the
/// loudest peak of the stretch of audio under it.
#[allow(clippy::too_many_arguments)]
fn paint_wave(
    window: &mut Window,
    bounds: Bounds<Pixels>,
    wave: &chukcut_engine::modules::media::Waveform,
    src_start: Micros,
    speed: f32,
    zoom: f32,
    x0: f32,
    lanes_w: f32,
    muted: bool,
) {
    let buckets = wave.buckets;
    if buckets == 0 || wave.duration <= 0 {
        return;
    }
    let clip_w = f32::from(bounds.size.width);
    let height = f32::from(bounds.size.height);
    let color = if muted {
        rgb(CLIP_AUDIO_WAVE).opacity(0.35)
    } else {
        rgb(CLIP_AUDIO_WAVE).opacity(1.0)
    };
    let per_px = speed as f64 / zoom as f64 * 1e6; // source µs per pixel
    let bucket_of = |at: f64| -> usize {
        ((at / wave.duration as f64 * buckets as f64).max(0.0) as usize).min(buckets - 1)
    };
    let mut x = ((-x0).max(0.0) / 2.0).floor() * 2.0;
    let end = clip_w.min(lanes_w - x0);
    while x < end {
        let from = src_start as f64 + x as f64 * per_px;
        let (a, b) = (bucket_of(from), bucket_of(from + 2.0 * per_px));
        let mut peak = 0.0f32;
        for i in a..=b.max(a) {
            peak = peak.max(wave.max[i]).max(-wave.min[i]);
        }
        let bar = (peak.clamp(0.0, 1.0) * height).max(1.0);
        window.paint_quad(fill(
            Bounds::new(
                point(bounds.origin.x + px(x), bounds.origin.y + px(height - bar)),
                size(px(1.0), px(bar)),
            ),
            color,
        ));
        x += 2.0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ruler_steps_keep_labels_apart() {
        // Default zoom, 60 px/s: labels every two seconds.
        assert_eq!(ruler_step(60.0, 30.0, 96.0).0, 2.0);
        // Far out: minutes.
        assert!(ruler_step(0.5, 30.0, 96.0).0 >= 120.0);
        // Far in: single frames.
        let (step, _) = ruler_step(2400.0, 30.0, 70.0);
        assert!((step - 1.0 / 30.0).abs() < 1e-9);
    }

    #[test]
    fn ruler_labels_read_like_capcut() {
        assert_eq!(ruler_label(0.0, 30.0), "00:00");
        assert_eq!(ruler_label(90.0, 30.0), "01:30");
        assert_eq!(ruler_label(1.5, 30.0), "15f");
        assert_eq!(ruler_label(3725.0, 30.0), "1:02:05");
    }

    #[test]
    fn the_zoom_slider_round_trips() {
        for zoom in [ZOOM_MIN, 1.0, 60.0, 500.0, ZOOM_MAX] {
            let back = slider_to_zoom(zoom_to_slider(zoom));
            assert!(
                (back - zoom).abs() / zoom < 1e-3,
                "{zoom} came back as {back}"
            );
        }
    }
}
