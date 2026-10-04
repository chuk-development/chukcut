//! Every operation the CLI, `batch` and the MCP server can perform.
//!
//! One argument struct per operation, and that struct is the whole contract:
//! clap parses it from the command line, serde from a batch file or an MCP
//! tool call, and schemars derives the MCP tool's input schema from it. Each
//! struct's `run` builds its edit through the engine's command layer — the
//! same functions the app calls — against one [`Session`], so one invocation
//! has one undo history.

pub mod analysis;
pub mod audio;
pub mod audiofx;
pub mod caption_edit;
pub mod clip;
pub mod cloud;
pub mod cloud_tools;
pub mod delivery;
pub mod frame;
pub mod layout;
pub mod library;
pub mod look;
pub mod markers;
pub mod mask;
pub mod ml;
pub mod motion;
pub mod project;
pub mod render;
pub mod sequence;
pub mod summary;
pub mod template;
pub mod text;
pub mod timeline;

use serde::de::DeserializeOwned;
use serde_json::{json, Value};

use crate::error::{CliError, CliResult};
use crate::session::Session;

/// What an operation did.
pub struct Outcome {
    /// One line for a person.
    pub message: String,
    /// Everything a script needs: ids that were made, values that were set.
    pub data: Value,
    /// Whether the document changed, so the caller saves it.
    pub mutated: bool,
}

impl Outcome {
    pub fn changed(message: impl Into<String>, data: Value) -> Self {
        Self {
            message: message.into(),
            data,
            mutated: true,
        }
    }

    pub fn read(message: impl Into<String>, data: Value) -> Self {
        Self {
            message: message.into(),
            data,
            mutated: false,
        }
    }
}

/// Where a progress report goes: a label and, when the stage has a length,
/// how far through it is, `0..1`.
pub type ProgressFn = Box<dyn Fn(&str, Option<f32>) + Send + Sync>;

/// How a long operation reports where it is.
pub struct Ctx {
    pub progress: ProgressFn,
}

impl Ctx {
    pub fn quiet() -> Self {
        Self {
            progress: Box::new(|_, _| {}),
        }
    }

    pub fn progress(&self, label: &str, fraction: Option<f32>) {
        (self.progress)(label, fraction)
    }
}

/// An operation on an open project.
pub trait Operation: DeserializeOwned + schemars::JsonSchema {
    /// The batch `op` and the MCP tool name.
    const NAME: &'static str;
    fn run(self, session: &mut Session, ctx: &Ctx) -> CliResult<Outcome>;
}

/// One MCP tool.
pub struct ToolSpec {
    pub name: &'static str,
    pub description: String,
    pub schema: Value,
}

fn schema_of<T: schemars::JsonSchema>() -> (String, Value) {
    let schema = schemars::schema_for!(T);
    let mut value = serde_json::to_value(schema).unwrap_or_else(|_| json!({"type": "object"}));
    let description = value
        .get("description")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    if let Some(map) = value.as_object_mut() {
        map.remove("$schema");
        map.remove("title");
        map.remove("description");
        map.entry("type").or_insert(json!("object"));
        map.entry("properties").or_insert(json!({}));
    }
    (description, value)
}

/// Describe `T` as a tool. Its doc comment is the description; its fields
/// are the schema.
pub fn tool<T: Operation>() -> ToolSpec {
    tool_spec::<T>(T::NAME)
}

/// Describe any argument struct as a tool called `name`.
pub fn tool_spec<T: schemars::JsonSchema>(name: &'static str) -> ToolSpec {
    let (description, schema) = schema_of::<T>();
    ToolSpec {
        name,
        description,
        schema,
    }
}

fn parse<T: DeserializeOwned>(name: &str, args: Value) -> CliResult<T> {
    let args = match args {
        Value::Null => json!({}),
        other => other,
    };
    serde_json::from_value(args).map_err(|e| CliError::usage(format!("{name}: {e}")))
}

macro_rules! operations {
    ($($ty:ty),* $(,)?) => {
        /// Run the operation called `name` with JSON `args`.
        pub fn run_named(name: &str, args: Value, session: &mut Session, ctx: &Ctx) -> CliResult<Outcome> {
            $(
                if name == <$ty as Operation>::NAME {
                    return parse::<$ty>(name, args)?.run(session, ctx);
                }
            )*
            Err(CliError::usage(format!(
                "there is no operation called {name:?}; the operations are: {}",
                names().join(", ")
            )))
        }

        /// The name of every operation on a project.
        pub fn names() -> Vec<&'static str> {
            vec![$(<$ty as Operation>::NAME),*]
        }

        /// Every operation on a project, as an MCP tool.
        pub fn tools() -> Vec<ToolSpec> {
            vec![$(tool::<$ty>()),*]
        }
    };
}

