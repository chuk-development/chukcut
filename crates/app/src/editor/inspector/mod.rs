//! The inspector on the right: the selected clip's properties, or the
//! project's details when nothing is selected.
//!
//! Laid out like CapCut's: a row of top tabs that depends on what is selected
//! (Video · Speed · Animation · Adjust for a picture, Basic · Voice changer ·
//! Speed for sound), segmented sub-tabs under it, and collapsible sections of
//! property rows. Every row is a slider and/or a number box, a reset and a
//! keyframe diamond.
//!
//! ## How a value reaches the document
//!
//! Every change is an engine command — `timeline_apply` with an `EditCommand`,
//! `inspector_set_color` or `inspector_set_speed` — so it lands on the one undo
//! stack. A slider drag must look live but be **one** undo step, so while the
//! thumb moves nothing is written to the document: the command is built against
//! the document as it was when the drag started and applied to a *copy*, which
//! becomes the editor's drawing snapshot (the preview renders it, the panel reads
//! it). On release the copy is dropped and the same command is applied once,
//! through the command layer, against the real document. See [`Preview`].

use std::collections::{HashMap, HashSet};

use chukcut_engine::modules::inspector::commands as inspector_commands;
use chukcut_engine::modules::inspector::edit::{self as inspector_edit, GradeControl, GradeEdit};
use chukcut_engine::modules::project::grade::{CurveChannel, WheelKind};
use chukcut_engine::modules::project::{AnimatableProperty, Easing, Keyframe, Segment, Transform};
use gpui::component::input::{InputEvent, InputState};
use gpui::component::slider::{SliderEvent, SliderState};
use gpui::{Entity, Focusable, Subscription};

use super::*;

mod analysis;
mod animation;
mod audio_fx;
mod clip;
mod controls;
mod details;
mod easing;
mod effects;
mod grading;
mod speed;
mod text_style;
mod tracking;
mod voice;

pub(super) use details::SettingsForm;
pub(super) use easing::{easing_submenu, EasingTarget};

// --- state -----------------------------------------------------------------------

/// Everything the inspector remembers between frames. One field on
/// [`Editor`], so the panel's state never spreads into the editor itself.
#[derive(Default)]
pub(crate) struct Inspector {
    /// The top tab, by label, so a video and an audio clip can each keep
    /// their own without an enum per clip kind.
    tab: Option<&'static str>,
    /// The sub-tab under each top tab.
    sub_tab: HashMap<&'static str, &'static str>,
    /// Collapsed sections, by title.
    collapsed: HashSet<&'static str>,
    /// CapCut's "uniform scale" switch. Presentation only: it decides whether
    /// the panel shows one scale or a width and a height.
    non_uniform_scale: bool,
    fields: HashMap<Prop, Field>,
    preview: Option<Preview>,
    /// The project settings form, while it is open.
    settings: Option<SettingsForm>,
    /// The Adjust tab's own state: HSL band, curve and wheel drags, LUT list.
    grading: grading::GradingState,
    /// The Effects tab's sliders and drag.
    effects: effects::EffectsPanel,
    /// The audio effect sliders of the Audio tab.
    audio_fx: audio_fx::AudioFxPanel,
    /// The Animation tab's hover, sliders and drags.
    animation: animation::AnimationTab,
    /// The Speed tab's curve editor.
    speed: speed::SpeedTab,
    /// The keyframe easing graph.
    easing: easing::EasingState,
    /// The Text tab's words field and colour pickers.
    text: text_style::TextTab,
}

/// The widgets behind one property: a number box and, for most, a slider.
struct Field {
    input: Entity<InputState>,
    slider: Option<Entity<SliderState>>,
    _subscriptions: Vec<Subscription>,
}

/// A slider drag in progress. `base` is the editor's snapshot from before the
/// drag, which every intermediate value is built against, so the copy shown
/// never accumulates the drag's own steps.
struct Preview {
    prop: Prop,
    base: Arc<Project>,
}

// --- properties ------------------------------------------------------------------

/// A value the panel shows and edits, in the units CapCut shows it in.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(crate) enum Prop {
    Scale,
    ScaleX,
    ScaleY,
    PosX,
    PosY,
    Rotation,
    Opacity,
    Speed,
    Duration,
    Volume,
    FadeIn,
    FadeOut,
    Temperature,
    Tint,
    Saturation,
    Brightness,
    Contrast,
    Highlights,
    Shadows,
    Whites,
    Blacks,
    Exposure,
    Vibrance,
    Sharpen,
    Clarity,
    Grain,
    Fade,
    Vignette,
    VignetteMidpoint,
    VignetteFeather,
    LutIntensity,
    /// One HSL band's three sliders, by band index.
    HslHue(u8),
    HslSaturation(u8),
    HslLuminance(u8),
    /// A colour wheel's luminance slider.
    WheelLuma(WheelKind),
    /// Not a slider: the drag of a curve point or a wheel puck, which uses
    /// the same preview-then-commit machinery as a slider drag.
    Curve(CurveChannel),
    WheelPuck(WheelKind),
    /// A point of the speed curve being dragged.
    SpeedCurve,
    /// A handle of the keyframe easing graph being dragged.
    Easing,
    // --- a title's material (`text_style.rs`) ---
    TextSize,
    LetterSpacing,
    LineSpacing,
    TextOpacity,
    StrokeWidth,
    ShadowX,
    ShadowY,
    ShadowBlur,
    BoxPadding,
    BoxRadius,
    /// Not rows: a colour drag, the words being typed, and the toggles, so
    /// each previews and commits through the same machinery.
    TextColor(text_style::ColorSlot),
    TextContent,
    TextFlags,
}

