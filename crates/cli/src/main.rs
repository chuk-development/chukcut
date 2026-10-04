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
use ops::analysis::*;
use ops::audio::*;
use ops::audiofx::*;
use ops::cloud::*;
use ops::delivery::*;
use ops::enhance::*;
use ops::frame::*;
use ops::layout::*;
use ops::look::*;
use ops::markers::*;
use ops::mask::*;
use ops::ml::*;
use ops::motion::*;
use ops::project::*;
use ops::render::*;
use ops::sequence::*;
use ops::template::*;
use ops::text::*;
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
    /// Add, change or remove a clip's shape masks.
    Mask(On<MaskArgs>),
    /// Key a colour out of a clip (green screen).
    ChromaKey(On<ChromaKeyArgs>),
    /// Remove a video clip's background: people or the main object (ML).
    RemoveBackground(On<RemoveBackgroundArgs>),
    /// Grade or effects only on the subject or only on the background, by
    /// the clip's matte.
    ApplyTo(On<ApplyToArgs>),
    /// Keep (or cut out) the object you point at, on every frame (ML).
    SelectObject(On<SelectObjectArgs>),
    /// Remove an object from every frame: clicked, boxed or painted (ML).
    RemoveObject(On<RemoveObjectArgs>),
    /// Make a clip's picture 2x or 4x larger and cleaner (ML).
    EnhanceQuality(On<EnhanceQualityArgs>),
    /// Set how a clip blends with the lanes beneath it, and its opacity.
    Blend(On<BlendArgs>),
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
    /// Add, change, list or remove audio effects (EQ, compressor, reverb, echo, pitch).
    #[command(subcommand)]
    AudioEffect(AudioEffectCommand),
    /// Give a clip's voice a preset: deep, chipmunk, robot, telephone, megaphone.
    Voice(On<VoiceArgs>),
    /// Whether a clip's pitch follows its speed ("Change audio pitch").
    AudioPitch(On<AudioPitchArgs>),
    /// Turn a music clip down wherever someone speaks on another lane.
    Duck(On<DuckArgs>),
    /// Record a voiceover from the default input onto a new audio lane.
    Record(On<RecordArgs>),
    /// Track a region of a video and optionally make an overlay follow it.
    Track(On<TrackArgs>),
    /// Attach, detach, bake, smooth or remove a clip's motion track.
    TrackSet(On<TrackSetArgs>),
    /// Add or remove transitions.
    #[command(subcommand)]
    Transition(TransitionCommand),
    /// Add, change, remove or list markers on the ruler.
    #[command(subcommand)]
    Marker(MarkerCommand),
    /// Crop a clip's picture.
    Crop(On<CropArgs>),
    /// Set one tone curve of a clip's grade from points.
    Curve(On<CurveArgs>),
    /// Hold the frame of a video clip at a time.
    Freeze(On<FreezeArgs>),
    /// Give a clip a speed ramp from a preset or points, or remove it.
    SpeedCurve(On<SpeedCurveArgs>),
    /// Frame blending for slow motion and speed ramps: none, blend or flow
    /// (optical flow, frames made by RIFE; baked before it returns).
    FrameBlend(On<FrameBlendArgs>),
    /// Smooth slow motion in one step: optical flow on, slowed to 0.5x
    /// (or --speed) unless it is slowed already, frames baked.
    SmoothSlowMo(On<SmoothSlowMoArgs>),
    /// Make an animated sticker loop or play once.
    StickerPlayback(On<StickerPlaybackArgs>),
    /// Picture in picture and split-screen layouts.
    #[command(subcommand)]
    Layout(LayoutCommand),
    /// Find scene changes in a video clip and cut there.
    #[command(subcommand)]
    Scenes(ScenesCommand),
    /// Stabilise a shaky video clip.
    #[command(subcommand)]
    Stabilise(StabiliseCommand),
    /// Find the beat of a clip's sound and cut to it.
    #[command(subcommand)]
    Beats(BeatsCommand),
    /// Follow the subject of clips for a new canvas shape.
    Reframe(On<ReframeArgs>),
    /// Show the scene changes, beats and stabilisation a clip carries.
    Analysis(On<AnalysisArgs>),
    /// Cloud features: caption translation, text to speech, stock media.
    #[command(subcommand)]
    Cloud(CloudCommand),
    /// Add, rename, delete, duplicate, list or open timelines.
    #[command(subcommand)]
    Timeline(TimelineCommand),
    /// Make, open, close or flatten compound clips.
    #[command(subcommand)]
    Compound(CompoundCommand),
    /// Render the timeline to a file: video, sound only or GIF.
    Export(On<ExportArgs>),
    /// List the export presets as they fit this project, with sizes and warnings.
    Presets(On<PresetsArgs>),
    /// Save or delete a preset of your own.
    #[command(subcommand)]
    Preset(PresetCommand),
    /// Say what an export would produce and how big it would be.
    Estimate(On<EstimateArgs>),
    /// Run several exports one after another: presets, ranges, other projects.
    ExportQueue(On<ExportQueueArgs>),
    /// Render one frame as a PNG.
    RenderFrame(On<RenderFrameArgs>),
    /// Project templates: list, make a project from one, save one, fill slots.
    #[command(subcommand)]
    Template(TemplateCommand),
    #[command(flatten)]
    Clip(ops::clip::ClipCommand),
    #[command(flatten)]
    Colour(ops::colour::ColourCommand),
    #[command(flatten)]
    Ai(ops::ai::AiCommand),
    #[command(flatten)]
    Library(ops::library::LibraryCommand),
    /// Run a JSON list of operations against one project, with one undo history.
    Batch(BatchArgs),
    /// List effects, audio effects, transitions, animations, grade controls, presets, encoders, models, LUTs or fonts.
    Catalog(CatalogArgs),
    /// Machine learning: models, runtime packs, status and speed of the ML worker.
    Ml(MlArgs),
    /// Serve every operation to an MCP client over stdio.
    Mcp,
}

