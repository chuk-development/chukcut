//! `chukcut-bench` — the performance suite for the whole pipeline.
//!
//! ```bash
//! cd crates/engine
//! cargo run --release --bin chukcut-bench -- --all
//! cargo run --release --bin chukcut-bench -- --filter decode --json runs/today.json
//! cargo run --release --bin chukcut-bench -- --compare runs/yesterday.json
//! ```
//!
//! Every measured number in `docs/STATUS.md` used to come from a different
//! ad-hoc harness — `examples/decode_bench.rs`, `examples/export_smoke.rs`,
//! `examples/preview_jpeg_bench.rs` — each with its own idea of how many runs
//! to take, whether to discard the first, and what to do about a busy machine.
//! Those examples are still worth keeping: they print things a table cannot,
//! like a DMA-BUF plane layout or a PSNR against a reference encode. But the
//! *numbers* should come from here, because here they are taken the same way.
//!
//! ## The rules this binary enforces
//!
//! - **It refuses to report on a busy machine.** Above `--max-load` (default
//!   4.0) it exits without measuring anything. `docs/STATUS.md` records
//!   run-to-run swings of 3× under load, and two wrong conclusions drawn from
//!   them. `--force` overrides it and stamps the result as forced.
//! - **Median, best and spread, always together.** A median with no spread next
//!   to it is how a noisy run gets quoted as a fact.
//! - **It generates its own media.** `target/bench-media/`, from `lavfi`, once
//!   per machine. Nothing here needs a particular directory of footage to exist.
//! - **It finishes.** No prompts, no network, no GUI, no unbounded loops.
//!   Anything the machine cannot do is a skip with a named reason, printed in
//!   the table next to everything that did run.
//!
//! ## What it deliberately does not do
//!
//! It does not run the app. The preview's ring buffer, its pacing clock and the
//! webview's own decode and paint are outside every number here, and the group
//! documentation says so where it matters. Measuring them needs a GUI, which
//! this binary is required not to need.

mod bench_composite;
mod bench_decode;
mod bench_export;
mod bench_preview;
mod bench_project;
mod fixtures;
mod harness;

use std::path::PathBuf;

use chukcut_engine::modules::export::hwaccel;
use chukcut_engine::modules::media::hwdecode;
use chukcut_engine::modules::preview::hardware_available;
use chukcut_engine::modules::render::RenderContext;

use harness::{load_average, Machine, Measurement, Report};

/// Above this one-minute load average the suite will not report.
///
/// Four rather than one on a twelve-thread machine: a completely idle desktop
/// is not a realistic requirement, and the failures that motivated the check
/// were at load 10 and above, with a compile running.
const DEFAULT_MAX_LOAD: f64 = 4.0;

struct Options {
    filter: Option<String>,
    json: Option<PathBuf>,
    compare: Option<PathBuf>,
    force: bool,
    max_load: f64,
    /// Longer runs of everything. Roughly triples the wall time and is what to
    /// use before writing a number into `docs/STATUS.md`.
    full: bool,
    media_dir: Option<PathBuf>,
    list: bool,
}

const GROUPS: [&str; 6] = [
    bench_decode::GROUP,
    bench_composite::GROUP,
    bench_preview::ENCODE_GROUP,
    bench_preview::FRAME_GROUP,
    bench_export::GROUP,
    bench_project::GROUP,
];

fn usage() -> String {
    format!(
        "chukcut-bench — the performance suite for the whole pipeline\n\
         \n\
         usage: cargo run --release --bin chukcut-bench -- [options]\n\
         \n\
         --all                  run everything (the default)\n\
         --filter <substring>   run only groups or rows whose name contains this\n\
         --json <file>          write the results as JSON, for diffing between runs\n\
         --compare <file>       print a regression table against an earlier --json file\n\
         --force                report even though the machine is busy\n\
         --max-load <n>         load average above which to refuse (default {DEFAULT_MAX_LOAD})\n\
         --full                 longer runs; roughly 3x the wall time\n\
         --media-dir <dir>      where to cache generated fixtures\n\
         --list                 print the group names and exit\n\
         -h, --help             this\n\
         \n\
         groups: {}\n",
        GROUPS.join(", ")
    )
}

