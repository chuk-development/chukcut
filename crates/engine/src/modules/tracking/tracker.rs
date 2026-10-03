//! The object tracker: one box, frame after frame.
//!
//! Each step does three things:
//!
//! 1. **KLT.** Corners inside the current box are followed into the next
//!    frame by pyramidal Lucas–Kanade ([`super::klt`]), starting from the
//!    box's velocity so a fast object starts its search where it is going. A
//!    forward-backward check drops points that do not come back to where they
//!    started.
//! 2. **Similarity fit.** RANSAC over point pairs (two pairs fix a similarity)
//!    and a least-squares refit on the inliers give translation, scale and
//!    rotation together — the one thing a box tracker cannot give.
//! 3. **Appearance check.** A small colour template of the object, weighted
//!    towards its centre, is searched for (normalised cross-correlation)
//!    around the position the box's velocity predicts. The flow's answer is
//!    kept when it looks as much like the object as the best match does —
//!    it carries scale and rotation; otherwise the match wins. That is what
//!    survives a fast throw, motion blur, a flat-coloured ball whose edges
//!    suffer from the aperture problem, and a flow that locked on to static
//!    background inside the box. Neither convincing → the frame is `LOST` and
//!    the box coasts on its velocity while the search widens.
//!
//! Everything is deterministic: the RANSAC sampler is a fixed-seed generator,
//! so the same frames always give the same track — a project must render the
//! same on every machine.

use super::colour::ColourModel;
use super::klt::{good_features, track_point, FlowParams, Gray, Pyramid};

/// A box in analysis pixels, with the accumulated rotation.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BoxState {
    pub cx: f32,
    pub cy: f32,
    pub w: f32,
    pub h: f32,
    /// Degrees clockwise (image y points down, so a positive angle from the
    /// fit is a clockwise turn as the viewer sees it).
    pub angle: f32,
}

/// What one step produced.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StepResult {
    pub pose: BoxState,
    pub confidence: f32,
    pub lost: bool,
}

/// Tuning, mostly for tests; the defaults are what the app uses.
#[derive(Debug, Clone, Copy)]
pub struct TrackerParams {
    pub pyramid_levels: usize,
    pub max_points: usize,
    pub flow: FlowParams,
    /// Forward-backward error above which a point is dropped, in pixels.
    pub fb_threshold: f32,
    /// Template cells along the box's longer side.
    pub template_cells: usize,
}

impl Default for TrackerParams {
    fn default() -> Self {
        Self {
            pyramid_levels: 4,
            max_points: 60,
            flow: FlowParams::default(),
            fb_threshold: 1.0,
            template_cells: 16,
        }
    }
}

/// One frame as the tracker needs it: a luma pyramid with gradients for the
/// flow, and colour planes at the same levels for the appearance template.
///
/// Colour matters for the template and only there. A red ball on a red
/// wall has almost no luma edge; it still differs in green and blue, and the
/// correlation normalises away how small that difference is.
#[derive(Clone)]
pub struct Frame {
    pub(super) luma: Pyramid,
    pub(super) colour: Vec<[Gray; 3]>,
}

impl Frame {
    pub fn from_rgba(rgba: &[u8], width: usize, height: usize, levels: usize) -> Self {
        let plane = |c: usize| Gray {
            width,
            height,
            data: rgba
                .as_chunks::<4>()
                .0
                .iter()
                .take(width * height)
                .map(|p| p[c] as f32)
                .collect(),
        };
        let rgb = [plane(0), plane(1), plane(2)];
        let luma = Gray::from_rgba(rgba, width, height);
        Self::build(luma, rgb, levels)
    }

    /// A grey frame: the same plane three times. For tests.
    pub fn from_gray(gray: Gray, levels: usize) -> Self {
        let rgb = [gray.clone(), gray.clone(), gray.clone()];
        Self::build(gray, rgb, levels)
    }

