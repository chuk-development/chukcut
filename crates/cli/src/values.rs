//! Values a person types: times, colours, `name=value` pairs.
//!
//! Every time in the engine is `i64` microseconds. People do not type
//! microseconds, so a [`Time`] accepts the spellings an editor uses and turns
//! into microseconds only once the project's frame rate is known (a frame
//! count needs it).

use std::borrow::Cow;
use std::fmt;
use std::str::FromStr;

use chukcut_engine::modules::project::Micros;
use schemars::{json_schema, JsonSchema, Schema, SchemaGenerator};
use serde::{Deserialize, Deserializer, Serialize};

pub const MICROS_PER_SECOND: f64 = 1_000_000.0;

/// A point or a length on the timeline, as typed.
///
/// - `2.5`, `2.5s` — seconds
/// - `250ms`, `1500000us` — milliseconds, microseconds
/// - `1:02.5`, `01:02:03.25` — minutes:seconds, hours:minutes:seconds
/// - `45f` — frames at the project's frame rate
///
/// In JSON, a bare number is seconds and a string takes any of the above.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Time {
    Micros(Micros),
    Frames(i64),
}

impl Default for Time {
    fn default() -> Self {
        Time::Micros(0)
    }
}

impl Time {
    pub fn resolve(self, fps: f64) -> Micros {
        match self {
            Time::Micros(us) => us,
            Time::Frames(n) => {
                let fps = if fps.is_finite() && fps > 0.0 {
                    fps
                } else {
                    30.0
                };
                (n as f64 * MICROS_PER_SECOND / fps).round() as Micros
            }
        }
    }

    pub fn from_seconds(seconds: f64) -> Result<Self, String> {
        if !seconds.is_finite() {
            return Err("a time must be a finite number".into());
        }
        Ok(Time::Micros((seconds * MICROS_PER_SECOND).round() as Micros))
    }
}

impl FromStr for Time {
    type Err = String;

    fn from_str(raw: &str) -> Result<Self, String> {
        let s = raw.trim();
        let bad = || {
            format!(
                "{raw:?} is not a time; write seconds (2.5 or 2.5s), 250ms, 1500000us, 1:02.5 or 45f"
            )
        };
        if s.is_empty() {
            return Err(bad());
        }
        let number = |t: &str| t.trim().parse::<f64>().ok().filter(|v| v.is_finite());

        if let Some(frames) = s.strip_suffix('f') {
            return frames
                .trim()
                .parse::<i64>()
                .map(Time::Frames)
                .map_err(|_| bad());
        }
        if let Some(us) = s.strip_suffix("us") {
            return us
                .trim()
                .parse::<i64>()
                .map(Time::Micros)
                .map_err(|_| bad());
        }
        if let Some(ms) = s.strip_suffix("ms") {
            return number(ms)
                .map(|v| Time::Micros((v * 1000.0).round() as Micros))
                .ok_or_else(bad);
        }
        if s.contains(':') {
            let negative = s.starts_with('-');
            let body = s.trim_start_matches('-');
            let parts: Vec<&str> = body.split(':').collect();
            if parts.len() > 3 {
                return Err(bad());
            }
            let mut seconds = 0.0;
            for (i, part) in parts.iter().enumerate() {
                let last = i + 1 == parts.len();
                let value = if last {
                    number(part)
                } else {
                    part.trim().parse::<u32>().ok().map(f64::from)
                }
                .ok_or_else(bad)?;
                seconds = seconds * 60.0 + value;
            }
            let seconds = if negative { -seconds } else { seconds };
            return Time::from_seconds(seconds);
        }
        let seconds = number(s.strip_suffix('s').unwrap_or(s)).ok_or_else(bad)?;
        Time::from_seconds(seconds)
    }
}

impl fmt::Display for Time {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Time::Micros(us) => write!(f, "{}s", *us as f64 / MICROS_PER_SECOND),
            Time::Frames(n) => write!(f, "{n}f"),
        }
    }
}

impl<'de> Deserialize<'de> for Time {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw {
            Number(f64),
            Text(String),
        }
        match Raw::deserialize(deserializer)? {
            Raw::Number(seconds) => Time::from_seconds(seconds).map_err(serde::de::Error::custom),
            Raw::Text(text) => text.parse().map_err(serde::de::Error::custom),
        }
    }
}

