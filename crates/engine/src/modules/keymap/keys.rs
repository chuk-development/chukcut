//! Key strings: GPUI's syntax (`ctrl-shift-z`, `ctrl--`, `g g` for a
//! sequence), checked and put into one spelling so two ways of writing the
//! same chord compare equal.

/// The modifiers, in the order a canonical key lists them.
const MODIFIERS: [&str; 5] = ["ctrl", "alt", "shift", "super", "fn"];

/// Named keys a chord may end in, besides one printable character and
/// `f1`–`f24`.
const NAMED: &[&str] = &[
    "space",
    "enter",
    "escape",
    "tab",
    "backspace",
    "delete",
    "insert",
    "home",
    "end",
    "pageup",
    "pagedown",
    "left",
    "right",
    "up",
    "down",
];

fn modifier(word: &str) -> Option<&'static str> {
    match word {
        "ctrl" | "control" => Some("ctrl"),
        "alt" | "option" => Some("alt"),
        "shift" => Some("shift"),
        "super" | "cmd" | "win" | "meta" | "platform" => Some("super"),
        "fn" => Some("fn"),
        _ => None,
    }
}

fn is_key(word: &str) -> bool {
    if NAMED.contains(&word) {
        return true;
    }
    if let Some(n) = word.strip_prefix('f') {
        if let Ok(n) = n.parse::<u32>() {
            return (1..=24).contains(&n);
        }
    }
    let mut chars = word.chars();
    matches!((chars.next(), chars.next()), (Some(c), None) if !c.is_whitespace() && !c.is_control())
}

/// One chord, canonical: `Shift+Ctrl+Z` → `ctrl-shift-z`.
fn chord(raw: &str) -> Result<String, String> {
    let raw = raw.trim().to_lowercase();
    if raw.is_empty() {
        return Err("an empty key".into());
    }
    // The key is everything after the last separator — except that the key
    // may itself be `-` or `+` (`ctrl--`, `ctrl-+`).
    let two_char_tail = ["--", "-+", "+-", "++"];
    let (mods, key) = if raw.len() > 1 && two_char_tail.iter().any(|t| raw.ends_with(t)) {
        (&raw[..raw.len() - 2], &raw[raw.len() - 1..])
    } else {
        match raw.rfind(['-', '+']) {
            Some(at) if at + 1 < raw.len() => (&raw[..at], &raw[at + 1..]),
            _ => ("", raw.as_str()),
        }
    };
    let mut held: Vec<&'static str> = Vec::new();
    for word in mods.split(['-', '+']).filter(|w| !w.is_empty()) {
        let m = modifier(word).ok_or_else(|| format!("{word:?} is not a modifier"))?;
        if !held.contains(&m) {
            held.push(m);
        }
    }
    if modifier(key).is_some() {
        return Err(format!("{raw:?} is only a modifier; add a key"));
    }
    if !is_key(key) {
        return Err(format!("{key:?} is not a key"));
    }
    let mut out: Vec<&str> = MODIFIERS
        .iter()
        .copied()
        .filter(|m| held.contains(m))
        .collect();
    out.push(key);
    Ok(out.join("-"))
}

/// A key or sequence, canonical. Chords are separated by spaces.
pub fn normalize(raw: &str) -> Result<String, String> {
    let chords: Vec<String> = raw
        .split_whitespace()
        .map(chord)
        .collect::<Result<_, _>>()?;
    if chords.is_empty() {
        return Err("an empty key".into());
    }
    Ok(chords.join(" "))
}

/// Whether the chords of `key` carry Ctrl, Alt or Super — keys that are safe
/// to leave on while typing.
pub fn has_command_modifier(key: &str) -> bool {
    key.split_whitespace().all(|chord| {
        let mut parts: Vec<&str> = chord.split('-').collect();
        // `ctrl--`: the last part is the key.
        if chord.ends_with("--") {
            parts.pop();
        }
        parts.pop();
        parts.iter().any(|p| matches!(*p, "ctrl" | "alt" | "super"))
    })
}