fn parse() -> Result<Options, String> {
    let mut options = Options {
        filter: None,
        json: None,
        compare: None,
        force: false,
        max_load: DEFAULT_MAX_LOAD,
        full: false,
        media_dir: None,
        list: false,
    };
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        // Both `--flag value` and `--flag=value`, because every other tool in
        // this repository accepts one or the other and nobody remembers which.
        let (flag, inline) = match arg.split_once('=') {
            Some((flag, value)) => (flag.to_string(), Some(value.to_string())),
            None => (arg.clone(), None),
        };
        let mut value = || inline.clone().or_else(|| args.next());
        match flag.as_str() {
            "--all" => {}
            "--filter" => {
                options.filter = Some(value().ok_or("--filter needs a substring")?.to_lowercase())
            }
            "--json" => options.json = Some(PathBuf::from(value().ok_or("--json needs a path")?)),
            "--compare" => {
                options.compare = Some(PathBuf::from(value().ok_or("--compare needs a path")?))
            }
            "--force" => options.force = true,
            "--full" => options.full = true,
            "--max-load" => {
                options.max_load = value()
                    .ok_or("--max-load needs a number")?
                    .parse()
                    .map_err(|_| "--max-load needs a number")?
            }
            "--media-dir" => {
                options.media_dir = Some(PathBuf::from(value().ok_or("--media-dir needs a path")?))
            }
            "--list" => options.list = true,
            "-h" | "--help" => {
                print!("{}", usage());
                std::process::exit(0);
            }
            other => return Err(format!("unknown option {other}\n\n{}", usage())),
        }
    }
    Ok(options)
}

