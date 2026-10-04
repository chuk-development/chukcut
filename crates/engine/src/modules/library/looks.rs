//! Our own colour looks, generated as `.cube` LUTs.
//!
//! A look is a small function from a display-referred Rec.709 colour to
//! another — white balance, a tone curve, split toning, a saturation change,
//! a fade — built from the helpers below and evaluated on a 33-point grid.
//! No file is downloaded and none is committed: the looks are code, GPL like
//! the rest, so a creator can use them anywhere. Names describe the look;
//! none names a film stock, because those names are trademarks.
//!
//! [`install`] writes them into the LUT library (`paths::luts_dir`) the first
//! time, and again only when [`LOOKS_VERSION`] moves — so a look the user
//! deleted stays deleted, and a file of theirs with the same name is never
//! overwritten. Tiles are drawn by the compositor through the same LUT path a
//! clip uses (`fx::tiles::look_tile`).

use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::modules::workspace::atomic::write_atomically;

/// Bump when a look's arithmetic changes, so installs pick it up.
pub const LOOKS_VERSION: u32 = 1;
/// Grid points per axis.
pub const SIZE: u32 = 33;
/// The first line of every file we write: how [`install`] recognises its own.
const MARKER: &str = "# chukcut look";
/// The stamp in the LUT library that records the installed version.
const STAMP: &str = ".chukcut-looks";

type Rgb = [f32; 3];

/// One look.
#[derive(Clone, Copy)]
pub struct Look {
    pub name: &'static str,
    pub category: &'static str,
    pub apply: fn(Rgb) -> Rgb,
}

/// What the panel lists.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LookEntry {
    pub name: String,
    pub category: String,
    /// The installed `.cube`, which a clip's `LutRef` stores.
    pub path: String,
}

/// The categories, in the order the Filters tab shows them.
pub const CATEGORIES: [&str; 5] = [
    "Cinematic",
    "Film",
    "Black & white",
    "Warm & cool",
    "Vintage",
];

// --- helpers -------------------------------------------------------------------

fn luma(c: Rgb) -> f32 {
    0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2]
}

fn clamp(c: Rgb) -> Rgb {
    c.map(|v| v.clamp(0.0, 1.0))
}

fn mix(a: Rgb, b: Rgb, t: f32) -> Rgb {
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
    ]
}

fn saturate(c: Rgb, amount: f32) -> Rgb {
    let l = luma(c);
    c.map(|v| l + (v - l) * amount)
}

/// An S-curve around mid grey: 0 is none, 1 is a full smoothstep.
fn s_curve(x: f32, amount: f32) -> f32 {
    let x = x.clamp(0.0, 1.0);
    let smooth = x * x * (3.0 - 2.0 * x);
    x + (smooth - x) * amount
}

fn contrast(c: Rgb, amount: f32) -> Rgb {
    c.map(|v| s_curve(v, amount))
}

/// Lift the blacks to `floor` and pull the whites to `ceiling`.
fn fade(c: Rgb, floor: f32, ceiling: f32) -> Rgb {
    c.map(|v| floor + v * (ceiling - floor))
}

/// Warm (positive) or cool (negative): red and blue gains in opposition.
fn temperature(c: Rgb, t: f32) -> Rgb {
    [
        c[0] * (1.0 + 0.12 * t),
        c[1] * (1.0 + 0.02 * t),
        c[2] * (1.0 - 0.14 * t),
    ]
}

/// Green (negative) or magenta (positive).
fn tint(c: Rgb, t: f32) -> Rgb {
    [
        c[0] * (1.0 + 0.04 * t),
        c[1] * (1.0 - 0.08 * t),
        c[2] * (1.0 + 0.04 * t),
    ]
}

fn exposure(c: Rgb, stops: f32) -> Rgb {
    let k = 2f32.powf(stops);
    c.map(|v| v * k)
}

