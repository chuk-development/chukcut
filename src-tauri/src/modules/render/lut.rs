//! .cube LUT files: parsing, a reference sampler, and the GPU cache.
//!
//! A `.cube` file (Adobe/Iridas) is a text file describing a 3D look-up
//! table: `LUT_3D_SIZE N` followed by `N³` RGB triples, red varying fastest.
//! This module owns everything about them:
//!
//! - [`parse`] turns the text into a [`Cube`], failing with a message that
//!   names the offending line — files found in the wild carry comments, CRLF
//!   endings and trailing whitespace, and all three are tolerated.
//! - [`Cube::sample`] is a CPU trilinear reference. The shader implements the
//!   same arithmetic; tests compare rendered pixels against this, so a change
//!   to either fails a test instead of cancelling out.
//! - [`LutCache`] maps a *path* to an uploaded 3D texture, keyed by the
//!   file's mtime. The document stores only the path (a project must reopen
//!   with the LUT file gone — `Project::validate` warns, the renderer renders
//!   unadjusted), so the parsed cube is strictly runtime state and lives
//!   here, next to the render context that owns the texture.
//!
//! ## Why the texture is `Rgba32Float` and the shader filters by hand
//!
//! Hardware trilinear filtering quantises its interpolation weights (8-bit
//! subtexel precision is typical), which puts errors up to a code value into
//! *every* LUT application — including an identity LUT, which is required to
//! render byte-identically to no LUT at all. Eight `textureLoad`s and an
//! f32 lerp in the shader cost a little more ALU but make the interpolation
//! exact, need no filterable-float feature, and no f16 conversion. The cost
//! is measured in `benches/bench_composite.rs`.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::SystemTime;

use parking_lot::Mutex;

use super::context::RenderContext;

/// The largest `LUT_3D_SIZE` accepted. 65 is the biggest size in common use;
/// 256 is far beyond it and still only a 256 MB upload, but anything larger
/// is a malformed file, not a look.
const MAX_SIZE: u32 = 256;

/// One parsed .cube file.
#[derive(Debug, Clone, PartialEq)]
pub struct Cube {
    pub title: Option<String>,
    /// Edge length `N`; the table has `N³` entries.
    pub size: u32,
    pub domain_min: [f32; 3],
    pub domain_max: [f32; 3],
    /// `N³` RGB triples, red varying fastest, then green, then blue — the
    /// file's own order, which is also exactly a 3D texture's memory order
    /// with `x = r, y = g, z = b`.
    pub data: Vec<[f32; 3]>,
}

impl Cube {
    /// Trilinear lookup, the arithmetic `quad.wgsl`'s `sample_lut` mirrors.
    pub fn sample(&self, rgb: [f32; 3]) -> [f32; 3] {
        let n = self.size as usize;
        let mut base = [0usize; 3];
        let mut t = [0f32; 3];
        for c in 0..3 {
            let span = self.domain_max[c] - self.domain_min[c];
            let coord =
                ((rgb[c] - self.domain_min[c]) / span).clamp(0.0, 1.0) * (n - 1) as f32;
            let i = (coord.floor() as usize).min(n - 2);
            base[c] = i;
            t[c] = coord - i as f32;
        }
        let at = |r: usize, g: usize, b: usize| self.data[r + g * n + b * n * n];
        let mut out = [0f32; 3];
        for c in 0..3 {
            let mut accum = 0f32;
            for corner in 0..8usize {
                let (dr, dg, db) = (corner & 1, (corner >> 1) & 1, (corner >> 2) & 1);
                let weight = (if dr == 1 { t[0] } else { 1.0 - t[0] })
                    * (if dg == 1 { t[1] } else { 1.0 - t[1] })
                    * (if db == 1 { t[2] } else { 1.0 - t[2] });
                accum += weight * at(base[0] + dr, base[1] + dg, base[2] + db)[c];
            }
            out[c] = accum;
        }
        out
    }
}

