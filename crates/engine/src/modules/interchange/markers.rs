// The CSV writer is adapted from FilmCraft (https://github.com/storytold/
// filmcraft), crates/engine/src/marker_export.rs: quoted fields, CRLF rows,
// and the formula guard on free text.
// Copyright (c) 2026 ArtCraft Team and the FilmCraft contributors.
// Licensed under the Apache License, Version 2.0 or the MIT License.
// The chapters writer and the changes for chukcut are under the GPL-3.0.
// See NOTICE.md.

//! Markers out of chukcut: a spreadsheet of review notes, or the chapter
//! list a YouTube description turns into chapters.

use serde::{Deserialize, Serialize};

use super::timeline::{Marker, Rate};
use super::Report;
use crate::modules::project::Micros;

/// What a marker list is written as.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MarkerFormat {
    /// A spreadsheet: one row per marker.
    Csv,
    /// `0:00 Intro` lines, which YouTube reads from a video's description.
    Chapters,
}

impl MarkerFormat {
    pub fn extension(self) -> &'static str {
        match self {
            MarkerFormat::Csv => "csv",
            MarkerFormat::Chapters => "txt",
        }
    }
}

/// One CSV field: quoted, quotes doubled, and free text that a spreadsheet
/// would run as a formula (`=`, `+`, `-`, `@`) given a leading `'`.
fn field(out: &mut String, text: &str, free_text: bool) {
    out.push('"');
    if free_text && text.trim_start().starts_with(['=', '+', '-', '@']) {
        out.push('\'');
    }
    for c in text.chars() {
        if c == '"' {
            out.push('"');
        }
        out.push(c);
    }
    out.push('"');
}

/// The markers as CSV: name, timecode, seconds, colour, timeline.
pub fn csv(timeline: &str, markers: &[Marker], rate: Rate) -> String {
    let mut out = String::from("\"Timeline\",\"Name\",\"Timecode\",\"Seconds\",\"Color\"\r\n");
    for m in markers {
        field(&mut out, timeline, true);
        out.push(',');
        field(&mut out, &m.label, true);
        out.push(',');
        field(&mut out, &rate.timecode(rate.frames(m.time)), false);
        out.push(',');
        field(&mut out, &format!("{:.3}", m.time as f64 / 1e6), false);
        out.push(',');
        field(&mut out, &m.color.to_ascii_lowercase(), false);
        out.push_str("\r\n");
    }
    out
}

/// YouTube's rules for a chapter list: it starts at 0:00, has at least
/// three chapters, and each lasts at least ten seconds.
const MIN_CHAPTERS: usize = 3;
const MIN_CHAPTER: Micros = 10_000_000;

fn clock(t: Micros, hours: bool) -> String {
    let s = t.max(0) / 1_000_000;
    if hours {
        format!("{}:{:02}:{:02}", s / 3_600, (s / 60) % 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

/// The markers as a chapter list for a video description. A chapter at
/// 0:00 named "Intro" is added when no marker sits in the first second;
/// unnamed markers are called "Chapter N". What breaks YouTube's rules goes
/// into `report`; the list is written anyway, so the user can fix it there.
pub fn chapters(markers: &[Marker], duration: Micros, report: &mut Report) -> String {
    let mut list: Vec<(Micros, String)> = markers
        .iter()
        .enumerate()
        .map(|(i, m)| {
            let name = m.label.trim();
            let name = if name.is_empty() {
                format!("Chapter {}", i + 1)
            } else {
                name.to_string()
            };
            (m.time, name)
        })
        .collect();
    // Whole seconds are what a chapter line can say; a marker at 0.4 s is
    // the 0:00 chapter.
    if list.first().is_none_or(|(t, _)| *t >= 1_000_000) {
        list.insert(0, (0, "Intro".to_string()));
    } else {
        list[0].0 = 0;
    }
    if list.len() < MIN_CHAPTERS {
        report.warn(format!(
            "YouTube shows chapters only from {MIN_CHAPTERS} on; this list has {}",
            list.len()
        ));
    }
    for (i, (t, name)) in list.iter().enumerate() {
        let end = list.get(i + 1).map_or(duration, |(n, _)| *n);
        if end - t < MIN_CHAPTER {
            report.warn(format!(
                "\"{name}\" is shorter than 10 s; YouTube needs every chapter to last 10 s"
            ));
        }
    }
    let hours = duration >= 3_600_000_000;
    let mut out = String::new();
    for (t, name) in list {
        out.push_str(&format!("{} {name}\n", clock(t, hours)));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn marker(t: Micros, label: &str) -> Marker {
        Marker {
            time: t,
            label: label.into(),
            color: "RED",
        }
    }

    #[test]
    fn a_csv_row_is_quoted_and_a_formula_is_defused() {
        let text = csv(
            "Trip",
            &[marker(61_500_000, "=HYPERLINK(\"x\")"), marker(0, "start")],
            Rate::new(25.0),
        );
        let lines: Vec<&str> = text.split("\r\n").collect();
        assert_eq!(
            lines[0],
            "\"Timeline\",\"Name\",\"Timecode\",\"Seconds\",\"Color\""
        );
        assert_eq!(
            lines[1],
            "\"Trip\",\"'=HYPERLINK(\"\"x\"\")\",\"00:01:01:13\",\"61.500\",\"red\""
        );
        assert_eq!(
            lines[2],
            "\"Trip\",\"start\",\"00:00:00:00\",\"0.000\",\"red\""
        );
    }

    #[test]
    fn chapters_start_at_zero_and_say_what_youtube_will_refuse() {
        let mut report = Report::default();
        let text = chapters(
            &[marker(30_000_000, "Setup"), marker(95_000_000, "")],
            300_000_000,
            &mut report,
        );
        assert_eq!(text, "0:00 Intro\n0:30 Setup\n1:35 Chapter 2\n");
        assert!(report.warnings.is_empty(), "{:?}", report.warnings);

        let mut report = Report::default();
        let text = chapters(
            &[marker(400_000, "Cold open"), marker(5_000_000, "Hook")],
            3_700_000_000,
            &mut report,
        );
        assert_eq!(text, "0:00:00 Cold open\n0:00:05 Hook\n");
        assert_eq!(report.warnings.len(), 2, "{:?}", report.warnings);
        assert!(report.warnings[0].contains("from 3 on"));
        assert!(report.warnings[1].contains("\"Cold open\" is shorter"));
    }
}
