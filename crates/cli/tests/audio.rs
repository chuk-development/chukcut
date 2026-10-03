//! Audio tools from the command line: an effect stack built and changed in
//! separate invocations, the voice changer, the pitch switch, and ducking
//! music under FFmpeg-synthesised speech.

mod common;

use std::process::Command;

use common::{ok, run};

fn ffmpeg(args: &[&str]) -> bool {
    Command::new("ffmpeg")
        .args(["-loglevel", "error", "-y"])
        .args(args)
        .status()
        .is_ok_and(|s| s.success())
}

#[test]
fn an_audio_effect_stack_from_separate_invocations() {
    require_ffmpeg!();
    let dir = common::scratch("audio-stack");
    let tone = dir.join("tone.wav");
    assert!(ffmpeg(&[
        "-f",
        "lavfi",
        "-i",
        "sine=frequency=440:duration=3",
        tone.to_str().unwrap()
    ]));
    let project = dir.join("audio.chukcut");
    let p = project.to_str().unwrap();
    ok(&dir, &["new", p]);
    ok(&dir, &["import", p, tone.to_str().unwrap(), "--append"]);

    let catalog = ok(&dir, &["catalog", "audio"]);
    assert!(catalog
        .as_array()
        .unwrap()
        .iter()
        .any(|d| d["id"] == "reverb"));

    let added = ok(
        &dir,
        &[
            "audio-effect",
            "add",
            p,
            "1:0",
            "reverb",
            "--set",
            "mix=0.6",
        ],
    );
    let reverb = added["effect_id"].as_str().unwrap().to_string();
    ok(
        &dir,
        &["audio-effect", "add", p, "1:0", "eq3", "--set", "low=4"],
    );
    ok(
        &dir,
        &[
            "audio-effect",
            "set",
            p,
            "1:0",
            "eq3",
            "--index",
            "0",
            "--enabled",
            "false",
        ],
    );
    ok(&dir, &["voice", p, "1:0", "robot", "--intensity", "0.5"]);
    ok(&dir, &["audio-pitch", p, "1:0", "--follow-speed"]);

    let listed = ok(&dir, &["audio-effect", "list", p, "1:0"]);
    let effects = listed["effects"].as_array().unwrap();
    let kinds: Vec<_> = effects
        .iter()
        .map(|e| e["kind"].as_str().unwrap())
        .collect();
    assert_eq!(kinds, ["eq3", "reverb", "voice_robot"]);
    assert_eq!(effects[0]["enabled"], false);
    assert_eq!(effects[0]["params"]["low"], 4.0);
    assert_eq!(effects[1]["id"], reverb.as_str());
    assert!((effects[1]["params"]["mix"].as_f64().unwrap() - 0.6).abs() < 1e-6);
    assert!((effects[2]["params"]["intensity"].as_f64().unwrap() - 0.5).abs() < 1e-6);
    assert_eq!(listed["pitch_follows_speed"], true);

    ok(&dir, &["audio-effect", "remove", p, "1:0", "1"]);
    let listed = ok(&dir, &["audio-effect", "list", p, "1:0"]);
    assert_eq!(listed["effects"].as_array().unwrap().len(), 2);

    let refused = run(&dir, &["audio-effect", "add", p, "1:0", "flanger"]);
    assert_ne!(refused.code, 0);
}

#[test]
fn duck_writes_volume_keyframes_under_speech() {
    require_ffmpeg!();
    let dir = common::scratch("audio-duck");
    let music = dir.join("music.wav");
    let speech = dir.join("speech.wav");
    assert!(ffmpeg(&[
        "-f",
        "lavfi",
        "-i",
        "sine=frequency=220:duration=10",
        music.to_str().unwrap()
    ]));
    if !ffmpeg(&[
        "-f",
        "lavfi",
        "-i",
        "flite=text='Hello there, this is a voiceover over some music.'",
        "-af",
        "adelay=2000|2000",
        speech.to_str().unwrap(),
    ]) {
        eprintln!("skipping: this FFmpeg has no flite");
        return;
    }
    let project = dir.join("duck.chukcut");
    let p = project.to_str().unwrap();
    ok(&dir, &["new", p]);
    ok(&dir, &["import", p, music.to_str().unwrap(), "--append"]);
    ok(&dir, &["import", p, speech.to_str().unwrap()]);
    // Speech at 0 on a spot the music occupies: a second audio lane.
    ok(&dir, &["place", p, "speech.wav", "--at", "0"]);

    let ducked = ok(
        &dir,
        &[
            "duck",
            p,
            "1:0",
            "--depth",
            "10",
            "--attack",
            "200ms",
            "--release",
            "300ms",
        ],
    );
    assert_eq!(ducked["ducking"]["params"]["depth_db"], 10.0);
    assert!(
        ducked["volume_keyframes"].as_u64().unwrap() >= 4,
        "{ducked}"
    );

    ok(&dir, &["duck", p, "1:0", "--off"]);
    let listed = ok(&dir, &["audio-effect", "list", p, "1:0"]);
    assert!(listed["ducking"].is_null());
}