#[derive(Subcommand)]
enum TimelineCommand {
    /// List the timelines and compound clips, and which is open.
    List(On<TimelineListArgs>),
    /// Add an empty timeline and open it.
    New(On<TimelineNewArgs>),
    /// Rename a timeline.
    Rename(On<TimelineRenameArgs>),
    /// Delete a timeline.
    Delete(On<TimelineDeleteArgs>),
    /// Copy a timeline into a new one.
    Duplicate(On<TimelineDuplicateArgs>),
    /// Open another timeline.
    Switch(On<TimelineSwitchArgs>),
}

#[derive(Subcommand)]
enum CompoundCommand {
    /// Move clips into a new compound clip in their place.
    Create(On<CompoundCreateArgs>),
    /// Open a compound clip; later commands edit its lanes.
    Open(On<CompoundOpenArgs>),
    /// Close the open compound clip (--all: every level).
    Close(On<CompoundCloseArgs>),
    /// Put a compound clip's clips back on the timeline.
    Flatten(On<CompoundFlattenArgs>),
}

#[derive(Subcommand)]
enum EffectCommand {
    /// Add an effect to a clip, or as an effect clip over the lanes beneath.
    Add(On<EffectAddArgs>),
    /// Set an effect's parameters or switch it on or off.
    Set(On<EffectSetArgs>),
    /// Take an effect off a clip.
    Remove(On<EffectRemoveArgs>),
    #[command(flatten)]
    More(ops::clip::EffectMoreCommand),
}

#[derive(Subcommand)]
enum AudioEffectCommand {
    /// Add an audio effect to a clip's sound.
    Add(On<AudioEffectAddArgs>),
    /// Set an audio effect's parameters, switch it, or move it in the stack.
    Set(On<AudioEffectSetArgs>),
    /// Take an audio effect off a clip.
    Remove(On<AudioEffectRemoveArgs>),
    /// List a clip's audio effects and settings.
    List(On<AudioEffectsArgs>),
}

#[derive(Subcommand)]
enum TitleCommand {
    /// Put a title on the timeline.
    Add(On<TitleAddArgs>),
    /// Change a title's words or look.
    Set(On<TitleSetArgs>),
    /// Add a title in a style, or restyle a title.
    Style(On<TitleStyleArgs>),
    /// Add a title from a template, or give a title a template.
    Template(On<TitleTemplateArgs>),
    /// Move a title to a cell of the 3 x 3 grid.
    Position(On<TitlePositionArgs>),
    /// Copy a title with its own words and style.
    Duplicate(On<TitleDuplicateArgs>),
    #[command(flatten)]
    More(ops::clip::TitleMoreCommand),
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
    #[command(flatten)]
    Edit(ops::caption_edit::CaptionsEditCommand),
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
    #[command(flatten)]
    More(ops::clip::TransitionMoreCommand),
}

