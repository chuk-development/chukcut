//! `chukcut-cli`: chukcut's editing commands from a shell, a script or an AI
//! agent.
//!
//! Every subcommand opens a project file, applies one operation through the
//! engine's command layer — the functions the app calls — validates the
//! result and saves it atomically. `batch` runs a list of operations against
//! one open project with one undo history; `mcp` serves the same operations
//! to an MCP client over stdio. `docs/cli.md` is the manual.

mod error;
mod mcp;
mod ops;
mod output;
mod select;
mod session;
mod values;

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;

use clap::{Args, Parser, Subcommand};
use serde_json::{json, Value};

use error::{CliError, CliResult};
use ops::audio::*;
use ops::look::*;
use ops::project::*;
use ops::render::*;
use ops::timeline::*;
use ops::{Ctx, Operation, Outcome};
use session::Session;

#[derive(Parser)]
#[command(
    name = "chukcut-cli",
    version,
    about = "Edit chukcut projects from the command line, or serve them to an AI agent over MCP.",
    long_about = "Edit chukcut projects from the command line, or serve them to an AI agent over MCP.\n\n\
        Every command takes the project file first, applies one edit through the same engine \
        commands the app uses, validates the result and saves it atomically. Clips are named by \
        id, a unique id prefix, or lane:index as `info` lists them. Times are seconds (2.5), \
        2.5s, 250ms, 1:02.5 or 45f.\n\nExit codes: 0 done, 1 refused by the engine, 2 usage, \
        3 project file, 4 validation, 5 render.",
    arg_required_else_help = true
)]
struct Cli {
    /// Print the result as JSON on stdout, and progress as JSON lines on stderr.
    #[arg(long, global = true)]
    json: bool,
    /// Run the command but do not save the project.
    #[arg(long, global = true)]
    dry_run: bool,
    /// Log engine diagnostics to stderr (-v, -vv). RUST_LOG overrides.
    #[arg(short, long, global = true, action = clap::ArgAction::Count)]
    verbose: u8,
    #[command(subcommand)]
    command: Command,
}

/// An operation and the project it applies to.
#[derive(Args)]
struct On<T: Args> {
    /// The project file (.chukcut).
    project: PathBuf,
    #[command(flatten)]
    args: T,
}

#[derive(Subcommand)]
enum Command {
    /// Create a new, empty project file.
    New(On<NewArgs>),
    /// Describe the project: lanes, clips (with the refs other commands take), media, issues.
    Info(On<InfoArgs>),
    /// Check the project for missing media and inconsistencies.
    Validate(On<ValidateArgs>),
    /// Change the project's name, canvas size, frame rate or background.
    Configure(On<ConfigureArgs>),
    /// Add media files to the project, optionally appending them to the timeline.
    Import(On<ImportArgs>),
    /// Put an imported material at the end of its lane.
    Append(On<AppendArgs>),
    /// Put an imported material on the timeline at a time.
    Place(On<PlaceArgs>),
    /// Cut a clip, or everything under a time, in two.
    Split(On<SplitArgs>),
    /// Take clips off the timeline, optionally closing the gap.
    Delete(On<DeleteArgs>),
    /// Move a clip to another time or lane.
    Move(On<MoveArgs>),
    /// Move a clip's edges, or give it a new length.
    Trim(On<TrimArgs>),
    /// Set a clip's position, scale, rotation, opacity, volume or speed.
    Set(On<ClipSetArgs>),
    /// Colour-grade a clip: controls, LUT, resets.
    Grade(On<GradeArgs>),
    /// Add, change or remove effects.
    #[command(subcommand)]
    Effect(EffectCommand),
    /// Give a clip an In, Out or Combo animation preset.
    Animate(On<AnimateArgs>),
    /// Animate a title's letters, words or lines.
    AnimateText(On<AnimateTextArgs>),
    /// Punch-in zoom on a clip, or auto zoom across jump cuts.
    Zoom(On<ZoomArgs>),
    /// Add, change or remove a keyframe on a clip property.
    Keyframe(On<KeyframeArgs>),
    /// Add or change titles.
    #[command(subcommand)]
    Title(TitleCommand),
    /// Transcribe, import, export and style captions.
    #[command(subcommand)]
    Captions(CaptionsCommand),
    /// Find and cut pauses.
    #[command(subcommand)]
    Silence(SilenceCommand),
    /// Bring clips' speech to a loudness.
    Normalize(On<NormalizeArgs>),
    /// Reduce background noise in a clip.
    Denoise(On<DenoiseArgs>),
    /// Measure loudness of a clip or the whole mix.
    Loudness(On<LoudnessArgs>),
    /// Track a region of a video and optionally make an overlay follow it.
    Track(On<TrackArgs>),
    /// Add or remove transitions.
    #[command(subcommand)]
    Transition(TransitionCommand),
    /// Render the timeline to a video file.
    Export(On<ExportArgs>),
    /// Render one frame as a PNG.
    RenderFrame(On<RenderFrameArgs>),
    /// Run a JSON list of operations against one project, with one undo history.
    Batch(BatchArgs),
    /// List effects, transitions, animations, grade controls, presets, encoders, models, LUTs or fonts.
    Catalog(CatalogArgs),
    /// Serve every operation to an MCP client over stdio.
    Mcp,
}