    fn build(luma: Gray, rgb: [Gray; 3], levels: usize) -> Self {
        let luma = Pyramid::new(luma, levels);
        let mut colour = vec![rgb];
        while colour.len() < luma.levels.len() {
            let last = colour.last().expect("the base level");
            let next = [
                last[0].downsample(),
                last[1].downsample(),
                last[2].downsample(),
            ];
            colour.push(next);
        }
        Self { luma, colour }
    }

    pub fn width(&self) -> usize {
        self.luma.width()
    }

    pub fn height(&self) -> usize {
        self.luma.height()
    }
}

/// The appearance of the object: the box resampled onto a fixed grid in each
/// colour channel, mean-removed per channel, and weighted towards the centre
/// so the background in the box's corners — which changes as the object
/// moves — counts for little.
#[derive(Debug, Clone)]
struct Template {
    cols: usize,
    rows: usize,
    values: Vec<f32>,
    weights: Vec<f32>,
}

pub struct Tracker {
    params: TrackerParams,
    prev: Frame,
    state: BoxState,
    velocity: (f32, f32),
    template: Template,
    colour: ColourModel,
    /// The object's colour area and box size on the first frame: the size
    /// measured by colour is relative to these.
    colour_reference: (f32, f32, f32),
    /// Frames in a row without a confident position.
    lost_run: u32,
}

/// Correlation below which a frame is lost.
const MIN_SCORE: f32 = 0.45;

impl Tracker {
    /// Start on `frame` with the object in `init`.
    pub fn new(frame: Frame, init: BoxState, params: TrackerParams) -> Self {
        let template = sample_template(&frame, &init, params.template_cells);
        let colour = ColourModel::new(&frame, &init);
        let colour_reference = (colour.mass(&frame, &init).max(1.0), init.w, init.h);
        Self {
            params,
            prev: frame,
            state: init,
            velocity: (0.0, 0.0),
            template,
            colour,
            colour_reference,
            lost_run: 0,
        }
    }

    pub fn state(&self) -> BoxState {
        self.state
    }