#[derive(Subcommand)]
enum MarkerCommand {
    /// Put a marker on the ruler.
    Add(On<MarkerAddArgs>),
    /// Move, rename or recolour a marker.
    Set(On<MarkerSetArgs>),
    /// Remove markers.
    Remove(On<MarkerRemoveArgs>),
    /// List the markers in time order.
    List(On<MarkerListArgs>),
}

#[derive(Subcommand)]
enum LayoutCommand {
    /// Make a clip a picture in picture in a corner.
    Pip(On<LayoutPipArgs>),
    /// Arrange clips into a split screen.
    Split(On<LayoutSplitArgs>),
}

#[derive(Subcommand)]
enum ScenesCommand {
    /// Find the shot changes in a video clip and mark them.
    Detect(On<ScenesDetectArgs>),
    /// Split a clip at the scene changes found.
    Split(On<ScenesSplitArgs>),
    /// Forget a clip's scene changes.
    Clear(On<ScenesClearArgs>),
}

#[derive(Subcommand)]
enum StabiliseCommand {
    /// Measure the camera shake and stabilise the clip.
    Apply(On<StabiliseArgs>),
    /// Change strength or crop, or switch the stabilisation on or off.
    Set(On<StabiliseSetArgs>),
    /// Take the stabilisation away.
    Remove(On<StabiliseRemoveArgs>),
}

#[derive(Subcommand)]
enum BeatsCommand {
    /// Find the beats in a clip's sound.
    Detect(On<BeatsDetectArgs>),
    /// Forget a clip's beats.
    Clear(On<BeatsClearArgs>),
    /// Cut video clips on the beats.
    Cut(On<BeatsCutArgs>),
    /// Move cuts onto the nearest beat.
    Snap(On<BeatsSnapArgs>),
}

#[derive(Subcommand)]
enum CloudCommand {
    /// Translate the captions onto a new caption lane.
    Translate(On<TranslateCaptionsArgs>),
    /// Speak a text with a cloud voice and import it.
    Tts(On<TtsArgs>),
    /// List the kinds of stock an account has.
    StockKinds(On<StockKindsArgs>),
    /// Search a stock library.
    StockSearch(On<StockSearchArgs>),
    /// Download a stock result and import it.
    StockDownload(On<StockDownloadArgs>),
    #[command(flatten)]
    More(ops::cloud_tools::CloudMoreCommand),
}

// Parsed once per process; the size difference costs nothing.
#[allow(clippy::large_enum_variant)]
#[derive(Subcommand)]
enum PresetCommand {
    /// Save export settings as a preset of your own.
    Save(On<PresetSaveArgs>),
    /// Delete one of your own presets.
    Remove(On<PresetRemoveArgs>),
}

