//! Decoding real files through the real decoder.
//!
//! The unit tests in `media::decoder` cover the arithmetic — rotation of a
//! two-pixel buffer, scaled dimensions, timestamp normalisation. What they
//! cannot cover is whether asking a demuxer for the frame at 1.5 s actually
//! produces the frame that was 1.5 s into the file, because that depends on
//! seek behaviour, keyframe placement, B-frame reordering and the decoder's own
//! forward-decode policy all agreeing with each other.
//!
//! So every test here decodes a file that was generated to be *identifiable*:
//! the counter clip writes its own frame index into eight black/white stripes,
//! and [`support::read_counter_rgba`] reads it back. "Returned a frame" is not
//! an assertion; "returned frame 45" is.
//!
//! ## Why nearly every test runs twice
//!
//! There are two decoders behind `VideoDecoder` — libavcodec on the CPU and
//! VAAPI on the iGPU — and seek accuracy is what the whole preview design rests
//! on. Hardware decoders have a reputation for handling seeks differently, and
//! a suite that only covered the software path would let the hardware one drift
//! silently until somebody scrubbed a timeline and saw the wrong picture.
//!
//! So the cases below are written once and run through [`on_every_path`], which
//! executes them against every decoder this machine actually has. On a box with
//! no GPU that is one path and the suite still passes; the skip is printed, not
//! swallowed.

mod support;

use std::time::Instant;

use chukcut_engine::modules::media::decoder::Acceleration;
use chukcut_engine::modules::media::{hwdecode, probe, HwCodec, VideoDecoder};
use support::{
    assert_pixel_near, counter_frame_time, pixel, read_counter_rgba, COUNTER_FPS, COUNTER_FRAMES,
    COUNTER_HEIGHT, COUNTER_WIDTH, QUAD_BL, QUAD_BR, QUAD_TL, QUAD_TR,
};

// ---------------------------------------------------------------------------
// Running a case against every decoder this machine has
// ---------------------------------------------------------------------------

/// Every decode path available here, software first.
///
/// The fixtures are H.264, so H.264 is the question asked of the hardware. A
/// machine that decodes it gets both paths; one that does not gets software and
/// a printed reason, because a red suite that means "this box has no GPU"
/// trains people to ignore failures.
fn available_paths() -> Vec<Acceleration> {
    let mut paths = vec![Acceleration::Software];
    if hwdecode::supports(HwCodec::H264) {
        paths.push(Acceleration::Vaapi);
    } else {
        eprintln!(
            "{}: the hardware path is not exercised — this machine does not \
             hardware-decode H.264",
            support::test_name()
        );
    }
    paths
}

/// Run `body` once per decode path.
///
/// The label is passed through to every assertion message below, so a failure
/// says *which* decoder got it wrong rather than leaving that to be bisected.
fn on_every_path(body: impl Fn(Acceleration)) {
    for path in available_paths() {
        body(path);
    }
}

#[track_caller]
fn open(file: &std::path::Path, acceleration: Acceleration) -> VideoDecoder {
    VideoDecoder::open_with(file, acceleration)
        .unwrap_or_else(|e| panic!("opening {} as {acceleration:?} failed: {e}", file.display()))
}

/// The frame the decoder handed back, as its own counter says.
#[track_caller]
fn decode_counter(decoder: &mut VideoDecoder, at: i64) -> u64 {
    let frame = decoder
        .seek_and_decode(at)
        .unwrap_or_else(|e| panic!("decoding at {at} µs failed: {e}"));
    assert_eq!(
        frame.data.len(),
        (frame.width * frame.height * 4) as usize,
        "the decoder must hand back a tight RGBA buffer"
    );
    read_counter_rgba(&frame.data, frame.width, frame.height)
        .unwrap_or_else(|| panic!("the frame decoded at {at} µs does not carry a readable counter"))
}

/// Halfway into frame `n`, which is where a playhead normally sits.
fn mid_frame(n: u64) -> i64 {
    counter_frame_time(n) + (1_000_000.0 / COUNTER_FPS / 2.0) as i64
}

// ---------------------------------------------------------------------------
// Landing on the right frame
// ---------------------------------------------------------------------------

#[test]
fn seeking_to_a_frame_boundary_returns_that_frame() {
    let media = require_media!();
    on_every_path(|path| {
        let mut decoder = open(&media.counter, path);

        // Spread across the file so some land on a keyframe (every 30th) and
        // some are deep inside a GOP.
        for n in [0u64, 1, 5, 29, 30, 31, 59, 60, 89, 90, 119] {
            assert_eq!(
                decode_counter(&mut decoder, counter_frame_time(n)),
                n,
                "{path:?}: asking for the instant frame {n} starts at must return frame {n}"
            );
        }
    });
}

#[test]
fn seeking_inside_a_frame_returns_that_frame_and_not_the_next_one() {
    let media = require_media!();
    on_every_path(|path| {
        let mut decoder = open(&media.counter, path);

        for n in [0u64, 7, 30, 61, 118] {
            assert_eq!(
                decode_counter(&mut decoder, mid_frame(n)),
                n,
                "{path:?}: mid frame {n}"
            );
            // One microsecond before the next frame still belongs to this one:
            // frames are half-open, exactly like segments.
            assert_eq!(
                decode_counter(&mut decoder, counter_frame_time(n + 1) - 1),
                n,
                "{path:?}: the last microsecond of frame {n}"
            );
        }
    });
}

