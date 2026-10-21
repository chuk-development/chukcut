//! Timeline interchange: hand a cut to another editor.
//!
//! chukcut writes the active timeline as
//!
//! - **OpenTimelineIO** JSON (`.otio`, [`otio`]) — DaVinci Resolve, Premiere
//!   (through its OTIO plug-in), Kdenlive, Avid's and Nuke Studio's adapters;
//! - **CMX 3600 EDL** (`.edl`, [`edl`]) — every editor and every online
//!   facility reads one; it holds one picture track and up to four sound
//!   tracks;
//! - **FCPXML 1.10** (`.fcpxml`, [`fcpxml`]) — Final Cut Pro and DaVinci
//!   Resolve.
//!
//! What goes across is the edit: which part of which file sits where, on
//! which track, with its speed, the dissolves at the cuts and the markers.
//! Looks, effects, titles, keyframes and masks are chukcut's own and stay
//! behind; every format writes what it could not carry into the
//! [`Report`], so the user hears about it instead of finding out in the
//! other program.
//!
//! The structure — a flattening pass into a format-neutral timeline
//! ([`timeline`]), one writer per format, and a report of what was left out —
//! follows FilmCraft's `interchange` crate (storytold/filmcraft,
//! Apache-2.0 OR MIT; NOTICE.md). The writers are chukcut's own, over its
//! document and its microsecond time.

pub mod commands;
pub mod edl;
pub mod fcpxml;
pub mod otio;
pub mod timeline;

use serde::{Deserialize, Serialize};

/// An interchange format chukcut can write.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Format {
    Otio,
    Edl,
    Fcpxml,
}

impl Format {
    pub const ALL: [Format; 3] = [Format::Otio, Format::Fcpxml, Format::Edl];

    pub fn extension(self) -> &'static str {
        match self {
            Format::Otio => "otio",
            Format::Edl => "edl",
            Format::Fcpxml => "fcpxml",
        }
    }

    /// What the export menu calls it.
    pub fn label(self) -> &'static str {
        match self {
            Format::Otio => "OpenTimelineIO (.otio)",
            Format::Edl => "CMX 3600 EDL (.edl)",
            Format::Fcpxml => "Final Cut Pro XML (.fcpxml)",
        }
    }

    /// The format a file name asks for, by its extension.
    pub fn from_path(path: &std::path::Path) -> Option<Format> {
        let ext = path.extension()?.to_str()?.to_ascii_lowercase();
        Format::ALL.into_iter().find(|f| f.extension() == ext)
    }
}

/// What an export could not carry, in sentences for the user. Each sentence
/// appears once, however many clips it applies to.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Report {
    pub warnings: Vec<String>,
}

impl Report {
    pub fn warn(&mut self, message: impl Into<String>) {
        let message = message.into();
        if !self.warnings.contains(&message) {
            self.warnings.push(message);
        }
    }
}

/// Percent-encode a path as a `file://` URL, the form OTIO and FCPXML name
/// media by. Unreserved characters and `/` stay as they are.
pub(crate) fn file_url(path: &str) -> String {
    let mut out = String::from("file://");
    for b in path.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_url_escapes_what_a_url_cannot_hold() {
        assert_eq!(file_url("/a b/ü#1.mp4"), "file:///a%20b/%C3%BC%231.mp4");
        assert_eq!(
            Format::from_path(std::path::Path::new("/x/Cut.FCPXML")),
            Some(Format::Fcpxml)
        );
    }
}
