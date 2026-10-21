//! FCPXML 1.10 (`.fcpxml`), for Final Cut Pro and DaVinci Resolve.
//!
//! The main lane is the primary storyline (the `spine`), with gaps where it
//! is empty and dissolves at its cuts; every other picture track becomes
//! connected clips on lanes 1, 2, …, and the sound tracks lanes −1, −2, ….
//! A connected clip hangs off the spine item it starts over, its `offset`
//! counted in that item's own time, which is what FCPXML asks for.
//!
//! Picture clips play only their picture (`srcEnable="video"`): chukcut keeps
//! a clip's sound as a linked clip on a sound track, and that one is written
//! too. Speed is a `timeMap` from the clip's local time to the file's.
//!
//! Which elements to write for which part of an edit follows FilmCraft's
//! FCPXML writer (storytold/filmcraft, `crates/interchange/src/fcpxml.rs`,
//! Apache-2.0 OR MIT; NOTICE.md).

use std::collections::HashMap;
use std::fmt::Write as _;

use super::timeline::{Clip, ClipKind, Kind, Rate, Timeline};
use super::{file_url, Report};
use crate::modules::project::Micros;

/// Final Cut's Cross Dissolve, by the id every FCPXML names it with.
const CROSS_DISSOLVE_UID: &str = "FxPlug:4731E73A-8DAC-4113-9A30-AE85B1761265";

fn esc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            c => out.push(c),
        }
    }
    out
}

fn gcd(a: i64, b: i64) -> i64 {
    if b == 0 {
        a.abs().max(1)
    } else {
        gcd(b, a % b)
    }
}

/// `frames` as an FCPXML time: a reduced fraction of a second.
fn time_frames(frames: i64, rate: Rate) -> String {
    let (num, den) = rate.frame_duration();
    let n = frames * num;
    let g = gcd(n, den);
    let (n, d) = (n / g, den / g);
    if d == 1 {
        format!("{n}s")
    } else {
        format!("{n}/{d}s")
    }
}

fn time(t: Micros, rate: Rate) -> String {
    time_frames(rate.frames(t), rate)
}

/// One item of the primary storyline.
struct SpineItem<'a> {
    start: Micros,
    end: Micros,
    clip: Option<&'a Clip>,
}

struct Writer<'a> {
    rate: Rate,
    assets: HashMap<&'a str, String>,
    resources: String,
    next_id: usize,
}

impl<'a> Writer<'a> {
    fn id(&mut self) -> String {
        self.next_id += 1;
        format!("r{}", self.next_id)
    }

    /// The asset of a file, declared the first time a clip uses it.
    fn asset(&mut self, c: &'a Clip, format: &str) -> String {
        if let Some(id) = self.assets.get(c.path.as_str()) {
            return id.clone();
        }
        let id = self.id();
        let duration = c
            .media_duration
            .map_or("0s".to_string(), |d| time(d, self.rate));
        let has_video = c.kind != ClipKind::Audio;
        let has_audio = c.has_audio;
        let _ = writeln!(
            self.resources,
            "    <asset id=\"{id}\" name=\"{}\" start=\"0s\" duration=\"{duration}\" hasVideo=\"{}\" hasAudio=\"{}\"{}{}>",
            esc(&c.name),
            u8::from(has_video),
            u8::from(has_audio),
            if has_video { format!(" format=\"{format}\"") } else { String::new() },
            if has_audio { " audioSources=\"1\" audioChannels=\"2\" audioRate=\"48000\"" } else { "" },
        );
        let _ = writeln!(
            self.resources,
            "      <media-rep kind=\"original-media\" src=\"{}\"/>\n    </asset>",
            esc(&file_url(&c.path))
        );
        self.assets.insert(&c.path, id.clone());
        id
    }