fn main() {
    // Warnings only. The engine logs a line per decoded frame at debug and
    // several per export; at info the table would be unreadable, and the
    // logging itself would be inside the measurement.
    tracing_subscriber::fmt()
        .with_env_filter(
            std::env::var("RUST_LOG").unwrap_or_else(|_| "chukcut=warn,warn".to_string()),
        )
        .with_writer(std::io::stderr)
        .init();

    // libavcodec's own log, which `tracing` does not route. Both hardware
    // probes work by *trying* things that are expected to fail — `hwaccel`
    // opens `av1_vaapi` on a chip that cannot encode AV1, `hwdecode` attaches a
    // VAAPI device to every codec — so a normal run prints a dozen alarming
    // lines about entrypoints and child device handles that are the probe
    // working correctly. Fatal keeps a genuine failure visible and drops those.
    // `RUST_LOG` containing `ffmpeg` puts them back, for when a skip reason is
    // not enough to explain why a path was refused.
    if !std::env::var("RUST_LOG")
        .unwrap_or_default()
        .contains("ffmpeg")
    {
        ffmpeg_next::util::log::set_level(ffmpeg_next::util::log::Level::Fatal);
    }

    let options = match parse() {
        Ok(options) => options,
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(2);
        }
    };

    if options.list {
        for group in GROUPS {
            println!("{group}");
        }
        return;
    }

    let load_before = load_average().unwrap_or(0.0);
    if load_before > options.max_load && !options.force {
        eprintln!(
            "refusing to measure: one-minute load average is {load_before:.2}, above the \
             {:.2} threshold.\n\
             \n\
             This is not caution for its own sake. Numbers on this machine swing by a factor of \
             three under load, and `docs/STATUS.md` records two conclusions drawn from such runs \
             that were later found to be wrong. Wait for the machine to go quiet, or pass \
             --force and treat everything that comes out as indicative only.",
            options.max_load
        );
        std::process::exit(1);
    }
    if options.force && load_before > options.max_load {
        eprintln!(
            "WARNING: measuring under load {load_before:.2} because --force was given. Nothing \
             from this run belongs in docs/STATUS.md."
        );
    }

    let started = std::time::Instant::now();
    let wanted = |group: &str| match &options.filter {
        Some(filter) => group.to_lowercase().contains(filter),
        None => true,
    };

    // Fixtures first: generating them is the slow part of a first run, and it
    // should not happen after twenty seconds of unrelated benchmarking.
    let media_dir = fixtures::media_dir(options.media_dir.as_deref());
    let needs_media = wanted(bench_decode::GROUP)
        || wanted(bench_preview::FRAME_GROUP)
        || wanted(bench_export::GROUP);
    let media = if needs_media {
        match fixtures::ensure(&media_dir, if options.full { 6.0 } else { 4.0 }, 30) {
            Ok(media) => media,
            Err(error) => {
                eprintln!(
                    "could not prepare fixtures in {}: {error}",
                    media_dir.display()
                );
                fixtures::Fixtures {
                    clips: Vec::new(),
                    missing: vec![("all".into(), error.to_string())],
                }
            }
        }
    } else {
        fixtures::Fixtures {
            clips: Vec::new(),
            missing: Vec::new(),
        }
    };

    // One GPU device for the whole process — the library's own, which is the
    // only one there is. Concurrent Vulkan instances were observed crashing this
    // driver, so a benchmark that opened one per group would be reproducing a
    // known bug rather than measuring. Asked for lazily so a run of the decode
    // group alone still needs no adapter.
    let needs_gpu = wanted(bench_composite::GROUP)
        || wanted(bench_preview::FRAME_GROUP)
        || wanted(bench_export::GROUP);
    let ctx = needs_gpu
        .then(chukcut_engine::modules::gpu::render_context)
        .flatten();

    let mut results: Vec<Measurement> = Vec::new();

    if wanted(bench_decode::GROUP) {
        eprintln!("· decode …");
        results.extend(bench_decode::run(
            &media,
            &bench_decode::Budget {
                frames: if options.full { 90 } else { 48 },
                rounds: if options.full { 5 } else { 3 },
                seeks: if options.full { 16 } else { 6 },
                seek_rounds: if options.full { 3 } else { 2 },
            },
        ));
    }

    if wanted(bench_composite::GROUP) {
        eprintln!("· composite …");
        match &ctx {
            Some(ctx) => results.extend(bench_composite::run(
                ctx,
                &bench_composite::Budget {
                    frames: if options.full { 60 } else { 30 },
                    rounds: if options.full { 5 } else { 3 },
                },
            )),
            None => results.push(Measurement::skip(
                bench_composite::GROUP,
                "all",
                "no usable GPU adapter on this machine",
            )),
        }
    }

    if wanted(bench_preview::ENCODE_GROUP) {
        eprintln!("· preview-encode …");
        results.extend(bench_preview::run_encode(&bench_preview::Budget {
            runs: if options.full { 21 } else { 9 },
            frames: 0,
            rounds: 0,
        }));
    }

    if wanted(bench_preview::FRAME_GROUP) {
        eprintln!("· preview-frame …");
        match &ctx {
            Some(ctx) => results.extend(bench_preview::run_frame(
                ctx,
                &media,
                &bench_preview::Budget {
                    runs: 0,
                    frames: if options.full { 45 } else { 24 },
                    rounds: 3,
                },
            )),
            None => results.push(Measurement::skip(
                bench_preview::FRAME_GROUP,
                "all",
                "no usable GPU adapter on this machine",
            )),
        }
    }

    if wanted(bench_export::GROUP) {
        eprintln!("· export …");
        match &ctx {
            Some(ctx) => results.extend(bench_export::run(
                ctx,
                &media,
                &bench_export::Budget {
                    seconds: if options.full { 8.0 } else { 3.0 },
                    rounds: if options.full { 3 } else { 2 },
                },
            )),
            None => results.push(Measurement::skip(
                bench_export::GROUP,
                "all",
                "no usable GPU adapter on this machine",
            )),
        }
    }

    if wanted(bench_project::GROUP) {
        eprintln!("· project …");
        results.extend(bench_project::run(&bench_project::Budget {
            segments: 500,
            edits: 1000,
            rounds: if options.full { 7 } else { 5 },
        }));
    }

    // A row-level filter on top of the group-level one, so `--filter vp9` or
    // `--filter "tier 3"` works as well as `--filter decode`.
    if let Some(filter) = &options.filter {
        let group_matched = GROUPS.iter().any(|g| g.to_lowercase().contains(filter));
        if !group_matched {
            results.retain(|row| {
                row.name.to_lowercase().contains(filter)
                    || row.group.to_lowercase().contains(filter)
            });
        }
    }

    if results.is_empty() {
        eprintln!(
            "nothing matched. Groups are: {}. Rows can be filtered too, e.g. --filter vp9.",
            GROUPS.join(", ")
        );
        std::process::exit(1);
    }

    let load_after = load_average().unwrap_or(0.0);
    let report = Report {
        schema: 1,
        command: std::env::args().collect::<Vec<_>>().join(" "),
        unix_time: harness::unix_time(),
        load_before,
        load_after,
        machine: describe_machine(ctx.as_deref()),
        results,
    };

    print_header(&report, started.elapsed().as_secs_f64());
    harness::print_table(&report.results);
    print_footer(&report);

    if let Some(path) = &options.json {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        match serde_json::to_string_pretty(&report) {
            Ok(json) => match std::fs::write(path, json) {
                Ok(()) => eprintln!("\nwrote {}", path.display()),
                Err(error) => eprintln!("\ncould not write {}: {error}", path.display()),
            },
            Err(error) => eprintln!("\ncould not serialize the report: {error}"),
        }
    }

    if let Some(path) = &options.compare {
        match std::fs::read_to_string(path)
            .map_err(|e| e.to_string())
            .and_then(|text| serde_json::from_str::<Report>(&text).map_err(|e| e.to_string()))
        {
            Ok(before) if before.schema != report.schema => eprintln!(
                "\n{} was written by schema {} and this is schema {}; not comparing.",
                path.display(),
                before.schema,
                report.schema
            ),
            Ok(before) => harness::print_comparison(&before, &report),
            Err(error) => eprintln!("\ncould not read {}: {error}", path.display()),
        }
    }
}