impl JsonSchema for Time {
    fn schema_name() -> Cow<'static, str> {
        "Time".into()
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "description": "A time. A number is seconds; a string may be \"2.5s\", \"250ms\", \"1500000us\", \"1:02.5\" or \"45f\" (frames at the project rate).",
            "type": ["number", "string"]
        })
    }

    fn inline_schema() -> bool {
        true
    }
}

/// Microseconds as seconds, for output.
pub fn seconds(us: Micros) -> f64 {
    us as f64 / MICROS_PER_SECOND
}

/// A colour as typed: `#rgb`, `#rrggbb`, `#rrggbbaa`, a few names, or
/// `r,g,b[,a]` with components in `0..1`. Returned as sRGB-encoded RGBA in
/// `0..1`, which is how titles and captions store colour.
pub fn parse_color(raw: &str) -> Result<[f32; 4], String> {
    let s = raw.trim().to_ascii_lowercase();
    let named = match s.as_str() {
        "white" => Some([1.0, 1.0, 1.0, 1.0]),
        "black" => Some([0.0, 0.0, 0.0, 1.0]),
        "red" => Some([1.0, 0.0, 0.0, 1.0]),
        "green" => Some([0.0, 1.0, 0.0, 1.0]),
        "blue" => Some([0.0, 0.0, 1.0, 1.0]),
        "yellow" => Some([1.0, 1.0, 0.0, 1.0]),
        "transparent" => Some([0.0, 0.0, 0.0, 0.0]),
        _ => None,
    };
    if let Some(c) = named {
        return Ok(c);
    }
    let bad = || format!("{raw:?} is not a colour; write #rrggbb, #rrggbbaa or r,g,b[,a] in 0..1");
    if let Some(hex) = s.strip_prefix('#') {
        let hex: Cow<str> = if hex.len() == 3 {
            hex.chars().flat_map(|c| [c, c]).collect::<String>().into()
        } else {
            hex.into()
        };
        if !(hex.len() == 6 || hex.len() == 8) || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(bad());
        }
        let byte = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).map(|b| b as f32 / 255.0);
        let alpha = if hex.len() == 8 { byte(6) } else { Ok(1.0) };
        return Ok([
            byte(0).map_err(|_| bad())?,
            byte(2).map_err(|_| bad())?,
            byte(4).map_err(|_| bad())?,
            alpha.map_err(|_| bad())?,
        ]);
    }
    let parts: Vec<f32> = s
        .split(',')
        .map(|p| p.trim().parse::<f32>())
        .collect::<Result<_, _>>()
        .map_err(|_| bad())?;
    match parts.as_slice() {
        [r, g, b] => Ok([*r, *g, *b, 1.0]),
        [r, g, b, a] => Ok([*r, *g, *b, *a]),
        _ => Err(bad()),
    }
    .and_then(|c: [f32; 4]| {
        if c.iter().all(|v| v.is_finite() && (0.0..=1.0).contains(v)) {
            Ok(c)
        } else {
            Err(bad())
        }
    })
}

/// sRGB-encoded to linear light, alpha untouched. Effect colour parameters
/// are linear; titles are not.
pub fn srgb_to_linear(c: [f32; 4]) -> [f32; 4] {
    let f = |e: f32| {
        if e <= 0.04045 {
            e / 12.92
        } else {
            ((e + 0.055) / 1.055).powf(2.4)
        }
    };
    [f(c[0]), f(c[1]), f(c[2]), c[3]]
}

/// `#rrggbbaa` for an sRGB-encoded colour, for output.
pub fn hex(c: [f32; 4]) -> String {
    let b = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    format!(
        "#{:02x}{:02x}{:02x}{:02x}",
        b(c[0]),
        b(c[1]),
        b(c[2]),
        b(c[3])
    )
}

/// Split `name=value`.
pub fn key_value(raw: &str) -> Result<(&str, &str), String> {
    raw.split_once('=')
        .map(|(k, v)| (k.trim(), v.trim()))
        .filter(|(k, _)| !k.is_empty())
        .ok_or_else(|| format!("{raw:?} is not name=value"))
}

/// A list argument that accepts both `--set a=1 --set b=2` on the command
/// line and `{"a": 1, "b": 2}` or `["a=1", "b=2"]` in JSON.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Assignments(pub Vec<(String, String)>);