/// The fixed facts about a [`Prop`].
pub(crate) struct Spec {
    pub label: &'static str,
    pub min: f32,
    pub max: f32,
    pub step: f32,
    pub default: f32,
    pub decimals: usize,
    pub suffix: &'static str,
    pub slider: bool,
    /// Whether the engine can store this value. Unsupported properties are
    /// drawn, disabled, so the panel has CapCut's shape and says what is
    /// missing.
    pub supported: bool,
}

/// The speed slider's marks. CapCut spaces them evenly although the values
/// are not, so the slider moves through a piecewise-linear map of these.
pub(crate) const SPEED_KNOTS: [f32; 6] = [0.1, 1.0, 2.0, 5.0, 10.0, 100.0];

/// Volume below this many dB is silence.
const MIN_DB: f32 = -60.0;

impl Prop {
    pub(crate) fn spec(self) -> Spec {
        let spec = |label, min, max, step, default, decimals, suffix, slider| Spec {
            label,
            min,
            max,
            step,
            default,
            decimals,
            suffix,
            slider,
            supported: true,
        };
        let colour = |label, min: f32| spec(label, min, 100.0, 1.0, 0.0, 0, "", true);
        match self {
            Prop::Scale => spec("Scale", 1.0, 400.0, 1.0, 100.0, 0, "%", true),
            Prop::ScaleX => spec("Scale width", 1.0, 400.0, 1.0, 100.0, 0, "%", true),
            Prop::ScaleY => spec("Scale height", 1.0, 400.0, 1.0, 100.0, 0, "%", true),
            Prop::PosX => spec("X", -20000.0, 20000.0, 1.0, 0.0, 0, "", false),
            Prop::PosY => spec("Y", -20000.0, 20000.0, 1.0, 0.0, 0, "", false),
            Prop::Rotation => spec("Rotate", -360.0, 360.0, 1.0, 0.0, 2, "°", false),
            Prop::Opacity => spec("Opacity", 0.0, 100.0, 1.0, 100.0, 0, "%", true),
            Prop::Speed => spec("Speed", 0.1, 100.0, 0.1, 1.0, 2, "x", true),
            Prop::Duration => spec("Duration", 0.01, 36000.0, 0.1, 0.0, 1, "s", false),
            Prop::Volume => spec("Volume", MIN_DB, 12.0, 0.1, 0.0, 1, "dB", true),
            Prop::FadeIn => spec("Fade in", 0.0, 10.0, 0.1, 0.0, 1, "s", true),
            Prop::FadeOut => spec("Fade out", 0.0, 10.0, 0.1, 0.0, 1, "s", true),
            Prop::Temperature => colour("Temperature", -100.0),
            Prop::Tint => colour("Tint", -100.0),
            Prop::Saturation => colour("Saturation", -100.0),
            Prop::Brightness => colour("Brightness", -100.0),
            Prop::Contrast => colour("Contrast", -100.0),
            Prop::Exposure => colour("Exposure", -100.0),
            Prop::Highlights => colour("Highlights", -100.0),
            Prop::Shadows => colour("Shadows", -100.0),
            Prop::Whites => colour("Whites", -100.0),
            Prop::Blacks => colour("Blacks", -100.0),
            Prop::Vibrance => colour("Vibrance", -100.0),
            Prop::Sharpen => colour("Sharpen", 0.0),
            Prop::Clarity => colour("Clarity", -100.0),
            Prop::Grain => colour("Grain", 0.0),
            Prop::Fade => colour("Fade", 0.0),
            Prop::Vignette => colour("Vignette", -100.0),
            Prop::VignetteMidpoint => spec("Midpoint", 0.0, 100.0, 1.0, 50.0, 0, "", true),
            Prop::VignetteFeather => spec("Feather", 0.0, 100.0, 1.0, 50.0, 0, "", true),
            Prop::LutIntensity => spec("Intensity", 0.0, 100.0, 1.0, 100.0, 0, "%", true),
            Prop::HslHue(_) => colour("Hue", -100.0),
            Prop::HslSaturation(_) => colour("Saturation", -100.0),
            Prop::HslLuminance(_) => colour("Luminance", -100.0),
            Prop::WheelLuma(_) => colour("Luminance", -100.0),
            Prop::Curve(_) => spec("Curve", 0.0, 1.0, 0.01, 0.0, 2, "", false),
            Prop::WheelPuck(_) => spec("Wheel", 0.0, 1.0, 0.01, 0.0, 2, "", false),
            Prop::SpeedCurve => spec("Speed curve", 0.0, 1.0, 0.01, 0.0, 2, "", false),
            Prop::Easing => spec("Easing", 0.0, 1.0, 0.01, 0.0, 2, "", false),
            Prop::TextSize => spec("Size", 4.0, 400.0, 1.0, 96.0, 0, "", true),
            Prop::LetterSpacing => spec("Letter spacing", -50.0, 200.0, 1.0, 0.0, 0, "", true),
            Prop::LineSpacing => spec("Line spacing", 50.0, 300.0, 1.0, 120.0, 0, "%", true),
            Prop::TextOpacity => spec("Opacity", 0.0, 100.0, 1.0, 100.0, 0, "%", true),
            Prop::StrokeWidth => spec("Width", 0.0, 100.0, 1.0, 4.0, 0, "", true),
            Prop::ShadowX => spec("X", -500.0, 500.0, 1.0, 0.0, 0, "", false),
            Prop::ShadowY => spec("Y", -500.0, 500.0, 1.0, 0.0, 0, "", false),
            Prop::ShadowBlur => spec("Blur", 0.0, 200.0, 1.0, 0.0, 0, "", true),
            Prop::BoxPadding => spec("Padding", 0.0, 300.0, 1.0, 0.0, 0, "", true),
            Prop::BoxRadius => spec("Corner radius", 0.0, 300.0, 1.0, 0.0, 0, "", true),
            Prop::TextColor(_) | Prop::TextContent | Prop::TextFlags => {
                spec("Text", 0.0, 1.0, 0.01, 0.0, 2, "", false)
            }
        }
    }