    /// Follow the object into `next`.
    pub fn step(&mut self, next: Frame) -> StepResult {
        let prior = self.state;
        let predicted = BoxState {
            cx: prior.cx + self.velocity.0,
            cy: prior.cy + self.velocity.1,
            ..prior
        };

        let flow = self.flow(&next.luma);
        let flow_score = flow.map(|(pose, _)| ncc(&self.template, &next, &pose));

        // The appearance search, always: it is what catches a throw the flow
        // could not follow, and what overrules a flow that locked on to the
        // static background inside the box. Scale and rotation come from the
        // flow when it has them.
        let base_size = prior.w.max(prior.h);
        let speed = (self.velocity.0.powi(2) + self.velocity.1.powi(2)).sqrt();
        let lost_bonus = 1.0 + self.lost_run.min(10) as f32 * 0.25;
        let radius = ((0.6 * base_size + 2.0 * speed) * lost_bonus)
            .max(8.0)
            .min(0.5 * next.width().max(next.height()) as f32);
        let (w, h, angle) = flow.map_or((prior.w, prior.h, prior.angle), |(p, _)| {
            (p.w, p.h, p.angle)
        });
        let seed = BoxState {
            w,
            h,
            angle,
            ..predicted
        };
        // Colour first when the object's colours set it apart: it is absolute
        // (no drift) and blind to blur and background changes. The flow then
        // only refines it and supplies scale and rotation.
        let distinct = self.colour.distinctness;
        let by_colour = (distinct > 0.25)
            .then(|| self.colour.detect(&next, &seed, radius))
            .flatten()
            .filter(|&(_, _, score)| score > 0.25);

        let accepted = if let Some((x, y, score)) = by_colour {
            let confidence = (score / 0.8).clamp(0.0, 1.0);
            let position = match flow {
                Some((pose, _))
                    if (pose.cx - x).abs().max((pose.cy - y).abs()) < 0.25 * base_size =>
                {
                    let k = 0.5 * distinct;
                    (pose.cx + (x - pose.cx) * k, pose.cy + (y - pose.cy) * k)
                }
                _ => (x, y),
            };
            let quality = flow.map_or(0.0, |(_, q)| q);
            // Size from the colour area, eased in: absolute, so it cannot
            // drift, but noisy frame to frame. Rotation from the flow.
            let (mass, w0, h0) = self.colour_reference;
            let at = BoxState {
                cx: position.0,
                cy: position.1,
                ..prior
            };
            let size = (self.colour.mass(&next, &at) / mass).sqrt().clamp(0.5, 2.0);
            let ease = |current: f32, target: f32| current + 0.25 * (target - current);
            let pose = BoxState {
                cx: position.0,
                cy: position.1,
                w: ease(prior.w, w0 * size),
                h: ease(prior.h, h0 * size),
                angle: flow.map_or(prior.angle, |(p, _)| p.angle),
            };
            Some((pose, 0.3 * quality + 0.7 * confidence))
        } else {
            let found = search(&self.template, &next, &seed, radius);
            match (flow, flow_score, found) {
                (Some((pose, quality)), Some(score), found)
                    if score >= MIN_SCORE && found.is_none_or(|(_, s)| s <= score + 0.08) =>
                {
                    Some((pose, 0.4 * quality + 0.6 * score))
                }
                (_, _, Some((pose, score))) if score >= MIN_SCORE => Some((pose, 0.9 * score)),
                _ => None,
            }
        };

        let result = match accepted {
            Some((pose, confidence)) => {
                let damping = if self.lost_run > 0 { 0.5 } else { 0.7 };
                let measured = (pose.cx - prior.cx, pose.cy - prior.cy);
                self.velocity = (
                    damping * measured.0 + (1.0 - damping) * self.velocity.0,
                    damping * measured.1 + (1.0 - damping) * self.velocity.1,
                );
                self.state = pose;
                self.lost_run = 0;
                if confidence > 0.6 {
                    let fresh = sample_template(&next, &pose, self.params.template_cells);
                    blend_template(&mut self.template, &fresh, 0.15);
                    self.colour.update(&next, &pose, 0.05);
                }
                StepResult {
                    pose,
                    confidence: confidence.clamp(0.0, 1.0),
                    lost: false,
                }
            }
            None => {
                self.lost_run += 1;
                self.state = predicted;
                self.velocity = (self.velocity.0 * 0.8, self.velocity.1 * 0.8);
                StepResult {
                    pose: predicted,
                    confidence: 0.0,
                    lost: true,
                }
            }
        };
        self.prev = next;
        result
    }

    /// The KLT answer: the new box and a quality in `0..1`.
    fn flow(&self, next: &Pyramid) -> Option<(BoxState, f32)> {
        let s = self.state;
        let margin = 0.05;
        let region = (
            s.cx - s.w * (0.5 - margin),
            s.cy - s.h * (0.5 - margin),
            s.cx + s.w * (0.5 - margin),
            s.cy + s.h * (0.5 - margin),
        );
        let min_distance = (s.w.min(s.h) / 12.0).clamp(2.0, 12.0);
        let points = good_features(
            &self.prev.luma.levels[0],
            region,
            self.params.max_points,
            0.02,
            min_distance,
        );
        if points.len() < 4 {
            return None;
        }
        let guess = self.velocity;
        let mut pairs: Vec<Pair> = Vec::with_capacity(points.len());
        for p in points.iter().copied() {
            let Some(q) = track_point(&self.prev.luma, next, p, guess, &self.params.flow) else {
                continue;
            };
            let back_guess = (p.0 - q.0, p.1 - q.1);
            let Some(back) = track_point(next, &self.prev.luma, q, back_guess, &self.params.flow)
            else {
                continue;
            };
            let fb = ((back.0 - p.0).powi(2) + (back.1 - p.1).powi(2)).sqrt();
            if fb <= self.params.fb_threshold {
                pairs.push((p, q));
            }
        }
        if pairs.len() < 4 {
            return None;
        }

        let size = s.w.max(s.h);
        let threshold = (0.03 * size).clamp(0.75, 3.0);
        let fit = fit_similarity(&pairs, threshold)?;
        let inliers = fit.inliers as f32;
        let ratio = inliers / points.len() as f32;
        if fit.inliers < 4 || ratio < 0.3 {
            return None;
        }
        // A real object does not change size by a quarter between frames;
        // a fit that says so is fitting noise.
        let scale = fit.scale.clamp(0.85, 1.18);
        let (nx, ny) = fit.apply((s.cx, s.cy));
        let pose = BoxState {
            cx: nx,
            cy: ny,
            w: s.w * scale,
            h: s.h * scale,
            angle: s.angle + fit.angle_degrees(),
        };
        let quality = (ratio * (inliers / 12.0).min(1.0)).clamp(0.0, 1.0);
        Some((pose, quality))
    }
}

