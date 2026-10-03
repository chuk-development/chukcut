//! When a proxy is worth making.
//!
//! "Is it 4K" is the rule everybody writes first and it is wrong in both
//! directions. H.264 at 1080p on this machine decodes inside the frame budget
//! and a proxy for it is pure waste — disk, wall clock, and a softer picture
//! for nothing. HEVC at 2160p does not come close, and neither does AV1 at
//! 1440p. An intra-only source at 4K is expensive to decode too, but a proxy
//! buys much less there, because the reason a proxy helps most is not
//! throughput: it is that a long-GOP file cannot be seeked into without
//! decoding forward from a keyframe, and an intra-only one can.
//!
//! So the rule has three inputs — **resolution, codec, and a measured decode
//! cost when one exists** — and it is a pure function of them, which is what
//! makes it testable. [`decide`] takes a [`SourceProfile`] and an optional
//! measurement and returns a [`Decision`] carrying the arithmetic it used, so a
//! log line or a tooltip can explain itself.
//!
//! ## The shape of it
//!
//! ```text
//!   budget    = 1000 / fps                     the whole frame, in ms
//!   allowance = budget × DECODE_SHARE × bonus  what decode may have of it
//!   cost      = measured, or megapixels × cost-per-megapixel for the codec
//!
//!   build a proxy  ⟺  the proxy is meaningfully smaller than the source
//!                     AND cost > allowance
//! ```
//!
//! `DECODE_SHARE` is 0.7 rather than 1.0 because decode is not the only thing
//! in the frame. `docs/STATUS.md` measures preview playback at ~13.7 ms per
//! frame against a 33.3 ms budget with decode at 7.2 ms of it, and the rest —
//! compositing, the readback, the JPEG — does not get cheaper because the
//! footage got heavier. It is 0.7 and not 0.5 because the JPEG encode runs on a
//! thread that overlaps compositing, so decode legitimately gets most of the
//! budget; what it cannot have is all of it, because the compositor's GPU stall
//! and the readback are serialised with it.
//!
//! Two rows of the table in this file's tests sit close to that line on
//! purpose, and they are the two arguments worth having: **HEVC at 1080p30**
//! (≈21 ms against a 23 ms allowance — iPhone footage, and proxying all of it
//! would be heavy-handed) and **H.264 at 1080p60** (≈10 ms against 12 ms — the
//! footage this project benchmarks with). Both are judged playable. Neither has
//! much margin, which is exactly why [`decide`] takes a measurement and lets it
//! win.
//!
//! `bonus` is [`INTRA_ALLOWANCE_BONUS`] for intra-only codecs and 1.0 for
//! everything else. It is the "an intra-only codec may not need one at all"
//! clause, and it is an allowance multiplier rather than a flat exemption
//! because ProRes 4444 at 4K is genuinely slow and does deserve a proxy — it is
//! only *scrubbing* that intra-only gets for free.
//!
//! ## Where the numbers come from
//!
//! The per-megapixel costs in [`cost_per_megapixel_ms`] are calibrated on the
//! Raptor Lake development machine against decode plus the swscale pass down to
//! preview size — which is what `media::provider` actually pays per frame, and
//! is **single-threaded**, because `media::decoder` never sets `thread_count`
//! and libavcodec's default is one. Measured 2026-07-26, best of two runs of
//! 240 frames, `ffmpeg -threads 1 -i F -vf scale=-2:1080,format=rgba -f null -`:
//!
//! | source | per frame | per source megapixel |
//! |---|---:|---:|
//! | H.264 1920×1080 (native RGBA) | 11.4 ms | 5.5 |
//! | H.264 3840×2160 | 40.9 ms | 4.9 |
//! | HEVC 3840×2160 | 56.1 ms | 6.8 |
//! | MJPEG 3840×2160 | 44.4 ms | 5.4 |
//!
//! The HEVC and MJPEG fixtures were synthesised at a lower bitrate than real
//! footage of that size, which understates them — a bitstream is work, and a
//! 100 Mbps camera original is more of it than a 0.5 Mbps transcode. The table
//! below therefore keeps HEVC at twice H.264 rather than at the 1.4× measured
//! here, which is the ratio the two codecs have at equal bitrate.
//!
//! These are estimates for machines we have not measured, and they are only
//! used when no measurement is available: pass a `measured` and it wins
//! outright.