    /// The grade control behind a colour row, with how panel units map to
    /// document units: `document = offset + panel * scale`.
    pub(crate) fn grade_control(self) -> Option<(GradeControl, f32, f32)> {
        use GradeControl as G;
        let unit = |c| Some((c, 0.01, 0.0));
        match self {
            Prop::Temperature => unit(G::Temperature),
            Prop::Tint => unit(G::Tint),
            Prop::Saturation => Some((G::Saturation, 0.01, 1.0)),
            Prop::Brightness => unit(G::Brightness),
            Prop::Contrast => Some((G::Contrast, 0.01, 1.0)),
            // The slider's ±100 is ±2 stops: a wider range is a broken
            // shot, not a grade, and the number box still takes it.
            Prop::Exposure => Some((G::Exposure, 0.02, 0.0)),
            Prop::Highlights => unit(G::Highlights),
            Prop::Shadows => unit(G::Shadows),
            Prop::Whites => unit(G::Whites),
            Prop::Blacks => unit(G::Blacks),
            Prop::Vibrance => unit(G::Vibrance),
            Prop::Sharpen => unit(G::Sharpen),
            Prop::Clarity => unit(G::Clarity),
            Prop::Grain => unit(G::Grain),
            Prop::Fade => unit(G::Fade),
            Prop::Vignette => unit(G::VignetteAmount),
            Prop::VignetteMidpoint => unit(G::VignetteMidpoint),
            Prop::VignetteFeather => unit(G::VignetteFeather),
            Prop::LutIntensity => unit(G::LutIntensity),
            Prop::HslHue(band) => unit(G::HslHue(band)),
            Prop::HslSaturation(band) => unit(G::HslSaturation(band)),
            Prop::HslLuminance(band) => unit(G::HslLuminance(band)),
            Prop::WheelLuma(kind) => unit(G::WheelLuma(kind)),
            _ => None,
        }
    }

    /// The document properties a keyframe on this row animates.
    pub(crate) fn animated(self) -> &'static [AnimatableProperty] {
        use AnimatableProperty as A;
        match self {
            Prop::Scale => &[A::ScaleX, A::ScaleY],
            Prop::ScaleX => &[A::ScaleX],
            Prop::ScaleY => &[A::ScaleY],
            Prop::PosX => &[A::PositionX],
            Prop::PosY => &[A::PositionY],
            Prop::Rotation => &[A::Rotation],
            Prop::Opacity => &[A::Opacity],
            // No diamond on volume: the `Volume` keyframe track is the fade
            // envelope (it multiplies the clip volume), owned by the fade rows
            // and the timeline's fade handles.
            _ => &[],
        }
    }

    /// Slider position for a value. Linear except for speed, which moves
    /// through [`SPEED_KNOTS`] evenly.
    fn to_slider(self, value: f32) -> f32 {
        if self != Prop::Speed {
            return value;
        }
        let v = value.clamp(SPEED_KNOTS[0], SPEED_KNOTS[5]);
        for i in 0..5 {
            let (a, b) = (SPEED_KNOTS[i], SPEED_KNOTS[i + 1]);
            if v <= b {
                return i as f32 + (v - a) / (b - a);
            }
        }
        5.0
    }

    fn from_slider(self, position: f32) -> f32 {
        if self != Prop::Speed {
            return position;
        }
        let mut p = position.clamp(0.0, 5.0);
        // The marks are sticky, as in CapCut: a drag near "2x" lands on 2x
        // rather than 2.02x.
        if (p - p.round()).abs() < 0.04 {
            p = p.round();
        }
        let i = (p.floor() as usize).min(4);
        let (a, b) = (SPEED_KNOTS[i], SPEED_KNOTS[i + 1]);
        let v = a + (b - a) * (p - i as f32);
        // Two decimals is what the box shows; a drag lands on what it says.
        (v * 100.0).round() / 100.0
    }

    fn slider_range(self) -> (f32, f32, f32) {
        let spec = self.spec();
        if self == Prop::Speed {
            (0.0, 5.0, 0.001)
        } else {
            (spec.min, spec.max, spec.step)
        }
    }

    pub(crate) fn format(self, value: f32) -> String {
        let spec = self.spec();
        let value = if value.abs() < 0.5 * 10f32.powi(-(spec.decimals as i32)) {
            0.0
        } else {
            value
        };
        format!("{:.*}", spec.decimals, value)
    }
}