/// A point matched across two frames: where it was, where it went.
pub type Pair = ((f32, f32), (f32, f32));

/// `q = s·R(θ)·p + t`, stored as `a = s cos θ`, `b = s sin θ`.
#[derive(Debug, Clone, Copy)]
pub struct Similarity {
    pub a: f32,
    pub b: f32,
    pub tx: f32,
    pub ty: f32,
    pub scale: f32,
    pub inliers: usize,
}

impl Similarity {
    pub fn apply(&self, p: (f32, f32)) -> (f32, f32) {
        (
            self.a * p.0 - self.b * p.1 + self.tx,
            self.b * p.0 + self.a * p.1 + self.ty,
        )
    }

    pub fn angle_degrees(&self) -> f32 {
        self.b.atan2(self.a).to_degrees()
    }

    fn from_pairs(pairs: &[Pair]) -> Option<Self> {
        let n = pairs.len() as f32;
        if pairs.is_empty() {
            return None;
        }
        let (mut px, mut py, mut qx, mut qy) = (0.0, 0.0, 0.0, 0.0);
        for (p, q) in pairs {
            px += p.0;
            py += p.1;
            qx += q.0;
            qy += q.1;
        }
        let (px, py, qx, qy) = (px / n, py / n, qx / n, qy / n);
        let (mut dot, mut cross, mut norm) = (0.0f32, 0.0f32, 0.0f32);
        for (p, q) in pairs {
            let (ux, uy) = (p.0 - px, p.1 - py);
            let (vx, vy) = (q.0 - qx, q.1 - qy);
            dot += ux * vx + uy * vy;
            cross += ux * vy - uy * vx;
            norm += ux * ux + uy * uy;
        }
        let (a, b) = if norm < 1e-6 {
            (1.0, 0.0)
        } else {
            (dot / norm, cross / norm)
        };
        let scale = (a * a + b * b).sqrt();
        if !scale.is_finite() || scale < 1e-3 {
            return None;
        }
        Some(Self {
            a,
            b,
            tx: qx - (a * px - b * py),
            ty: qy - (b * px + a * py),
            scale,
            inliers: pairs.len(),
        })
    }
}

/// RANSAC over minimal samples of two pairs, then a least-squares refit on
/// the best consensus. Falls back to the plain fit when every sample is
/// degenerate.
pub fn fit_similarity(pairs: &[Pair], threshold: f32) -> Option<Similarity> {
    if pairs.len() < 2 {
        return None;
    }
    let mut seed: u32 = 0x9e37_79b9;
    let mut random = |n: usize| {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        seed as usize % n
    };
    let t2 = threshold * threshold;
    let count_inliers = |model: &Similarity| {
        pairs
            .iter()
            .filter(|(p, q)| {
                let r = model.apply(*p);
                (r.0 - q.0).powi(2) + (r.1 - q.1).powi(2) <= t2
            })
            .count()
    };

    let mut best: Option<(usize, Similarity)> = None;
    let iterations = 64.min(pairs.len() * (pairs.len() - 1) / 2).max(1);
    for _ in 0..iterations {
        let i = random(pairs.len());
        let mut j = random(pairs.len());
        if j == i {
            j = (j + 1) % pairs.len();
        }
        let (pi, pj) = (pairs[i].0, pairs[j].0);
        if (pi.0 - pj.0).powi(2) + (pi.1 - pj.1).powi(2) < 4.0 {
            continue;
        }
        let Some(model) = Similarity::from_pairs(&[pairs[i], pairs[j]]) else {
            continue;
        };
        let n = count_inliers(&model);
        if best.as_ref().is_none_or(|(b, _)| n > *b) {
            best = Some((n, model));
        }
    }
    let model = match best {
        Some((_, model)) => model,
        None => Similarity::from_pairs(pairs)?,
    };
    let inliers: Vec<_> = pairs
        .iter()
        .copied()
        .filter(|(p, q)| {
            let r = model.apply(*p);
            (r.0 - q.0).powi(2) + (r.1 - q.1).powi(2) <= t2
        })
        .collect();
    let mut refit = Similarity::from_pairs(&inliers)?;
    refit.inliers = count_inliers(&refit).max(inliers.len());
    Some(refit)
}

