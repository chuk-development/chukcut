//! The measuring apparatus: timing, summarising, refusing, and reporting.
//!
//! Nothing in here knows what a frame is. It exists so that every group in the
//! suite reports the same way, because the thing that has repeatedly gone wrong
//! in this repository is not the measurement but the *reading* of it —
//! `docs/STATUS.md` records two conclusions drawn from single runs taken while
//! the machine was busy, both of which were wrong.
//!
//! Three deliberate choices, each one a reaction to a mistake already made:
//!
//! - **Median and best, never mean.** Every source of noise on this machine
//!   (thermal state, another rustc, a rayon worker) only ever *adds* time, so
//!   the minimum is the closest thing to what the work costs and the median is
//!   what a user experiences. A mean is decided by the worst sample.
//! - **The spread is reported next to the number, always.** `worst / best`. If
//!   it is above about 1.5 the run was not quiet and no single figure from it
//!   should be quoted.
//! - **A loaded machine refuses to report.** Above `--max-load` the suite exits
//!   rather than producing a plausible-looking table, because a plausible-looking
//!   table is exactly what caused the earlier wrong conclusions.

use std::time::Instant;

use serde::{Deserialize, Serialize};

/// Which way is better, so `--compare` can colour a change correctly.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    /// Milliseconds, bytes — down is a win.
    Lower,
    /// Frames per second — up is a win.
    Higher,
}

/// One row of the table.
///
/// A skipped row is still a row. A benchmark that silently omits what it could
/// not measure produces a table that reads as complete, and the next person has
/// no way to tell "this machine has no VAAPI" from "nobody thought to measure
/// it".
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Measurement {
    pub group: String,
    pub name: String,
    pub unit: String,
    pub direction: Direction,
    pub runs: usize,
    pub median: f64,
    pub best: f64,
    pub worst: f64,
    /// `worst / best`. 1.0 is a perfectly quiet machine; above ~1.5 means the
    /// numbers in this row are not worth a percentage comparison.
    pub spread: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// Present when the row could not be measured. The string says why, in
    /// prose, and is never empty.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skipped: Option<String>,
}

impl Measurement {
    pub fn new(
        group: &str,
        name: impl Into<String>,
        unit: &str,
        direction: Direction,
        mut samples: Vec<f64>,
    ) -> Self {
        assert!(
            !samples.is_empty(),
            "a measurement needs at least one sample"
        );
        samples.sort_by(|a, b| a.partial_cmp(b).expect("no NaN timings"));
        let best = samples[0];
        let worst = samples[samples.len() - 1];
        let median = samples[samples.len() / 2];
        Self {
            group: group.to_string(),
            name: name.into(),
            unit: unit.to_string(),
            direction,
            runs: samples.len(),
            median,
            best,
            worst,
            spread: if best > 0.0 { worst / best } else { 1.0 },
            note: None,
            skipped: None,
        }
    }

    /// Milliseconds, lower is better. The common case.
    pub fn ms(group: &str, name: impl Into<String>, samples: Vec<f64>) -> Self {
        Self::new(group, name, "ms", Direction::Lower, samples)
    }

    /// Frames per second, higher is better.
    pub fn fps(group: &str, name: impl Into<String>, samples: Vec<f64>) -> Self {
        Self::new(group, name, "fps", Direction::Higher, samples)
    }

    /// A row that could not be measured, and the reason a human needs.
    pub fn skip(group: &str, name: impl Into<String>, reason: impl Into<String>) -> Self {
        Self {
            group: group.to_string(),
            name: name.into(),
            unit: String::new(),
            direction: Direction::Lower,
            runs: 0,
            median: f64::NAN,
            best: f64::NAN,
            worst: f64::NAN,
            spread: f64::NAN,
            note: None,
            skipped: Some(reason.into()),
        }
    }

    pub fn with_note(mut self, note: impl Into<String>) -> Self {
        self.note = Some(note.into());
        self
    }

    pub fn key(&self) -> String {
        format!("{}/{}", self.group, self.name)
    }

    /// `best` on the scale where the *fastest* run is the extreme. For fps the
    /// best run is the largest sample, which `new` sorted to the end.
    pub fn best_run(&self) -> f64 {
        match self.direction {
            Direction::Lower => self.best,
            Direction::Higher => self.worst,
        }
    }
}