/// Parse a .cube file. Errors name the 1-based line they were found on.
pub fn parse(text: &str) -> Result<Cube, String> {
    let mut title = None;
    let mut size: Option<u32> = None;
    let mut domain_min = [0f32; 3];
    let mut domain_max = [1f32; 3];
    let mut data: Vec<[f32; 3]> = Vec::new();

    for (index, raw) in text.lines().enumerate() {
        let line_no = index + 1;
        // CRLF files reach `lines()` with the `\r` still attached; trailing
        // whitespace is common after hand edits. Both are meaningless.
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        let mut words = line.split_whitespace();
        let keyword = words.next().expect("a trimmed non-empty line has a word");

        match keyword {
            "TITLE" => {
                // The spec says a quoted string; the wild also has bare ones.
                let rest = line["TITLE".len()..].trim();
                title = Some(rest.trim_matches('"').to_string());
            }
            "LUT_3D_SIZE" => {
                let n: u32 = words
                    .next()
                    .ok_or_else(|| format!("line {line_no}: LUT_3D_SIZE needs a number"))?
                    .parse()
                    .map_err(|_| format!("line {line_no}: LUT_3D_SIZE is not a whole number"))?;
                if !(2..=MAX_SIZE).contains(&n) {
                    return Err(format!(
                        "line {line_no}: LUT_3D_SIZE must be between 2 and {MAX_SIZE}, got {n}"
                    ));
                }
                data.reserve((n as usize).pow(3));
                size = Some(n);
            }
            "LUT_1D_SIZE" => {
                return Err(format!(
                    "line {line_no}: this is a 1D LUT; only 3D LUTs (LUT_3D_SIZE) are supported"
                ));
            }
            "DOMAIN_MIN" | "DOMAIN_MAX" => {
                let triple = parse_triple(&mut words)
                    .ok_or_else(|| format!("line {line_no}: {keyword} needs three numbers"))?;
                if keyword == "DOMAIN_MIN" {
                    domain_min = triple;
                } else {
                    domain_max = triple;
                }
            }
            first => {
                // Anything else is either a data triple or a mistake.
                let Ok(r) = first.parse::<f32>() else {
                    return Err(format!(
                        "line {line_no}: expected an RGB triple or a known keyword, got \"{first}\""
                    ));
                };
                let (Some(g), Some(b)) = (
                    words.next().and_then(|w| w.parse::<f32>().ok()),
                    words.next().and_then(|w| w.parse::<f32>().ok()),
                ) else {
                    return Err(format!(
                        "line {line_no}: a data line needs three numbers, got \"{line}\""
                    ));
                };
                if words.next().is_some() {
                    return Err(format!(
                        "line {line_no}: a data line needs exactly three numbers, got \"{line}\""
                    ));
                }
                if ![r, g, b].iter().all(|v| v.is_finite()) {
                    return Err(format!("line {line_no}: a LUT value is not a finite number"));
                }
                data.push([r, g, b]);
            }
        }
    }

    let size = size.ok_or("the file never declares LUT_3D_SIZE")?;
    let expected = (size as usize).pow(3);
    if data.len() != expected {
        return Err(format!(
            "LUT_3D_SIZE {size} promises {expected} entries, the file has {}",
            data.len()
        ));
    }
    for c in 0..3 {
        if !(domain_max[c] - domain_min[c]).is_normal() || domain_max[c] <= domain_min[c] {
            return Err("DOMAIN_MIN and DOMAIN_MAX describe an empty domain".into());
        }
    }

    Ok(Cube {
        title,
        size,
        domain_min,
        domain_max,
        data,
    })
}

fn parse_triple<'a>(words: &mut impl Iterator<Item = &'a str>) -> Option<[f32; 3]> {
    let mut out = [0f32; 3];
    for slot in &mut out {
        *slot = words.next()?.parse().ok()?;
    }
    Some(out)
}

// ---------------------------------------------------------------------------
// GPU upload and the runtime cache
// ---------------------------------------------------------------------------

/// A cube on the device, plus what the shader needs to address it.
pub struct GpuLut {
    pub view: wgpu::TextureView,
    pub size: u32,
    pub domain_min: [f32; 3],
    pub domain_max: [f32; 3],
}

