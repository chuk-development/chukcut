//! Which clips are selected, and the pure rules behind every way of changing
//! that: a click, Ctrl+click, Shift+click along a lane, and a rubber band.
//!
//! The editor's `selected` field stays the *primary* clip — the one the
//! inspector shows and the one other panels act on. The timeline keeps the
//! whole set next to it. When another panel sets `selected` to a clip outside
//! the set, the set is just that clip again, so the two never disagree about
//! what is selected.

use chukcut_engine::modules::project::Track;

/// A clip as drawn, for the rubber band: lanes-local pixels.
#[derive(Debug, Clone)]
pub(crate) struct Drawn {
    pub segment_id: String,
    pub left: f32,
    pub right: f32,
    pub top: f32,
    pub bottom: f32,
}

/// A rectangle in lanes-local pixels, from two corners in any order.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Band {
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
}

impl Band {
    pub(crate) fn from_corners(a: (f32, f32), b: (f32, f32)) -> Self {
        Self {
            left: a.0.min(b.0),
            top: a.1.min(b.1),
            right: a.0.max(b.0),
            bottom: a.1.max(b.1),
        }
    }

    pub(crate) fn width(&self) -> f32 {
        self.right - self.left
    }

    pub(crate) fn height(&self) -> f32 {
        self.bottom - self.top
    }
}

/// Every clip the band touches, in the order given.
pub(crate) fn band_hits(clips: &[Drawn], band: Band) -> Vec<String> {
    clips
        .iter()
        .filter(|c| {
            c.left < band.right && band.left < c.right && c.top < band.bottom && band.top < c.bottom
        })
        .map(|c| c.segment_id.clone())
        .collect()
}

/// The selection a click makes: plain replaces, Ctrl toggles the clip in or
/// out, Shift extends along the lane from the primary clip (or adds, when the
/// primary is on another lane).
///
/// Returns the new set and the new primary.
pub(crate) fn clicked(
    current: &[String],
    primary: Option<&str>,
    clicked: &str,
    lane: &Track,
    ctrl: bool,
    shift: bool,
) -> (Vec<String>, Option<String>) {
    if shift {
        if let Some(anchor) = primary {
            if let Some(range) = lane_range(lane, anchor, clicked) {
                let mut set: Vec<String> = current.to_vec();
                for id in range {
                    if !set.contains(&id) {
                        set.push(id);
                    }
                }
                return (set, Some(clicked.to_string()));
            }
        }
        return clicked_with_ctrl(current, clicked, true);
    }
    if ctrl {
        return clicked_with_ctrl(current, clicked, false);
    }
    (vec![clicked.to_string()], Some(clicked.to_string()))
}

fn clicked_with_ctrl(
    current: &[String],
    clicked: &str,
    only_add: bool,
) -> (Vec<String>, Option<String>) {
    let mut set: Vec<String> = current.to_vec();
    if let Some(at) = set.iter().position(|id| id == clicked) {
        if only_add {
            return (set, Some(clicked.to_string()));
        }
        set.remove(at);
        let primary = set.last().cloned();
        return (set, primary);
    }
    set.push(clicked.to_string());
    (set, Some(clicked.to_string()))
}

/// The clips of `lane` from `a` to `b`, both included, when both are on it.
pub(crate) fn lane_range(lane: &Track, a: &str, b: &str) -> Option<Vec<String>> {
    let i = lane.segments.iter().position(|s| s.id == a)?;
    let j = lane.segments.iter().position(|s| s.id == b)?;
    let (from, to) = (i.min(j), i.max(j));
    Some(
        lane.segments[from..=to]
            .iter()
            .map(|s| s.id.clone())
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::timeline::batch::tests::project;

    fn drawn(id: &str, left: f32, right: f32, top: f32) -> Drawn {
        Drawn {
            segment_id: id.into(),
            left,
            right,
            top,
            bottom: top + 40.0,
        }
    }

    #[test]
    fn a_band_catches_what_it_touches_on_every_lane() {
        let clips = [
            drawn("a", 0.0, 100.0, 0.0),
            drawn("b", 120.0, 200.0, 0.0),
            drawn("c", 50.0, 150.0, 60.0),
            drawn("d", 300.0, 400.0, 60.0),
        ];
        let band = Band::from_corners((140.0, 90.0), (90.0, 10.0));
        assert_eq!(band_hits(&clips, band), vec!["a", "b", "c"]);
        assert!(band_hits(&clips, Band::from_corners((210.0, 0.0), (290.0, 200.0))).is_empty());
    }

    #[test]
    fn ctrl_click_toggles_and_keeps_a_primary() {
        let (project, ids) = project(&[1, 1, 1]);
        let lane = &project.tracks[0];
        let (set, primary) = clicked(&[], None, &ids[0], lane, true, false);
        assert_eq!(set, vec![ids[0].clone()]);
        let (set, primary) = clicked(&set, primary.as_deref(), &ids[2], lane, true, false);
        assert_eq!(set.len(), 2);
        assert_eq!(primary.as_deref(), Some(ids[2].as_str()));
        // Clicking the primary again takes it out; the other one leads.
        let (set, primary) = clicked(&set, primary.as_deref(), &ids[2], lane, true, false);
        assert_eq!(set, vec![ids[0].clone()]);
        assert_eq!(primary.as_deref(), Some(ids[0].as_str()));
    }

    #[test]
    fn shift_click_selects_the_run_along_the_lane() {
        let (project, ids) = project(&[1, 1, 1, 1]);
        let lane = &project.tracks[0];
        let (set, _) = clicked(&[ids[3].clone()], Some(&ids[3]), &ids[1], lane, false, true);
        assert_eq!(set, vec![ids[3].clone(), ids[1].clone(), ids[2].clone()]);
    }

    #[test]
    fn a_plain_click_replaces_the_selection() {
        let (project, ids) = project(&[1, 1]);
        let (set, primary) = clicked(
            &ids,
            Some(&ids[0]),
            &ids[1],
            &project.tracks[0],
            false,
            false,
        );
        assert_eq!(set, vec![ids[1].clone()]);
        assert_eq!(primary.as_deref(), Some(ids[1].as_str()));
    }
}