/// What one property change does to the document, as the command layer
/// spells it.
#[derive(Clone)]
enum Change {
    Edit(EditCommand),
    /// The clip's whole grade; `None` clears it.
    Grade(Option<GradeEdit>),
    Speed(f32),
}

/// Whether a value is being dragged (shown, not written) or settled.
#[derive(Clone, Copy, PartialEq)]
enum Phase {
    Preview,
    Commit,
}

/// The fade-in and fade-out lengths a clip's `Volume` envelope describes, read
/// by pattern: a fade-in is a first keyframe at 0 with value 0 followed by a
/// 1, a fade-out the mirror at the end. Anything else is no fade.
fn fades(segment: &Segment) -> (Micros, Micros) {
    let Some(track) = segment
        .keyframes
        .iter()
        .find(|t| t.property == AnimatableProperty::Volume)
    else {
        return (0, 0);
    };
    let k = track.keyframes.as_slice();
    let len = segment.target_range.duration;
    let near = |a: f32, b: f32| (a - b).abs() < 1e-3;
    let fade_in = match k {
        [first, second, ..]
            if first.time == 0 && near(first.value, 0.0) && near(second.value, 1.0) =>
        {
            second.time
        }
        _ => 0,
    };
    let fade_out = match k {
        [.., before, last]
            if last.time == len && near(last.value, 0.0) && near(before.value, 1.0) =>
        {
            len - before.time
        }
        _ => 0,
    };
    (fade_in, fade_out)
}

/// Rewrite the `Volume` envelope as the two fades: every keyframe on it goes
/// and the ramps are put back, as one undo step. The panel owns the envelope;
/// the UI has no other volume automation.
fn fade_command(
    segment: &Segment,
    fade_in: Micros,
    fade_out: Micros,
) -> Result<EditCommand, String> {
    use AnimatableProperty as A;
    let len = segment.target_range.duration;
    let fade_in = fade_in.clamp(0, len);
    let fade_out = fade_out.clamp(0, len - fade_in);
    let mut commands: Vec<EditCommand> = segment
        .keyframes
        .iter()
        .filter(|t| t.property == A::Volume)
        .flat_map(|t| t.keyframes.iter())
        .map(|&keyframe| EditCommand::RemoveKeyframe {
            segment_id: segment.id.clone(),
            property: A::Volume,
            keyframe,
        })
        .collect();
    let mut points: Vec<(Micros, f32)> = Vec::new();
    if fade_in > 0 {
        points.push((0, 0.0));
        points.push((fade_in, 1.0));
    }
    if fade_out > 0 {
        let apex = len - fade_out;
        if points.last().map(|p| p.0) != Some(apex) {
            points.push((apex, 1.0));
        }
        points.push((len, 0.0));
    }
    commands.extend(
        points
            .into_iter()
            .map(|(time, value)| EditCommand::AddKeyframe {
                segment_id: segment.id.clone(),
                property: A::Volume,
                keyframe: Keyframe {
                    time,
                    value,
                    easing: Easing::Linear,
                },
            }),
    );
    if commands.is_empty() {
        return Err("the clip has no fade to remove".into());
    }
    Ok(EditCommand::Composite {
        label: "Change fade".into(),
        commands,
    })
}

fn db_to_gain(db: f32) -> f32 {
    if db <= MIN_DB {
        0.0
    } else {
        10f32.powf(db / 20.0)
    }
}

fn gain_to_db(gain: f32) -> f32 {
    if gain <= 0.001 {
        MIN_DB
    } else {
        (20.0 * gain.log10()).max(MIN_DB)
    }
}

/// What a clip is, for choosing its tabs.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum ClipKind {
    Video,
    Image,
    Audio,
    Text,
}

// --- reading values ------------------------------------------------------------------

impl Editor {
    pub(super) fn selected_segment(&self) -> Option<(&Track, &Segment)> {
        let id = self.selected.as_ref()?;
        self.project.segment(id)
    }