    /// A clip element. `offset` is in the parent's time; `lane` is `None` on
    /// the spine.
    fn clip(
        &mut self,
        c: &'a Clip,
        format: &str,
        offset: Micros,
        lane: Option<i32>,
        audio_lane: bool,
    ) -> String {
        let asset = self.asset(c, format);
        let rate = self.rate;
        let lane_attr = lane.map(|l| format!(" lane=\"{l}\"")).unwrap_or_default();
        let enabled = if c.enabled { "" } else { " enabled=\"0\"" };
        let duration = time(c.record.duration, rate);
        if c.kind == ClipKind::Image {
            return format!(
                "<video ref=\"{asset}\"{lane_attr} offset=\"{}\" name=\"{}\" start=\"0s\" duration=\"{duration}\"{enabled}/>",
                time(offset, rate),
                esc(&c.name)
            );
        }
        let src_enable = match (c.kind, audio_lane) {
            (ClipKind::Video, false) if c.has_audio => " srcEnable=\"video\"",
            (ClipKind::Video, true) => " srcEnable=\"audio\"",
            _ => "",
        };
        if c.speed == 1.0 {
            return format!(
                "<asset-clip ref=\"{asset}\"{lane_attr} offset=\"{}\" name=\"{}\" start=\"{}\" duration=\"{duration}\"{src_enable}{enabled}/>",
                time(offset, rate),
                esc(&c.name),
                time(c.source_start, rate),
            );
        }
        // The clip's own time runs `speed` times slower than the file's, so
        // local L shows source L × speed: the map is a straight line through
        // the origin, and the clip starts at the local time of its first
        // source frame.
        let speed = c.speed as f64;
        let media = c
            .media_duration
            .unwrap_or(c.source_start + c.source_duration());
        let local = |source: Micros| (source as f64 / speed).round() as Micros;
        format!(
            "<asset-clip ref=\"{asset}\"{lane_attr} offset=\"{}\" name=\"{}\" start=\"{}\" duration=\"{duration}\"{src_enable}{enabled}>\n\
             <timeMap><timept time=\"0s\" value=\"0s\" interp=\"linear\"/><timept time=\"{}\" value=\"{}\" interp=\"linear\"/></timeMap>\n\
             </asset-clip>",
            time(offset, rate),
            esc(&c.name),
            time(local(c.source_start), rate),
            time(local(media), rate),
            time(media, rate),
        )
    }
}