#[derive(Subcommand)]
enum TemplateCommand {
    /// List the templates: the built-ins, then your own.
    List(TemplateListArgs),
    /// Make a new project file from a template, your files filling its slots.
    Apply(On<TemplateApplyArgs>),
    /// Save a project as a template of your own.
    Save(On<TemplateSaveArgs>),
    /// Put a file into a slot or any video or photo clip.
    Replace(On<TemplateReplaceArgs>),
    /// List a project's slots.
    Slots(On<TemplateSlotsArgs>),
    /// Delete one of your own templates.
    Delete(TemplateDeleteArgs),
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
    // a line for every encoder this machine lacks — NVENC at *fatal* level,
    // hence Quiet rather than Fatal. A CLI's stderr is for its own progress
    // unless asked for more, and every failure reaches the user as the
    // engine's own error message anyway.
    chukcut_engine::modules::media::ensure_initialized();
    if verbose == 0 {
        ffmpeg_next::util::log::set_level(ffmpeg_next::util::log::Level::Quiet);
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
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(error) => return usage_error(error),
    };
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

/// clap's own errors, as JSON too when `--json` was asked for: a script that
/// reads stdout must get its answer there whatever went wrong.
fn usage_error(error: clap::Error) -> ExitCode {
    use clap::error::ErrorKind as K;
    if matches!(
        error.kind(),
        K::DisplayHelp | K::DisplayVersion | K::DisplayHelpOnMissingArgumentOrSubcommand
    ) {
        error.exit();
    }
    if std::env::args().any(|a| a == "--json") {
        let text = error.render().to_string();
        let message = text
            .lines()
            .next()
            .unwrap_or_default()
            .trim_start_matches("error: ")
            .to_string();
        output::Printer::new(true).failure(&CliError::usage(message));
        return ExitCode::from(error::ErrorKind::Usage.exit_code());
    }
    let _ = error.print();
    ExitCode::from(error::ErrorKind::Usage.exit_code())
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
        Command::Mask(o) => on(o, dry, ctx),
        Command::ChromaKey(o) => on(o, dry, ctx),
        Command::RemoveBackground(o) => on(o, dry, ctx),
        Command::ApplyTo(o) => on(o, dry, ctx),
        Command::SelectObject(o) => on(o, dry, ctx),
        Command::RemoveObject(o) => on(o, dry, ctx),
        Command::EnhanceQuality(o) => on(o, dry, ctx),
        Command::Blend(o) => on(o, dry, ctx),
        Command::Effect(EffectCommand::Add(o)) => on(o, dry, ctx),
        Command::Effect(EffectCommand::Set(o)) => on(o, dry, ctx),
        Command::Effect(EffectCommand::Remove(o)) => on(o, dry, ctx),
        Command::Animate(o) => on(o, dry, ctx),
        Command::AnimateText(o) => on(o, dry, ctx),
        Command::Zoom(o) => on(o, dry, ctx),
        Command::Keyframe(o) => on(o, dry, ctx),
        Command::Title(TitleCommand::Add(o)) => on(o, dry, ctx),
        Command::Title(TitleCommand::Set(o)) => on(o, dry, ctx),
        Command::Title(TitleCommand::Style(o)) => on(o, dry, ctx),
        Command::Title(TitleCommand::Template(o)) => on(o, dry, ctx),
        Command::Title(TitleCommand::Position(o)) => on(o, dry, ctx),
        Command::Title(TitleCommand::Duplicate(o)) => on(o, dry, ctx),
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
        Command::AudioEffect(AudioEffectCommand::Add(o)) => on(o, dry, ctx),
        Command::AudioEffect(AudioEffectCommand::Set(o)) => on(o, dry, ctx),
        Command::AudioEffect(AudioEffectCommand::Remove(o)) => on(o, dry, ctx),
        Command::AudioEffect(AudioEffectCommand::List(o)) => on(o, dry, ctx),
        Command::Voice(o) => on(o, dry, ctx),
        Command::AudioPitch(o) => on(o, dry, ctx),
        Command::Duck(o) => on(o, dry, ctx),
        Command::Record(o) => on(o, dry, ctx),
        Command::Track(o) => on(o, dry, ctx),
        Command::TrackSet(o) => on(o, dry, ctx),
        Command::Transition(TransitionCommand::Add(o)) => on(o, dry, ctx),
        Command::Transition(TransitionCommand::Remove(o)) => on(o, dry, ctx),
        Command::Marker(MarkerCommand::Add(o)) => on(o, dry, ctx),
        Command::Marker(MarkerCommand::Set(o)) => on(o, dry, ctx),
        Command::Marker(MarkerCommand::Remove(o)) => on(o, dry, ctx),
        Command::Marker(MarkerCommand::List(o)) => on(o, dry, ctx),
        Command::Crop(o) => on(o, dry, ctx),
        Command::Curve(o) => on(o, dry, ctx),
        Command::Freeze(o) => on(o, dry, ctx),
        Command::SpeedCurve(o) => on(o, dry, ctx),
        Command::FrameBlend(o) => on(o, dry, ctx),
        Command::SmoothSlowMo(o) => on(o, dry, ctx),
        Command::StickerPlayback(o) => on(o, dry, ctx),
        Command::Layout(LayoutCommand::Pip(o)) => on(o, dry, ctx),
        Command::Layout(LayoutCommand::Split(o)) => on(o, dry, ctx),
        Command::Scenes(ScenesCommand::Detect(o)) => on(o, dry, ctx),
        Command::Scenes(ScenesCommand::Split(o)) => on(o, dry, ctx),
        Command::Scenes(ScenesCommand::Clear(o)) => on(o, dry, ctx),
        Command::Stabilise(StabiliseCommand::Apply(o)) => on(o, dry, ctx),
        Command::Stabilise(StabiliseCommand::Set(o)) => on(o, dry, ctx),
        Command::Stabilise(StabiliseCommand::Remove(o)) => on(o, dry, ctx),
        Command::Beats(BeatsCommand::Detect(o)) => on(o, dry, ctx),
        Command::Beats(BeatsCommand::Clear(o)) => on(o, dry, ctx),
        Command::Beats(BeatsCommand::Cut(o)) => on(o, dry, ctx),
        Command::Beats(BeatsCommand::Snap(o)) => on(o, dry, ctx),
        Command::Reframe(o) => on(o, dry, ctx),
        Command::Analysis(o) => on(o, dry, ctx),
        Command::Cloud(CloudCommand::Translate(o)) => on(o, dry, ctx),
        Command::Cloud(CloudCommand::Tts(o)) => on(o, dry, ctx),
        Command::Cloud(CloudCommand::StockKinds(o)) => on(o, dry, ctx),
        Command::Cloud(CloudCommand::StockSearch(o)) => on(o, dry, ctx),
        Command::Cloud(CloudCommand::StockDownload(o)) => on(o, dry, ctx),
        Command::Timeline(TimelineCommand::List(o)) => on(o, dry, ctx),
        Command::Timeline(TimelineCommand::New(o)) => on(o, dry, ctx),
        Command::Timeline(TimelineCommand::Rename(o)) => on(o, dry, ctx),
        Command::Timeline(TimelineCommand::Delete(o)) => on(o, dry, ctx),
        Command::Timeline(TimelineCommand::Duplicate(o)) => on(o, dry, ctx),
        Command::Timeline(TimelineCommand::Switch(o)) => on(o, dry, ctx),
        Command::Compound(CompoundCommand::Create(o)) => on(o, dry, ctx),
        Command::Compound(CompoundCommand::Open(o)) => on(o, dry, ctx),
        Command::Compound(CompoundCommand::Close(o)) => on(o, dry, ctx),
        Command::Compound(CompoundCommand::Flatten(o)) => on(o, dry, ctx),
        Command::Export(o) => on(o, dry, ctx),
        Command::Presets(o) => on(o, dry, ctx),
        Command::Preset(PresetCommand::Save(o)) => on(o, dry, ctx),
        Command::Preset(PresetCommand::Remove(o)) => on(o, dry, ctx),
        Command::Estimate(o) => on(o, dry, ctx),
        Command::ExportQueue(o) => on(o, dry, ctx),
        Command::RenderFrame(o) => on(o, dry, ctx),
        Command::Template(TemplateCommand::List(args)) => Ok(("template_list", args.run()?, false)),
        Command::Template(TemplateCommand::Apply(o)) => {
            let (mut session, outcome) = o.args.create(&o.project)?;
            if !dry {
                session.save()?;
            }
            Ok(("template_apply", outcome, !dry))
        }
        Command::Template(TemplateCommand::Save(o)) => on(o, dry, ctx),
        Command::Template(TemplateCommand::Replace(o)) => on(o, dry, ctx),
        Command::Template(TemplateCommand::Slots(o)) => on(o, dry, ctx),
        Command::Template(TemplateCommand::Delete(args)) => {
            Ok(("template_delete", args.run()?, false))
        }
        Command::Clip(c) => c.dispatch(dry, ctx),
        Command::Colour(c) => c.dispatch(dry, ctx),
        Command::Ai(c) => c.dispatch(dry, ctx),
        Command::Library(c) => c.dispatch(dry, ctx),
        Command::Effect(EffectCommand::More(c)) => c.dispatch(dry, ctx),
        Command::Title(TitleCommand::More(c)) => c.dispatch(dry, ctx),
        Command::Captions(CaptionsCommand::Edit(c)) => c.dispatch(dry, ctx),
        Command::Transition(TransitionCommand::More(c)) => c.dispatch(dry, ctx),
        Command::Cloud(CloudCommand::More(c)) => c.dispatch(dry, ctx),
        Command::Catalog(args) => Ok(("catalog", args.run()?, false)),
        Command::Ml(args) => Ok(("ml", args.run()?, false)),
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