#[derive(Subcommand)]
enum EffectCommand {
    /// Add an effect to a clip, or as an effect clip over the lanes beneath.
    Add(On<EffectAddArgs>),
    /// Set an effect's parameters or switch it on or off.
    Set(On<EffectSetArgs>),
    /// Take an effect off a clip.
    Remove(On<EffectRemoveArgs>),
}

#[derive(Subcommand)]
enum TitleCommand {
    /// Put a title on the timeline.
    Add(On<TitleAddArgs>),
    /// Change a title's words or look.
    Set(On<TitleSetArgs>),
}

#[derive(Subcommand)]
enum CaptionsCommand {
    /// Transcribe the timeline's speech into captions.
    Transcribe(On<CaptionsTranscribeArgs>),
    /// Read an .srt or .vtt file onto the caption lane.
    Import(On<CaptionsImportArgs>),
    /// Write the captions as .srt or .vtt.
    Export(On<CaptionsExportArgs>),
    /// Restyle every caption, or one.
    Style(On<CaptionsStyleArgs>),
    /// List the captions.
    List(On<CaptionsListArgs>),
}

#[derive(Subcommand)]
enum SilenceCommand {
    /// List the pauses in a clip's sound.
    Detect(On<SilenceDetectArgs>),
    /// Cut the pauses out and close the gaps.
    Remove(On<SilenceRemoveArgs>),
}

#[derive(Subcommand)]
enum TransitionCommand {
    /// Put a transition on the cut at the start of a clip.
    Add(On<TransitionAddArgs>),
    /// Remove the transition at the start of a clip.
    Remove(On<TransitionRemoveArgs>),
}

#[derive(Args)]
struct BatchArgs {
    /// The project file. Created when the first operation is `new`.
    project: PathBuf,
    /// A JSON file of operations, or - for stdin.
    file: PathBuf,
}

