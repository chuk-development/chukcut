//! What the CLI prints, and where.
//!
//! stdout carries the result and nothing else, so `--json` output can be
//! piped straight into `jq`. Progress and diagnostics go to stderr: a
//! rewritten line on a terminal, one line per step when stderr is a file or
//! a pipe, JSON lines with `--json`.

use std::io::{IsTerminal, Write};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::error::CliError;
use crate::ops::{Ctx, Outcome};

pub struct Printer {
    json: bool,
}

struct Throttle {
    label: String,
    fraction: f32,
    at: Instant,
}

impl Printer {
    pub fn new(json: bool) -> Self {
        Self { json }
    }

    /// Progress reporting for this output mode.
    pub fn ctx(&self) -> Ctx {
        let json = self.json;
        let terminal = std::io::stderr().is_terminal();
        let last = Mutex::new(Throttle {
            label: String::new(),
            fraction: -1.0,
            at: Instant::now() - Duration::from_secs(1),
        });
        Ctx {
            progress: Box::new(move |label: &str, fraction: Option<f32>| {
                let mut last = last.lock().unwrap_or_else(|e| e.into_inner());
                let f = fraction.unwrap_or(-1.0);
                // A new stage always prints; within a stage, at most four
                // updates a second and only when the number moved.
                let new_stage = label.split(' ').next() != last.label.split(' ').next();
                if !new_stage
                    && (last.at.elapsed() < Duration::from_millis(250)
                        || (f - last.fraction).abs() < 0.01)
                {
                    return;
                }
                last.label = label.to_string();
                last.fraction = f;
                last.at = Instant::now();
                let mut err = std::io::stderr().lock();
                if json {
                    let _ = writeln!(err, "{}", json!({"progress": fraction, "label": label}));
                } else if terminal {
                    let percent = fraction
                        .map(|f| format!("{:>3.0}% ", f * 100.0))
                        .unwrap_or_default();
                    let _ = write!(err, "\r\x1b[2K{percent}{label}");
                    let _ = err.flush();
                } else {
                    let percent = fraction
                        .map(|f| format!(" ({:.0}%)", f * 100.0))
                        .unwrap_or_default();
                    let _ = writeln!(err, "{label}{percent}");
                }
            }),
        }
    }

    fn end_progress_line(&self) {
        if !self.json && std::io::stderr().is_terminal() {
            let _ = write!(std::io::stderr(), "\r\x1b[2K");
        }
    }

    pub fn success(&self, op: &str, outcome: &Outcome, saved: bool) {
        self.end_progress_line();
        let mut out = std::io::stdout().lock();
        if self.json {
            let doc = json!({
                "ok": true,
                "op": op,
                "message": outcome.message,
                "saved": saved,
                "data": outcome.data,
            });
            let _ = writeln!(
                out,
                "{}",
                serde_json::to_string_pretty(&doc).unwrap_or_default()
            );
            return;
        }
        match op {
            "info" if outcome.data.get("tracks").is_some() => {
                let _ = writeln!(out, "{}", outcome.message);
                let _ = write!(out, "{}", info_text(&outcome.data));
            }
            "catalog" | "captions_list" | "batch" => {
                let _ = writeln!(out, "{}", outcome.message);
                if let Value::Array(items) = &outcome.data {
                    for item in items {
                        let _ = writeln!(out, "  {}", one_line(item));
                    }
                }
            }
            _ => {
                let _ = writeln!(out, "{}", outcome.message);
            }
        }
    }

    pub fn failure(&self, error: &CliError) {
        self.end_progress_line();
        if self.json {
            let doc = json!({
                "ok": false,
                "error": {"kind": error.kind, "code": error.kind.exit_code(), "message": error.message},
            });
            let _ = writeln!(
                std::io::stdout(),
                "{}",
                serde_json::to_string_pretty(&doc).unwrap_or_default()
            );
        }
        let _ = writeln!(std::io::stderr(), "error: {}", error.message);
    }
}

/// A catalog entry or a batch result, in one line.
fn one_line(item: &Value) -> String {
    for key in ["id", "name", "op"] {
        if let Some(name) = item.get(key).and_then(Value::as_str) {
            let detail = ["label", "message", "text"]
                .iter()
                .find_map(|k| item.get(*k).and_then(Value::as_str))
                .unwrap_or("");
            return if detail.is_empty() {
                name.to_string()
            } else {
                format!("{name:<28} {detail}")
            };
        }
    }
    item.to_string()
}

/// `info` for a person: one line per lane, one per clip.
fn info_text(data: &Value) -> String {
    let mut s = String::new();
    let empty = Vec::new();
    for track in data["tracks"].as_array().unwrap_or(&empty) {
        let flags: Vec<&str> = [
            ("muted", "muted"),
            ("locked", "locked"),
            ("hidden", "hidden"),
        ]
        .iter()
        .filter(|(k, _)| track[*k].as_bool() == Some(true))
        .map(|(_, v)| *v)
        .collect();
        s.push_str(&format!(
            "lane {} {:?} ({}){}\n",
            track["index"],
            track["name"].as_str().unwrap_or(""),
            track["kind"].as_str().unwrap_or(""),
            if flags.is_empty() {
                String::new()
            } else {
                format!(" [{}]", flags.join(", "))
            }
        ));
        for clip in track["clips"].as_array().unwrap_or(&empty) {
            let id = clip["id"].as_str().unwrap_or("");
            let source = match &clip["source"] {
                Value::String(s) => {
                    let name = std::path::Path::new(s)
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_else(|| s.clone());
                    let mut short: String = name.chars().take(40).collect();
                    if name.chars().count() > 40 {
                        short.push('…');
                    }
                    short
                }
                _ => String::new(),
            };
            let mut extras = Vec::new();
            if let Some(e) = clip["effects"].as_array() {
                extras.push(format!("{} effect(s)", e.len()));
            }
            if clip.get("grade").is_some() {
                extras.push("graded".into());
            }
            if clip.get("transition").is_some() {
                extras.push("transition".into());
            }
            if clip.get("animation").is_some() {
                extras.push("animated".into());
            }
            s.push_str(&format!(
                "  {:<6} {}  {:<7} {:>8.3} – {:<8.3} {}{}\n",
                clip["ref"].as_str().unwrap_or(""),
                &id[..id.len().min(8)],
                clip["kind"].as_str().unwrap_or(""),
                clip["start"].as_f64().unwrap_or(0.0),
                clip["end"].as_f64().unwrap_or(0.0),
                source,
                if extras.is_empty() {
                    String::new()
                } else {
                    format!("  [{}]", extras.join(", "))
                }
            ));
        }
    }
    let empty = Vec::new();
    for issue in data["issues"].as_array().unwrap_or(&empty) {
        s.push_str(&format!(
            "{}: {}\n",
            issue["severity"].as_str().unwrap_or(""),
            issue["message"].as_str().unwrap_or("")
        ));
    }
    s
}