    pub(crate) fn clip_kind(&self, segment: &Segment) -> ClipKind {
        let pool = &self.project.materials;
        let id = &segment.material_id;
        if pool.videos.iter().any(|m| &m.id == id) {
            ClipKind::Video
        } else if pool.images.iter().any(|m| &m.id == id) {
            ClipKind::Image
        } else if pool.audios.iter().any(|m| &m.id == id) {
            ClipKind::Audio
        } else {
            ClipKind::Text
        }
    }

    /// The playhead relative to the segment start, which is the time base of
    /// every keyframe. `None` when the playhead is outside the clip.
    fn relative_time(&self, segment: &Segment) -> Option<Micros> {
        let rel = self.clock.position() - segment.target_range.start;
        (0..=segment.target_range.duration)
            .contains(&rel)
            .then_some(rel)
    }

    /// Half a frame: a keyframe this close to the playhead is "at" it.
    fn keyframe_tolerance(&self) -> Micros {
        (500_000.0 / self.project.fps.max(1.0)) as Micros
    }

    /// The keyframe track value at the playhead, clamped into the clip.
    fn sampled(&self, segment: &Segment, property: AnimatableProperty) -> Option<f32> {
        let rel = (self.clock.position() - segment.target_range.start)
            .clamp(0, segment.target_range.duration);
        segment
            .keyframes
            .iter()
            .find(|t| t.property == property)
            .and_then(|t| t.sample(rel))
    }

    fn half_canvas(&self) -> (f32, f32) {
        (
            self.project.canvas.width as f32 * 0.5,
            self.project.canvas.height as f32 * 0.5,
        )
    }

    /// A property's value in panel units, as the clip shows it at the
    /// playhead.
    pub(crate) fn prop_value(&self, prop: Prop, segment: &Segment) -> f32 {
        use AnimatableProperty as A;
        let t = &segment.transform;
        let anim = |p: A, base: f32| self.sampled(segment, p).unwrap_or(base);
        let (hw, hh) = self.half_canvas();
        let colour = self.project.materials.color_adjust_of(segment);
        match prop {
            Prop::Scale | Prop::ScaleX => anim(A::ScaleX, t.scale[0]) * 100.0,
            Prop::ScaleY => anim(A::ScaleY, t.scale[1]) * 100.0,
            Prop::PosX => anim(A::PositionX, t.position[0]) * hw,
            Prop::PosY => anim(A::PositionY, t.position[1]) * hh,
            Prop::Rotation => anim(A::Rotation, t.rotation),
            Prop::Opacity => anim(A::Opacity, t.opacity) * 100.0,
            Prop::Speed => segment.speed,
            Prop::Duration => segment.target_range.duration as f32 / 1_000_000.0,
            Prop::Volume => gain_to_db(segment.volume),
            Prop::FadeIn => fades(segment).0 as f32 / 1_000_000.0,
            Prop::FadeOut => fades(segment).1 as f32 / 1_000_000.0,
            _ if prop.is_text() => self
                .project
                .materials
                .text(&segment.material_id)
                .map_or(prop.spec().default, |m| prop.text_value(m)),
            _ => match prop.grade_control() {
                Some((control, scale, offset)) => {
                    (control.get(&GradeEdit::of(colour)) - offset) / scale
                }
                None => prop.spec().default,
            },
        }
    }

    /// A panel value in document units, for one animated property.
    fn document_value(&self, property: AnimatableProperty, value: f32) -> f32 {
        use AnimatableProperty as A;
        let (hw, hh) = self.half_canvas();
        match property {
            A::ScaleX | A::ScaleY => value / 100.0,
            A::PositionX => value / hw,
            A::PositionY => value / hh,
            A::Rotation => value,
            A::Opacity => value / 100.0,
            A::Volume => db_to_gain(value),
        }
    }

    /// Whether this row's property has keyframes, and whether one sits at the
    /// playhead.
    pub(crate) fn keyframe_state(&self, prop: Prop, segment: &Segment) -> (bool, bool) {
        let Some(&first) = prop.animated().first() else {
            return (false, false);
        };
        let Some(track) = segment.keyframes.iter().find(|t| t.property == first) else {
            return (false, false);
        };
        let at = self.relative_time(segment).is_some_and(|rel| {
            let tolerance = self.keyframe_tolerance();
            track
                .keyframes
                .iter()
                .any(|k| (k.time - rel).abs() <= tolerance)
        });
        (true, at)
    }

    // --- building changes ----------------------------------------------------------------

