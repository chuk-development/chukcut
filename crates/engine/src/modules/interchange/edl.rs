//! CMX 3600 edit decision lists (`.edl`).
//!
//! The layout follows FilmCraft's EDL writer (storytold/filmcraft,
//! `crates/interchange/src/edl.rs`, Apache-2.0 OR MIT; NOTICE.md): events
//! with the `* FROM CLIP NAME:` and `* SOURCE FILE:` comments that Resolve
//! and Premiere relink by, `M2` lines for speed changes, `D` dissolves, and
//! markers as `* LOC:` comments.
//!
//! An EDL has one picture channel and a few sound channels, so it carries
//! the main lane (the lowest picture track) and the first two sound tracks
//! as `A` and `A2`; the report says what else was left out. The record side
//! starts at 01:00:00:00, the default start of a timeline in every editor
//! that reads EDLs; sources without timecode count from 00:00:00:00. Every
//! reel is `AX`, the convention for file-based media, with the file named in
//! the comments.

use std::fmt::Write as _;

use super::timeline::{Clip, Rate, Timeline, Track};
use super::Report;
use crate::modules::project::Micros;

/// One hour, the record start.
const RECORD_START_SECONDS: i64 = 3_600;

struct Writer {
    rate: Rate,
    record_offset: i64,
    out: String,
    event: usize,
}

impl Writer {
    fn tc(&self, frame: i64) -> String {
        self.rate.timecode(frame)
    }

    fn rec(&self, t: Micros) -> String {
        self.tc(self.rate.frames(t) + self.record_offset)
    }

    fn src(&self, t: Micros) -> String {
        self.tc(self.rate.frames(t.max(0)))
    }

    /// One event line. `dur` is the transition length in frames, for `D`.
    #[allow(clippy::too_many_arguments)] // the columns of a CMX line
    fn line(
        &mut self,
        reel: &str,
        channel: &str,
        transition: &str,
        dur: Option<i64>,
        src: (Micros, Micros),
        rec: (Micros, Micros),
    ) {
        let dur = dur.map(|d| format!("{d:03}")).unwrap_or_default();
        let line = format!(
            "{:03}  {reel:<8} {channel:<5} {transition:<4} {dur:>3} {} {} {} {}",
            self.event,
            self.src(src.0),
            self.src(src.1),
            self.rec(rec.0),
            self.rec(rec.1),
        );
        let _ = writeln!(self.out, "{}", line.trim_end());
    }

    fn comment(&mut self, text: &str) {
        let _ = writeln!(self.out, "* {text}");
    }

    fn speed(&mut self, c: &Clip, entry: Micros) {
        if c.speed != 1.0 {
            let fps = c.speed as f64 * self.rate.timecode_base() as f64;
            let line = format!(
                "M2   {:<8}       {fps:05.1}                {}",
                "AX",
                self.src(entry)
            );
            let _ = writeln!(self.out, "{line}");
        }
    }

    fn names(&mut self, from: Option<&Clip>, to: &Clip) {
        if let Some(from) = from {
            self.comment(&format!("FROM CLIP NAME: {}", from.name));
            self.comment(&format!("TO CLIP NAME: {}", to.name));
            self.comment(&format!("SOURCE FILE: {}", from.path));
        } else {
            self.comment(&format!("FROM CLIP NAME: {}", to.name));
        }
        self.comment(&format!("SOURCE FILE: {}", to.path));
    }
}

/// An event of one track, before events of all tracks are put in record
/// order and numbered.
struct Event<'a> {
    start: Micros,
    channel: String,
    order: usize,
    clip: &'a Clip,
    /// The clip before it, when a dissolve comes from it (`None` with a
    /// dissolve is a dissolve from black).
    from: Option<&'a Clip>,
    dissolve: Option<Micros>,
    /// Where this clip's own event stops: earlier than its end when the next
    /// clip dissolves in from it.
    end: Micros,
}

fn events<'a>(track: &'a Track, channel: &str, order: usize, out: &mut Vec<Event<'a>>) {
    for (i, c) in track.clips.iter().enumerate() {
        let prev = i.checked_sub(1).map(|j| &track.clips[j]);
        let next = track.clips.get(i + 1);
        let next_half = next
            .filter(|n| n.record.start == c.record.end())
            .and_then(|n| n.transition_in)
            .map(|d| d / 2)
            .unwrap_or(0);
        let from = prev.filter(|p| p.record.end() == c.record.start);
        let dissolve = c.transition_in;
        out.push(Event {
            start: c.record.start - dissolve.map_or(0, |d| d / 2),
            channel: channel.to_string(),
            order,
            clip: c,
            from,
            dissolve,
            end: c.record.end() - next_half,
        });
    }
}