/// Push shadows towards `shadow` and highlights towards `highlight` (colours
/// as offsets from neutral), weighted by brightness.
fn split_tone(c: Rgb, shadow: Rgb, highlight: Rgb, amount: f32) -> Rgb {
    let l = luma(c).clamp(0.0, 1.0);
    let ws = (1.0 - l).powi(2) * amount;
    let wh = l.powi(2) * amount;
    [
        c[0] + shadow[0] * ws + highlight[0] * wh,
        c[1] + shadow[1] * ws + highlight[1] * wh,
        c[2] + shadow[2] * ws + highlight[2] * wh,
    ]
}

/// Grey from a channel mix: the filter on a black-and-white camera.
fn mono(c: Rgb, weights: Rgb) -> Rgb {
    let sum = weights[0] + weights[1] + weights[2];
    let l = (c[0] * weights[0] + c[1] * weights[1] + c[2] * weights[2]) / sum;
    [l, l, l]
}

/// Colour a grey image: `shadow` at black, `highlight` at white.
fn tone(l: f32, shadow: Rgb, highlight: Rgb) -> Rgb {
    mix(shadow, highlight, l.clamp(0.0, 1.0))
}

/// Teal and orange: warm colours (skin) towards orange, everything cool
/// towards teal, at the same brightness.
fn teal_orange(c: Rgb, amount: f32) -> Rgb {
    let l = luma(c);
    let warmth = ((c[0] - c[2]) * 2.0).clamp(-1.0, 1.0);
    let orange = [l * 1.35, l * 0.95, l * 0.6];
    let teal = [l * 0.6, l * 1.02, l * 1.12];
    let target = if warmth > 0.0 { orange } else { teal };
    let weight = warmth.abs().sqrt() * amount;
    mix(c, target, weight.min(1.0))
}

/// One curve per channel, as a cross-processed slide looks.
fn channel_curves(c: Rgb, r: f32, g: f32, b: f32) -> Rgb {
    [
        c[0].clamp(0.0, 1.0).powf(r),
        c[1].clamp(0.0, 1.0).powf(g),
        c[2].clamp(0.0, 1.0).powf(b),
    ]
}

/// Darken the edges of the gamut: highlights roll off instead of clipping.
fn roll_off(c: Rgb, knee: f32) -> Rgb {
    c.map(|v| {
        if v <= knee {
            v
        } else {
            let over = v - knee;
            knee + over / (1.0 + over / (1.0 - knee))
        }
    })
}

// --- the looks -----------------------------------------------------------------