    /// The change that sets `prop` to `value` on `segment_id`, built against
    /// `project`.
    fn build_change(
        &self,
        project: &Project,
        segment_id: &str,
        prop: Prop,
        value: f32,
    ) -> Result<Change, String> {
        let (_, segment) = project
            .segment(segment_id)
            .ok_or("the clip is no longer on the timeline")?;
        match prop {
            Prop::Speed => Ok(Change::Speed(value)),
            Prop::Duration => {
                let duration = (value as f64 * 1_000_000.0).round();
                if duration < 1.0 {
                    return Err("a clip must be longer than nothing".into());
                }
                let speed = segment.source_range.duration as f64 / duration;
                Ok(Change::Speed(speed as f32))
            }
            Prop::Volume => Ok(Change::Edit(EditCommand::SetVolume {
                segment_id: segment.id.clone(),
                before: segment.volume,
                after: db_to_gain(value).clamp(0.0, 4.0),
            })),
            Prop::FadeIn | Prop::FadeOut => {
                let (fade_in, fade_out) = fades(segment);
                let wanted = (value as f64 * 1_000_000.0).round() as Micros;
                let (fade_in, fade_out) = if prop == Prop::FadeIn {
                    (wanted, fade_out)
                } else {
                    (fade_in, wanted)
                };
                Ok(Change::Edit(fade_command(segment, fade_in, fade_out)?))
            }
            _ if prop.is_text() => {
                let before = project
                    .materials
                    .text(&segment.material_id)
                    .ok_or("the clip is not a title")?
                    .clone();
                let after = prop.with_text_value(&before, value);
                Ok(Change::Edit(EditCommand::SetTextMaterial { before, after }))
            }
            _ if prop.grade_control().is_some() => {
                let (control, scale, offset) = prop.grade_control().expect("checked");
                let mut edit = GradeEdit::of(project.materials.color_adjust_of(segment));
                if control == GradeControl::LutIntensity && edit.lut.is_none() {
                    return Err("the clip has no LUT".into());
                }
                control.set(&mut edit, offset + value * scale);
                Ok(Change::Grade(Some(edit)))
            }
            _ if prop.spec().supported => {
                Ok(Change::Edit(self.value_command(segment, prop, value)?))
            }
            _ => Err(format!("{} is not supported yet", prop.spec().label)),
        }
    }

    /// The command for a transform or volume value. A property with keyframes
    /// is edited *at the playhead* — the keyframe there gets the value, or a
    /// new one is added — because its static value is not what is shown.
    fn value_command(
        &self,
        segment: &Segment,
        prop: Prop,
        value: f32,
    ) -> Result<EditCommand, String> {
        use AnimatableProperty as A;
        let rel = (self.clock.position() - segment.target_range.start)
            .clamp(0, segment.target_range.duration);
        let tolerance = self.keyframe_tolerance();
        let mut commands = Vec::new();
        let mut transform: Transform = segment.transform;
        let mut transform_changed = false;
        for &property in prop.animated() {
            let doc = self.document_value(property, value);
            if let Some(track) = segment.keyframes.iter().find(|t| t.property == property) {
                match track
                    .keyframes
                    .iter()
                    .find(|k| (k.time - rel).abs() <= tolerance)
                {
                    Some(k) => commands.push(EditCommand::MoveKeyframe {
                        segment_id: segment.id.clone(),
                        property,
                        from_time: k.time,
                        to_time: k.time,
                        before_value: k.value,
                        after_value: doc,
                    }),
                    None => commands.push(EditCommand::AddKeyframe {
                        segment_id: segment.id.clone(),
                        property,
                        keyframe: Keyframe {
                            time: rel,
                            value: doc,
                            easing: Easing::default(),
                        },
                    }),
                }
                continue;
            }
            match property {
                A::ScaleX => transform.scale[0] = doc,
                A::ScaleY => transform.scale[1] = doc,
                A::PositionX => transform.position[0] = doc,
                A::PositionY => transform.position[1] = doc,
                A::Rotation => transform.rotation = doc,
                A::Opacity => transform.opacity = doc.clamp(0.0, 1.0),
                A::Volume => {
                    commands.push(EditCommand::SetVolume {
                        segment_id: segment.id.clone(),
                        before: segment.volume,
                        after: doc.clamp(0.0, 4.0),
                    });
                    continue;
                }
            }
            transform_changed = true;
        }
        if transform_changed {
            commands.insert(
                0,
                EditCommand::SetTransform {
                    segment_id: segment.id.clone(),
                    before: segment.transform,
                    after: transform,
                },
            );
        }
        match commands.len() {
            0 => Err("nothing to change".into()),
            1 => Ok(commands.pop().expect("one command")),
            _ => Ok(EditCommand::Composite {
                label: "Transform clip".into(),
                commands,
            }),
        }
    }

    /// `project` with `change` applied, for showing a drag. Never stored.
    fn previewed(project: &Project, segment_id: &str, change: &Change) -> Result<Project, String> {
        let mut copy = project.clone();
        match change {
            Change::Edit(command) => command.apply(&mut copy)?,
            Change::Grade(grade) => {
                let (material, command) =
                    inspector_edit::set_grade_command(&copy, segment_id, grade.clone())?;
                if let Some(material) = material {
                    copy.materials.color_adjusts.push(material);
                }
                command.apply(&mut copy)?;
            }
            Change::Speed(speed) => {
                inspector_edit::set_speed_command(&copy, segment_id, *speed)?.apply(&mut copy)?
            }
        }
        Ok(copy)
    }