/// Regression: nudging the playhead forward by less than a frame used to
/// return the *next* frame.
///
/// The decoder keeps the frame it overshot on in `pending` so a sequential read
/// does not lose it. It did not keep the frame it had just handed out, so a
/// request landing between the two had nothing to answer with except the
/// overshoot — and the preview showed the picture one frame ahead of the
/// playhead for every sub-frame scrub.
#[test]
fn nudging_the_playhead_forward_inside_a_frame_stays_on_that_frame() {
    let media = require_media!();
    on_every_path(|path| {
        let mut decoder = open(&media.counter, path);

        for n in [0u64, 1, 44] {
            let start = counter_frame_time(n);
            // Land on the frame first, which is what leaves the next one
            // pending.
            assert_eq!(decode_counter(&mut decoder, start), n, "{path:?}");
            // Then creep forward through it. Every one of these instants is
            // still inside frame `n`.
            for offset in [1, 100, 5_000, 16_000, 33_000] {
                assert_eq!(
                    decode_counter(&mut decoder, start + offset),
                    n,
                    "{path:?}: {offset} µs into frame {n} is still frame {n}"
                );
            }
            // And asking for the same instant twice must not advance either.
            assert_eq!(decode_counter(&mut decoder, start), n, "{path:?}");
            assert_eq!(decode_counter(&mut decoder, start), n, "{path:?}");
        }
    });
}

#[test]
fn seeking_backwards_lands_on_the_right_frame() {
    let media = require_media!();
    on_every_path(|path| {
        let mut decoder = open(&media.counter, path);

        // Walk to the end, then jump back over several keyframes at a time. A
        // decoder that only knows how to go forwards answers all of these with
        // the last frame it saw.
        decode_counter(&mut decoder, mid_frame(115));
        for n in [90u64, 61, 45, 30, 12, 0] {
            assert_eq!(
                decode_counter(&mut decoder, mid_frame(n)),
                n,
                "{path:?}: back to {n}"
            );
        }
    });
}

#[test]
fn seeking_forwards_over_a_long_gap_lands_on_the_right_frame() {
    let media = require_media!();
    on_every_path(|path| {
        let mut decoder = open(&media.counter, path);

        // Each hop is well past the forward-decode window, so every one takes
        // the seek path rather than decoding through.
        for n in [0u64, 70, 119] {
            assert_eq!(
                decode_counter(&mut decoder, mid_frame(n)),
                n,
                "{path:?}: forward to {n}"
            );
        }
    });
}

#[test]
fn reading_sequentially_returns_every_frame_in_order() {
    let media = require_media!();
    on_every_path(|path| {
        let mut decoder = open(&media.counter, path);

        // The `pending` frame the decoder keeps after overshooting is what this
        // catches: without it a sequential walk drops every frame it decoded
        // past, and playback shows every other frame.
        //
        // On the hardware path it catches a second thing: every frame held in
        // `last` and `pending` is a surface out of a fixed pool, so a leak
        // shows up here as the decoder failing part-way through the file.
        for n in 0..COUNTER_FRAMES {
            assert_eq!(
                decode_counter(&mut decoder, mid_frame(n)),
                n,
                "{path:?}: sequential read reached the wrong frame at {n}"
            );
        }
    });
}

#[test]
fn seeking_to_exactly_zero_returns_the_first_frame() {
    let media = require_media!();
    on_every_path(|path| {
        let mut decoder = open(&media.counter, path);

        assert_eq!(decode_counter(&mut decoder, 0), 0, "{path:?}");
        // And still does after the decoder has been somewhere else, which is
        // the case that needs a real rewind rather than "we are already there".
        decode_counter(&mut decoder, mid_frame(100));
        assert_eq!(decode_counter(&mut decoder, 0), 0, "{path:?}");
        // A negative playhead is clamped rather than refused.
        assert_eq!(decode_counter(&mut decoder, -500_000), 0, "{path:?}");
    });
}

#[test]
fn seeking_past_the_end_returns_the_last_frame_rather_than_failing() {
    let media = require_media!();
    on_every_path(|path| {
        let mut decoder = open(&media.counter, path);

        let last = COUNTER_FRAMES - 1;
        // A playhead parked past the end of a clip is routine — the timeline is
        // longer than the media on it — and must show the last frame, not an
        // error and not a blank viewer.
        assert_eq!(decode_counter(&mut decoder, 10_000_000), last, "{path:?}");
        // Asking again from that parked position must not lose it: the decoder
        // has already drained its demuxer and has to rewind to answer.
        assert_eq!(decode_counter(&mut decoder, 10_000_000), last, "{path:?}");
        assert_eq!(decode_counter(&mut decoder, 60_000_000), last, "{path:?}");
        // And it can still come back.
        assert_eq!(decode_counter(&mut decoder, mid_frame(10)), 10, "{path:?}");
    });
}

