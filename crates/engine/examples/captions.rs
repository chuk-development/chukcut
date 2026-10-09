//! Transcribe a media file and print captions as SRT.
//!
//! ```text
//! cargo run --release -p chukcut-engine --features local-whisper --example captions -- \
//!     clip.mp4 [--local tiny|base|small|medium|large] [--account <cloud account id>] \
//!     [--lang de] [--words 2]
//! ```
//!
//! Without `--account` it runs whisper.cpp locally (the model is downloaded on
//! first use). The same functions the captions panel calls, minus the
//! timeline: the file's own audio is read at 16 kHz mono directly.
//!
//! On a machine with `nvcc` the build also produces the CUDA helper and the
//! local path runs on the GPU; `CHUKCUT_WHISPER_HELPER=off` forces the CPU,
//! which is how the two are compared (docs/STATUS.md).

use std::sync::atomic::AtomicBool;

use chukcut_engine::modules::audio::{AudioClipReader, ClipReader as _};
use chukcut_engine::modules::captions::{group, srt, CaptionMode};
use chukcut_engine::modules::cloud::{self, CloudStore};
use chukcut_engine::modules::speech::{audio, commands, local, LocalModel};

fn main() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let path = args.first().ok_or("usage: captions <media> [options]")?;
    let option = |name: &str| {
        args.iter()
            .position(|a| a == name)
            .and_then(|i| args.get(i + 1))
            .cloned()
    };
    let language = option("--lang");
    let mode = match option("--words") {
        Some(n) => CaptionMode::words(n.parse().map_err(|_| "--words takes a number")?),
        None => CaptionMode::default(),
    };

    let mut reader = AudioClipReader::open(path, audio::RATE, 1).map_err(|e| e.to_string())?;
    let mut samples = Vec::new();
    let block = audio::RATE as usize * 10;
    let mut at = 0i64;
    loop {
        let mut out = vec![0f32; block];
        reader
            .read(at, block, &mut out)
            .map_err(|e| e.to_string())?;
        let silent_tail = out.iter().all(|s| *s == 0.0);
        samples.extend_from_slice(&out);
        at += block as i64;
        // The reader zero-pads past the end; a whole silent block after the
        // first means the file is done.
        if silent_tail && at > block as i64 {
            break;
        }
    }
    while samples.last() == Some(&0.0) {
        samples.pop();
    }
    eprintln!(
        "{:.1} s of audio",
        samples.len() as f64 / audio::RATE as f64
    );

    let cancel = AtomicBool::new(false);
    let started = std::time::Instant::now();
    let transcript = match option("--account") {
        Some(account) => {
            let transcriber = cloud::transcriber(&CloudStore::user(), &account)?;
            commands::transcribe_chunked(
                &samples,
                transcriber.as_ref(),
                language.as_deref(),
                None,
                audio::MAX_CHUNK,
                &|p| eprintln!("{}", p.label),
                &cancel,
            )?
        }
        None => {
            let model = match option("--local").as_deref() {
                Some("tiny") => LocalModel::Tiny,
                Some("small") => LocalModel::Small,
                Some("medium") => LocalModel::Medium,
                Some("large") => LocalModel::LargeV3Turbo,
                _ => LocalModel::Base,
            };
            let file = model.ensure(
                &|done, total| eprint!("\rdownloading {} / {} MB  ", done >> 20, total >> 20),
                &cancel,
            )?;
            eprintln!();
            // The device, once: "on the GPU (…, CUDA)" or "on the CPU".
            let named = std::sync::Once::new();
            local::transcribe(
                &file,
                &samples,
                language.as_deref(),
                &|device, _| named.call_once(|| eprintln!("transcribing on {device}")),
                &cancel,
            )?
        }
    };
    eprintln!(
        "{} words, language {:?}, {:.1} s",
        transcript.timed_words().len(),
        transcript.language,
        started.elapsed().as_secs_f64()
    );
    print!(
        "{}",
        srt::to_srt(&group::group(&transcript.timed_words(), mode))
    );
    Ok(())
}