/// The timeline as an FCPXML 1.10 document.
pub fn write(t: &Timeline, report: &mut Report) -> String {
    let rate = t.rate;
    let mut w = Writer {
        rate,
        assets: HashMap::new(),
        resources: String::new(),
        next_id: 0,
    };
    let format = w.id();
    let _ = writeln!(
        w.resources,
        "    <format id=\"{format}\" name=\"FFVideoFormatRateUndefined\" frameDuration=\"{}\" width=\"{}\" height=\"{}\"/>",
        time_frames(1, rate),
        t.width,
        t.height
    );
    let dissolve = w.id();
    let _ = writeln!(
        w.resources,
        "    <effect id=\"{dissolve}\" name=\"Cross Dissolve\" uid=\"{CROSS_DISSOLVE_UID}\"/>"
    );

    // The spine covers the whole timeline: clips of the main lane and gaps.
    let videos: Vec<_> = t.video_tracks().collect();
    let mut spine: Vec<SpineItem> = Vec::new();
    let mut at = 0;
    if let Some(main) = videos.first() {
        for c in &main.clips {
            if c.record.start > at {
                spine.push(SpineItem {
                    start: at,
                    end: c.record.start,
                    clip: None,
                });
            }
            spine.push(SpineItem {
                start: c.record.start,
                end: c.record.end(),
                clip: Some(c),
            });
            at = c.record.end();
        }
    }
    if t.duration > at {
        spine.push(SpineItem {
            start: at,
            end: t.duration,
            clip: None,
        });
    }

    // Connected clips, by the spine item they start over.
    let mut connected: Vec<Vec<String>> = vec![Vec::new(); spine.len()];
    let mut lanes: Vec<(i32, &super::timeline::Track)> = Vec::new();
    for (i, track) in videos.iter().enumerate().skip(1) {
        lanes.push((i as i32, track));
    }
    for (i, track) in t.audio_tracks().enumerate() {
        lanes.push((-(i as i32) - 1, track));
    }
    for (lane, track) in lanes {
        for c in &track.clips {
            if c.transition_in.is_some() {
                report.warn("FCPXML carries the transitions of the main lane only");
            }
            let Some(parent) = spine
                .iter()
                .position(|s| c.record.start >= s.start && c.record.start < s.end)
            else {
                continue;
            };
            let item = &spine[parent];
            let parent_local = item.clip.map_or(0, |p| p.source_start_local());
            let offset = parent_local + (c.record.start - item.start);
            let audio_lane = track.kind == Kind::Audio;
            let element = w.clip(c, &format, offset, Some(lane), audio_lane);
            connected[parent].push(element);
        }
    }

    let mut body = String::new();
    for (i, item) in spine.iter().enumerate() {
        if let Some(c) = item.clip {
            if let Some(d) = c.transition_in {
                if i > 0 && spine[i - 1].end == item.start && spine[i - 1].clip.is_some() {
                    let _ = writeln!(
                        body,
                        "          <transition name=\"Cross Dissolve\" offset=\"{}\" duration=\"{}\">\n            <filter-video ref=\"{dissolve}\" name=\"Cross Dissolve\"/>\n          </transition>",
                        time(item.start - d / 2, rate),
                        time(d, rate)
                    );
                } else {
                    report.warn("a dissolve from black on the main lane is written as a cut");
                }
            }
        }
        let children = std::mem::take(&mut connected[i]);
        let open = match item.clip {
            Some(c) => w.clip(c, &format, item.start, None, false),
            None => format!(
                "<gap name=\"Gap\" offset=\"{}\" start=\"0s\" duration=\"{}\"/>",
                time(item.start, rate),
                time(item.end - item.start, rate)
            ),
        };
        if children.is_empty() {
            let _ = writeln!(body, "          {open}");
        } else {
            // Re-open a self-closing element to hold its connected clips.
            let (head, tail) = match open.strip_suffix("/>") {
                Some(head) => (format!("{head}>"), close_tag(&open)),
                None => {
                    let cut = open.rfind("</").unwrap_or(open.len());
                    (open[..cut].to_string(), open[cut..].to_string())
                }
            };
            let _ = writeln!(body, "          {head}");
            for child in children {
                let _ = writeln!(body, "            {child}");
            }
            let _ = writeln!(body, "          {tail}");
        }
    }

    let body = attach_markers(body, t, &spine, rate);

    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE fcpxml>\n<fcpxml version=\"1.10\">\n  <resources>\n{}  </resources>\n  <library>\n    <event name=\"chukcut\">\n      <project name=\"{}\">\n        <sequence format=\"{format}\" duration=\"{}\" tcStart=\"0s\" tcFormat=\"{}\" audioLayout=\"stereo\" audioRate=\"48k\">\n          <spine>\n{body}          </spine>\n        </sequence>\n      </project>\n    </event>\n  </library>\n</fcpxml>\n",
        w.resources,
        esc(&t.name),
        time(t.duration, rate),
        if rate.drop_frame() { "DF" } else { "NDF" },
    )
}

fn close_tag(open: &str) -> String {
    let name: String = open
        .trim_start_matches('<')
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '-')
        .collect();
    format!("</{name}>")
}

/// Put each marker into the spine item at its time, as a `<marker>` child
/// in that item's own time.
fn attach_markers(body: String, t: &Timeline, spine: &[SpineItem], rate: Rate) -> String {
    if t.markers.is_empty() {
        return body;
    }
    let mut lines: Vec<String> = body.lines().map(str::to_string).collect();
    for m in &t.markers {
        let Some(item) = spine.iter().find(|s| m.time >= s.start && m.time < s.end) else {
            continue;
        };
        let local = item.clip.map_or(0, |c| c.source_start_local()) + (m.time - item.start);
        let marker = format!(
            "            <marker start=\"{}\" duration=\"{}\" value=\"{}\"/>",
            time(local, rate),
            time_frames(1, rate),
            esc(if m.label.is_empty() {
                "Marker"
            } else {
                &m.label
            })
        );
        let offset = format!("offset=\"{}\"", time(item.start, rate));
        // The spine element of that item: the line at its indent with its
        // offset. Self-closing ones are re-opened.
        if let Some(i) = lines.iter().position(|l| {
            l.starts_with("          <")
                && !l.starts_with("          </")
                && !l.contains("<transition")
                && l.contains(&offset)
        }) {
            let line = lines[i].clone();
            if let Some(head) = line.strip_suffix("/>") {
                lines[i] = format!("{head}>");
                lines.insert(i + 1, marker);
                lines.insert(i + 2, format!("          {}", close_tag(line.trim_start())));
            } else {
                // After the time map: FCPXML wants a clip's markers after
                // its retiming.
                let at = if lines
                    .get(i + 1)
                    .is_some_and(|l| l.trim_start().starts_with("<timeMap>"))
                {
                    i + 2
                } else {
                    i + 1
                };
                lines.insert(at, marker);
            }
        }
    }
    let mut out = lines.join("\n");
    out.push('\n');
    out
}