/// Process-wide setup for a headless engine.
///
/// Not `chukcut_engine::init`: that clears the preview cache, which the app
/// may be using right now, and logs to stdout, which is the MCP channel.
fn init_engine(verbose: u8) {
    use chukcut_engine::modules::{audio, export, project, proxy, workspace};

    let default = match verbose {
        0 => "error",
        1 => "warn,chukcut_engine=info",
        _ => "info,chukcut_engine=debug",
    };
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(default));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .with_target(false)
        .try_init();

    // The engine sets libav to Error; the hardware encoder probes then print
    // a line for every encoder this machine lacks. A CLI's stderr is for its
    // own progress unless asked for more.
    chukcut_engine::modules::media::ensure_initialized();
    if verbose == 0 {
        ffmpeg_next::util::log::set_level(ffmpeg_next::util::log::Level::Fatal);
    }

    // The working copy is the app's crash recovery, shared per user; a CLI
    // run must not write it.
    project::autosave::disable_for_process();
    // Proxies are for the preview. A one-shot process would start encoding
    // them and exit halfway through.
    proxy::commands::proxy_set_policy(workspace::settings::ProxyPolicy::Off);
    // Without this the exporter mixes a silent audio track.
    export::job::register_audio_source(Arc::new(audio::FileAudioSource));
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    init_engine(cli.verbose);
    let printer = output::Printer::new(cli.json);

    if let Command::Mcp = cli.command {
        return match mcp::serve() {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("chukcut-cli mcp: {error}");
                ExitCode::from(1)
            }
        };
    }

    let ctx = printer.ctx();
    match dispatch(cli.command, cli.dry_run, &ctx) {
        Ok((name, outcome, saved)) => {
            printer.success(name, &outcome, saved);
            ExitCode::SUCCESS
        }
        Err(error) => {
            printer.failure(&error);
            ExitCode::from(error.kind.exit_code())
        }
    }
}

/// Open, run, save.
fn on<T: Args + Operation>(
    on: On<T>,
    dry_run: bool,
    ctx: &Ctx,
) -> CliResult<(&'static str, Outcome, bool)> {
    let mut session = Session::open(&on.project)?;
    let outcome = on.args.run(&mut session, ctx)?;
    let saved = outcome.mutated && !dry_run;
    if saved {
        session.save()?;
    }
    Ok((T::NAME, outcome, saved))
}

fn dispatch(command: Command, dry: bool, ctx: &Ctx) -> CliResult<(&'static str, Outcome, bool)> {
    match command {
        Command::New(o) => {
            let (mut session, outcome) = o.args.create(&o.project)?;
            if !dry {
                session.save()?;
            }
            Ok(("new", outcome, !dry))
        }
        Command::Info(o) => on(o, dry, ctx),
        Command::Validate(o) => on(o, dry, ctx),
        Command::Configure(o) => on(o, dry, ctx),
        Command::Import(o) => on(o, dry, ctx),
        Command::Append(o) => on(o, dry, ctx),
        Command::Place(o) => on(o, dry, ctx),
        Command::Split(o) => on(o, dry, ctx),
        Command::Delete(o) => on(o, dry, ctx),
        Command::Move(o) => on(o, dry, ctx),
        Command::Trim(o) => on(o, dry, ctx),
        Command::Set(o) => on(o, dry, ctx),
        Command::Grade(o) => on(o, dry, ctx),
        Command::Effect(EffectCommand::Add(o)) => on(o, dry, ctx),
        Command::Effect(EffectCommand::Set(o)) => on(o, dry, ctx),
        Command::Effect(EffectCommand::Remove(o)) => on(o, dry, ctx),
        Command::Animate(o) => on(o, dry, ctx),
        Command::AnimateText(o) => on(o, dry, ctx),
        Command::Zoom(o) => on(o, dry, ctx),
        Command::Keyframe(o) => on(o, dry, ctx),
        Command::Title(TitleCommand::Add(o)) => on(o, dry, ctx),
        Command::Title(TitleCommand::Set(o)) => on(o, dry, ctx),
        Command::Captions(CaptionsCommand::Transcribe(o)) => on(o, dry, ctx),
        Command::Captions(CaptionsCommand::Import(o)) => on(o, dry, ctx),
        Command::Captions(CaptionsCommand::Export(o)) => on(o, dry, ctx),
        Command::Captions(CaptionsCommand::Style(o)) => on(o, dry, ctx),
        Command::Captions(CaptionsCommand::List(o)) => on(o, dry, ctx),
        Command::Silence(SilenceCommand::Detect(o)) => on(o, dry, ctx),
        Command::Silence(SilenceCommand::Remove(o)) => on(o, dry, ctx),
        Command::Normalize(o) => on(o, dry, ctx),
        Command::Denoise(o) => on(o, dry, ctx),
        Command::Loudness(o) => on(o, dry, ctx),
        Command::Track(o) => on(o, dry, ctx),
        Command::Transition(TransitionCommand::Add(o)) => on(o, dry, ctx),
        Command::Transition(TransitionCommand::Remove(o)) => on(o, dry, ctx),
        Command::Export(o) => on(o, dry, ctx),
        Command::RenderFrame(o) => on(o, dry, ctx),
        Command::Catalog(args) => Ok(("catalog", args.run()?, false)),
        Command::Batch(args) => {
            let raw = read_input(&args.file)?;
            let ops = batch_ops(&raw)?;
            let (outcome, saved) = run_batch(&args.project, ops, dry, ctx)?;
            Ok(("batch", outcome, saved))
        }
        Command::Mcp => unreachable!("handled before dispatch"),
    }
}