/// Whether pressing `a` would run into `b`: the same keys, or one a
/// sequence the other starts with (GPUI waits for the rest of the longer
/// one and the shorter never fires at once).
pub fn collide(a: &str, b: &str) -> bool {
    let a: Vec<&str> = a.split_whitespace().collect();
    let b: Vec<&str> = b.split_whitespace().collect();
    let n = a.len().min(b.len());
    n > 0 && a[..n] == b[..n]
}

/// How a key reads in a list: `Ctrl+Shift+Z`.
pub fn display(key: &str) -> String {
    key.split_whitespace()
        .map(|chord| {
            let mut parts: Vec<String> = Vec::new();
            let (mods, last) = match chord.strip_suffix("--") {
                _ if chord.chars().count() == 1 => ("", chord),
                Some(mods) => (mods, "-"),
                None => match chord.rsplit_once('-') {
                    Some((mods, key)) => (mods, key),
                    None => ("", chord),
                },
            };
            for m in mods.split('-').filter(|m| !m.is_empty()) {
                parts.push(
                    match m {
                        "ctrl" => "Ctrl",
                        "alt" => "Alt",
                        "shift" => "Shift",
                        "super" => "Super",
                        other => other,
                    }
                    .to_string(),
                );
            }
            let key = match last {
                "space" => "Space".to_string(),
                "escape" => "Esc".to_string(),
                "backspace" => "Backspace".to_string(),
                "delete" => "Delete".to_string(),
                "left" => "\u{2190}".to_string(),
                "right" => "\u{2192}".to_string(),
                "up" => "\u{2191}".to_string(),
                "down" => "\u{2193}".to_string(),
                other if other.chars().count() == 1 => other.to_uppercase(),
                other => {
                    let mut c = other.chars();
                    c.next()
                        .map(|f| f.to_uppercase().collect::<String>() + c.as_str())
                        .unwrap_or_default()
                }
            };
            parts.push(key);
            parts.join("+")
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spellings_of_one_chord_compare_equal() {
        assert_eq!(normalize("Shift+Ctrl+Z").unwrap(), "ctrl-shift-z");
        assert_eq!(normalize("ctrl-shift-z").unwrap(), "ctrl-shift-z");
        assert_eq!(normalize("cmd-k").unwrap(), "super-k");
        assert_eq!(normalize("ctrl--").unwrap(), "ctrl--");
        assert_eq!(normalize("Ctrl++").unwrap(), "ctrl-+");
        assert_eq!(normalize("ctrl-+").unwrap(), "ctrl-+");
        assert_eq!(normalize("?").unwrap(), "?");
        assert_eq!(normalize("-").unwrap(), "-");
        assert_eq!(normalize("g  g").unwrap(), "g g");
        assert_eq!(normalize("F12").unwrap(), "f12");
    }

    #[test]
    fn nonsense_is_refused_with_the_reason() {
        assert!(normalize("").is_err());
        assert!(normalize("ctrl").unwrap_err().contains("only a modifier"));
        assert!(normalize("hyper-x").unwrap_err().contains("not a modifier"));
        assert!(normalize("ctrl-banana").unwrap_err().contains("not a key"));
        assert!(normalize("f25").is_err());
    }

    #[test]
    fn sequences_collide_with_their_prefixes() {
        assert!(collide("ctrl-k", "ctrl-k"));
        assert!(collide("g", "g g"));
        assert!(!collide("g h", "g g"));
        assert!(!collide("ctrl-k", "ctrl-shift-k"));
    }

    #[test]
    fn command_modifiers_are_told_apart_from_shift() {
        assert!(has_command_modifier("ctrl-c"));
        assert!(has_command_modifier("alt-x"));
        assert!(has_command_modifier("ctrl--"));
        assert!(!has_command_modifier("shift-left"));
        assert!(!has_command_modifier("s"));
        assert!(!has_command_modifier("-"));
    }

    #[test]
    fn keys_read_like_a_menu() {
        assert_eq!(display("ctrl-shift-z"), "Ctrl+Shift+Z");
        assert_eq!(display("ctrl--"), "Ctrl+-");
        assert_eq!(display("space"), "Space");
        assert_eq!(display("shift-left"), "Shift+\u{2190}");
        assert_eq!(display("g g"), "G G");
        assert_eq!(display("-"), "-");
    }
}
