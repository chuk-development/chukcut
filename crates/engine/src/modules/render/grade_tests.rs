// GPU tests for the extended grade, run through the real compositor.
//
// A child of `compositor::tests`, in its own file only because that module
// is already thousands of lines; it uses the same sources, providers and
// sRGB compositor. Every expectation comes from `render::grade`'s CPU
// reference or from arithmetic spelled out here, never from a previous
// render, so a change to the shader fails a test instead of agreeing with
// itself.

use super::*;
use crate::modules::project::document::{ColorAdjustMaterial, LutRef};
use crate::modules::project::grade::{Grade, HslBand, Wheel};
use crate::modules::render::grade::{reference_encoded, vignette_mask};

fn encode(linear: f32) -> f32 {
    let l = linear.clamp(0.0, 1.0);
    if l <= 0.0031308 {
        l * 12.92
    } else {
        1.055 * l.powf(1.0 / 2.4) - 0.055
    }
}

fn decode(encoded: f32) -> f32 {
    if encoded <= 0.04045 {
        encoded / 12.92
    } else {
        ((encoded + 0.055) / 1.055).powf(2.4)
    }
}

/// Attach a full grade to the only clip of `project`.
fn attach(project: &mut Project, color: [f32; 4], grade: Grade, lut: Option<(&std::path::Path, f32)>) {
    let mut material = ColorAdjustMaterial::identity();
    material.id = "graded".into();
    material.brightness = color[0];
    material.contrast = color[1];
    material.saturation = color[2];
    material.temperature = color[3];
    material.grade = grade;
    material.lut = lut.map(|(path, intensity)| LutRef {
        path: path.to_string_lossy().into_owned(),
        intensity,
    });
    project.materials.color_adjusts.retain(|m| m.id != "graded");
    project.materials.color_adjusts.push(material);
    let extras = &mut project.tracks[0].segments[0].extras;
    if !extras.iter().any(|id| id == "graded") {
        extras.push("graded".into());
    }
}

fn rgb(p: [u8; 4]) -> [f32; 3] {
    [p[0] as f32 / 255.0, p[1] as f32 / 255.0, p[2] as f32 / 255.0]
}

fn assert_close(name: &str, actual: [u8; 4], expected: [f32; 3], tolerance: i32) {
    for c in 0..3 {
        let want = (expected[c].clamp(0.0, 1.0) * 255.0).round() as i32;
        assert!(
            (actual[c] as i32 - want).abs() <= tolerance,
            "{name}: channel {c} rendered {actual:?}, the reference says {expected:?}"
        );
    }
}