impl Clip {
    /// Where the clip starts in its own time: its source start, or for a
    /// retimed clip the local time the time map gives that frame.
    fn source_start_local(&self) -> Micros {
        if self.speed == 1.0 {
            self.source_start
        } else {
            (self.source_start as f64 / self.speed as f64).round() as Micros
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::interchange::timeline::{collect, tests::project};

    fn parse(text: &str) -> Result<roxmltree::Document<'_>, roxmltree::Error> {
        let options = roxmltree::ParsingOptions {
            allow_dtd: true,
            ..Default::default()
        };
        roxmltree::Document::parse_with_options(text, options)
    }

    fn xml() -> String {
        let mut report = Report::default();
        let t = collect(&project(), &mut report);
        write(&t, &mut report)
    }

    #[test]
    fn it_is_well_formed_xml_with_one_asset_per_file() {
        let text = xml();
        let doc = parse(&text).expect(&text);
        let assets: Vec<_> = doc
            .descendants()
            .filter(|n| n.has_tag_name("asset"))
            .collect();
        assert_eq!(assets.len(), 3, "two videos and the music");
        let fmt = doc
            .descendants()
            .find(|n| n.has_tag_name("format"))
            .unwrap();
        assert_eq!(fmt.attribute("frameDuration"), Some("1/25s"));
        assert_eq!(fmt.attribute("width"), Some("1920"));
    }

    #[test]
    fn the_spine_holds_the_cut_and_the_lanes_hang_off_it() {
        let text = xml();
        let doc = parse(&text).unwrap();
        let spine = doc.descendants().find(|n| n.has_tag_name("spine")).unwrap();
        let kids: Vec<_> = spine.children().filter(|n| n.is_element()).collect();
        let tags: Vec<&str> = kids.iter().map(|n| n.tag_name().name()).collect();
        assert_eq!(tags, ["asset-clip", "transition", "asset-clip"]);
        let first = kids[0];
        assert_eq!(first.attribute("start"), Some("10s"));
        assert_eq!(first.attribute("duration"), Some("4s"));
        assert_eq!(first.attribute("srcEnable"), Some("video"));
        // 3.5 s is frame 87.5, written on frame 88: FCPXML times are whole frames.
        assert_eq!(kids[1].attribute("offset"), Some("88/25s"));
        // The overlay at 2 s on lane 1, and the music on lane −1, both over
        // shot A whose own time starts at 10 s.
        let lane1 = first
            .children()
            .find(|n| n.attribute("lane") == Some("1"))
            .unwrap();
        assert_eq!(lane1.attribute("offset"), Some("12s"));
        assert!(lane1.children().any(|n| n.has_tag_name("timeMap")));
        let music = first
            .children()
            .find(|n| n.attribute("lane") == Some("-1"))
            .unwrap();
        assert_eq!(music.attribute("offset"), Some("10s"));
        assert_eq!(music.attribute("duration"), Some("8s"));
        // The marker at 6 s is in shot B, 2 s into it: B's own time 4 s.
        let marker = kids[2]
            .children()
            .find(|n| n.has_tag_name("marker"))
            .expect(&text);
        assert_eq!(marker.attribute("value"), Some("drop"));
        assert_eq!(marker.attribute("start"), Some("4s"));
    }
}
