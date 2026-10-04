//! A fingerprint of everything a compound clip's picture depends on.
//!
//! The compositor caches what it built for a compound clip — the nested view
//! of the document, and the nested frames themselves — and needs a key that
//! changes whenever the picture could. The key is a hash of the sequence's
//! lanes, the lanes of every compound clip inside it, and every pool entry any
//! of their clips names: its material, and each id in its `extras` (grade,
//! LUT reference, effects, masks, animation, speed curve, transition, motion
//! track, stabilisation). Ids those entries name in turn are followed too.
//!
//! The hash is proportional to the compound clip's contents, not to the
//! project: a long project with thousands of materials pays for the handful
//! its compound clip uses. Hashing the whole pool per frame would cost more
//! than the clone it replaces.
//!
//! Not covered, on purpose: the contents of files on disk (a LUT edited in
//! place, a missing file that comes back). Those are the provider's and the
//! LUT cache's business, and a document edit refreshes the key.

use std::collections::BTreeSet;
use std::hash::{Hash, Hasher};

use serde::Serialize;

use crate::modules::project::{MaterialPool, Track};

/// The fingerprint of sequence `id` rendered on a canvas of `canvas`. `None`
/// for a sequence that is not parked in `pool`, or one nested in a loop.
pub fn digest(pool: &MaterialPool, canvas: (u32, u32), id: &str) -> Option<u64> {
    let mut hasher = std::hash::DefaultHasher::new();
    canvas.hash(&mut hasher);
    let mut ids: BTreeSet<&str> = BTreeSet::new();
    let mut seen: Vec<&str> = Vec::new();
    lanes(pool, id, &mut hasher, &mut ids, &mut seen)?;
    referenced(pool, ids, &mut hasher);
    Some(hasher.finish())
}

/// Hash the lanes of sequence `id` and of every compound clip inside it, and
/// collect the ids their clips name.
fn lanes<'a>(
    pool: &'a MaterialPool,
    id: &'a str,
    hasher: &mut std::hash::DefaultHasher,
    ids: &mut BTreeSet<&'a str>,
    seen: &mut Vec<&'a str>,
) -> Option<()> {
    if seen.contains(&id) || seen.len() > super::MAX_DEPTH {
        return None;
    }
    let sequence = pool.sequence(id)?;
    id.hash(hasher);
    feed(hasher, &sequence.tracks);
    seen.push(id);
    for segment in sequence
        .tracks
        .iter()
        .flat_map(|t: &Track| t.segments.iter())
    {
        if pool.sequence(&segment.material_id).is_some() {
            lanes(pool, &segment.material_id, hasher, ids, seen)?;
        } else {
            ids.insert(&segment.material_id);
        }
        ids.extend(segment.extras.iter().map(String::as_str));
    }
    seen.pop();
    Some(())
}

/// Hash every pool entry named by `ids`, following the ids those entries
/// name in turn (a follow names its motion track; a stabilisation names its
/// camera path).
fn referenced(pool: &MaterialPool, ids: BTreeSet<&str>, hasher: &mut std::hash::DefaultHasher) {
    // Destructured without `..` on purpose: a new pool category fails to
    // compile here until someone decides whether a compound clip's picture
    // depends on it. Forgetting it would serve stale cached frames.
    let MaterialPool {
        videos,
        audios: _, // sound only
        images,
        texts,
        transitions,
        color_adjusts,
        effects,
        animations,
        trackings,
        follows,
        speed_curves,
        compositing,
        sequences: _, // hashed as lanes above
        links: _,     // which clips move together; nothing drawn
        origins: _,   // credits
        extras,
    } = pool;

    let mut queue: Vec<String> = ids.into_iter().map(str::to_string).collect();
    let mut done: BTreeSet<String> = BTreeSet::new();
    while let Some(id) = queue.pop() {
        if !done.insert(id.clone()) {
            continue;
        }
        let id = id.as_str();
        id.hash(hasher);
        if let Some(m) = videos.iter().find(|m| m.id == id) {
            feed(hasher, m);
        } else if let Some(m) = images.iter().find(|m| m.id == id) {
            feed(hasher, m);
        } else if let Some(m) = texts.iter().find(|m| m.id == id) {
            feed(hasher, m);
        } else if let Some(m) = transitions.iter().find(|m| m.id == id) {
            feed(hasher, m);
        } else if let Some(m) = color_adjusts.iter().find(|m| m.id == id) {
            feed(hasher, m);
        } else if let Some(m) = effects.iter().find(|m| m.id == id) {
            feed(hasher, m);
        } else if let Some(m) = animations.iter().find(|m| m.id == id) {
            feed(hasher, m);
        } else if let Some(m) = speed_curves.iter().find(|m| m.id == id) {
            feed(hasher, m);
        } else if let Some(m) = compositing.iter().find(|m| m.id == id) {
            feed(hasher, m);
        } else if let Some(m) = follows.iter().find(|m| m.id == id) {
            feed(hasher, m);
            queue.push(m.track_id.clone());
        } else if let Some(m) = trackings.iter().find(|m| m.id == id) {
            feed(hasher, m);
        } else if let Some(value) = extras.get(id) {
            feed(hasher, value);
            strings(value, &mut |s| {
                if extras.contains_key(s) {
                    queue.push(s.to_string());
                }
            });
        }
    }
}

/// Every string inside a JSON value.
fn strings(value: &serde_json::Value, found: &mut impl FnMut(&str)) {
    match value {
        serde_json::Value::String(s) => found(s),
        serde_json::Value::Array(items) => items.iter().for_each(|v| strings(v, found)),
        serde_json::Value::Object(map) => map.values().for_each(|v| strings(v, found)),
        _ => {}
    }
}

/// Feed the serialised form of `value` into `hasher`.
fn feed(hasher: &mut std::hash::DefaultHasher, value: &impl Serialize) {
    struct Writer<'a>(&'a mut std::hash::DefaultHasher);
    impl std::io::Write for Writer<'_> {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.write(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    // Serialising plain document data cannot fail; a failure would only make
    // two different documents share a key less often, never more.
    let _ = serde_json::to_writer(Writer(hasher), value);
    // A separator, so two values cannot run together into a third.
    hasher.write_u8(0xff);
}