    fn commit_change(&mut self, segment_id: String, change: Change, cx: &mut Context<Self>) {
        let result = match change {
            Change::Edit(command) => timeline_commands::timeline_apply(&self.state, command),
            Change::Grade(grade) => {
                inspector_commands::inspector_set_grade(&self.state, segment_id, grade)
            }
            Change::Speed(speed) => {
                inspector_commands::inspector_set_speed(&self.state, segment_id, speed)
            }
        }
        .map(|_| ());
        self.refresh(cx);
        self.report(result, cx);
    }

    /// Set a property of the selected clip. A preview shows the value without
    /// writing it; a commit writes it as one undo step (and ends any preview).
    fn set_prop(&mut self, prop: Prop, value: f32, phase: Phase, cx: &mut Context<Self>) {
        let Some(segment_id) = self.target_id(prop) else {
            return;
        };
        let spec = prop.spec();
        if !value.is_finite() || !spec.supported {
            return;
        }
        let value = value.clamp(spec.min, spec.max);

        if phase == Phase::Preview {
            let base = match &self.inspector.preview {
                Some(preview) if preview.prop == prop => Arc::clone(&preview.base),
                _ => {
                    let base = Arc::clone(&self.project);
                    self.inspector.preview = Some(Preview {
                        prop,
                        base: Arc::clone(&base),
                    });
                    base
                }
            };
            let shown = self
                .build_change(&base, &segment_id, prop, value)
                .and_then(|change| Self::previewed(&base, &segment_id, &change));
            if let Ok(project) = shown {
                self.project = Arc::new(project);
                self.generation += 1;
                self.audio.set_project(Arc::clone(&self.project));
                cx.notify();
            }
            return;
        }

        // A commit is built against the document as it is, never against a
        // preview copy.
        if let Some(preview) = self.inspector.preview.take() {
            self.project = preview.base;
            self.generation += 1;
        }
        let current = self
            .project
            .segment(&segment_id)
            .map(|(_, s)| self.prop_value(prop, s));
        let tiny = 0.5 * 10f32.powi(-(spec.decimals as i32));
        // Under a speed curve the constant speed is dormant, so setting the
        // value it already holds still means something: leave the curve.
        let curved = matches!(prop, Prop::Speed | Prop::Duration)
            && self
                .project
                .segment(&segment_id)
                .is_some_and(|(_, s)| self.project.materials.speed_curve_of(s).is_some());
        if !curved && current.is_some_and(|c| (c - value).abs() < tiny) {
            self.refresh(cx);
            return;
        }
        let project = Arc::clone(&self.project);
        match self.build_change(&project, &segment_id, prop, value) {
            Ok(change) => self.commit_change(segment_id, change, cx),
            Err(error) => {
                self.refresh(cx);
                self.report(Err(error), cx);
            }
        }
    }

    pub(crate) fn reset_prop(&mut self, prop: Prop, cx: &mut Context<Self>) {
        if self.reset_text_prop(prop, cx) {
            return;
        }
        self.set_prop(prop, prop.spec().default, Phase::Commit, cx);
    }

    pub(crate) fn step_prop(&mut self, prop: Prop, direction: f32, cx: &mut Context<Self>) {
        let Some(segment) = self.target_segment(prop) else {
            return;
        };
        let current = self.prop_value(prop, segment);
        self.set_prop(
            prop,
            current + direction * prop.spec().step,
            Phase::Commit,
            cx,
        );
    }

    /// The diamond: add a keyframe at the playhead with the value shown, or
    /// take away the one that is there.
    pub(crate) fn toggle_keyframe(&mut self, prop: Prop, cx: &mut Context<Self>) {
        let Some(segment) = self.target_segment(prop) else {
            return;
        };
        let Some(rel) = self.relative_time(segment) else {
            self.report(
                Err("move the playhead onto the clip to add a keyframe".into()),
                cx,
            );
            return;
        };
        let tolerance = self.keyframe_tolerance();
        let value = self.prop_value(prop, segment);
        let (_, at) = self.keyframe_state(prop, segment);
        let mut commands = Vec::new();
        for &property in prop.animated() {
            let existing = segment
                .keyframes
                .iter()
                .find(|t| t.property == property)
                .and_then(|t| {
                    t.keyframes
                        .iter()
                        .find(|k| (k.time - rel).abs() <= tolerance)
                        .copied()
                });
            match (at, existing) {
                (true, Some(keyframe)) => commands.push(EditCommand::RemoveKeyframe {
                    segment_id: segment.id.clone(),
                    property,
                    keyframe,
                }),
                (false, None) => commands.push(EditCommand::AddKeyframe {
                    segment_id: segment.id.clone(),
                    property,
                    keyframe: Keyframe {
                        time: rel,
                        value: self.document_value(property, value),
                        easing: Easing::default(),
                    },
                }),
                _ => {}
            }
        }
        let command = match commands.len() {
            0 => return,
            1 => commands.pop().expect("one command"),
            _ => EditCommand::Composite {
                label: if at {
                    "Delete keyframe".into()
                } else {
                    "Add keyframe".into()
                },
                commands,
            },
        };
        self.apply(Ok(command), cx);
    }