#[test]
fn a_stream_whose_first_timestamp_is_not_zero_still_starts_at_zero() {
    let media = require_media!();

    // MPEG-TS puts the first frame at 1.466 s. "Zero" has to keep meaning
    // "the start of the file", or every clip trimmed from a transport stream
    // is offset by a second and a half.
    let info = probe(&media.counter_late_start).expect("probe the transport stream");
    assert!(
        info.duration >= 3_900_000,
        "the fixture should be about four seconds, got {} µs",
        info.duration
    );

    on_every_path(|path| {
        let mut decoder = open(&media.counter_late_start, path);
        assert_eq!(decode_counter(&mut decoder, 0), 0, "{path:?}");
        for n in [1u64, 30, 75, 119] {
            assert_eq!(
                decode_counter(&mut decoder, mid_frame(n)),
                n,
                "{path:?}: frame {n} of a stream that starts late"
            );
        }
    });
}

/// Regression: an index-less container answered most direct seeks with the
/// wrong frame.
///
/// MPEG-TS carries no index, so `av_seek_frame` bisects the byte stream and
/// lands wherever the search converged — routinely *past* the instant asked
/// for. Decoding forward from there can only get further away, so every seek
/// beyond the forward-decode window came back with the next keyframe, and
/// anything past the last keyframe came back as an error. Every frame of every
/// container has to be reachable by asking for it directly.
#[test]
fn every_frame_is_reachable_by_seeking_straight_to_it() {
    let media = require_media!();

    on_every_path(|path| {
        // Both fixtures run at a constant 30 fps; the variable-rate one has its
        // own test because a frame index there is not a time.
        for (label, file) in [
            ("mp4", &media.counter),
            ("mpeg-ts", &media.counter_late_start),
        ] {
            // A fresh decoder per frame, so nothing is answered out of the
            // state left by the previous request — this is the cold-seek path a
            // scrub after an edit and the exporter's first frame both take. The
            // indices straddle keyframes (every sixteenth) and reach past the
            // last one, which is where this failed outright rather than merely
            // returning the wrong picture.
            for n in [0u64, 33, 63, 64, 80, 95, 96, 111, 112, 115, 119] {
                let mut decoder = open(file, path);
                assert_eq!(
                    decode_counter(&mut decoder, mid_frame(n)),
                    n,
                    "{path:?} {label}: a cold seek to frame {n}"
                );
            }
        }
    });
}

#[test]
fn the_reported_timestamp_is_the_frame_that_came_back() {
    let media = require_media!();
    on_every_path(|path| {
        let mut decoder = open(&media.counter, path);

        for n in [0u64, 17, 64, 119] {
            let frame = decoder.seek_and_decode(mid_frame(n)).expect("decode");
            let counter = read_counter_rgba(&frame.data, frame.width, frame.height)
                .expect("readable counter");
            assert_eq!(counter, n, "{path:?}");
            // The caller needs the pts to know which frame it is looking at, so
            // it has to describe the frame that was returned and not the
            // request.
            let want = counter_frame_time(n);
            assert!(
                (frame.pts - want).abs() <= 1_000,
                "{path:?}: frame {n} reported pts {} µs, expected about {want}",
                frame.pts
            );
        }
    });
}

// ---------------------------------------------------------------------------
// Variable frame rate
// ---------------------------------------------------------------------------

#[test]
fn a_variable_frame_rate_file_returns_the_frame_its_own_timestamps_name() {
    let media = require_media!();
    on_every_path(|path| {
        let mut decoder = open(&media.counter_vfr, path);

        // The fixture runs at 30 fps for a second and 15 fps after it, so no
        // single interval describes the file and a decoder that assumes one
        // drifts.
        //
        // Sweep the playhead across it in steps smaller than any frame,
        // recording which frame is on screen. That needs no knowledge of where
        // the frame boundaries are, which is the point: the assertions below
        // are the ones that hold whatever the rate did.
        let mut sweep: Vec<(i64, u64)> = Vec::new();
        let mut at = 0i64;
        while at < 2_400_000 {
            let frame = decoder.seek_and_decode(at).expect("decode");
            let counter = read_counter_rgba(&frame.data, frame.width, frame.height)
                .expect("readable counter");
            sweep.push((frame.pts, counter));
            at += 10_000;
        }

        assert!(
            sweep
                .windows(2)
                .all(|w| w[1].0 >= w[0].0 && w[1].1 >= w[0].1),
            "{path:?}: sweeping the playhead forwards must never go back a frame"
        );
        for pair in sweep.windows(2) {
            let ((pts, counter), (next_pts, next_counter)) = (pair[0], pair[1]);
            if next_pts != pts {
                assert_eq!(
                    next_counter,
                    counter + 1,
                    "{path:?}: the playhead crossed from {pts} µs to {next_pts} µs and \
                     skipped a frame"
                );
            } else {
                assert_eq!(
                    next_counter, counter,
                    "{path:?}: the same timestamp is the same frame"
                );
            }
        }

        // Distinct frames, in order. The rate really does change across them,
        // or this fixture is not testing anything.
        let mut frames: Vec<(i64, u64)> = sweep.clone();
        frames.dedup_by_key(|(_, counter)| *counter);
        assert!(
            frames.len() > 30,
            "{path:?}: only saw {} frames",
            frames.len()
        );
        let early = frames[5].0 - frames[4].0;
        let late = frames[frames.len() - 2].0 - frames[frames.len() - 3].0;
        assert!(
            late > early + 10_000,
            "{path:?}: the fixture is not variable rate: {early} µs then {late} µs"
        );

        // Now seek back to every one of those timestamps in reverse. A decoder
        // that assumes a fixed interval computes the wrong frame index here and
        // hands back its neighbour.
        for (pts, counter) in frames.iter().rev() {
            let frame = decoder.seek_and_decode(*pts).expect("decode");
            let got = read_counter_rgba(&frame.data, frame.width, frame.height)
                .expect("readable counter");
            assert_eq!(
                got, *counter,
                "{path:?}: seeking back to {pts} µs returned frame {got}, not {counter}"
            );
        }
    });
}