// --- the appearance template -----------------------------------------------------

/// The pyramid level whose pixels are about one template cell: sampling there
/// with a bilinear tap is close to an area average, so the template does not
/// alias on a large box.
fn level_for(frame: &Frame, cell: f32) -> usize {
    let mut level = 0;
    while level + 1 < frame.colour.len() && (2u32 << level) as f32 <= cell {
        level += 1;
    }
    level
}

fn grid(b: &BoxState, cells: usize) -> (usize, usize) {
    let long = b.w.max(b.h).max(1.0);
    let cols = ((cells as f32 * b.w / long).round() as usize).clamp(6, cells);
    let rows = ((cells as f32 * b.h / long).round() as usize).clamp(6, cells);
    (cols, rows)
}

/// The box's cells, channel after channel, each channel mean-removed under
/// `weights`.
fn sample_box(
    frame: &Frame,
    b: &BoxState,
    cols: usize,
    rows: usize,
    weights: &[f32],
    out: &mut Vec<f32>,
) {
    let cell = (b.w / cols as f32).max(b.h / rows as f32);
    let level = level_for(frame, cell);
    let scale = (1u32 << level) as f32;
    let planes = &frame.colour[level];
    out.clear();
    let total: f32 = weights.iter().sum::<f32>().max(1e-6);
    for plane in planes {
        let begin = out.len();
        for r in 0..rows {
            let y = b.cy - 0.5 * b.h + (r as f32 + 0.5) * b.h / rows as f32;
            for c in 0..cols {
                let x = b.cx - 0.5 * b.w + (c as f32 + 0.5) * b.w / cols as f32;
                out.push(plane.sample(x / scale, y / scale));
            }
        }
        let channel = &mut out[begin..];
        let mean = channel.iter().zip(weights).map(|(v, w)| v * w).sum::<f32>() / total;
        for v in channel.iter_mut() {
            *v -= mean;
        }
    }
}

/// A Gaussian over the box, σ a third of each side: the centre is the
/// object, the corners mostly are not.
fn centre_weights(cols: usize, rows: usize) -> Vec<f32> {
    let mut weights = Vec::with_capacity(cols * rows);
    for r in 0..rows {
        let v = (r as f32 + 0.5) / rows as f32 - 0.5;
        for c in 0..cols {
            let u = (c as f32 + 0.5) / cols as f32 - 0.5;
            let d2 = (u * u + v * v) / (0.33 * 0.33);
            weights.push((-0.5 * d2).exp());
        }
    }
    weights
}

fn sample_template(frame: &Frame, b: &BoxState, cells: usize) -> Template {
    let (cols, rows) = grid(b, cells);
    let weights = centre_weights(cols, rows);
    let mut values = Vec::with_capacity(3 * cols * rows);
    sample_box(frame, b, cols, rows, &weights, &mut values);
    Template {
        cols,
        rows,
        values,
        weights,
    }
}