/// Everything one invocation produced, which is what `--json` writes and
/// `--compare` reads.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Report {
    /// Bumped if the shape of this file ever changes incompatibly, so a
    /// `--compare` against an old file says so instead of misreading it.
    pub schema: u32,
    pub command: String,
    /// Seconds since the epoch. Deliberately not a formatted date: this file is
    /// diffed, and a locale-dependent string is noise in a diff.
    pub unix_time: u64,
    pub load_before: f64,
    pub load_after: f64,
    pub machine: Machine,
    pub results: Vec<Measurement>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Machine {
    pub cpu_threads: usize,
    pub gpu: String,
    pub gpu_backend: String,
    pub ffmpeg: String,
    pub hardware_decode: Vec<String>,
    pub hardware_encode: Vec<String>,
    pub hardware_jpeg: bool,
}

/// Milliseconds a closure took.
pub fn time_ms<T>(body: impl FnOnce() -> T) -> (T, f64) {
    let started = Instant::now();
    let value = body();
    (value, started.elapsed().as_secs_f64() * 1000.0)
}

/// Run `body` `rounds` times, collecting one sample each, stopping at the first
/// error.
///
/// Errors are not swallowed into a slow sample. A round that failed measured
/// nothing, and averaging a failure in is how a benchmark reports that a broken
/// path is fast.
pub fn rounds<E>(
    count: usize,
    mut body: impl FnMut(usize) -> Result<f64, E>,
) -> Result<Vec<f64>, E> {
    let mut samples = Vec::with_capacity(count);
    for round in 0..count.max(1) {
        samples.push(body(round)?);
    }
    Ok(samples)
}

/// Run every candidate once per round, in the same order, rather than all the
/// rounds of A and then all the rounds of B.
///
/// `examples/preview_jpeg_bench.rs` found the reason the hard way: measured the
/// other arrangement, the same encoder came out at 8 ms in one ordering and
/// 31 ms in another, because whichever candidate ran second paid for the heat
/// the first one generated. Interleaving does not remove the noise; it stops it
/// all landing in one column.
///
/// The untimed first pass is not politeness either — the first call of each
/// candidate opens a device, allocates a pool or warms a cache, and the
/// thirtieth frame of playback pays for none of that.
pub fn interleave(count: usize, candidates: &mut [&mut dyn FnMut()]) -> Vec<Vec<f64>> {
    for body in candidates.iter_mut() {
        body();
    }
    let mut samples: Vec<Vec<f64>> = vec![Vec::with_capacity(count); candidates.len()];
    for _ in 0..count.max(1) {
        for (index, body) in candidates.iter_mut().enumerate() {
            let (_, ms) = time_ms(body);
            samples[index].push(ms);
        }
    }
    samples
}

/// The one-minute load average, or `None` where `/proc` is not Linux's.
pub fn load_average() -> Option<f64> {
    let text = std::fs::read_to_string("/proc/loadavg").ok()?;
    text.split_whitespace().next()?.parse().ok()
}

pub fn unix_time() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// Printing
// ---------------------------------------------------------------------------

const NAME_WIDTH: usize = 42;

pub fn print_table(results: &[Measurement]) {
    let mut current = String::new();
    for row in results {
        if row.group != current {
            current.clone_from(&row.group);
            println!();
            println!("## {current}");
            println!();
            println!(
                "{:<width$} {:>14} {:>10} {:>8}  ",
                "",
                "median",
                "best",
                "spread",
                width = NAME_WIDTH
            );
        }
        match &row.skipped {
            Some(reason) => println!(
                "{:<width$} {:>14} {:>10} {:>8}  skipped: {reason}",
                truncate(&row.name),
                "—",
                "—",
                "—",
                width = NAME_WIDTH
            ),
            None => println!(
                "{:<width$} {:>14} {:>10} {:>8}  {}",
                truncate(&row.name),
                format!("{:.2} {}", row.median, row.unit),
                format!("{:.2}", row.best_run()),
                format!("{:.2}x", row.spread),
                row.note.as_deref().unwrap_or(""),
                width = NAME_WIDTH
            ),
        }
    }
}

fn truncate(name: &str) -> String {
    if name.chars().count() <= NAME_WIDTH {
        name.to_string()
    } else {
        name.chars().take(NAME_WIDTH - 1).collect::<String>() + "…"
    }
}