// ---------------------------------------------------------------------------
// Rotation
// ---------------------------------------------------------------------------

/// The four corner colours of a decoded frame, sampled well inside each
/// quadrant so no sample lands on the boundary.
fn corners(data: &[u8], width: u32, height: u32) -> [[u8; 4]; 4] {
    let (x0, x1) = (width / 4, width * 3 / 4);
    let (y0, y1) = (height / 4, height * 3 / 4);
    [
        pixel(data, width, x0, y0),
        pixel(data, width, x1, y0),
        pixel(data, width, x0, y1),
        pixel(data, width, x1, y1),
    ]
}

fn opaque(rgb: [u8; 3]) -> [u8; 4] {
    [rgb[0], rgb[1], rgb[2], 255]
}

#[test]
fn an_unrotated_video_decodes_the_way_it_was_encoded() {
    let media = require_media!();
    on_every_path(|path| {
        let mut decoder = open(&media.quadrants, path);
        let frame = decoder.seek_and_decode(500_000).expect("decode");

        assert_eq!((frame.width, frame.height), (320, 240), "{path:?}");
        assert_eq!(decoder.rotation(), 0, "{path:?}");
        let [tl, tr, bl, br] = corners(&frame.data, frame.width, frame.height);
        assert_pixel_near(tl, opaque(QUAD_TL), 12, "top left");
        assert_pixel_near(tr, opaque(QUAD_TR), 12, "top right");
        assert_pixel_near(bl, opaque(QUAD_BL), 12, "bottom left");
        assert_pixel_near(br, opaque(QUAD_BR), 12, "bottom right");
    });
}

#[test]
fn a_quarter_turned_video_decodes_upright_with_swapped_dimensions() {
    let media = require_media!();
    on_every_path(|path| {
        let mut decoder = open(&media.quadrants_rot90, path);

        assert_eq!(
            decoder.rotation(),
            90,
            "{path:?}: the file is tagged to display a quarter turn clockwise"
        );
        assert_eq!(
            decoder.output_size(),
            (240, 320),
            "{path:?}: a rotated frame is reported at its display size, not its coded size"
        );

        let frame = decoder.seek_and_decode(500_000).expect("decode");
        assert_eq!((frame.width, frame.height), (240, 320), "{path:?}");
        assert_eq!(frame.data.len(), (240 * 320 * 4) as usize, "{path:?}");

        // Turning the coded picture a quarter clockwise moves its bottom-left
        // corner to the top left. This is what `ffmpeg -autorotate` produces
        // from the same file, which is the definition worth matching.
        let [tl, tr, bl, br] = corners(&frame.data, frame.width, frame.height);
        assert_pixel_near(tl, opaque(QUAD_BL), 12, "displayed top left");
        assert_pixel_near(tr, opaque(QUAD_TL), 12, "displayed top right");
        assert_pixel_near(bl, opaque(QUAD_BR), 12, "displayed bottom left");
        assert_pixel_near(br, opaque(QUAD_TR), 12, "displayed bottom right");
    });
}

#[test]
fn a_three_quarter_turned_video_decodes_upright_too() {
    let media = require_media!();
    on_every_path(|path| {
        let mut decoder = open(&media.quadrants_rot270, path);

        assert_eq!(decoder.rotation(), 270, "{path:?}");
        assert_eq!(decoder.output_size(), (240, 320), "{path:?}");

        let frame = decoder.seek_and_decode(500_000).expect("decode");
        let [tl, tr, bl, br] = corners(&frame.data, frame.width, frame.height);
        assert_pixel_near(tl, opaque(QUAD_TR), 12, "displayed top left");
        assert_pixel_near(tr, opaque(QUAD_BR), 12, "displayed top right");
        assert_pixel_near(bl, opaque(QUAD_TL), 12, "displayed bottom left");
        assert_pixel_near(br, opaque(QUAD_BL), 12, "displayed bottom right");
    });
}