use serde::{Deserialize, Serialize};

use crate::modules::media::MediaInfo;

/// The longest side a proxy is allowed to have.
///
/// 1280 rather than a fixed 720p because half the footage this editor sees is
/// portrait, and capping the *height* at 720 would turn a 2160×3840 phone clip
/// into a 405×720 proxy — a 2.7× upscale on a 1080×1920 canvas, which looks
/// like the editor broke. Capping the long side gives 720×1280 either way.
pub const PROXY_LONG_SIDE: u32 = 1280;

/// How much smaller the proxy has to be before it is worth having.
///
/// Below this the transcode costs more than it saves and the only thing that
/// changes is that the picture got softer. Expressed on the long side, so 1.4
/// means "a 1792-wide source is left alone, a 1920-wide one is not".
pub const MIN_SHRINK: f64 = 1.4;

/// The share of one frame's budget that decode may take before the machine is
/// judged unable to play the file.
pub const DECODE_SHARE: f64 = 0.7;

/// Extra allowance granted to intra-only sources.
///
/// Every frame is a keyframe, so a seek costs one frame rather than a run-up
/// from the last one, and the decoder's forward-window policy never has to
/// throw work away. Those are most of what a proxy buys, so an intra-only
/// source has to be twice as slow before it earns one.
pub const INTRA_ALLOWANCE_BONUS: f64 = 2.0;

/// Frame rates outside this range are clamped before the budget is computed.
///
/// A 1000 fps `r_frame_rate` on a variable-rate phone recording would otherwise
/// produce a one-millisecond budget and proxy everything; a 1 fps timelapse
/// would produce a one-second budget and proxy nothing.
const FPS_CLAMP: (f64, f64) = (15.0, 120.0);

/// What kind of random access the codec offers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CodecClass {
    /// Every frame stands alone. Seeking is a single decode.
    IntraOnly,
    /// Frames reference each other. Reaching frame *n* means decoding from the
    /// last keyframe before it.
    LongGop,
    /// Not a codec we have an opinion about. Treated as long-GOP and
    /// pessimistically expensive, because being wrong towards "build a proxy"
    /// costs disk and being wrong the other way costs playback.
    Unknown,
}

impl CodecClass {
    fn allowance_bonus(self) -> f64 {
        match self {
            CodecClass::IntraOnly => INTRA_ALLOWANCE_BONUS,
            _ => 1.0,
        }
    }
}

/// Classify an FFmpeg codec name.
///
/// The names are what `AVCodecDescriptor::name` returns, which is what
/// `media::probe` puts in `VideoStreamInfo::codec` — so `"h264"`, not
/// `"avc1"` and not `"libx264"`. Matching is on the whole name rather than a
/// prefix except where a family shares one (`prores_ks`, `dnxhr`).
pub fn classify(codec: &str) -> CodecClass {
    let codec = codec.trim().to_ascii_lowercase();
    const INTRA: &[&str] = &[
        "mjpeg",
        "mjpegb",
        "jpeg2000",
        "jpegls",
        "png",
        "tiff",
        "bmp",
        "qtrle",
        "rawvideo",
        "v210",
        "v308",
        "v410",
        "y41p",
        "yuv4",
        "huffyuv",
        "ffvhuff",
        "ffv1",
        "utvideo",
        "magicyuv",
        "dv",
        "cfhd",
        "sheervideo",
        "prores",
        "dnxhd",
        "avrp",
        "r210",
        "r10k",
    ];
    if INTRA
        .iter()
        .any(|name| codec == *name || codec.starts_with(name))
    {
        return CodecClass::IntraOnly;
    }
    const LONG_GOP: &[&str] = &[
        "h264",
        "hevc",
        "h265",
        "av1",
        "vp8",
        "vp9",
        "vc1",
        "mpeg1video",
        "mpeg2video",
        "mpeg4",
        "msmpeg4v1",
        "msmpeg4v2",
        "msmpeg4v3",
        "wmv1",
        "wmv2",
        "wmv3",
        "theora",
        "vvc",
        "h266",
    ];
    if LONG_GOP.iter().any(|name| codec == *name) {
        return CodecClass::LongGop;
    }
    CodecClass::Unknown
}

