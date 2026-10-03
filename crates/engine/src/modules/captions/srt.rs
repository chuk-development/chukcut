//! SubRip (`.srt`) and WebVTT (`.vtt`), read and written.
//!
//! Both formats are a list of blocks separated by blank lines, each with one
//! timing line (`start --> end`) and the text below it. They differ in the
//! decimal separator (`,` against `.`), in whether hours are optional, and in
//! VTT's header, notes and markup. One tolerant reader handles both, because
//! real files in the wild mix the two: SRTs with dots, VTTs without hours,
//! Windows line endings and byte-order marks everywhere.
//!
//! Times are rounded to the millisecond on the way out, which is all either
//! format can hold. A cue whose times are whole milliseconds survives a round
//! trip exactly; that is the test.

use super::Cue;
use crate::modules::project::Micros;

/// Which of the two formats to write.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubtitleFormat {
    Srt,
    Vtt,
}

impl SubtitleFormat {
    /// From a file name's extension; `None` for anything else.
    pub fn from_path(path: &std::path::Path) -> Option<Self> {
        let ext = path.extension()?.to_str()?.to_ascii_lowercase();
        match ext.as_str() {
            "srt" => Some(Self::Srt),
            "vtt" | "webvtt" => Some(Self::Vtt),
            _ => None,
        }
    }

    pub fn extension(self) -> &'static str {
        match self {
            Self::Srt => "srt",
            Self::Vtt => "vtt",
        }
    }
}

/// Read subtitles in either format.
///
/// The format is sniffed from the content rather than trusted from the file
/// name: a `WEBVTT` header means VTT, anything else is read as SRT, and the
/// reader accepts both separators either way.
pub fn parse(text: &str) -> Result<Vec<Cue>, String> {
    let text = text.strip_prefix('\u{FEFF}').unwrap_or(text);
    let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
    let is_vtt = normalized.trim_start().starts_with("WEBVTT");

    let mut cues = Vec::new();
    for block in normalized.split("\n\n") {
        let lines: Vec<&str> = block.lines().collect();
        let Some(timing_at) = lines.iter().position(|l| l.contains("-->")) else {
            // A header, a note, a style block, or a stray index.
            continue;
        };
        if is_vtt {
            let first = lines.first().map(|l| l.trim()).unwrap_or("");
            if first.starts_with("NOTE")
                || first.starts_with("STYLE")
                || first.starts_with("REGION")
            {
                continue;
            }
        }
        let (start, end) = parse_timing(lines[timing_at]).ok_or_else(|| {
            format!(
                "this line is not a subtitle timing: \"{}\"",
                lines[timing_at].trim()
            )
        })?;
        let body: Vec<String> = lines[timing_at + 1..]
            .iter()
            .map(|line| clean_line(line, is_vtt))
            .collect();
        let text = body.join("\n").trim().to_string();
        if text.is_empty() || end <= start {
            continue;
        }
        cues.push(Cue::new(start, end, text));
    }

    if cues.is_empty() && !normalized.trim().is_empty() {
        return Err("no subtitles were found in that file".to_string());
    }
    cues.sort_by_key(|c| c.start);
    Ok(cues)
}

/// Write cues as SubRip.
pub fn to_srt(cues: &[Cue]) -> String {
    let mut out = String::new();
    for (index, cue) in cues.iter().enumerate() {
        out.push_str(&format!(
            "{}\n{} --> {}\n{}\n\n",
            index + 1,
            format_time(cue.start, ','),
            format_time(cue.end, ','),
            single_spaced(&cue.text)
        ));
    }
    out
}

/// Write cues as WebVTT.
pub fn to_vtt(cues: &[Cue]) -> String {
    let mut out = String::from("WEBVTT\n\n");
    for cue in cues {
        let text = single_spaced(&cue.text)
            .replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;");
        out.push_str(&format!(
            "{} --> {}\n{}\n\n",
            format_time(cue.start, '.'),
            format_time(cue.end, '.'),
            text
        ));
    }
    out
}

pub fn format(cues: &[Cue], format: SubtitleFormat) -> String {
    match format {
        SubtitleFormat::Srt => to_srt(cues),
        SubtitleFormat::Vtt => to_vtt(cues),
    }
}