#[test]
fn an_upside_down_video_keeps_its_dimensions_and_mirrors_both_axes() {
    let media = require_media!();
    on_every_path(|path| {
        let mut decoder = open(&media.quadrants_rot180, path);

        assert_eq!(decoder.rotation(), 180, "{path:?}");
        assert_eq!(
            decoder.output_size(),
            (320, 240),
            "{path:?}: a half turn does not swap the axes"
        );

        let frame = decoder.seek_and_decode(500_000).expect("decode");
        let [tl, tr, bl, br] = corners(&frame.data, frame.width, frame.height);
        assert_pixel_near(tl, opaque(QUAD_BR), 12, "displayed top left");
        assert_pixel_near(tr, opaque(QUAD_BL), 12, "displayed top right");
        assert_pixel_near(bl, opaque(QUAD_TR), 12, "displayed bottom left");
        assert_pixel_near(br, opaque(QUAD_TL), 12, "displayed bottom right");
    });
}

#[test]
fn a_portrait_file_keeps_its_shape_through_probe_and_decode() {
    let media = require_media!();

    let info = probe(&media.solid_green_portrait).expect("probe");
    let video = info.video.expect("the fixture has a video stream");
    assert_eq!((video.width, video.height), (240, 320));
    assert_eq!((video.display_width, video.display_height), (240, 320));
    assert_eq!(video.rotation, 0);

    on_every_path(|path| {
        let mut decoder = open(&media.solid_green_portrait, path);
        let frame = decoder.seek_and_decode(500_000).expect("decode");
        assert_eq!((frame.width, frame.height), (240, 320), "{path:?}");
        assert_pixel_near(
            pixel(&frame.data, frame.width, 120, 160),
            [0, 255, 0, 255],
            12,
            "the middle of a solid green clip",
        );
    });
}

// ---------------------------------------------------------------------------
// Scaled decoding
// ---------------------------------------------------------------------------

#[test]
fn scaled_decoding_constrains_the_display_height_and_still_reads_back() {
    let media = require_media!();
    on_every_path(|path| {
        let mut decoder =
            VideoDecoder::open_scaled_with(&media.counter, 120, path).expect("open scaled");
        assert_eq!(decoder.output_size(), (160, 120), "{path:?}");
        let frame = decoder.seek_and_decode(mid_frame(42)).expect("decode");
        assert_eq!((frame.width, frame.height), (160, 120), "{path:?}");
        // The counter stripes are still 20 pixels wide at half size, so the
        // frame number survives the downscale — which is the point of scaling
        // during colour conversion rather than after it.
        assert_eq!(
            read_counter_rgba(&frame.data, frame.width, frame.height),
            Some(42),
            "{path:?}"
        );
    });
}

#[test]
fn a_rotated_file_scales_to_the_display_height_it_was_asked_for() {
    let media = require_media!();
    on_every_path(|path| {
        // Coded 320x240, displayed 240x320. Asking for 160 display pixels tall
        // has to constrain the coded *width*, or the caller gets a frame twice
        // the size it asked for.
        let decoder =
            VideoDecoder::open_scaled_with(&media.quadrants_rot90, 160, path).expect("open scaled");
        let (width, height) = decoder.output_size();
        assert_eq!(height, 160, "{path:?}: the target is a display height");
        assert_eq!(width, 120, "{path:?}: and the aspect ratio is preserved");
    });
}

#[test]
fn scaling_never_upscales_a_small_source() {
    let media = require_media!();
    on_every_path(|path| {
        let decoder =
            VideoDecoder::open_scaled_with(&media.counter, 4000, path).expect("open scaled");
        assert_eq!(
            decoder.output_size(),
            (COUNTER_WIDTH, COUNTER_HEIGHT),
            "{path:?}"
        );
    });
}

// ---------------------------------------------------------------------------
// Files that are not video
// ---------------------------------------------------------------------------

#[test]
fn an_audio_only_file_probes_as_audio_and_refuses_to_open_as_video() {
    let media = require_media!();

    let info = probe(&media.audio_only).expect("probe the audio file");
    assert!(!info.has_video, "the fixture has no video stream");
    assert!(info.has_audio);
    let audio = info.audio.expect("audio stream info");
    assert_eq!(audio.sample_rate, 48_000);
    assert!(info.duration >= 1_900_000, "about two seconds");

    // Both paths have to fail the same way. A hardware decoder that produced a
    // different error here would leak the acceleration choice into a message
    // the user reads.
    for path in [
        Acceleration::Software,
        Acceleration::Auto,
        Acceleration::Vaapi,
    ] {
        let error = VideoDecoder::open_with(&media.audio_only, path)
            .err()
            .expect("a decoder cannot be opened on a file with no video");
        let message = error.to_string();
        assert!(
            message.contains("no video stream"),
            "{path:?}: the error must say what is wrong, got: {message}"
        );
    }
}

#[test]
fn a_video_without_audio_says_so_and_one_with_it_says_so_too() {
    let media = require_media!();

    let silent = probe(&media.counter).expect("probe");
    assert!(silent.has_video);
    assert!(!silent.has_audio, "the counter fixture carries no audio");

    let noisy = probe(&media.counter_with_audio).expect("probe");
    assert!(noisy.has_video && noisy.has_audio);
    assert_eq!(noisy.audio.expect("audio stream").sample_rate, 48_000);
}