/// Milliseconds per megapixel of source, decoding to RGBA in software.
///
/// Calibrated on the development machine — see the module docs. The ordering
/// matters more than the absolute values: HEVC is roughly twice H.264 and AV1
/// roughly twice HEVC on any CPU decoder, and that ratio is what makes the rule
/// pick the right files on a machine we have never measured.
pub fn cost_per_megapixel_ms(codec: &str) -> f64 {
    match codec.trim().to_ascii_lowercase().as_str() {
        // Long-GOP, in rough order of how much work a frame is. H.264 is the
        // one that is measured; the rest are placed relative to it.
        "mpeg1video" | "mpeg2video" => 2.5,
        "mpeg4" | "msmpeg4v1" | "msmpeg4v2" | "msmpeg4v3" | "wmv1" | "wmv2" => 3.0,
        "theora" | "vc1" | "wmv3" => 4.0,
        "vp8" => 4.5,
        "h264" => 5.0,
        "vp9" => 7.5,
        "hevc" | "h265" => 10.0,
        "av1" => 18.0,
        "vvc" | "h266" => 26.0,
        // Intra-only. Cheap per pixel; the reason they are ever proxied is
        // sheer pixel count, not complexity.
        "rawvideo" | "v210" | "v308" | "v410" => 1.5,
        "huffyuv" | "ffvhuff" | "utvideo" | "magicyuv" | "qtrle" => 3.0,
        "dv" => 3.5,
        "dnxhd" => 4.5,
        "mjpeg" | "mjpegb" => 5.0,
        "cfhd" => 6.0,
        "prores" => 7.0,
        "jpeg2000" => 14.0,
        // Unknown. Pessimistic on purpose — assumed at least as expensive as
        // HEVC, because being wrong towards a proxy costs disk and being wrong
        // the other way costs playback. See `CodecClass::Unknown`.
        _ => 12.0,
    }
}

/// Everything the rule needs to know about a source file.
///
/// Dimensions are the *display* ones, i.e. with container rotation applied,
/// because that is the picture the user sees and the shape the proxy has to be.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SourceProfile {
    pub width: u32,
    pub height: u32,
    pub fps: f64,
    /// FFmpeg's short name for the codec: `"h264"`, `"hevc"`, `"prores"`.
    pub codec: String,
}

impl SourceProfile {
    pub fn new(width: u32, height: u32, fps: f64, codec: impl Into<String>) -> Self {
        Self {
            width,
            height,
            fps,
            codec: codec.into(),
        }
    }

    /// Read a profile out of a probe result. `None` when the file has no video.
    pub fn from_media_info(info: &MediaInfo) -> Option<Self> {
        let video = info.video.as_ref()?;
        Some(Self {
            width: video.display_width,
            height: video.display_height,
            fps: video.fps,
            codec: video.codec.clone(),
        })
    }

    pub fn class(&self) -> CodecClass {
        classify(&self.codec)
    }

    pub fn megapixels(&self) -> f64 {
        (self.width as f64 * self.height as f64) / 1_000_000.0
    }

    fn long_side(&self) -> u32 {
        self.width.max(self.height)
    }

    /// Whether a proxy would be meaningfully smaller than the source — the
    /// clause of [`decide`] that holds regardless of decode cost, and the only
    /// one the "Always" policy keeps.
    pub fn worth_shrinking(&self) -> bool {
        let (width, height) = self.proxy_size();
        self.long_side() as f64 / width.max(height).max(1) as f64 >= MIN_SHRINK
    }