fn read_input(file: &Path) -> CliResult<String> {
    if file == Path::new("-") {
        let mut raw = String::new();
        std::io::stdin()
            .read_to_string(&mut raw)
            .map_err(|e| CliError::usage(format!("cannot read stdin: {e}")))?;
        Ok(raw)
    } else {
        std::fs::read_to_string(file)
            .map_err(|e| CliError::usage(format!("cannot read {}: {e}", file.display())))
    }
}

/// A batch file is a JSON list of operations, or an object with an `ops`
/// list. Each operation is an object with an `op` name and its arguments.
pub fn batch_ops(raw: &str) -> CliResult<Vec<Value>> {
    let value: Value = serde_json::from_str(raw)
        .map_err(|e| CliError::usage(format!("the batch is not JSON: {e}")))?;
    let list = match value {
        Value::Array(list) => list,
        Value::Object(mut map) => match map.remove("ops") {
            Some(Value::Array(list)) => list,
            _ => return Err(CliError::usage("a batch object needs an \"ops\" list")),
        },
        _ => return Err(CliError::usage("a batch is a list of operations")),
    };
    Ok(list)
}

/// Run `ops` against the project at `path` as one session, all or nothing:
/// the project is saved once, at the end, and only when every operation
/// succeeded. A first operation `new` creates the project.
pub fn run_batch(path: &Path, ops: Vec<Value>, dry: bool, ctx: &Ctx) -> CliResult<(Outcome, bool)> {
    let mut ops = ops.into_iter().enumerate().peekable();
    let mut results = Vec::new();
    let mut mutated = false;
    let first_is_new = ops
        .peek()
        .is_some_and(|(_, op)| op.get("op").and_then(Value::as_str) == Some("new"));
    let mut session = if first_is_new {
        let (_, op) = ops.next().expect("peeked");
        let args: NewArgs = serde_json::from_value(strip_op(op))
            .map_err(|e| CliError::usage(format!("operation 0 (new): {e}")))?;
        let (session, outcome) = args.create(path)?;
        results.push(json!({"op": "new", "message": outcome.message, "data": outcome.data}));
        mutated = true;
        session
    } else {
        Session::open(path)?
    };
    for (index, op) in ops {
        let name = op
            .get("op")
            .and_then(Value::as_str)
            .ok_or_else(|| CliError::usage(format!("operation {index} has no \"op\" name")))?
            .to_string();
        let outcome = ops::run_named(&name, strip_op(op), &mut session, ctx)
            .map_err(|e| e.context(format!("operation {index} ({name})")))?;
        mutated |= outcome.mutated;
        results.push(json!({"op": name, "message": outcome.message, "data": outcome.data}));
    }
    let saved = mutated && !dry;
    if saved {
        session.save()?;
    }
    let count = results.len();
    Ok((
        Outcome {
            message: format!(
                "ran {count} operation(s){}",
                if saved { "; saved" } else { "" }
            ),
            data: json!(results),
            mutated,
        },
        saved,
    ))
}

fn strip_op(mut op: Value) -> Value {
    if let Some(map) = op.as_object_mut() {
        map.remove("op");
    }
    op
}