#[test]
fn opening_a_file_that_is_not_there_names_the_file() {
    let missing = std::path::Path::new("/nonexistent/definitely-not-a-video.mp4");
    for path in [
        Acceleration::Software,
        Acceleration::Auto,
        Acceleration::Vaapi,
    ] {
        let error = VideoDecoder::open_with(missing, path)
            .err()
            .expect("no such file");
        let message = error.to_string();
        assert!(
            message.contains("definitely-not-a-video.mp4"),
            "{path:?}: the error is shown to the user unchanged, got: {message}"
        );
    }
}

// ---------------------------------------------------------------------------
// The property that makes playback possible
// ---------------------------------------------------------------------------

/// Decode `frames` in the given order, returning how long it took.
///
/// On the hardware path the frames are *mapped* rather than converted to RGBA.
/// That is not a shortcut, it is the only way to measure what these two tests
/// are about. The download out of GPU memory is a fixed per-frame cost of
/// several milliseconds that is identical whether the frame was reached by
/// decoding forward or by seeking, so leaving it in swamps the very difference
/// being asserted on — the ratio collapses towards one for a reason that has
/// nothing to do with the seek policy. `seek_and_map` reaches the frame by the
/// same `locate` and stops there.
fn time_walk(decoder: &mut VideoDecoder, order: &[u64], path: Acceleration) -> std::time::Duration {
    if path == Acceleration::Software {
        let started = Instant::now();
        for n in order {
            let got = decode_counter(decoder, mid_frame(*n));
            assert_eq!(got, *n, "the timed walk decoded the wrong frame");
        }
        return started.elapsed();
    }

    let started = Instant::now();
    for n in order {
        let mapped = decoder
            .seek_and_map(mid_frame(*n))
            .unwrap_or_else(|e| panic!("mapping frame {n} failed: {e}"));
        // A mapped surface carries no readable counter, so the timestamp is
        // what proves the walk reached the frames it claims to have.
        let want = counter_frame_time(*n);
        assert!(
            (mapped.pts - want).abs() <= 1_000,
            "the timed walk reached {} µs, expected about {want}",
            mapped.pts
        );
    }
    started.elapsed()
}

#[test]
fn decoding_sequentially_is_dramatically_cheaper_than_seeking_to_each_frame() {
    let media = require_media!();

    on_every_path(|path| {
        let forwards: Vec<u64> = (0..60).collect();
        let backwards: Vec<u64> = (0..60).rev().collect();

        // Best of several runs on each side rather than one run each.
        //
        // These walks take single-digit milliseconds, which is short enough that
        // a scheduler hiccup on a loaded machine can triple one of them — and a
        // test that fails because something else was compiling is a test people
        // learn to rerun. Interference can only ever make a run *slower*, so the
        // fastest of a few is the honest figure, and it makes the comparison
        // stable without weakening it.
        let best = |order: &[u64]| {
            (0..4)
                .map(|_| {
                    let mut decoder = open(&media.counter, path);
                    time_walk(&mut decoder, order, path)
                })
                .min()
                .expect("four rounds")
        };

        let sequential = best(&forwards);
        let random = best(&backwards);

        // Exactly the same sixty frames, in the opposite order. Every backwards
        // step is behind the decoder's position, so each one costs a seek to
        // the preceding keyframe plus a decode through the whole GOP; every
        // forwards step costs one frame. Measured on this machine the gap is
        // about seven times on the software path; the assertion is deliberately
        // looser than that, because what it exists to catch is sequential reads
        // starting to seek — at which point the ratio collapses to one and
        // playback silently becomes a slideshow. It holds on both paths because
        // `time_walk` measures the same thing on both — see its comment.
        assert!(
            random > sequential * 3,
            "{path:?}: sequential decode took {sequential:?} and random access {random:?}; \
             sequential reads appear to be seeking"
        );
    });
}

#[test]
fn a_long_forward_jump_seeks_instead_of_decoding_through_everything_in_between() {
    let media = require_media!();

    on_every_path(|path| {
        // The policy is "decode forward within the current GOP, seek beyond
        // it", expressed as a half-second window. The other test here covers the
        // first half — a decoder that always seeks cannot play back. This covers
        // the second: a decoder that *never* seeks stays fast sequentially but
        // has to decode a hundred frames to answer a three-second jump, and
        // scrubbing a long timeline becomes unusable.
        //
        // Both figures are the total of twenty measurements rather than one, so
        // a single scheduling hiccup cannot decide the outcome.
        let mut steps = std::time::Duration::ZERO;
        let mut jumps = std::time::Duration::ZERO;

        for round in 0..20u64 {
            let mut decoder = open(&media.counter, path);
            decode_counter(&mut decoder, mid_frame(round));
            let started = Instant::now();
            assert_eq!(
                decode_counter(&mut decoder, mid_frame(round + 1)),
                round + 1,
                "{path:?}"
            );
            steps += started.elapsed();

            let mut decoder = open(&media.counter, path);
            decode_counter(&mut decoder, mid_frame(round));
            let started = Instant::now();
            // Well past the forward-decode window, so this one has to seek
            // rather than decode the ninety frames in between.
            assert_eq!(
                decode_counter(&mut decoder, mid_frame(round + 90)),
                round + 90,
                "{path:?}"
            );
            jumps += started.elapsed();
        }

        // A jump costs a seek plus at most one GOP; decoding through would cost
        // ninety frames, which is six GOPs of this fixture. The bound is loose
        // because what is being ruled out is an order of magnitude, not a
        // factor.
        assert!(
            jumps < steps * 40,
            "{path:?}: one-frame steps took {steps:?} in total and three-second jumps \
             {jumps:?}; a long jump appears to be decoding through rather than seeking"
        );
    });
}