pub const LOOKS: &[Look] = &[
    // Cinematic
    Look {
        name: "Teal & Orange",
        category: "Cinematic",
        apply: |c| clamp(contrast(teal_orange(c, 0.55), 0.25)),
    },
    Look {
        name: "Blockbuster",
        category: "Cinematic",
        apply: |c| clamp(saturate(contrast(teal_orange(c, 0.8), 0.45), 1.1)),
    },
    Look {
        name: "Night Blue",
        category: "Cinematic",
        apply: |c| {
            clamp(split_tone(
                saturate(exposure(temperature(c, -0.8), -0.3), 0.75),
                [-0.04, 0.0, 0.08],
                [0.0, 0.02, 0.04],
                1.0,
            ))
        },
    },
    Look {
        name: "Moody Green",
        category: "Cinematic",
        apply: |c| {
            clamp(fade(
                contrast(saturate(tint(c, -0.6), 0.7), 0.35),
                0.03,
                0.95,
            ))
        },
    },
    Look {
        name: "Desert Heat",
        category: "Cinematic",
        apply: |c| {
            clamp(roll_off(
                contrast(saturate(temperature(c, 1.0), 1.1), 0.3),
                0.8,
            ))
        },
    },
    Look {
        name: "Bleach Bypass",
        category: "Cinematic",
        apply: |c| {
            let l = luma(c);
            clamp(contrast(mix(c, [l, l, l], 0.55), 0.6))
        },
    },
    // Film-inspired
    Look {
        name: "Soft Print",
        category: "Film",
        apply: |c| {
            clamp(fade(
                roll_off(
                    split_tone(
                        saturate(c, 0.9),
                        [0.0, 0.01, 0.03],
                        [0.03, 0.01, -0.02],
                        1.0,
                    ),
                    0.75,
                ),
                0.03,
                0.97,
            ))
        },
    },
    Look {
        name: "Warm Negative",
        category: "Film",
        apply: |c| {
            clamp(fade(
                saturate(temperature(contrast(c, 0.15), 0.45), 0.85),
                0.04,
                0.96,
            ))
        },
    },
    Look {
        name: "Slide 100",
        category: "Film",
        apply: |c| clamp(saturate(contrast(c, 0.5), 1.35)),
    },
    Look {
        name: "Cool Chrome",
        category: "Film",
        apply: |c| {
            clamp(split_tone(
                saturate(contrast(temperature(c, -0.35), 0.35), 0.8),
                [0.0, 0.02, 0.05],
                [0.0, 0.0, 0.0],
                1.0,
            ))
        },
    },
    Look {
        name: "Golden Hour",
        category: "Film",
        apply: |c| {
            clamp(split_tone(
                fade(temperature(c, 0.6), 0.03, 0.98),
                [0.02, 0.0, -0.02],
                [0.05, 0.02, -0.04],
                1.0,
            ))
        },
    },
    Look {
        name: "Expired Stock",
        category: "Film",
        apply: |c| {
            clamp(fade(
                split_tone(
                    saturate(tint(c, 0.5), 0.75),
                    [0.0, 0.04, 0.03],
                    [0.06, 0.0, -0.03],
                    1.0,
                ),
                0.06,
                0.93,
            ))
        },
    },
    // Black and white
    Look {
        name: "Mono",
        category: "Black & white",
        apply: |c| clamp(mono(c, [0.2126, 0.7152, 0.0722])),
    },
    Look {
        name: "Mono Contrast",
        category: "Black & white",
        apply: |c| clamp(contrast(mono(c, [0.25, 0.65, 0.10]), 0.7)),
    },
    Look {
        name: "Mono Soft",
        category: "Black & white",
        apply: |c| clamp(fade(mono(c, [0.3, 0.6, 0.1]), 0.08, 0.92)),
    },
    Look {
        name: "Red Filter",
        category: "Black & white",
        apply: |c| clamp(contrast(mono(c, [0.8, 0.2, 0.0]), 0.45)),
    },
    Look {
        name: "Sepia",
        category: "Black & white",
        apply: |c| {
            let l = luma(c);
            clamp(tone(s_curve(l, 0.2), [0.12, 0.07, 0.03], [1.0, 0.93, 0.78]))
        },
    },
    Look {
        name: "Selenium",
        category: "Black & white",
        apply: |c| {
            let l = luma(c);
            clamp(tone(
                s_curve(l, 0.35),
                [0.07, 0.05, 0.09],
                [0.98, 0.98, 0.95],
            ))
        },
    },
    // Warm and cool
    Look {
        name: "Warm Sun",
        category: "Warm & cool",
        apply: |c| clamp(saturate(temperature(c, 0.8), 1.08)),
    },
    Look {
        name: "Amber",
        category: "Warm & cool",
        apply: |c| {
            clamp(split_tone(
                temperature(contrast(c, 0.2), 0.5),
                [0.03, 0.01, -0.03],
                [0.06, 0.03, -0.05],
                1.0,
            ))
        },
    },
    Look {
        name: "Cool Morning",
        category: "Warm & cool",
        apply: |c| clamp(fade(temperature(c, -0.6), 0.02, 1.0)),
    },
    Look {
        name: "Arctic",
        category: "Warm & cool",
        apply: |c| clamp(saturate(exposure(temperature(c, -1.0), 0.15), 0.7)),
    },
    // Vintage
    Look {
        name: "Faded Matte",
        category: "Vintage",
        apply: |c| clamp(fade(saturate(c, 0.8), 0.1, 0.94)),
    },
    Look {
        name: "Pastel",
        category: "Vintage",
        apply: |c| clamp(fade(saturate(exposure(c, 0.2), 0.65), 0.08, 1.0)),
    },
    Look {
        name: "Seventies",
        category: "Vintage",
        apply: |c| {
            clamp(fade(
                split_tone(
                    saturate(temperature(c, 0.7), 0.9),
                    [0.03, 0.02, -0.04],
                    [0.04, 0.03, -0.06],
                    1.0,
                ),
                0.07,
                0.95,
            ))
        },
    },
    Look {
        name: "Cross Process",
        category: "Vintage",
        apply: |c| clamp(saturate(channel_curves(c, 0.85, 0.9, 1.35), 1.15)),
    },
    Look {
        name: "Toy Camera",
        category: "Vintage",
        apply: |c| {
            clamp(saturate(
                contrast(channel_curves(c, 0.92, 0.88, 1.15), 0.5),
                1.3,
            ))
        },
    },
    Look {
        name: "Instant",
        category: "Vintage",
        apply: |c| {
            clamp(fade(
                split_tone(tint(c, -0.3), [0.0, 0.03, 0.04], [0.05, 0.03, 0.0], 1.0),
                0.08,
                0.95,
            ))
        },
    },
];