/// Below this a change is not distinguishable from the machine's own noise.
///
/// `docs/STATUS.md`: "a change of less than about 20% cannot be read off one
/// [run]". The threshold is printed with the table so nobody has to remember it.
pub const NOISE_FLOOR_PCT: f64 = 20.0;

/// The smallest absolute change worth a verdict, per unit.
///
/// A percentage floor on its own is not enough, and the first version of
/// `--compare` proved it: a DMA-BUF decode that moved from 0.88 to 1.06 ms was
/// reported as "SLOWER, +21%" when the difference is 0.18 ms — below what a
/// timer, a scheduler and a clock domain can resolve on this machine. Six of the
/// seventy-six rows in the first self-comparison were false verdicts of exactly
/// that shape, all of them on sub-millisecond quantities.
///
/// So a change has to clear *both* floors. Anything else is a benchmark
/// inventing regressions, which is the failure this whole suite exists to stop.
fn resolution(unit: &str) -> f64 {
    match unit {
        // A third of a millisecond. Below this we are timing the timer.
        "ms" => 0.3,
        // Two frames per second, which at 30 fps is under a millisecond a frame.
        "fps" => 2.0,
        "µs/edit" | "µs/undo" => 0.5,
        _ => 0.0,
    }
}

pub fn print_comparison(before: &Report, after: &Report) {
    println!();
    println!("## Regression table");
    println!();
    println!(
        "Baseline taken at unix {} under load {:.2}; this run at unix {} under load {:.2}.",
        before.unix_time, before.load_before, after.unix_time, after.load_before
    );
    println!(
        "A change is only a verdict if it clears ±{NOISE_FLOOR_PCT:.0}% *and* an absolute floor \
         (0.3 ms, 2 fps, 0.5 µs); anything else is marked `noise`, because this machine cannot \
         resolve smaller than that between runs."
    );
    println!();
    println!(
        "{:<width$} {:>12} {:>12} {:>10}  verdict",
        "",
        "before",
        "after",
        "change",
        width = NAME_WIDTH
    );

    let index: std::collections::HashMap<String, &Measurement> =
        before.results.iter().map(|m| (m.key(), m)).collect();

    for row in &after.results {
        let Some(old) = index.get(&row.key()) else {
            println!(
                "{:<width$} {:>12} {:>12} {:>10}  new",
                truncate(&row.name),
                "—",
                cell(row),
                "—",
                width = NAME_WIDTH
            );
            continue;
        };
        if row.skipped.is_some() || old.skipped.is_some() {
            println!(
                "{:<width$} {:>12} {:>12} {:>10}  {}",
                truncate(&row.name),
                cell(old),
                cell(row),
                "—",
                if row.skipped.is_some() {
                    "skipped now"
                } else {
                    "skipped before"
                },
                width = NAME_WIDTH
            );
            continue;
        }
        // Medians, not bests: the median is what the change has to survive.
        let change = (row.median - old.median) / old.median * 100.0;
        let improved = match row.direction {
            Direction::Lower => change < 0.0,
            Direction::Higher => change > 0.0,
        };
        let below_resolution = (row.median - old.median).abs() < resolution(&row.unit);
        let verdict = if change.abs() < NOISE_FLOOR_PCT || below_resolution {
            "noise".to_string()
        } else if improved {
            "FASTER".to_string()
        } else {
            "SLOWER".to_string()
        };
        let shaky = row.spread.max(old.spread) > 1.5;
        println!(
            "{:<width$} {:>12} {:>12} {:>10}  {verdict}{}",
            truncate(&row.name),
            cell(old),
            cell(row),
            format!("{change:+.1}%"),
            if shaky {
                "  (spread > 1.5x, do not trust)"
            } else {
                ""
            },
            width = NAME_WIDTH
        );
    }

    let missing: Vec<&Measurement> = before
        .results
        .iter()
        .filter(|m| !after.results.iter().any(|n| n.key() == m.key()))
        .collect();
    if !missing.is_empty() {
        println!();
        println!("Rows in the baseline that this run did not produce:");
        for row in missing {
            println!("  {}", row.key());
        }
    }
}

fn cell(row: &Measurement) -> String {
    match &row.skipped {
        Some(_) => "—".to_string(),
        None => format!("{:.2}{}", row.median, unit_suffix(&row.unit)),
    }
}

fn unit_suffix(unit: &str) -> String {
    if unit.is_empty() {
        String::new()
    } else {
        format!(" {unit}")
    }
}