// ---------------------------------------------------------------------------
// Choosing a decoder
// ---------------------------------------------------------------------------

#[test]
fn capability_detection_answers_with_a_reason_and_never_panics() {
    // Nothing here asserts that this machine has a GPU; what it asserts is the
    // invariant `export::hwaccel` also holds — a codec is either usable or says
    // why not, never neither and never both.
    let capabilities = hwdecode::capabilities();
    assert_eq!(
        capabilities.len(),
        HwCodec::ALL.len(),
        "every codec we care about has to be answered for"
    );
    for support in capabilities {
        assert_eq!(
            support.note.is_some(),
            !support.usable,
            "{:?} said usable={} note={:?}",
            support.codec,
            support.usable,
            support.note
        );
        assert!(!support.decoder_name.is_empty());
    }
}

#[test]
fn a_codec_the_gpu_cannot_decode_still_opens_and_still_decodes_correctly() {
    let media = require_media!();

    // MPEG-4 Part 2 is not on the list of codecs this build hardware-decodes,
    // so `Auto` has to notice and fall back silently. This is the promise the
    // whole feature rests on: turning hardware decode on must not make any file
    // stop opening.
    let mut decoder = VideoDecoder::open_with(&media.counter_mpeg4, Acceleration::Auto)
        .expect("an unaccelerated codec still opens under Auto");

    for n in [0u64, 7, 45, 100] {
        assert_eq!(decode_counter(&mut decoder, mid_frame(n)), n);
    }
    assert!(
        !decoder.is_hardware(),
        "MPEG-4 Part 2 is not decoded on this GPU; claiming otherwise would be a lie"
    );
    assert_eq!(decoder.acceleration(), Acceleration::Software);
    assert!(decoder.device_node().is_none());
}

#[test]
fn asking_for_hardware_that_is_not_there_is_an_error_rather_than_a_silent_downgrade() {
    let media = require_media!();

    // `Vaapi` is the strict spelling: a caller that uses it wants to know. This
    // is what makes the fallback in `Auto` a decision rather than an accident.
    let opened = VideoDecoder::open_with(&media.counter_mpeg4, Acceleration::Vaapi);
    let error = opened
        .err()
        .expect("MPEG-4 Part 2 cannot be decoded on this GPU");
    assert!(
        error.to_string().contains("hardware decode is unavailable"),
        "got: {error}"
    );
}

#[test]
fn the_decoder_reports_which_path_it_actually_took() {
    let media = require_media!();
    on_every_path(|path| {
        let mut decoder = open(&media.counter, path);
        // Before the first frame there is nothing to report but the request:
        // `get_format` has not run, so whether the accelerator initialised is
        // genuinely unknown.
        assert_eq!(
            decoder.acceleration(),
            path,
            "{path:?}: before the first frame"
        );

        decode_counter(&mut decoder, mid_frame(3));
        match path {
            Acceleration::Software => {
                assert!(!decoder.is_hardware());
                assert_eq!(decoder.acceleration(), Acceleration::Software);
                assert!(decoder.device_node().is_none());
            }
            _ => {
                assert!(
                    decoder.is_hardware(),
                    "{path:?}: H.264 probed as usable here"
                );
                assert_eq!(decoder.acceleration(), Acceleration::Vaapi);
                assert!(decoder.device_node().is_some());
            }
        }
    });
}

#[test]
fn hardware_and_software_decode_the_same_picture() {
    let media = require_media!();
    if !hwdecode::supports(HwCodec::H264) {
        eprintln!(
            "skipping {}: no hardware H.264 decode",
            support::test_name()
        );
        return;
    }

    // Two decoders, two vendors' idea of what the bitstream means, one picture.
    // The tolerance is what a chroma upsample and an sRGB round trip cost, not
    // a licence for the hardware to be roughly right: a chroma swap, a
    // range mismatch or an off-by-one frame all blow through it by a wide
    // margin. The quadrant fixture is chosen because it has saturated colour in
    // known places, which is exactly what a wrong colour matrix ruins.
    let mut software = open(&media.quadrants, Acceleration::Software);
    let mut hardware = open(&media.quadrants, Acceleration::Vaapi);

    let a = software.seek_and_decode(500_000).expect("software decode");
    let b = hardware.seek_and_decode(500_000).expect("hardware decode");
    assert_eq!((a.width, a.height), (b.width, b.height));
    assert_eq!(a.pts, b.pts, "the two paths must land on the same frame");

    let worst = a
        .data
        .iter()
        .zip(b.data.iter())
        .map(|(x, y)| (*x as i32 - *y as i32).abs())
        .max()
        .unwrap_or(0);
    let mean = a
        .data
        .iter()
        .zip(b.data.iter())
        .map(|(x, y)| (*x as i32 - *y as i32).unsigned_abs() as u64)
        .sum::<u64>() as f64
        / a.data.len() as f64;
    assert!(
        mean < 2.0,
        "the two decoders disagree by {mean:.2} on average (worst channel {worst}), \
         which is more than chroma upsampling explains"
    );
}