/// The `.cube` text of a look.
pub fn cube_text(look: &Look) -> String {
    let n = SIZE;
    let mut out = String::with_capacity((n * n * n * 24) as usize);
    out.push_str(&format!(
        "{MARKER} v{LOOKS_VERSION}: generated by chukcut, GPL-3.0-or-later, free to use in any video\nTITLE \"{}\"\nLUT_3D_SIZE {n}\n",
        look.name
    ));
    let last = (n - 1) as f32;
    for b in 0..n {
        for g in 0..n {
            for r in 0..n {
                let [x, y, z] = (look.apply)([r as f32 / last, g as f32 / last, b as f32 / last]);
                out.push_str(&format!("{x:.5} {y:.5} {z:.5}\n"));
            }
        }
    }
    out
}

/// The file a look is installed as.
pub fn path_in(dir: &Path, look: &Look) -> PathBuf {
    dir.join(format!("{}.cube", look.name.replace('&', "and")))
}

fn ours(path: &Path) -> bool {
    use std::io::Read as _;
    let mut head = [0u8; 16];
    std::fs::File::open(path)
        .and_then(|mut f| f.read_exact(&mut head))
        .is_ok_and(|()| head.starts_with(MARKER.as_bytes()))
}

/// Write the looks into `dir` unless this version is installed already.
/// Returns how many files were written.
pub fn install(dir: &Path) -> Result<usize, String> {
    let stamp = dir.join(STAMP);
    let installed: u32 = std::fs::read_to_string(&stamp)
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(0);
    if installed >= LOOKS_VERSION {
        return Ok(0);
    }
    std::fs::create_dir_all(dir).map_err(|e| format!("cannot create the LUT library: {e}"))?;
    let mut written = 0;
    for look in LOOKS {
        let path = path_in(dir, look);
        // A file of the user's under the same name stays theirs.
        if path.exists() && !ours(&path) {
            continue;
        }
        // Two installs can run at once — a template's preview tile and a new
        // project both make sure the looks are there — so each writes under a
        // partial name of its own. A shared `<name>.cube.part` let one
        // truncate the file the other was renaming, and the loser failed
        // with "cannot write …/Moody Green.cube".
        write_atomically(&path, cube_text(look).as_bytes())?;
        written += 1;
    }
    write_atomically(&stamp, LOOKS_VERSION.to_string().as_bytes())?;
    Ok(written)
}