fn describe_machine(ctx: Option<&RenderContext>) -> Machine {
    Machine {
        cpu_threads: std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(0),
        // "not opened" rather than "none": a filtered run that touches no GPU
        // group never asks for a device, and recording that as "no GPU" would
        // put a false negative into the JSON that `--compare` then reads.
        gpu: ctx
            .map(|c| c.adapter_info().name.clone())
            .unwrap_or_else(|| "not opened".into()),
        gpu_backend: ctx
            .map(|c| format!("{:?}", c.adapter_info().backend))
            .unwrap_or_else(|| "not opened".into()),
        ffmpeg: fixtures::ffmpeg_version(),
        // What the machine *proved* it can do, by decoding and encoding a real
        // frame. A codec being in the FFmpeg build says nothing, and this
        // repository has been bitten by believing otherwise more than once.
        hardware_decode: hwdecode::capabilities()
            .iter()
            .filter(|s| s.usable)
            .map(|s| s.codec.label().to_string())
            .collect(),
        hardware_encode: hwaccel::detect()
            .iter()
            .filter(|e| e.usable)
            .map(|e| e.encoder_name.clone())
            .collect(),
        hardware_jpeg: hardware_available(),
    }
}

fn print_header(report: &Report, elapsed: f64) {
    println!();
    println!("# chukcut benchmark");
    println!();
    println!("Command: `{}`", report.command);
    println!(
        "Machine: {} ({}), {} CPU threads, {}",
        report.machine.gpu,
        report.machine.gpu_backend,
        report.machine.cpu_threads,
        report.machine.ffmpeg
    );
    println!(
        "Hardware decode: {}. Hardware encode: {}. Hardware JPEG: {}.",
        list(&report.machine.hardware_decode),
        list(&report.machine.hardware_encode),
        if report.machine.hardware_jpeg {
            "yes"
        } else {
            "no"
        }
    );
    println!(
        "Load average {:.2} before, {:.2} after; {elapsed:.0} s of measuring.",
        report.load_before, report.load_after
    );
    println!();
    println!(
        "`median` and `best` are over the runs of each row; `spread` is worst/best. A spread \
         above about 1.5 means the machine was not quiet and that row should not be quoted."
    );
}

fn print_footer(report: &Report) {
    let skipped = report
        .results
        .iter()
        .filter(|r| r.skipped.is_some())
        .count();
    let shaky = report
        .results
        .iter()
        .filter(|r| r.skipped.is_none() && r.spread > 1.5)
        .count();
    println!();
    println!(
        "{} rows, {skipped} skipped, {shaky} with a spread above 1.5x.",
        report.results.len()
    );
    // Some of any rise is this process: the export group runs x264 on eight
    // threads and the decode group is a busy loop, so a clean run typically ends
    // a point or two above where it started. The threshold is set above that,
    // and the wording says so, because "something else started" is a claim and
    // the first version of this line made it wrongly.
    if report.load_after > report.load_before * 2.0 + 2.0 {
        println!(
            "The load average rose from {:.2} to {:.2} while this ran. Part of that is this \
             process — the export group runs x264 on eight threads — but not this much: something \
             else started, and the groups that ran last (export, project) are the ones to \
             distrust. Take it again, and use --compare against this run: a group that moved and \
             a group that did not is the signal.",
            report.load_before, report.load_after
        );
    }
}

fn list(items: &[String]) -> String {
    if items.is_empty() {
        "none".to_string()
    } else {
        items.join(", ")
    }
}