// ---------------------------------------------------------------------------
// The path that does not copy
// ---------------------------------------------------------------------------

#[test]
fn a_hardware_frame_exports_as_dma_buf_handles_a_gpu_can_import() {
    let media = require_media!();
    if !hwdecode::supports(HwCodec::H264) {
        eprintln!(
            "skipping {}: no hardware H.264 decode",
            support::test_name()
        );
        return;
    }

    let mut decoder = open(&media.counter, Acceleration::Vaapi);
    let mapped = decoder.seek_and_map(mid_frame(20)).expect("map a surface");

    assert_eq!(mapped.coded_size(), (COUNTER_WIDTH, COUNTER_HEIGHT));
    assert_eq!(mapped.rotation, 0);
    // Rotation is deliberately *not* applied on this path — applying it means
    // touching pixels, which is the thing being avoided.
    assert_eq!(mapped.display_size(), (COUNTER_WIDTH, COUNTER_HEIGHT));

    let planes = mapped.dmabuf.planes();
    assert!(
        !planes.is_empty(),
        "an exported surface has at least one plane"
    );
    assert!(
        mapped.dmabuf.is_importable(),
        "every plane needs a real modifier and pitch to be importable: {:?}",
        mapped.dmabuf
    );

    for plane in &planes {
        assert!(plane.pitch >= plane.width as u64, "{plane:?}");
        assert!(
            plane.offset + plane.pitch * plane.height as u64 <= plane.object_size as u64,
            "a plane has to fit inside the object it points into: {plane:?}"
        );
        // The descriptor's own fd stays with the frame; consumers get a
        // duplicate, because `texture_from_dmabuf_fd` closes what it is given.
        let duplicate = plane.dup_fd().expect("a duplicate descriptor");
        assert!(std::os::fd::AsRawFd::as_raw_fd(&duplicate) >= 0);
    }
}

#[test]
fn mapping_a_software_decoder_is_refused_rather_than_quietly_copying() {
    let media = require_media!();
    let mut decoder = open(&media.counter, Acceleration::Software);
    let error = decoder
        .seek_and_map(mid_frame(5))
        .expect_err("there is no GPU surface behind a software decode");
    assert!(error.to_string().contains("software"), "got: {error}");
}

#[test]
fn mapping_every_frame_of_a_file_does_not_exhaust_the_surface_pool() {
    let media = require_media!();
    if !hwdecode::supports(HwCodec::H264) {
        eprintln!(
            "skipping {}: no hardware H.264 decode",
            support::test_name()
        );
        return;
    }

    // The failure this rules out is specific and nasty: each mapped frame pins
    // a surface out of a fixed pool, and a leak shows up not as an error at the
    // leak but as `receive_frame` failing a few dozen frames later, which reads
    // as a corrupt file.
    let mut decoder = open(&media.counter, Acceleration::Vaapi);
    for n in 0..COUNTER_FRAMES {
        let mapped = decoder
            .seek_and_map(mid_frame(n))
            .unwrap_or_else(|e| panic!("mapping frame {n} failed: {e}"));
        assert_eq!(mapped.coded_size(), (COUNTER_WIDTH, COUNTER_HEIGHT));
    }

    // And holding several at once still works, because a compositor caching a
    // texture per material holds one per clip on screen.
    let held: Vec<_> = [0u64, 10, 20, 30]
        .iter()
        .map(|n| decoder.seek_and_map(mid_frame(*n)).expect("a held surface"))
        .collect();
    assert_eq!(held.len(), 4);
    assert!(held.iter().all(|m| m.dmabuf.objects() >= 1));
}

#[test]
fn a_mapped_frame_is_the_same_frame_the_rgba_path_returns() {
    let media = require_media!();
    if !hwdecode::supports(HwCodec::H264) {
        eprintln!(
            "skipping {}: no hardware H.264 decode",
            support::test_name()
        );
        return;
    }

    // The two entry points share `locate`, and this is what says so. If they
    // ever diverge, the zero-copy preview would show a different frame from the
    // one the scrubber reports, which is the kind of bug that gets blamed on
    // the timeline for a week.
    let mut mapping = open(&media.counter, Acceleration::Vaapi);
    let mut copying = open(&media.counter, Acceleration::Vaapi);
    for n in [0u64, 1, 29, 30, 61, 119] {
        let mapped = mapping.seek_and_map(mid_frame(n)).expect("map");
        let copied = copying.seek_and_decode(mid_frame(n)).expect("decode");
        assert_eq!(mapped.pts, copied.pts, "frame {n}");
        assert_eq!(
            read_counter_rgba(&copied.data, copied.width, copied.height),
            Some(n)
        );
    }
}