fn blend_template(template: &mut Template, fresh: &Template, weight: f32) {
    if template.cols != fresh.cols || template.rows != fresh.rows {
        return;
    }
    for (t, f) in template.values.iter_mut().zip(&fresh.values) {
        *t = (1.0 - weight) * *t + weight * f;
    }
}

/// Weighted normalised cross-correlation over all three channels at once.
fn correlate(template: &Template, values: &[f32]) -> f32 {
    let (mut dot, mut ta, mut tb) = (0.0f32, 0.0f32, 0.0f32);
    let n = template.weights.len();
    for (i, (a, b)) in template.values.iter().zip(values).enumerate() {
        let w = template.weights[i % n];
        dot += w * a * b;
        ta += w * a * a;
        tb += w * b * b;
    }
    if ta < 1e-3 || tb < 1e-3 {
        return 0.0;
    }
    dot / (ta * tb).sqrt()
}

/// Correlation of the template with the box `b` in `frame`.
fn ncc(template: &Template, frame: &Frame, b: &BoxState) -> f32 {
    let mut values = Vec::with_capacity(template.values.len());
    sample_box(
        frame,
        b,
        template.cols,
        template.rows,
        &template.weights,
        &mut values,
    );
    correlate(template, &values)
}

/// Best template match within `radius` of `around`: a grid search at one cell
/// spacing, then two refinements at a quarter and a sixteenth of it.
fn search(
    template: &Template,
    frame: &Frame,
    around: &BoxState,
    radius: f32,
) -> Option<(BoxState, f32)> {
    let cell = (around.w / template.cols as f32)
        .max(around.h / template.rows as f32)
        .max(1.0);
    let (w, h) = (frame.width() as f32, frame.height() as f32);
    let mut values = Vec::with_capacity(template.values.len());
    let mut best: Option<(BoxState, f32)> = None;
    let mut consider = |cx: f32, cy: f32, best: &mut Option<(BoxState, f32)>| {
        if cx < 0.0 || cy < 0.0 || cx >= w || cy >= h {
            return;
        }
        let candidate = BoxState { cx, cy, ..*around };
        sample_box(
            frame,
            &candidate,
            template.cols,
            template.rows,
            &template.weights,
            &mut values,
        );
        let score = correlate(template, &values);
        if best.as_ref().is_none_or(|(_, s)| score > *s) {
            *best = Some((candidate, score));
        }
    };

    let steps = (radius / cell).ceil() as i32;
    for iy in -steps..=steps {
        for ix in -steps..=steps {
            consider(
                around.cx + ix as f32 * cell,
                around.cy + iy as f32 * cell,
                &mut best,
            );
        }
    }
    for step in [cell / 4.0, cell / 16.0] {
        let Some((centre, _)) = best else {
            break;
        };
        for iy in -4..=4 {
            for ix in -4..=4 {
                consider(
                    centre.cx + ix as f32 * step,
                    centre.cy + iy as f32 * step,
                    &mut best,
                );
            }
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::super::klt::tests::texture;
    use super::*;

    /// `src` warped by a similarity about `centre`: the frame a camera sees
    /// when the object scales by `s`, turns by `deg` (clockwise) and moves by
    /// `(dx, dy)`.
    fn warp(src: &Gray, centre: (f32, f32), s: f32, deg: f32, dx: f32, dy: f32) -> Gray {
        let (sin, cos) = deg.to_radians().sin_cos();
        let mut out = Gray::new(src.width, src.height);
        for y in 0..src.height {
            for x in 0..src.width {
                // Invert q = c + d + s R (p - c).
                let (qx, qy) = (x as f32 - centre.0 - dx, y as f32 - centre.1 - dy);
                let px = (cos * qx + sin * qy) / s + centre.0;
                let py = (-sin * qx + cos * qy) / s + centre.1;
                out.data[y * src.width + x] = src.sample(px, py);
            }
        }
        out
    }

    struct Truth {
        cx: f32,
        cy: f32,
        scale: f32,
        angle: f32,
    }

    /// Run the tracker over a synthetic sequence and return the worst errors
    /// in centre (px), scale (fraction) and angle (degrees).
    fn run(frames: usize, motion: impl Fn(usize) -> Truth) -> (f32, f32, f32) {
        let base = texture(480, 360, 11);
        let centre = (240.0, 180.0);
        let init = BoxState {
            cx: centre.0,
            cy: centre.1,
            w: 90.0,
            h: 70.0,
            angle: 0.0,
        };
        let params = TrackerParams::default();
        let mut tracker = Tracker::new(
            Frame::from_gray(base.clone(), params.pyramid_levels),
            init,
            params,
        );
        let (mut pos, mut scl, mut ang) = (0.0f32, 0.0f32, 0.0f32);
        for i in 1..frames {
            let truth = motion(i);
            let frame = warp(
                &base,
                centre,
                truth.scale,
                truth.angle,
                truth.cx - centre.0,
                truth.cy - centre.1,
            );
            let r = tracker.step(Frame::from_gray(frame, params.pyramid_levels));
            assert!(!r.lost, "lost at frame {i}");
            pos = pos.max(((r.pose.cx - truth.cx).powi(2) + (r.pose.cy - truth.cy).powi(2)).sqrt());
            scl = scl.max((r.pose.w / init.w / truth.scale - 1.0).abs());
            ang = ang.max((r.pose.angle - truth.angle).abs());
        }
        (pos, scl, ang)
    }

    #[test]
    fn translation_is_followed_within_a_pixel() {
        let (pos, scl, ang) = run(30, |i| Truth {
            cx: 240.0 + 3.5 * i as f32,
            cy: 180.0 - 1.25 * i as f32,
            scale: 1.0,
            angle: 0.0,
        });
        assert!(pos < 1.0, "centre error {pos}");
        assert!(scl < 0.02, "scale error {scl}");
        assert!(ang < 0.5, "angle error {ang}");
    }

    #[test]
    fn fast_translation_is_followed() {
        // 14 px per frame at analysis size: a throw.
        let (pos, _, _) = run(20, |i| Truth {
            cx: 240.0 - 8.0 * i as f32 + 0.0,
            cy: 180.0 + 4.0 * i as f32,
            scale: 1.0,
            angle: 0.0,
        });
        assert!(pos < 1.5, "centre error {pos}");
    }

    #[test]
    fn scale_is_measured() {
        let (pos, scl, _) = run(30, |i| Truth {
            cx: 240.0 + i as f32,
            cy: 180.0,
            scale: 1.0 + 0.012 * i as f32,
            angle: 0.0,
        });
        assert!(pos < 1.5, "centre error {pos}");
        assert!(scl < 0.03, "scale error {scl}");
    }

    #[test]
    fn rotation_is_measured_clockwise() {
        let (pos, scl, ang) = run(30, |i| Truth {
            cx: 240.0,
            cy: 180.0 + 0.5 * i as f32,
            scale: 1.0,
            angle: 1.5 * i as f32,
        });
        assert!(pos < 1.5, "centre error {pos}");
        assert!(scl < 0.03, "scale error {scl}");
        assert!(ang < 1.5, "angle error {ang}");
    }

    #[test]
    fn similarity_fit_ignores_outliers() {
        let truth = Similarity {
            a: 1.1 * 0.2f32.cos(),
            b: 1.1 * 0.2f32.sin(),
            tx: 5.0,
            ty: -3.0,
            scale: 1.1,
            inliers: 0,
        };
        let mut pairs: Vec<_> = (0..40)
            .map(|i| {
                let p = ((i % 8) as f32 * 10.0, (i / 8) as f32 * 10.0);
                (p, truth.apply(p))
            })
            .collect();
        for pair in pairs.iter_mut().take(12) {
            pair.1 .0 += 30.0;
        }
        let fit = fit_similarity(&pairs, 0.5).unwrap();
        assert_eq!(fit.inliers, 28);
        assert!((fit.scale - 1.1).abs() < 1e-3);
        assert!((fit.angle_degrees() - 0.2f32.to_degrees()).abs() < 0.05);
    }
}