/// Upload `cube` as an `Rgba32Float` 3D texture (alpha unused, set to 1).
pub fn upload(ctx: &RenderContext, cube: &Cube) -> GpuLut {
    let n = cube.size;
    let mut texels: Vec<f32> = Vec::with_capacity(cube.data.len() * 4);
    for [r, g, b] in &cube.data {
        texels.extend_from_slice(&[*r, *g, *b, 1.0]);
    }
    let texture = ctx.device().create_texture(&wgpu::TextureDescriptor {
        label: Some("chukcut lut"),
        size: wgpu::Extent3d {
            width: n,
            height: n,
            depth_or_array_layers: n,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D3,
        format: wgpu::TextureFormat::Rgba32Float,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    ctx.queue().write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        bytemuck::cast_slice(&texels),
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(n * 16),
            rows_per_image: Some(n),
        },
        wgpu::Extent3d {
            width: n,
            height: n,
            depth_or_array_layers: n,
        },
    );
    GpuLut {
        view: texture.create_view(&wgpu::TextureViewDescriptor::default()),
        size: n,
        domain_min: cube.domain_min,
        domain_max: cube.domain_max,
    }
}

/// One cache slot: what the file looked like when it was read, and what came
/// of reading it. `lut: None` records a failure (missing, unreadable,
/// malformed), so a broken file is warned about once and not re-parsed every
/// frame — only a *changed* mtime triggers another attempt.
struct Entry {
    mtime: Option<SystemTime>,
    lut: Option<Arc<GpuLut>>,
}

/// Path → uploaded LUT, revalidated by mtime on every lookup.
///
/// The mtime check is one `stat` per graded clip per frame — microseconds —
/// and it is what makes editing a LUT file in an external tool show up on the
/// next frame without any invalidation plumbing. A missing file yields `None`
/// and the caller renders unadjusted; that is the contract that lets a
/// project whose LUT went away still open and play.
#[derive(Default)]
pub struct LutCache {
    entries: Mutex<HashMap<String, Entry>>,
}

impl LutCache {
    pub fn get(&self, ctx: &RenderContext, path: &str) -> Option<Arc<GpuLut>> {
        let mtime = std::fs::metadata(path).and_then(|m| m.modified()).ok();

        let mut entries = self.entries.lock();
        if let Some(entry) = entries.get(&path.to_string()) {
            if entry.mtime == mtime {
                return entry.lut.clone();
            }
        }

        let lut = if mtime.is_none() {
            tracing::warn!(path, "LUT file is missing; rendering without it");
            None
        } else {
            match std::fs::read_to_string(path).map_err(|e| e.to_string()).and_then(|text| parse(&text)) {
                Ok(cube) => Some(Arc::new(upload(ctx, &cube))),
                Err(error) => {
                    tracing::warn!(path, %error, "LUT file did not parse; rendering without it");
                    None
                }
            }
        };
        let out = lut.clone();
        entries.insert(path.to_string(), Entry { mtime, lut });
        out
    }
}

/// LUT text builders for tests, here so the compositor's pixel tests and the
/// parser's own tests share one definition of "the identity cube". Built in
/// code so their effect is analytically known rather than trusted from a
/// file.
#[cfg(test)]
pub mod fixtures {
    /// `LUT_3D_SIZE n` identity: every entry is its own grid coordinate.
    pub fn identity_cube(n: u32) -> String {
        cube_text(n, |r, g, b| [r, g, b])
    }

    /// A cube whose entry for grid point `(r, g, b)` (each `0..1`) is `f`'s
    /// answer, in the file's red-fastest order.
    pub fn cube_text(n: u32, f: impl Fn(f32, f32, f32) -> [f32; 3]) -> String {
        let mut out = format!("LUT_3D_SIZE {n}\n");
        let last = (n - 1) as f32;
        for b in 0..n {
            for g in 0..n {
                for r in 0..n {
                    let [x, y, z] = f(r as f32 / last, g as f32 / last, b as f32 / last);
                    out.push_str(&format!("{x} {y} {z}\n"));
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::fixtures::{cube_text, identity_cube};
    use super::*;

    #[test]
    fn parses_a_wild_looking_file() {
        // CRLF line endings, comments, trailing whitespace, a quoted title,
        // and an explicit domain — everything the wild throws at once.
        let text = "# generated by some grading tool\r\n\
                    TITLE \"Warm Look\"  \r\n\
                    DOMAIN_MIN 0.0 0.0 0.0\r\n\
                    DOMAIN_MAX 1.0 1.0 1.0\r\n\
                    LUT_3D_SIZE 2\r\n\
                    \r\n\
                    0 0 0  \r\n\
                    1 0 0\r\n\
                    0 1 0\r\n\
                    1 1 0\r\n\
                    0 0 1\r\n\
                    1 0 1\r\n\
                    0 1 1\r\n\
                    1 1 1\r\n";
        let cube = parse(text).expect("parses");
        assert_eq!(cube.title.as_deref(), Some("Warm Look"));
        assert_eq!(cube.size, 2);
        assert_eq!(cube.data.len(), 8);
        // Red varies fastest: the second entry is the red corner.
        assert_eq!(cube.data[1], [1.0, 0.0, 0.0]);
    }

    #[test]
    fn errors_name_the_line() {
        let text = "LUT_3D_SIZE 2\n0 0 0\n1 0 zebra\n";
        let error = parse(text).unwrap_err();
        assert!(error.contains("line 3"), "{error}");

        let error = parse("LUT_3D_SIZE 2\n0 0\n").unwrap_err();
        assert!(error.contains("line 2"), "{error}");

        let error = parse("BANANA 4\n").unwrap_err();
        assert!(error.contains("line 1") && error.contains("BANANA"), "{error}");
    }

    #[test]
    fn a_1d_lut_is_refused_with_a_reason() {
        let error = parse("LUT_1D_SIZE 4\n0 0 0\n").unwrap_err();
        assert!(error.contains("1D"), "{error}");
    }

    #[test]
    fn wrong_entry_counts_and_missing_size_are_refused() {
        let error = parse("LUT_3D_SIZE 2\n0 0 0\n").unwrap_err();
        assert!(error.contains("promises 8") && error.contains("has 1"), "{error}");

        let error = parse("0 0 0\n").unwrap_err();
        assert!(error.contains("LUT_3D_SIZE"), "{error}");

        let error = parse("LUT_3D_SIZE 1\n0 0 0\n").unwrap_err();
        assert!(error.contains("between 2"), "{error}");
    }

    #[test]
    fn an_empty_domain_is_refused() {
        let text = format!("DOMAIN_MAX 0 0 0\n{}", identity_cube(2));
        assert!(parse(&text).unwrap_err().contains("domain"));
    }

    #[test]
    fn the_identity_cube_samples_to_its_input() {
        let cube = parse(&identity_cube(2)).unwrap();
        for value in [[0.0, 0.0, 0.0], [1.0, 1.0, 1.0], [0.25, 0.5, 0.75]] {
            let out = cube.sample(value);
            for c in 0..3 {
                assert!((out[c] - value[c]).abs() < 1e-6, "{value:?} -> {out:?}");
            }
        }
    }

    #[test]
    fn invert_swap_and_gradient_sample_analytically() {
        let invert = parse(&cube_text(2, |r, g, b| [1.0 - r, 1.0 - g, 1.0 - b])).unwrap();
        let out = invert.sample([0.2, 0.5, 1.0]);
        for (actual, wanted) in out.iter().zip([0.8, 0.5, 0.0]) {
            assert!((actual - wanted).abs() < 1e-6, "{out:?}");
        }

        let swap = parse(&cube_text(2, |r, g, b| [b, g, r])).unwrap();
        let out = swap.sample([1.0, 0.25, 0.0]);
        for (actual, wanted) in out.iter().zip([0.0, 0.25, 1.0]) {
            assert!((actual - wanted).abs() < 1e-6, "{out:?}");
        }

        // A 2-point gradient: halve everything. Trilinear interpolation of a
        // linear function is exact, so every input is halved, not only the
        // corners.
        let half = parse(&cube_text(2, |r, g, b| [r * 0.5, g * 0.5, b * 0.5])).unwrap();
        let out = half.sample([0.9, 0.4, 0.1]);
        for (actual, wanted) in out.iter().zip([0.45, 0.2, 0.05]) {
            assert!((actual - wanted).abs() < 1e-6, "{out:?}");
        }
    }

    #[test]
    fn a_larger_cube_interpolates_between_its_grid_points() {
        // 3 points per axis: grid at 0, 0.5, 1. Identity data, so sampling at
        // 0.25 must interpolate the 0 and 0.5 entries to exactly 0.25.
        let cube = parse(&identity_cube(3)).unwrap();
        let out = cube.sample([0.25, 0.75, 0.5]);
        for (actual, wanted) in out.iter().zip([0.25, 0.75, 0.5]) {
            assert!((actual - wanted).abs() < 1e-6, "{out:?}");
        }
    }

    #[test]
    fn the_domain_rescales_the_input() {
        // Identity data over a 0..2 domain: sampling at 1.0 is the middle of
        // the domain, whose identity entry is... the *data* is 0..1 over the
        // grid, so input 1.0 maps to grid position 0.5 and reads back 0.5.
        let text = format!("DOMAIN_MAX 2 2 2\n{}", identity_cube(2));
        let cube = parse(&text).unwrap();
        let out = cube.sample([1.0, 2.0, 0.0]);
        for (actual, wanted) in out.iter().zip([0.5, 1.0, 0.0]) {
            assert!((actual - wanted).abs() < 1e-6, "{out:?}");
        }
    }
}