fn sources() -> [(&'static str, TestSource); 2] {
    [
        ("software", TestSource::Rgba([200, 120, 60, 255], 640, 480)),
        ("hardware", TestSource::Planar(150, 100, 170, 640, 480)),
    ]
}

/// A grade whose every control sits at rest — including the vignette's
/// shape, which means nothing without an amount — renders byte-identical to
/// no grade at all, and the original four sliders render exactly as they did
/// before the extended grade existed.
#[test]
fn an_extended_grade_at_rest_changes_no_byte() {
    let Some(c) = srgb_compositor() else {
        eprintln!("skipping: no GPU adapter");
        return;
    };
    for (name, source) in sources() {
        let provider = MixedProvider::default().with("clip", source);
        let mut project = split_project("clip");
        let before = frame(&c, &project, 0, &provider);

        let mut grade = Grade::default();
        grade.vignette.midpoint = 0.1;
        grade.vignette.feather = 0.9;
        grade.curves.master = vec![[0.0, 0.0], [1.0, 1.0]];
        attach(&mut project, [0.0, 1.0, 1.0, 0.0], grade, None);
        let at_rest = frame(&c, &project, 0, &provider);
        assert_eq!(before.data, at_rest.data, "{name}: a resting grade changed pixels");

        // The pre-existing four sliders, with and without a resting extended
        // grade next to them, are the same frame.
        let mut legacy = split_project("clip");
        grade_material(&mut legacy, [0.1, 1.2, 0.8, 0.3]);
        let old = frame(&c, &legacy, 0, &provider);
        attach(&mut project, [0.1, 1.2, 0.8, 0.3], Grade::default(), None);
        let new = frame(&c, &project, 0, &provider);
        assert_eq!(old.data, new.data, "{name}: the four sliders moved");
    }
}

/// The `grade` helper of the parent module, under a name that does not
/// collide with this module's use of the word.
fn grade_material(project: &mut Project, adjust: [f32; 4]) {
    super::grade(project, adjust);
}

/// Every per-pixel stage, one at a time and all together, on both decode
/// paths, against the CPU reference.
#[test]
fn every_per_pixel_stage_matches_the_cpu_reference() {
    let Some(c) = srgb_compositor() else {
        eprintln!("skipping: no GPU adapter");
        return;
    };
    let mut cases: Vec<(&str, [f32; 4], Grade)> = Vec::new();
    let g = Grade {
        exposure: 0.7,
        ..Default::default()
    };
    cases.push(("exposure", [0.0, 1.0, 1.0, 0.0], g));
    let g = Grade {
        tint: -0.6,
        ..Default::default()
    };
    cases.push(("tint", [0.0, 1.0, 1.0, 0.0], g));
    let g = Grade {
        highlights: -0.5,
        shadows: 0.6,
        whites: 0.3,
        blacks: -0.2,
        ..Default::default()
    };
    cases.push(("tone", [0.0, 1.0, 1.0, 0.0], g));
    let mut g = Grade::default();
    g.wheels.lift = Wheel { x: 0.4, y: -0.3, luma: 0.1 };
    g.wheels.gamma = Wheel { x: -0.2, y: 0.5, luma: 0.2 };
    g.wheels.gain = Wheel { x: 0.3, y: 0.3, luma: -0.1 };
    g.wheels.offset = Wheel { x: 0.0, y: -0.4, luma: 0.05 };
    cases.push(("wheels", [0.0, 1.0, 1.0, 0.0], g));
    let mut g = Grade::default();
    for (i, band) in g.hsl.bands.iter_mut().enumerate() {
        *band = HslBand {
            hue: 0.5 - i as f32 * 0.1,
            saturation: -0.4 + i as f32 * 0.1,
            luminance: 0.3,
        };
    }
    cases.push(("hsl", [0.0, 1.0, 1.0, 0.0], g));
    let g = Grade {
        vibrance: 0.8,
        ..Default::default()
    };
    cases.push(("vibrance", [0.0, 1.0, 1.0, 0.0], g));
    let mut g = Grade::default();
    g.curves.master = vec![[0.0, 0.0], [0.25, 0.15], [0.75, 0.85], [1.0, 1.0]];
    g.curves.red = vec![[0.0, 0.1], [0.5, 0.4], [1.0, 1.0]];
    g.curves.blue = vec![[0.0, 0.0], [1.0, 0.8]];
    cases.push(("curves", [0.0, 1.0, 1.0, 0.0], g));
    let g = Grade {
        fade: 0.7,
        ..Default::default()
    };
    cases.push(("fade", [0.0, 1.0, 1.0, 0.0], g));

    // Everything at once, over the original sliders: the order matters here,
    // and only the reference knows it independently.
    let mut all = Grade::default();
    for (_, _, g) in &cases {
        let g = g.clone();
        all.exposure += g.exposure;
        all.tint += g.tint;
        all.highlights += g.highlights;
        all.shadows += g.shadows;
        all.whites += g.whites;
        all.blacks += g.blacks;
        all.vibrance += g.vibrance;
        all.fade += g.fade;
        if g.hsl != Default::default() {
            all.hsl = g.hsl;
        }
        if !g.curves.is_identity() {
            all.curves = g.curves.clone();
        }
        if g.wheels != Default::default() {
            all.wheels = g.wheels;
        }
    }
    cases.push(("everything", [0.05, 1.1, 0.9, 0.2], all));

    for (source_name, source) in sources() {
        let provider = MixedProvider::default().with("clip", source);
        let base = frame(&c, &split_project("clip"), 0, &provider).pixel(320, 240);
        let input = rgb(base);
        for (name, color, grade) in &cases {
            let mut project = split_project("clip");
            attach(&mut project, *color, grade.clone(), None);
            let rendered = frame(&c, &project, 0, &provider).pixel(320, 240);

            // Exposure happens in light, before the encoded stages.
            let exposed = input.map(|v| encode(decode(v) * grade.exposure.exp2()));
            let scalars = (*color != [0.0, 1.0, 1.0, 0.0]).then_some(*color);
            let expected = reference_encoded(exposed, scalars, grade, None);
            // The input is a quantised readback, and the steepest stages
            // amplify half a code value of it; three codes covers that.
            assert_close(&format!("{source_name} {name}"), rendered, expected, 3);
        }
    }
}

/// The vignette is a multiply in light, measured from the quad's centre:
/// the centre is untouched and a corner takes the mask the reference says.
#[test]
fn the_vignette_darkens_the_corners_in_light() {
    let Some(c) = srgb_compositor() else {
        eprintln!("skipping: no GPU adapter");
        return;
    };
    let provider = MixedProvider::default().with("clip", TestSource::Rgba([200, 200, 200, 255], 640, 480));
    let base = frame(&c, &split_project("clip"), 0, &provider);

    for amount in [0.8f32, -0.6] {
        let mut project = split_project("clip");
        let mut g = Grade::default();
        g.vignette.amount = amount;
        g.vignette.midpoint = 0.4;
        g.vignette.feather = 0.6;
        attach(&mut project, [0.0, 1.0, 1.0, 0.0], g, None);
        let f = frame(&c, &project, 0, &provider);
        assert_eq!(f.pixel(320, 240), base.pixel(320, 240), "the centre moved");

        for (x, y) in [(3u32, 3u32), (636, 470), (100, 240)] {
            let local = [(x as f32 + 0.5) / 640.0, (y as f32 + 0.5) / 480.0];
            let mask = vignette_mask(local, 0.4, 0.6);
            let input = decode(rgb(base.pixel(x, y))[0]);
            let light = if amount > 0.0 {
                input * (1.0 - amount * mask)
            } else {
                input + (1.0 - input) * (-amount * mask)
            };
            assert_close(&format!("vignette {amount} at ({x},{y})"), f.pixel(x, y), [encode(light); 3], 2);
        }
    }
}

/// Grain moves pixels without moving the picture, is the same every time a
/// frame is rendered, and is different on the next frame.
#[test]
fn grain_is_deterministic_per_frame_and_keeps_the_mean() {
    let Some(c) = srgb_compositor() else {
        eprintln!("skipping: no GPU adapter");
        return;
    };
    let provider = MixedProvider::default().with("clip", TestSource::Rgba([128, 128, 128, 255], 640, 480));
    let base = frame(&c, &split_project("clip"), 0, &provider);
    let mut project = split_project("clip");
    let g = Grade {
        grain: 1.0,
        ..Default::default()
    };
    attach(&mut project, [0.0, 1.0, 1.0, 0.0], g, None);

    let a = frame(&c, &project, 40_000, &provider);
    let again = frame(&c, &project, 40_000, &provider);
    let next = frame(&c, &project, 80_000, &provider);
    assert_eq!(a.data, again.data, "grain is not deterministic");
    assert_ne!(a.data, next.data, "grain does not move between frames");
    assert_ne!(a.data, base.data, "grain did nothing");

    let mean = |f: &Frame| {
        f.data.chunks(4).map(|p| decode(p[1] as f32 / 255.0) as f64).sum::<f64>() / (640.0 * 480.0)
    };
    let (grained, plain) = (mean(&a), mean(&base));
    assert!((grained - plain).abs() < 0.01, "grain shifted the mean light: {plain} -> {grained}");
}

/// Sharpen and clarity read the neighbourhood. On a flat source there is
/// nothing to read and nothing changes; across an edge, sharpening
/// overshoots on both sides — the dark side darker, the bright side brighter.
#[test]
fn sharpen_and_clarity_act_on_edges_and_leave_flat_areas() {
    let Some(c) = srgb_compositor() else {
        eprintln!("skipping: no GPU adapter");
        return;
    };
    let flat = MixedProvider::default().with("clip", TestSource::Rgba([90, 140, 200, 255], 640, 480));
    let base = frame(&c, &split_project("clip"), 0, &flat);
    let mut project = split_project("clip");
    let g = Grade {
        sharpen: 1.0,
        clarity: 1.0,
        ..Default::default()
    };
    attach(&mut project, [0.0, 1.0, 1.0, 0.0], g, None);
    let graded = frame(&c, &project, 0, &flat);
    for (x, y) in [(320, 240), (10, 10), (600, 400)] {
        let (a, b) = (base.pixel(x, y), graded.pixel(x, y));
        for ch in 0..3 {
            assert!((a[ch] as i32 - b[ch] as i32).abs() <= 1, "flat area moved at ({x},{y}): {a:?} -> {b:?}");
        }
    }

    for (name, source) in [
        ("software", TestSource::SplitRgba([60, 60, 60, 255], [180, 180, 180, 255], 640, 480)),
        ("hardware", TestSource::SplitPlanar(60, 180, 640, 480)),
    ] {
        let provider = MixedProvider::default().with("clip", source);
        let base = frame(&c, &split_project("clip"), 0, &provider);
        let mut project = split_project("clip");
        let g = Grade {
            sharpen: 1.0,
            ..Default::default()
        };
        attach(&mut project, [0.0, 1.0, 1.0, 0.0], g, None);
        let sharp = frame(&c, &project, 0, &provider);
        let (dark, bright) = (base.pixel(319, 240)[1], base.pixel(320, 240)[1]);
        assert!(sharp.pixel(319, 240)[1] < dark, "{name}: the dark side of the edge did not darken");
        assert!(sharp.pixel(320, 240)[1] > bright, "{name}: the bright side did not brighten");
        assert_eq!(sharp.pixel(100, 240), base.pixel(100, 240), "{name}: far from the edge moved");

        let g = Grade {
            clarity: 1.0,
            ..Default::default()
        };
        attach(&mut project, [0.0, 1.0, 1.0, 0.0], g, None);
        let clear = frame(&c, &project, 0, &provider);
        // Within the ring radius of the edge, local contrast grows.
        assert!(clear.pixel(316, 240)[1] < base.pixel(316, 240)[1], "{name}: clarity left the dark side");
        assert!(clear.pixel(323, 240)[1] > base.pixel(323, 240)[1], "{name}: clarity left the bright side");
    }
}

/// Identity LUTs of every size and both kinds change no pixel by more than
/// one code value — the requirement for "within 1/255".
#[test]
fn identity_luts_of_every_size_are_a_no_op() {
    let Some(c) = srgb_compositor() else {
        eprintln!("skipping: no GPU adapter");
        return;
    };
    let fixtures = crate::modules::render::lut::fixtures::identity_cube;
    let files = [
        ("3d-33", write_lut("identity-33", &fixtures(33))),
        ("3d-65", write_lut("identity-65", &fixtures(65))),
        ("1d-2", write_lut("identity-1d-2", &crate::modules::render::lut::fixtures::table_text(2, |x| [x, x, x]))),
        ("1d-4096", write_lut("identity-1d-4096", &crate::modules::render::lut::fixtures::table_text(4096, |x| [x, x, x]))),
    ];
    for (source_name, source) in [
        ("software", TestSource::SplitRgba([17, 140, 230, 255], [250, 3, 99, 255], 640, 480)),
        ("hardware", TestSource::Planar(173, 90, 200, 640, 480)),
    ] {
        let provider = MixedProvider::default().with("clip", source);
        let before = frame(&c, &split_project("clip"), 0, &provider);
        for (name, path) in &files {
            let mut project = split_project("clip");
            attach(&mut project, [0.0, 1.0, 1.0, 0.0], Grade::default(), Some((path, 1.0)));
            let after = frame(&c, &project, 0, &provider);
            let worst = before
                .data
                .iter()
                .zip(&after.data)
                .map(|(a, b)| (*a as i32 - *b as i32).abs())
                .max()
                .unwrap_or(0);
            assert!(worst <= 1, "{source_name} {name}: an identity LUT moved a pixel by {worst}");
        }
    }
}

/// A non-trivial LUT of each kind renders what `Cube::sample` — the CPU
/// reference — says, at full and partial intensity, over a grade.
#[test]
fn known_luts_match_the_cpu_reference() {
    let Some(c) = srgb_compositor() else {
        eprintln!("skipping: no GPU adapter");
        return;
    };
    // A 17-point cube that mixes channels non-linearly, and a 1D table that
    // bends each channel differently: neither interpolates exactly, so the
    // shader and the reference must agree on *how* they interpolate.
    let cube_text = crate::modules::render::lut::fixtures::cube_text(17, |r, g, b| {
        [
            (0.8 * r * r + 0.2 * g).min(1.0),
            (g.sqrt() * 0.9 + 0.1 * b).min(1.0),
            (0.5 * b + 0.5 * r * g).min(1.0),
        ]
    });
    let table_text =
        crate::modules::render::lut::fixtures::table_text(9, |x| [x * x, x.sqrt(), 1.0 - x]);
    let looks = [
        ("cube", write_lut("known-cube", &cube_text), crate::modules::render::lut::parse(&cube_text).unwrap()),
        ("table", write_lut("known-table", &table_text), crate::modules::render::lut::parse(&table_text).unwrap()),
    ];
    let grade = Grade {
        fade: 0.2,
        tint: 0.1,
        ..Default::default()
    };
    for (source_name, source) in sources() {
        let provider = MixedProvider::default().with("clip", source);
        let input = rgb(frame(&c, &split_project("clip"), 0, &provider).pixel(320, 240));
        for (name, path, cube) in &looks {
            for intensity in [1.0f32, 0.4] {
                let mut project = split_project("clip");
                attach(&mut project, [0.05, 1.0, 1.0, 0.0], grade.clone(), Some((path, intensity)));
                let rendered = frame(&c, &project, 0, &provider).pixel(320, 240);
                let sample = |c: [f32; 3]| cube.sample(c);
                let expected = reference_encoded(
                    input,
                    Some([0.05, 1.0, 1.0, 0.0]),
                    &grade,
                    Some((&sample, intensity)),
                );
                assert_close(&format!("{source_name} {name} at {intensity}"), rendered, expected, 2);
            }
        }
    }
}

/// The preview path (RGBA readback) and the export path (the NV12 compute
/// pass) agree on a frame with every stage of the grade live — including
/// the neighbourhood stages, the vignette and the grain — on both decode
/// paths.
#[test]
fn the_preview_and_the_export_agree_on_a_fully_graded_frame() {
    let Some(c) = srgb_compositor() else {
        eprintln!("skipping: no GPU adapter");
        return;
    };
    let table = write_lut(
        "parity-table",
        &crate::modules::render::lut::fixtures::table_text(64, |x| [x.powf(0.8), x, x.powf(1.2)]),
    );
    let mut g = Grade {
        exposure: 0.3,
        tint: 0.2,
        highlights: -0.3,
        shadows: 0.3,
        whites: 0.1,
        blacks: 0.1,
        vibrance: 0.4,
        sharpen: 0.5,
        clarity: 0.4,
        ..Default::default()
    };
    g.vignette.amount = 0.5;
    g.grain = 0.3;
    g.fade = 0.1;
    g.hsl.bands[0].hue = 0.4;
    g.hsl.bands[5].saturation = -0.5;
    g.curves.master = vec![[0.0, 0.05], [0.5, 0.55], [1.0, 0.95]];
    g.wheels.gain = Wheel { x: 0.2, y: 0.1, luma: 0.0 };

    for (name, source) in [
        ("software", TestSource::SplitRgba([230, 120, 40, 255], [30, 90, 200, 255], 640, 480)),
        ("hardware", TestSource::Planar(200, 100, 180, 640, 480)),
    ] {
        let mut project = split_project("clip");
        attach(&mut project, [-0.05, 1.1, 0.9, 0.2], g.clone(), Some((&table, 0.7)));
        let provider = MixedProvider::default().with("clip", source);
        let at = 1_234_000;
        let preview = c.render(&project, at, (640, 480), &provider).expect("preview render");
        let Ok(export) = c.render_nv12(&project, at, (640, 480), &provider) else {
            eprintln!("skipping: no RGBA to NV12 compute pass on this device");
            return;
        };
        // NV12 halves chroma resolution, so per-pixel grain cannot survive
        // it exactly; compare 2x2 block means, which is what the export's
        // chroma actually carries, and single pixels where there is no grain
        // texture to speak of.
        for (x, y) in [(0u32, 0u32), (320, 240), (638, 478), (16, 300), (318, 120)] {
            let mut want = [0f32; 3];
            let mut got = [0f32; 3];
            for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                let p = preview.pixel(x + dx, y + dy);
                let q = nv12_pixel(&export, x + dx, y + dy);
                for ch in 0..3 {
                    want[ch] += p[ch] as f32 / 4.0;
                    got[ch] += q[ch] as f32 / 4.0;
                }
            }
            for ch in 0..3 {
                assert!(
                    (want[ch] - got[ch]).abs() <= 4.0,
                    "{name}: at ({x},{y}) preview {want:?} against export {got:?}"
                );
            }
        }
    }
}