/// The timeline as a CMX 3600 EDL.
pub fn write(t: &Timeline, report: &mut Report) -> String {
    let rate = t.rate;
    let mut w = Writer {
        rate,
        record_offset: RECORD_START_SECONDS * rate.timecode_base(),
        out: String::new(),
        event: 0,
    };
    let _ = writeln!(w.out, "TITLE: {}", t.name);
    let _ = writeln!(
        w.out,
        "FCM: {}",
        if rate.drop_frame() {
            "DROP FRAME"
        } else {
            "NON-DROP FRAME"
        }
    );

    let mut all = Vec::new();
    let video: Vec<&Track> = t.video_tracks().collect();
    if let Some(main) = video.first() {
        events(main, "V", 0, &mut all);
    }
    if video.iter().skip(1).any(|t| !t.clips.is_empty()) {
        report.warn("an EDL holds one picture track: only the main lane is written (OTIO and FCPXML carry them all)");
    }
    let audio: Vec<&Track> = t.audio_tracks().collect();
    for (i, track) in audio.iter().take(2).enumerate() {
        events(track, if i == 0 { "A" } else { "A2" }, i + 1, &mut all);
    }
    if audio.iter().skip(2).any(|t| !t.clips.is_empty()) {
        report.warn("an EDL holds two sound tracks here: the third and later are not written");
    }
    all.sort_by_key(|e| (e.start, e.order));

    for e in &all {
        w.event += 1;
        let _ = writeln!(w.out);
        let c = e.clip;
        let src_at = |rec: Micros| {
            c.source_start + ((rec - c.record.start) as f64 * c.speed as f64).round() as Micros
        };
        match e.dissolve {
            Some(d) => {
                let cut = c.record.start;
                let begin = cut - d / 2;
                // The outgoing side, held at its frame at the dissolve's start.
                match e.from {
                    Some(f) => {
                        let s = f.source_start
                            + ((begin - f.record.start) as f64 * f.speed as f64).round() as Micros;
                        w.line("AX", &e.channel, "C", None, (s, s), (begin, begin));
                    }
                    None => w.line("BL", &e.channel, "C", None, (0, 0), (begin, begin)),
                }
                let entry = src_at(begin);
                w.line(
                    "AX",
                    &e.channel,
                    "D",
                    Some(rate.frames(d)),
                    (entry, src_at(e.end)),
                    (begin, e.end),
                );
                w.speed(c, entry);
                w.comment("EFFECT NAME: CROSS DISSOLVE");
                w.names(e.from, c);
            }
            None => {
                let entry = c.source_start;
                w.line(
                    "AX",
                    &e.channel,
                    "C",
                    None,
                    (entry, src_at(e.end)),
                    (c.record.start, e.end),
                );
                w.speed(c, entry);
                w.names(None, c);
            }
        }
    }
    // Markers after the last event, which is where most readers look for
    // them; Resolve imports `* LOC:` lines anywhere.
    for m in &t.markers {
        w.comment(&format!(
            "LOC: {} {:<7} {}",
            w.rec(m.time),
            m.color,
            m.label
        ));
    }
    if all
        .iter()
        .any(|e| e.clip.kind == super::timeline::ClipKind::Image)
    {
        report.warn("stills are written as events from their file; some editors want them as one-frame clips");
    }
    w.out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::interchange::timeline::{collect, tests::project};

    fn edl() -> (String, Report) {
        let mut report = Report::default();
        let t = collect(&project(), &mut report);
        let text = write(&t, &mut report);
        (text, report)
    }

    #[test]
    fn the_header_and_the_first_cut_are_where_cmx_wants_them() {
        let (text, report) = edl();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines[0], "TITLE: Trailer");
        assert_eq!(lines[1], "FCM: NON-DROP FRAME");
        // Shot A: 10 s into the file, recorded from 01:00:00:00 until the
        // dissolve begins half a second before the cut at 4 s.
        assert!(
            text.contains(
                "001  AX       V     C        00:00:10:00 00:00:13:13 01:00:00:00 01:00:03:13"
            ),
            "{text}"
        );
        assert!(report
            .warnings
            .iter()
            .any(|w| w.contains("one picture track")));
    }

    #[test]
    fn the_dissolve_holds_the_outgoing_frame_then_fades_in_the_next_shot() {
        let (text, _) = edl();
        // The music starts at 0 too and comes second in that frame.
        assert!(
            text.contains(
                "002  AX       A     C        00:00:00:00 00:00:08:00 01:00:00:00 01:00:08:00"
            ),
            "{text}"
        );
        assert!(
            text.contains(
                "003  AX       V     C        00:00:13:13 00:00:13:13 01:00:03:13 01:00:03:13"
            ),
            "{text}"
        );
        assert!(
            text.contains(
                "003  AX       V     D    025 00:00:01:13 00:00:06:00 01:00:03:13 01:00:08:00"
            ),
            "{text}"
        );
        assert!(text.contains("* FROM CLIP NAME: shot a\n* TO CLIP NAME: b"));
        assert!(text.contains("* SOURCE FILE: /m/b.mov"));
        assert!(text.contains("* LOC: 01:00:06:00 RED     drop"));
    }

    #[test]
    fn a_speed_change_gets_an_m2_line() {
        let mut report = Report::default();
        let mut t = collect(&project(), &mut report);
        t.tracks[0].clips[0].speed = 2.0;
        let text = write(&t, &mut report);
        let m2 = text
            .lines()
            .find(|l| l.starts_with("M2   AX "))
            .expect(&text);
        assert!(m2.ends_with("050.0                00:00:10:00"), "{m2}");
    }
}