    /// The arrows beside the diamond: move the playhead to the previous or
    /// next keyframe of this row.
    pub(crate) fn jump_keyframe(&mut self, prop: Prop, forward: bool, cx: &mut Context<Self>) {
        let Some(segment) = self.target_segment(prop) else {
            return;
        };
        let start = segment.target_range.start;
        let rel = self.clock.position() - start;
        let tolerance = self.keyframe_tolerance();
        let times = segment
            .keyframes
            .iter()
            .filter(|t| prop.animated().contains(&t.property))
            .flat_map(|t| t.keyframes.iter().map(|k| k.time));
        let target = if forward {
            times.filter(|&t| t > rel + tolerance).min()
        } else {
            times.filter(|&t| t < rel - tolerance).max()
        };
        if let Some(time) = target {
            self.pause();
            self.seek(start + time);
            cx.notify();
        }
    }

    // --- the widgets behind a row ------------------------------------------------------------

    /// The number box and slider for `prop`, made on first use.
    fn field(
        &mut self,
        prop: Prop,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> (Entity<InputState>, Option<Entity<SliderState>>) {
        if let Some(field) = self.inspector.fields.get(&prop) {
            return (field.input.clone(), field.slider.clone());
        }
        let spec = prop.spec();
        let input = cx.new(|cx| InputState::new(window, cx));
        let mut subscriptions = vec![cx.subscribe_in(
            &input,
            window,
            move |this: &mut Editor, input, event: &InputEvent, window, cx| match event {
                InputEvent::PressEnter { .. } => {
                    this.commit_text(prop, input.clone(), cx);
                    // Back to the editor, so Space plays again.
                    window.focus(&this.focus, cx);
                }
                InputEvent::Blur => this.commit_text(prop, input.clone(), cx),
                _ => {}
            },
        )];
        let slider = spec.slider.then(|| {
            let (min, max, step) = prop.slider_range();
            let slider = cx.new(|_| {
                SliderState::new()
                    .min(min)
                    .max(max)
                    .step(step)
                    .default_value(prop.to_slider(spec.default))
            });
            subscriptions.push(cx.subscribe_in(
                &slider,
                window,
                move |this: &mut Editor, _, event: &SliderEvent, _, cx| match event {
                    SliderEvent::Change(value) => {
                        this.set_prop(prop, prop.from_slider(value.end()), Phase::Preview, cx)
                    }
                    SliderEvent::Release(value) => {
                        this.set_prop(prop, prop.from_slider(value.end()), Phase::Commit, cx)
                    }
                },
            ));
            slider
        });
        self.inspector.fields.insert(
            prop,
            Field {
                input: input.clone(),
                slider: slider.clone(),
                _subscriptions: subscriptions,
            },
        );
        (input, slider)
    }

    fn commit_text(&mut self, prop: Prop, input: Entity<InputState>, cx: &mut Context<Self>) {
        let text = input.read(cx).value().to_string();
        let cleaned: String = text
            .chars()
            .map(|c| if c == ',' { '.' } else { c })
            .filter(|c| c.is_ascii_digit() || *c == '.' || *c == '-')
            .collect();
        match cleaned.parse::<f32>() {
            Ok(value) => self.set_prop(prop, value, Phase::Commit, cx),
            // Unparseable text snaps back to the value on the next frame.
            Err(_) => cx.notify(),
        }
    }

    /// Make the widgets of `prop` show `value`, unless the user is typing in
    /// the box or dragging the slider.
    fn sync_field(&mut self, prop: Prop, value: f32, window: &mut Window, cx: &mut Context<Self>) {
        let (input, slider) = self.field(prop, window, cx);
        let text = prop.format(value);
        let focused = input.read(cx).focus_handle(cx).is_focused(window);
        if !focused && input.read(cx).value().as_ref() != text {
            input.update(cx, |state, cx| state.set_value(text, window, cx));
        }
        let dragging = self
            .inspector
            .preview
            .as_ref()
            .is_some_and(|p| p.prop == prop);
        if let Some(slider) = slider {
            let position = prop.to_slider(value);
            if !dragging && (slider.read(cx).value().end() - position).abs() > 1e-4 {
                slider.update(cx, |state, cx| state.set_value(position, window, cx));
            }
        }
    }

    // --- the panel -------------------------------------------------------------------------

    pub(super) fn render_inspector(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        self.inspector.text.begin_frame();
        self.inspector.effects.begin_frame();
        let body = match self.selected_segment().map(|(_, s)| self.clip_kind(s)) {
            Some(kind) => self
                .render_inspector_clip(kind, window, cx)
                .into_any_element(),
            None => self.render_details(window, cx).into_any_element(),
        };
        crate::ui::Panel::new("inspector").size_full().child(body)
    }
}