/// The looks installed in `dir`, in our order, skipping any the user deleted.
pub fn installed(dir: &Path) -> Vec<LookEntry> {
    LOOKS
        .iter()
        .map(|look| (look, path_in(dir, look)))
        .filter(|(_, path)| path.exists() && ours(path))
        .map(|(look, path)| LookEntry {
            name: look.name.to_string(),
            category: look.category.to_string(),
            path: path.to_string_lossy().to_string(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::render::lut;

    #[test]
    fn every_look_is_a_valid_cube_that_changes_the_picture() {
        assert!(LOOKS.len() >= 20 && LOOKS.len() <= 30, "{}", LOOKS.len());
        for look in LOOKS {
            assert!(CATEGORIES.contains(&look.category), "{}", look.name);
            let cube =
                lut::parse(&cube_text(look)).unwrap_or_else(|e| panic!("{}: {e}", look.name));
            assert_eq!(cube.size, SIZE);
            assert_eq!(cube.title.as_deref(), Some(look.name));
            // A skin tone and a sky: a look that leaves both alone is no look.
            let skin = cube.sample([0.8, 0.6, 0.5]);
            let sky = cube.sample([0.3, 0.5, 0.8]);
            let moved = (skin[0] - 0.8).abs()
                + (skin[1] - 0.6).abs()
                + (skin[2] - 0.5).abs()
                + (sky[0] - 0.3).abs()
                + (sky[1] - 0.5).abs()
                + (sky[2] - 0.8).abs();
            assert!(
                moved > 0.03,
                "{} barely changes anything ({moved})",
                look.name
            );
            for v in skin.iter().chain(sky.iter()) {
                assert!((0.0..=1.0).contains(v), "{} leaves the range", look.name);
            }
        }
    }

    #[test]
    fn black_and_white_looks_are_grey() {
        for look in LOOKS
            .iter()
            .filter(|l| l.name.starts_with("Mono") || l.name == "Red Filter")
        {
            let [r, g, b] = (look.apply)([0.7, 0.3, 0.2]);
            assert!(
                (r - g).abs() < 1e-5 && (g - b).abs() < 1e-5,
                "{}",
                look.name
            );
        }
    }

    /// The race `tests/templates.rs` hit with its tests in parallel: every
    /// one of them installs the looks into the same data directory.
    #[test]
    fn installs_running_at_once_all_succeed() {
        let dir = super::super::scratch("looks-concurrent");
        for _ in 0..3 {
            let _ = std::fs::remove_dir_all(&dir);
            std::thread::scope(|scope| {
                let installs: Vec<_> = (0..6).map(|_| scope.spawn(|| install(&dir))).collect();
                for install in installs {
                    install.join().unwrap().unwrap();
                }
            });
            assert_eq!(installed(&dir).len(), LOOKS.len());
            let stray: Vec<_> = std::fs::read_dir(&dir)
                .unwrap()
                .filter_map(|e| e.ok())
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|name| name.ends_with(".part"))
                .collect();
            assert!(stray.is_empty(), "partial files left behind: {stray:?}");
        }
    }

    #[test]
    fn install_writes_once_and_leaves_the_users_files_alone() {
        let dir = super::super::scratch("looks");
        std::fs::create_dir_all(&dir).unwrap();
        let theirs = path_in(&dir, &LOOKS[0]);
        std::fs::write(
            &theirs,
            "LUT_3D_SIZE 2\n0 0 0\n1 0 0\n0 1 0\n1 1 0\n0 0 1\n1 0 1\n0 1 1\n1 1 1\n",
        )
        .unwrap();
        let written = install(&dir).unwrap();
        assert_eq!(written, LOOKS.len() - 1);
        assert!(std::fs::read_to_string(&theirs)
            .unwrap()
            .starts_with("LUT_3D_SIZE 2"));
        assert_eq!(installed(&dir).len(), LOOKS.len() - 1);
        // Deleted by the user: stays deleted.
        std::fs::remove_file(path_in(&dir, &LOOKS[1])).unwrap();
        assert_eq!(install(&dir).unwrap(), 0);
        assert_eq!(installed(&dir).len(), LOOKS.len() - 2);
    }
}