/// A blank line inside a cue ends it in both formats, so a caption the user
/// typed with an empty line in it is written without one.
fn single_spaced(text: &str) -> String {
    text.lines()
        .map(str::trim_end)
        .filter(|l| !l.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

fn parse_timing(line: &str) -> Option<(Micros, Micros)> {
    let (left, right) = line.split_once("-->")?;
    let start = parse_time(left.trim())?;
    // VTT puts cue settings after the end time: `00:01.000 --> 00:02.000 line:0`.
    let end = parse_time(right.split_whitespace().next()?)?;
    Some((start, end))
}

/// `HH:MM:SS,mmm`, `HH:MM:SS.mmm` or `MM:SS.mmm`; hours may be any width.
fn parse_time(text: &str) -> Option<Micros> {
    let text = text.trim();
    let (clock, fraction) = match text.rfind([',', '.']) {
        Some(at) => (&text[..at], &text[at + 1..]),
        None => (text, ""),
    };
    let parts: Vec<&str> = clock.split(':').collect();
    let (hours, minutes, seconds) = match parts.as_slice() {
        [h, m, s] => (
            h.parse::<i64>().ok()?,
            m.parse::<i64>().ok()?,
            s.parse::<i64>().ok()?,
        ),
        [m, s] => (0, m.parse::<i64>().ok()?, s.parse::<i64>().ok()?),
        _ => return None,
    };
    if !(0..60).contains(&minutes) || !(0..60).contains(&seconds) || hours < 0 {
        return None;
    }
    // A fraction of any length: "5" is 500 ms, "05" is 50 ms, "0501" is 50.1 ms.
    let mut micros = 0i64;
    let mut unit = 100_000i64;
    for c in fraction.chars() {
        let digit = c.to_digit(10)? as i64;
        micros += digit * unit;
        unit /= 10;
        if unit == 0 {
            break;
        }
    }
    Some(((hours * 60 + minutes) * 60 + seconds) * 1_000_000 + micros)
}

fn format_time(time: Micros, separator: char) -> String {
    let millis = (time.max(0) + 500) / 1_000;
    let (hours, rest) = (millis / 3_600_000, millis % 3_600_000);
    let (minutes, rest) = (rest / 60_000, rest % 60_000);
    let (seconds, millis) = (rest / 1_000, rest % 1_000);
    format!("{hours:02}:{minutes:02}:{seconds:02}{separator}{millis:03}")
}

/// Drop markup: SRT's `<i>`/`<b>`/`<font>`, VTT's `<c.yellow>`, voice tags and
/// inline timestamps, and VTT's character references.
fn clean_line(line: &str, is_vtt: bool) -> String {
    let mut out = String::with_capacity(line.len());
    let mut rest = line;
    while let Some(open) = rest.find('<') {
        out.push_str(&rest[..open]);
        let after = &rest[open..];
        match after.find('>') {
            Some(close) if is_vtt || is_srt_tag(&after[1..close]) => {
                rest = &after[close + 1..];
            }
            _ => {
                // In SRT a "<" that does not open a known tag is just text.
                out.push('<');
                rest = &after[1..];
            }
        }
    }
    out.push_str(rest);
    // SRT's ASS-style override blocks, `{\an8}`.
    while let (Some(open), Some(close)) = (out.find("{\\"), out.find('}')) {
        if close < open {
            break;
        }
        out.replace_range(open..=close, "");
    }
    if is_vtt {
        out = out
            .replace("&lt;", "<")
            .replace("&gt;", ">")
            .replace("&nbsp;", "\u{a0}")
            .replace("&amp;", "&");
    }
    out.trim_end().to_string()
}

/// SubRip's whole markup vocabulary: `<i>`, `<b>`, `<u>`, `<s>` and `<font …>`.
fn is_srt_tag(inner: &str) -> bool {
    let name = inner
        .trim_start_matches('/')
        .split_whitespace()
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    matches!(name.as_str(), "i" | "b" | "u" | "s" | "font")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Vec<Cue> {
        vec![
            Cue::new(1_000_000, 2_500_000, "Hello there"),
            Cue::new(2_500_000, 4_001_000, "Two lines\nof text"),
            Cue::new(3_723_045_000, 3_725_000_000, "An hour in & <counting>"),
        ]
    }

    #[test]
    fn srt_round_trips_exactly() {
        let cues = sample();
        let written = to_srt(&cues);
        assert!(written.starts_with("1\n00:00:01,000 --> 00:00:02,500\nHello there\n\n2\n"));
        assert!(written.contains("01:02:03,045 --> 01:02:05,000"));
        assert_eq!(parse(&written).unwrap(), cues);
    }

    #[test]
    fn vtt_round_trips_exactly_with_escapes() {
        let cues = sample();
        let written = to_vtt(&cues);
        assert!(written.starts_with("WEBVTT\n\n00:00:01.000 --> 00:00:02.500\n"));
        assert!(written.contains("An hour in &amp; &lt;counting&gt;"));
        assert_eq!(parse(&written).unwrap(), cues);
    }

    #[test]
    fn windows_line_endings_a_bom_and_markup_are_tolerated() {
        let file = "\u{FEFF}1\r\n00:00:00,500 --> 00:00:01,250\r\n<i>Hi</i> {\\an8}there\r\n\r\n2\r\n00:00:02.000 --> 00:00:03.000\r\nnext\r\n";
        let cues = parse(file).unwrap();
        assert_eq!(cues.len(), 2);
        assert_eq!(cues[0].start, 500_000);
        assert_eq!(cues[0].end, 1_250_000);
        assert_eq!(cues[0].text, "Hi there");
        assert_eq!(cues[1].text, "next");
    }

    #[test]
    fn vtt_short_times_settings_notes_and_ids() {
        let file = "WEBVTT - a title\n\nNOTE this is ignored\n--> even with an arrow\n\nintro\n00:01.5 --> 00:03.000 align:start line:0\n<v Speaker>Hi</v>\n\n";
        let cues = parse(file).unwrap();
        assert_eq!(cues, vec![Cue::new(1_500_000, 3_000_000, "Hi")]);
    }

    #[test]
    fn garbage_is_an_error_and_empty_is_nothing() {
        assert!(parse("this is not a subtitle file").is_err());
        assert!(parse("1\n00:00:01,000 --> soon\nhi\n").is_err());
        assert_eq!(parse("").unwrap(), Vec::new());
    }

    #[test]
    fn times_round_to_the_millisecond() {
        assert_eq!(format_time(1_499, ','), "00:00:00,001");
        assert_eq!(format_time(1_500, ','), "00:00:00,002");
        assert_eq!(parse_time("00:00:00,0015"), Some(1_500));
    }

    #[test]
    fn the_format_comes_from_the_extension() {
        use std::path::Path;
        assert_eq!(
            SubtitleFormat::from_path(Path::new("a.SRT")),
            Some(SubtitleFormat::Srt)
        );
        assert_eq!(
            SubtitleFormat::from_path(Path::new("a.vtt")),
            Some(SubtitleFormat::Vtt)
        );
        assert_eq!(SubtitleFormat::from_path(Path::new("a.txt")), None);
    }
}
