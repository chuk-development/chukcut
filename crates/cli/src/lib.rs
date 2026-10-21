//! `chukcut-cli` as a library: the operations, the MCP server, and the
//! control socket the app opens for it.
//!
//! The binary (`main.rs`) is the command line over these modules. The app
//! links the library for one reason: `chukcut --control` serves the same
//! operations over a local socket ([`control`]), so `chukcut-cli mcp
//! --bridge` can drive the window a person is looking at. That is the design
//! of FilmCraft's MCP bridge (storytold/filmcraft), taken on purpose; see
//! `docs/research/mcp-filmcraft-comparison.md`.

pub mod bridge;
pub mod commands;
pub mod control;
pub mod error;
pub mod inspect;
pub mod mcp;
pub mod ops;
pub mod output;
pub mod select;
pub mod session;
pub mod values;

use std::io::Read;
use std::path::{Path, PathBuf};

use clap::Args;
use serde_json::{json, Value};

use error::{CliError, CliResult};
use ops::project::NewArgs;
use ops::{Ctx, Operation, Outcome};
use session::Session;

/// An operation and the project it applies to.
#[derive(Args)]
pub struct On<T: Args> {
    /// The project file (.chukcut).
    pub project: PathBuf,
    #[command(flatten)]
    pub args: T,
}

/// Open, run, save.
pub fn on<T: Args + Operation>(
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

pub fn read_input(file: &Path) -> CliResult<String> {
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

pub fn strip_op(mut op: Value) -> Value {
    if let Some(map) = op.as_object_mut() {
        map.remove("op");
    }
    op
}