impl<'de> Deserialize<'de> for Assignments {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = serde_json::Value::deserialize(deserializer)?;
        let text = |v: &serde_json::Value| match v {
            serde_json::Value::String(s) => s.clone(),
            other => other.to_string(),
        };
        match value {
            serde_json::Value::Null => Ok(Self::default()),
            serde_json::Value::Object(map) => Ok(Self(
                map.iter().map(|(k, v)| (k.clone(), text(v))).collect(),
            )),
            serde_json::Value::Array(items) => items
                .iter()
                .map(|item| {
                    let s = text(item);
                    key_value(&s)
                        .map(|(k, v)| (k.to_string(), v.to_string()))
                        .map_err(serde::de::Error::custom)
                })
                .collect::<Result<_, _>>()
                .map(Self),
            serde_json::Value::String(s) => key_value(&s)
                .map(|(k, v)| Self(vec![(k.to_string(), v.to_string())]))
                .map_err(serde::de::Error::custom),
            other => Err(serde::de::Error::custom(format!(
                "expected an object of name: value, got {other}"
            ))),
        }
    }
}

impl JsonSchema for Assignments {
    fn schema_name() -> Cow<'static, str> {
        "Assignments".into()
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "description": "Values to set, as an object {\"name\": value} or a list of \"name=value\" strings.",
            "type": ["object", "array"],
            "additionalProperties": {"type": ["number", "string", "boolean"]},
            "items": {"type": "string"}
        })
    }

    fn inline_schema() -> bool {
        true
    }
}

/// What every clap `--set name=value` parses to before it is collected.
pub fn parse_assignment(raw: &str) -> Result<(String, String), String> {
    key_value(raw).map(|(k, v)| (k.to_string(), v.to_string()))
}

impl Serialize for Time {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn us(s: &str) -> Micros {
        s.parse::<Time>().unwrap().resolve(30.0)
    }

    #[test]
    fn times_in_every_spelling() {
        assert_eq!(us("2.5"), 2_500_000);
        assert_eq!(us("2.5s"), 2_500_000);
        assert_eq!(us("250ms"), 250_000);
        assert_eq!(us("1500000us"), 1_500_000);
        assert_eq!(us("1:02.5"), 62_500_000);
        assert_eq!(us("01:00:01"), 3_601_000_000);
        assert_eq!(us("45f"), 1_500_000);
        assert_eq!(us("-1s"), -1_000_000);
        assert_eq!("3f".parse::<Time>().unwrap().resolve(24.0), 125_000);
        for bad in ["", "abc", "1:2:3:4", "1.5f", "nan", "2x"] {
            assert!(bad.parse::<Time>().is_err(), "{bad:?}");
        }
    }

    #[test]
    fn json_times_are_seconds_or_strings() {
        let t: Time = serde_json::from_str("1.25").unwrap();
        assert_eq!(t.resolve(30.0), 1_250_000);
        let t: Time = serde_json::from_str("\"10f\"").unwrap();
        assert_eq!(t.resolve(25.0), 400_000);
    }

    #[test]
    fn colours() {
        assert_eq!(parse_color("#ff0000").unwrap(), [1.0, 0.0, 0.0, 1.0]);
        assert_eq!(parse_color("#fff").unwrap(), [1.0, 1.0, 1.0, 1.0]);
        assert_eq!(parse_color("#00000080").unwrap()[3], 128.0 / 255.0);
        assert_eq!(parse_color("0,0.5,1").unwrap(), [0.0, 0.5, 1.0, 1.0]);
        assert_eq!(parse_color("White").unwrap(), [1.0; 4]);
        assert!(parse_color("#12345").is_err());
        assert!(parse_color("2,0,0").is_err());
        assert_eq!(hex([1.0, 0.0, 0.0, 1.0]), "#ff0000ff");
    }

    #[test]
    fn assignments_from_json_object_list_or_string() {
        let a: Assignments = serde_json::from_str(r#"{"exposure": 0.5, "lut": "x"}"#).unwrap();
        assert_eq!(a.0.len(), 2);
        let a: Assignments = serde_json::from_str(r#"["a=1", "b = 2"]"#).unwrap();
        assert_eq!(a.0[1], ("b".to_string(), "2".to_string()));
        assert!(serde_json::from_str::<Assignments>(r#"["nope"]"#).is_err());
    }
}