    /// The size a proxy of this source would be: the long side capped at
    /// [`PROXY_LONG_SIDE`], aspect preserved, both dimensions even.
    ///
    /// Even because several swscale paths assume even chroma dimensions and
    /// produce a green edge column otherwise — the same reason
    /// `media::decoder::scaled_dimensions` does it — and because no hardware
    /// H.264 encoder will take an odd height.
    pub fn proxy_size(&self) -> (u32, u32) {
        let long = self.long_side().max(1);
        if long <= PROXY_LONG_SIDE {
            return (even(self.width), even(self.height));
        }
        let factor = PROXY_LONG_SIDE as f64 / long as f64;
        (
            even((self.width as f64 * factor).round() as u32),
            even((self.height as f64 * factor).round() as u32),
        )
    }

    /// One frame's whole budget in milliseconds, at this file's frame rate.
    pub fn frame_budget_ms(&self) -> f64 {
        let fps = if self.fps.is_finite() && self.fps > 0.0 {
            self.fps.clamp(FPS_CLAMP.0, FPS_CLAMP.1)
        } else {
            30.0
        };
        1000.0 / fps
    }

    /// What decode is allowed to cost before this file needs a proxy.
    pub fn decode_allowance_ms(&self) -> f64 {
        self.frame_budget_ms() * DECODE_SHARE * self.class().allowance_bonus()
    }

    /// The model's guess at what one frame costs to decode, in milliseconds.
    pub fn estimated_decode_ms(&self) -> f64 {
        self.megapixels() * cost_per_megapixel_ms(&self.codec)
    }
}

fn even(value: u32) -> u32 {
    let value = value.max(2);
    value - (value % 2)
}

/// The rule's answer, with the arithmetic that produced it.
///
/// Carrying the numbers rather than just the verdict is what lets the UI say
/// "HEVC at 3840×2160 costs about 99 ms a frame against a 17 ms budget"
/// instead of "recommended", and what lets a test assert on the reason.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Decision {
    /// Whether a proxy should be built.
    pub build: bool,
    /// One sentence, written for a person.
    pub reason: String,
    /// Milliseconds a frame is expected — or was measured — to cost.
    pub cost_ms: f64,
    /// What it was compared against.
    pub allowance_ms: f64,
    /// Whether `cost_ms` came from a measurement rather than the model.
    pub measured: bool,
    pub class: CodecClass,
    pub target_width: u32,
    pub target_height: u32,
}