operations!(
    project::InfoArgs,
    project::ValidateArgs,
    project::ConfigureArgs,
    project::ImportArgs,
    project::UndoArgs,
    project::RedoArgs,
    delivery::PresetsArgs,
    delivery::PresetSaveArgs,
    delivery::PresetRemoveArgs,
    delivery::EstimateArgs,
    delivery::ExportQueueArgs,
    timeline::AppendArgs,
    timeline::PlaceArgs,
    timeline::SplitArgs,
    timeline::DeleteArgs,
    timeline::MoveArgs,
    timeline::TrimArgs,
    timeline::ClipSetArgs,
    look::GradeArgs,
    look::EffectAddArgs,
    look::EffectSetArgs,
    look::EffectRemoveArgs,
    look::AnimateArgs,
    look::AnimateTextArgs,
    look::ZoomArgs,
    look::KeyframeArgs,
    look::TitleAddArgs,
    look::TitleSetArgs,
    look::TransitionAddArgs,
    look::TransitionRemoveArgs,
    look::TrackArgs,
    look::TrackSetArgs,
    mask::MaskArgs,
    mask::ChromaKeyArgs,
    mask::BlendArgs,
    mask::RemoveBackgroundArgs,
    mask::SelectObjectArgs,
    audio::CaptionsTranscribeArgs,
    audio::CaptionsImportArgs,
    audio::CaptionsExportArgs,
    audio::CaptionsStyleArgs,
    audio::CaptionsListArgs,
    audio::SilenceDetectArgs,
    audio::SilenceRemoveArgs,
    audio::NormalizeArgs,
    audio::DenoiseArgs,
    audio::LoudnessArgs,
    audiofx::AudioEffectAddArgs,
    audiofx::AudioEffectSetArgs,
    audiofx::AudioEffectRemoveArgs,
    audiofx::AudioEffectsArgs,
    audiofx::VoiceArgs,
    audiofx::AudioPitchArgs,
    audiofx::DuckArgs,
    audiofx::RecordArgs,
    markers::MarkerAddArgs,
    markers::MarkerSetArgs,
    markers::MarkerRemoveArgs,
    markers::MarkerListArgs,
    frame::CropArgs,
    frame::CurveArgs,
    frame::FreezeArgs,
    frame::SpeedCurveArgs,
    layout::LayoutPipArgs,
    layout::LayoutSplitArgs,
    text::TitleStyleArgs,
    text::TitleTemplateArgs,
    text::TitlePositionArgs,
    text::TitleDuplicateArgs,
    analysis::ScenesDetectArgs,
    analysis::ScenesSplitArgs,
    analysis::ScenesClearArgs,
    analysis::StabiliseArgs,
    analysis::StabiliseSetArgs,
    analysis::StabiliseRemoveArgs,
    analysis::BeatsDetectArgs,
    analysis::BeatsClearArgs,
    analysis::BeatsCutArgs,
    analysis::BeatsSnapArgs,
    analysis::ReframeArgs,
    analysis::AnalysisArgs,
    cloud::TranslateCaptionsArgs,
    cloud::TtsArgs,
    cloud::StockKindsArgs,
    cloud::StockSearchArgs,
    cloud::StockDownloadArgs,
    render::ExportArgs,
    render::RenderFrameArgs,
    template::TemplateSaveArgs,
    template::TemplateReplaceArgs,
    template::TemplateSlotsArgs,
    caption_edit::CaptionsAddArgs,
    caption_edit::CaptionsTextArgs,
    caption_edit::CaptionsSplitArgs,
    caption_edit::CaptionsMergeArgs,
    caption_edit::CaptionsClearArgs,
    caption_edit::CaptionsRegroupArgs,
    clip::RenameArgs,
    clip::LinkArgs,
    clip::UnlinkArgs,
    clip::PasteAttributesArgs,
    clip::GradeToAllArgs,
    clip::LookArgs,
    clip::EffectMoveArgs,
    clip::EffectResetArgs,
    clip::EffectKeyframeArgs,
    clip::TransitionSetArgs,
    clip::TitleTextArgs,
    clip::TitleFontArgs,
    clip::MaskMoveArgs,
    clip::LaneAddArgs,
    library::StickerArgs,
    motion::FrameBlendArgs,
    motion::StickerPlaybackArgs,
    library::MusicArgs,
    library::SfxArgs,
    cloud_tools::SoundArgs,
    cloud_tools::FalArgs,
    cloud_tools::CreditsArgs,
    sequence::TimelineListArgs,
    sequence::TimelineNewArgs,
    sequence::TimelineRenameArgs,
    sequence::TimelineDeleteArgs,
    sequence::TimelineDuplicateArgs,
    sequence::TimelineSwitchArgs,
    sequence::CompoundCreateArgs,
    sequence::CompoundOpenArgs,
    sequence::CompoundCloseArgs,
    sequence::CompoundFlattenArgs,
);

/// Parse a snake_case engine enum from its name, listing the valid names
/// when it is not one.
pub fn enum_named<T: DeserializeOwned>(what: &str, name: &str, valid: &[&str]) -> CliResult<T> {
    let normalized = name.trim().to_ascii_lowercase().replace(['-', ' '], "_");
    serde_json::from_value(Value::String(normalized)).map_err(|_| {
        CliError::usage(format!(
            "{name:?} is not a {what}; choose one of: {}",
            valid.join(", ")
        ))
    })
}

/// Deserialize a clap-style list of `name=value` pairs from JSON written as
/// an object, a list of strings, or one string.
pub fn assignments<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<Vec<(String, String)>, D::Error> {
    use serde::Deserialize;
    crate::values::Assignments::deserialize(d).map(|a| a.0)
}