/// Decide whether `profile` is worth a proxy.
///
/// `measured` is milliseconds per frame observed by actually decoding this
/// file. When present it replaces the model outright — a measurement of the
/// real thing beats a table every time, and the table exists because measuring
/// every file at import would cost more than it saves.
pub fn decide(profile: &SourceProfile, measured: Option<f64>) -> Decision {
    let (target_width, target_height) = profile.proxy_size();
    let class = profile.class();
    let allowance_ms = profile.decode_allowance_ms();
    let measured_ok = measured.filter(|ms| ms.is_finite() && *ms >= 0.0);
    let cost_ms = measured_ok.unwrap_or_else(|| profile.estimated_decode_ms());

    let mut decision = Decision {
        build: false,
        reason: String::new(),
        cost_ms,
        allowance_ms,
        measured: measured_ok.is_some(),
        class,
        target_width,
        target_height,
    };

    // Nothing to gain. Checked first because it is the one clause that is true
    // regardless of how expensive the file is: a proxy that is not smaller than
    // the source is a slower copy of it.
    if !profile.worth_shrinking() {
        decision.reason = format!(
            "{}×{} is already close to the {PROXY_LONG_SIDE}-pixel proxy size, \
             so a proxy would not be meaningfully smaller",
            profile.width, profile.height
        );
        return decision;
    }

    if cost_ms > allowance_ms {
        decision.build = true;
        decision.reason = format!(
            "{} at {}×{} {} about {:.0} ms a frame to decode, against the {:.0} ms \
             this file's {:.0} fps allows",
            profile.codec,
            profile.width,
            profile.height,
            if decision.measured {
                "measured"
            } else {
                "costs"
            },
            cost_ms,
            allowance_ms,
            profile.fps.max(0.0),
        );
    } else {
        decision.reason = format!(
            "{} at {}×{} {} about {:.0} ms a frame, inside the {:.0} ms budget — \
             the original plays back fine",
            profile.codec,
            profile.width,
            profile.height,
            if decision.measured {
                "measured"
            } else {
                "costs"
            },
            cost_ms,
            allowance_ms,
        );
    }

    decision
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One row of the table below.
    struct Case {
        what: &'static str,
        width: u32,
        height: u32,
        fps: f64,
        codec: &'static str,
        build: bool,
    }

    /// The rule, against every case anybody has argued about.
    ///
    /// This is the specification. If a change to the constants above flips a
    /// row here, the change is wrong or the row is — but one of them is, and
    /// the argument happens before the commit rather than after the release.
    #[test]
    fn the_rule_against_a_table_of_sources() {
        const CASES: &[Case] = &[
            // The two the brief names explicitly.
            Case {
                what: "H.264 1080p — the everyday case, plays fine",
                width: 1920,
                height: 1080,
                fps: 30.0,
                codec: "h264",
                build: false,
            },
            Case {
                what: "HEVC 4K — the case proxies exist for",
                width: 3840,
                height: 2160,
                fps: 30.0,
                codec: "hevc",
                build: true,
            },
            // Resolution alone is not the rule: same pixels, different codecs.
            Case {
                what: "H.264 4K at 30 — heavy but not hopeless",
                width: 3840,
                height: 2160,
                fps: 30.0,
                codec: "h264",
                build: true,
            },
            Case {
                what: "AV1 1440p — half the pixels of 4K, twice the codec",
                width: 2560,
                height: 1440,
                fps: 30.0,
                codec: "av1",
                build: true,
            },
            Case {
                what: "VP9 1080p — inside budget",
                width: 1920,
                height: 1080,
                fps: 30.0,
                codec: "vp9",
                build: false,
            },
            Case {
                what: "HEVC 1080p — inside budget at 30",
                width: 1920,
                height: 1080,
                fps: 30.0,
                codec: "hevc",
                build: false,
            },
            // Frame rate is part of the budget, so the same file at 60 is a
            // different answer. This is the clause a resolution-only rule
            // cannot express at all.
            Case {
                what: "HEVC 1080p at 60 — half the budget, now over it",
                width: 1920,
                height: 1080,
                fps: 60.0,
                codec: "hevc",
                build: true,
            },
            Case {
                what: "H.264 1080p at 60 — still inside",
                width: 1920,
                height: 1080,
                fps: 60.0,
                codec: "h264",
                build: false,
            },
            // Intra-only. Cheap per pixel and free to seek, so it takes real
            // size before one is worth building.
            Case {
                what: "ProRes 1080p — no proxy, obviously",
                width: 1920,
                height: 1080,
                fps: 30.0,
                codec: "prores",
                build: false,
            },
            Case {
                what: "ProRes 4K — intra, but 8 megapixels is 8 megapixels",
                width: 3840,
                height: 2160,
                fps: 30.0,
                codec: "prores",
                build: true,
            },
            Case {
                what: "DNxHD 1080p — cheapest of the intra family",
                width: 1920,
                height: 1080,
                fps: 30.0,
                codec: "dnxhd",
                build: false,
            },
            Case {
                what: "MJPEG 4K at 30 — intra bonus covers it",
                width: 3840,
                height: 2160,
                fps: 30.0,
                codec: "mjpeg",
                build: false,
            },
            Case {
                what: "MJPEG 4K at 60 — half the budget, no longer covered",
                width: 3840,
                height: 2160,
                fps: 60.0,
                codec: "mjpeg",
                build: true,
            },
            // Small sources are never proxied whatever the codec costs,
            // because the proxy would not be smaller.
            Case {
                what: "AV1 720p — expensive codec, nothing to shrink",
                width: 1280,
                height: 720,
                fps: 30.0,
                codec: "av1",
                build: false,
            },
            Case {
                what: "H.264 480p — nothing to shrink",
                width: 854,
                height: 480,
                fps: 30.0,
                codec: "h264",
                build: false,
            },
            // Portrait: the long side is the height, and the rule has to see
            // that. A height-capped rule would proxy nothing here.
            Case {
                what: "HEVC 2160×3840 portrait — 4K on its side",
                width: 2160,
                height: 3840,
                fps: 30.0,
                codec: "hevc",
                build: true,
            },
            Case {
                what: "H.264 1080×1920 portrait — the phone footage we test with",
                width: 1080,
                height: 1920,
                fps: 60.0,
                codec: "h264",
                build: false,
            },
            // 6K/8K: nothing decodes this in real time.
            Case {
                what: "H.264 8K",
                width: 7680,
                height: 4320,
                fps: 30.0,
                codec: "h264",
                build: true,
            },
            Case {
                what: "Raw 4K — trivially decoded, but 8 MP of memcpy",
                width: 3840,
                height: 2160,
                fps: 30.0,
                codec: "rawvideo",
                build: false,
            },
            // A codec we have never heard of is treated as expensive.
            Case {
                what: "Unknown codec at 4K",
                width: 3840,
                height: 2160,
                fps: 30.0,
                codec: "notacodec",
                build: true,
            },
            Case {
                what: "Unknown codec at 1080p",
                width: 1920,
                height: 1080,
                fps: 30.0,
                codec: "notacodec",
                build: true,
            },
        ];

        for case in CASES {
            let profile = SourceProfile::new(case.width, case.height, case.fps, case.codec);
            let decision = decide(&profile, None);
            assert_eq!(
                decision.build,
                case.build,
                "{}: expected build={} but got {} — {} ({:.1} ms against {:.1} ms)",
                case.what,
                case.build,
                decision.build,
                decision.reason,
                decision.cost_ms,
                decision.allowance_ms
            );
        }
    }

    #[test]
    fn a_measurement_overrules_the_model() {
        // H.264 at 1080p is inside budget by the table…
        let profile = SourceProfile::new(1920, 1080, 30.0, "h264");
        assert!(!decide(&profile, None).build);

        // …but this particular machine, or this particular file, is not.
        let measured = decide(&profile, Some(40.0));
        assert!(measured.build);
        assert!(measured.measured);
        assert_eq!(measured.cost_ms, 40.0);
        assert!(measured.reason.contains("measured"));

        // And the other way: a 4K HEVC file that turns out to be cheap.
        let heavy = SourceProfile::new(3840, 2160, 30.0, "hevc");
        assert!(decide(&heavy, None).build);
        assert!(!decide(&heavy, Some(3.0)).build);
    }

    #[test]
    fn a_nonsense_measurement_falls_back_to_the_model() {
        let profile = SourceProfile::new(3840, 2160, 30.0, "hevc");
        for bad in [f64::NAN, f64::INFINITY, -1.0] {
            let decision = decide(&profile, Some(bad));
            assert!(
                !decision.measured,
                "{bad} should not count as a measurement"
            );
            assert!(decision.build);
        }
    }

    #[test]
    fn shrink_is_measured_on_the_long_side_either_way_up() {
        let landscape = SourceProfile::new(3840, 2160, 30.0, "hevc");
        assert_eq!(landscape.proxy_size(), (1280, 720));
        let portrait = SourceProfile::new(2160, 3840, 30.0, "hevc");
        assert_eq!(portrait.proxy_size(), (720, 1280));
    }

    #[test]
    fn proxy_dimensions_are_always_even() {
        // 1440×1079 is not a real camera, but an odd number reaching a
        // hardware encoder is a real crash.
        for (w, h) in [(3841u32, 2161u32), (1999, 1001), (2701, 1519), (3, 5)] {
            let (pw, ph) = SourceProfile::new(w, h, 30.0, "h264").proxy_size();
            assert_eq!(pw % 2, 0, "{w}x{h} gave an odd proxy width {pw}");
            assert_eq!(ph % 2, 0, "{w}x{h} gave an odd proxy height {ph}");
            assert!(pw >= 2 && ph >= 2);
        }
    }

    #[test]
    fn a_source_already_small_is_never_proxied_however_expensive() {
        // Every codec, at a size where the proxy would be the same size.
        for codec in ["h264", "hevc", "av1", "prores", "notacodec"] {
            let profile = SourceProfile::new(1280, 720, 60.0, codec);
            let decision = decide(&profile, Some(1_000.0));
            assert!(!decision.build, "{codec} at 720p should never be proxied");
            assert!(decision.reason.contains("already close"));
        }
    }

    #[test]
    fn frame_rate_nonsense_does_not_decide_anything() {
        // `r_frame_rate` on a variable-rate phone recording is routinely 600
        // or 1000. Left unclamped that is a 1 ms budget and everything gets a
        // proxy; a 0 fps still image container would be the reverse.
        let mad = SourceProfile::new(1920, 1080, 1000.0, "h264");
        let sane = SourceProfile::new(1920, 1080, 120.0, "h264");
        assert_eq!(mad.frame_budget_ms(), sane.frame_budget_ms());

        let zero = SourceProfile::new(1920, 1080, 0.0, "h264");
        assert_eq!(zero.frame_budget_ms(), 1000.0 / 30.0);
        let nan = SourceProfile::new(1920, 1080, f64::NAN, "h264");
        assert_eq!(nan.frame_budget_ms(), 1000.0 / 30.0);
    }

    #[test]
    fn codec_classification_covers_the_families() {
        assert_eq!(classify("h264"), CodecClass::LongGop);
        assert_eq!(classify("hevc"), CodecClass::LongGop);
        assert_eq!(classify("av1"), CodecClass::LongGop);
        // Family prefixes: FFmpeg reports `prores` for the decoder but the
        // encoders are `prores_ks`/`prores_aw`, and DNxHR shows up as `dnxhd`.
        assert_eq!(classify("prores"), CodecClass::IntraOnly);
        assert_eq!(classify("prores_ks"), CodecClass::IntraOnly);
        assert_eq!(classify("dnxhd"), CodecClass::IntraOnly);
        assert_eq!(classify("mjpeg"), CodecClass::IntraOnly);
        assert_eq!(classify("ffv1"), CodecClass::IntraOnly);
        // Case and whitespace are not signal.
        assert_eq!(classify(" H264 "), CodecClass::LongGop);
        assert_eq!(classify("something-new"), CodecClass::Unknown);
    }

    #[test]
    fn the_cost_model_keeps_the_codecs_in_the_right_order() {
        // The absolute numbers are calibrated and will drift. The ordering is
        // a property of the codecs themselves and must not.
        let h264 = cost_per_megapixel_ms("h264");
        let hevc = cost_per_megapixel_ms("hevc");
        let av1 = cost_per_megapixel_ms("av1");
        let mpeg2 = cost_per_megapixel_ms("mpeg2video");
        assert!(mpeg2 < h264, "MPEG-2 must be cheaper than H.264");
        assert!(h264 < hevc, "H.264 must be cheaper than HEVC");
        assert!(hevc < av1, "HEVC must be cheaper than AV1");
        assert!(cost_per_megapixel_ms("rawvideo") < cost_per_megapixel_ms("mjpeg"));
        assert!(cost_per_megapixel_ms("notacodec") >= h264);
    }

    #[test]
    fn the_decision_explains_itself() {
        let decision = decide(&SourceProfile::new(3840, 2160, 30.0, "hevc"), None);
        assert!(decision.build);
        assert!(decision.reason.contains("hevc"));
        assert!(decision.reason.contains("3840"));
        assert_eq!((decision.target_width, decision.target_height), (1280, 720));
    }

    #[test]
    fn a_profile_comes_out_of_a_probe_display_side_up() {
        use crate::modules::media::VideoStreamInfo;
        let info = MediaInfo {
            path: "/x.mp4".into(),
            format: "mov,mp4".into(),
            duration: 1_000_000,
            file_size: 10,
            has_video: true,
            has_audio: false,
            audio: None,
            video: Some(VideoStreamInfo {
                index: 0,
                // Coded landscape, displayed portrait — the phone case.
                width: 3840,
                height: 2160,
                display_width: 2160,
                display_height: 3840,
                fps: 30.0,
                codec: "hevc".into(),
                rotation: 90,
                duration: 1_000_000,
            }),
        };
        let profile = SourceProfile::from_media_info(&info).expect("has video");
        assert_eq!((profile.width, profile.height), (2160, 3840));
        assert_eq!(profile.proxy_size(), (720, 1280));
    }
}
