//! What this machine is and how the app uses it, written once per run.
//!
//! Every line of the block starts with `startup:` and a topic, so
//! `grep 'startup:'` prints the whole block and `grep 'startup: gpu'` one
//! line of it:
//!
//! ```text
//! startup: app version=0.1.0 commit=18a3708 build=release pid=41180
//! startup: os name="Ubuntu 24.04.3 LTS" kernel=6.8.0-139-generic display=x11 desktop=ubuntu:GNOME
//! startup: cpu model="AMD Ryzen 7 5800X 8-Core Processor" cores=8 threads=16 ram_mb=31842
//! startup: ffmpeg avcodec=60.31.102 avformat=60.16.100 avutil=58.29.100 swscale=7.5.100 swresample=4.12.100
//! startup: nvidia driver=580.95.05
//! startup: settings decode=Auto proxies=Auto preview_scale=1 ml_fast=false
//! startup: transcription engine=whisper.cpp gpu=none
//! startup: gpu adapter="NVIDIA GeForce RTX 3060" backend=Vulkan type=DiscreteGpu driver="NVIDIA" driver_info="580.95.05" dmabuf=true
//! startup: decode vaapi="" nvdec="h264 hevc vp9 av1" vaapi_device=none
//! startup: encode hardware="h264_nvenc hevc_nvenc av1_nvenc" refused="…"
//! startup: display scale=1 window=1600x960
//! startup: hardware probes took ms=850
//! ```
//!
//! The first lines are cheap facts, written at once. The `gpu`, `decode` and
//! `encode` lines come from the probes the app runs anyway — the render
//! device of `modules::gpu`, the decode probe of `media::hwdecode` and the
//! trial encode of `export::hwaccel`, each cached for the process — on a
//! thread of their own, so the window does not wait for them. Nothing here
//! opens a device a second time.
//!
//! Lines that only exist later (the ML worker's providers, which it prints
//! when it loads its runtime; the window's scale) reach the log through
//! [`line`], or through the helper's own stderr.

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::sync::OnceLock;
use std::time::Instant;

use parking_lot::Mutex;

/// What the shell knows about its own build and the engine cannot: the
/// commit comes from the app's build script.
#[derive(Debug, Clone, Copy)]
pub struct Build {
    pub version: &'static str,
    pub commit: &'static str,
}

/// The adapter the render device picked, once the startup thread has asked.
static ADAPTER: OnceLock<String> = OnceLock::new();

/// The render adapter's name, for NVML to pick the same card.
pub fn adapter_name() -> Option<String> {
    ADAPTER.get().cloned()
}

/// Write one `startup: <topic> <text>` line. The same line twice is written
/// once: a second editor window has nothing new to say.
pub fn line(topic: &str, text: &str) {
    static SEEN: Mutex<BTreeSet<String>> = parking_lot::const_mutex(BTreeSet::new());
    let full = format!("startup: {topic} {text}");
    if SEEN.lock().insert(full.clone()) {
        tracing::info!("{full}");
    }
}

/// Write the block: the cheap lines now, the hardware lines from a thread.
pub fn report(build: Build) {
    line(
        "app",
        &format!(
            "version={} commit={} build={} pid={}",
            build.version,
            build.commit,
            if cfg!(debug_assertions) {
                "debug"
            } else {
                "release"
            },
            std::process::id()
        ),
    );
    line("os", &os_line());
    line("cpu", &cpu_line());
    line("ffmpeg", &ffmpeg_line());
    if let Some(driver) = nvidia_driver() {
        line("nvidia", &format!("driver={driver}"));
    }
    line("settings", &settings_line());
    line("transcription", &transcription_line());

    let spawned = std::thread::Builder::new()
        .name("diag-startup".into())
        .spawn(hardware_lines);
    if let Err(error) = spawned {
        tracing::warn!(%error, "could not start the hardware report");
    }
}

fn hardware_lines() {
    let started = Instant::now();
    match crate::modules::gpu::render_context() {
        Some(ctx) => {
            let info = ctx.adapter_info();
            let _ = ADAPTER.set(info.name.clone());
            let mut text = String::new();
            kv(&mut text, "adapter", &info.name);
            kv(&mut text, "backend", &format!("{:?}", info.backend));
            kv(&mut text, "type", &format!("{:?}", info.device_type));
            kv(&mut text, "vendor", &format!("0x{:04x}", info.vendor));
            kv(&mut text, "driver", &info.driver);
            kv(&mut text, "driver_info", &info.driver_info);
            kv(&mut text, "dmabuf", &ctx.can_import_dmabuf().to_string());
            line("gpu", text.trim_start());
        }
        None => line("gpu", "adapter=none (nothing can be composited)"),
    }

    use crate::modules::media::hwdecode::{self, HwBackend};
    let usable = |rows: &[hwdecode::HwDecodeSupport]| -> String {
        rows.iter()
            .filter(|row| row.usable)
            .map(|row| row.decoder_name.clone())
            .collect::<Vec<_>>()
            .join(" ")
    };
    let vaapi = hwdecode::capabilities_for(HwBackend::Vaapi);
    let cuda = hwdecode::capabilities_for(HwBackend::Cuda);
    let refused = group_refused(
        vaapi
            .iter()
            .chain(cuda.iter())
            .filter(|row| row.in_build && !row.usable)
            .map(|row| {
                (
                    format!(
                        "{}/{}",
                        row.decoder_name,
                        row.backend.label().to_lowercase()
                    ),
                    row.note.clone().unwrap_or_else(|| "refused".into()),
                )
            }),
    );
    let mut text = String::new();
    kv(&mut text, "vaapi", &usable(vaapi));
    kv(&mut text, "nvdec", &usable(cuda));
    kv(
        &mut text,
        "vaapi_device",
        &crate::modules::gpu::vaapi_device()
            .map(|device| device.node().to_string())
            .unwrap_or_else(|| "none".into()),
    );
    if !refused.is_empty() {
        kv(&mut text, "refused", &refused);
    }
    line("decode", text.trim_start());

    let encoders = crate::modules::export::hwaccel::detect();
    let usable: Vec<&str> = encoders
        .iter()
        .filter(|e| e.usable)
        .map(|e| e.encoder_name.as_str())
        .collect();
    let refused = group_refused(
        encoders
            .iter()
            .filter(|e| e.available && !e.usable)
            .map(|e| {
                (
                    e.encoder_name.clone(),
                    e.note.clone().unwrap_or_else(|| "refused".into()),
                )
            }),
    );
    let mut text = String::new();
    kv(&mut text, "hardware", &usable.join(" "));
    if !refused.is_empty() {
        kv(&mut text, "refused", &refused);
    }
    line("encode", text.trim_start());
    line(
        "hardware",
        &format!("probes took ms={}", started.elapsed().as_millis()),
    );
}

/// `a b c: why; d: why not` from `(name, note)` pairs, one reason per group.
///
/// The notes are written for the settings panel, a sentence each, and on a
/// machine without VAAPI all eight say the same thing. The log takes the
/// reason in the note's parentheses when it has one ("no VAAPI device on this
/// machine"), and lists the names that share it once.
pub fn group_refused(items: impl Iterator<Item = (String, String)>) -> String {
    let mut groups: Vec<(String, Vec<String>)> = Vec::new();
    for (name, note) in items {
        let reason = note
            .split_once('(')
            .and_then(|(_, rest)| rest.rsplit_once(')'))
            .map(|(inside, _)| inside.to_string())
            .unwrap_or(note);
        let reason: String = reason.chars().take(160).collect();
        match groups.iter_mut().find(|(r, _)| *r == reason) {
            Some((_, names)) => names.push(name),
            None => groups.push((reason, vec![name])),
        }
    }
    groups
        .into_iter()
        .map(|(reason, names)| format!("{}: {reason}", names.join(" ")))
        .collect::<Vec<_>>()
        .join("; ")
}

/// ` key=value`, quoting the value when it holds a space, a quote or nothing.
pub fn kv(out: &mut String, key: &str, value: &str) {
    let plain = !value.is_empty()
        && value
            .chars()
            .all(|c| !c.is_whitespace() && c != '"' && c != ';' && c != '=');
    if plain {
        let _ = write!(out, " {key}={value}");
    } else {
        let _ = write!(out, " {key}={value:?}");
    }
}

fn os_line() -> String {
    let mut text = String::new();
    let name = std::fs::read_to_string("/etc/os-release")
        .ok()
        .and_then(|release| {
            release.lines().find_map(|line| {
                line.strip_prefix("PRETTY_NAME=")
                    .map(|v| v.trim_matches('"').to_string())
            })
        })
        .unwrap_or_else(|| std::env::consts::OS.to_string());
    kv(&mut text, "name", &name);
    if let Ok(kernel) = std::fs::read_to_string("/proc/sys/kernel/osrelease") {
        kv(&mut text, "kernel", kernel.trim());
    }
    kv(&mut text, "arch", std::env::consts::ARCH);
    kv(&mut text, "display", &display_server());
    if let Ok(desktop) = std::env::var("XDG_CURRENT_DESKTOP") {
        kv(&mut text, "desktop", &desktop);
    }
    text.trim_start().to_string()
}

/// X11 or Wayland, as the session says. GPUI picks Wayland when
/// `WAYLAND_DISPLAY` is set, so that is what decides it.
fn display_server() -> String {
    let session = std::env::var("XDG_SESSION_TYPE").unwrap_or_default();
    let wayland = std::env::var_os("WAYLAND_DISPLAY").is_some();
    let x11 = std::env::var_os("DISPLAY").is_some();
    let used = if wayland {
        "wayland"
    } else if x11 {
        "x11"
    } else {
        "none"
    };
    if session.is_empty() || session == used {
        used.to_string()
    } else {
        format!("{used} (session {session})")
    }
}

fn cpu_line() -> String {
    let cpuinfo = std::fs::read_to_string("/proc/cpuinfo").unwrap_or_default();
    let model = cpuinfo
        .lines()
        .find_map(|line| {
            let (key, value) = line.split_once(':')?;
            (key.trim() == "model name").then(|| value.trim().to_string())
        })
        .unwrap_or_else(|| "unknown".into());
    // Physical cores: distinct (package, core) pairs.
    let mut cores = BTreeSet::new();
    let mut package = String::new();
    for line in cpuinfo.lines() {
        if let Some((key, value)) = line.split_once(':') {
            match key.trim() {
                "physical id" => package = value.trim().to_string(),
                "core id" => {
                    cores.insert((package.clone(), value.trim().to_string()));
                }
                _ => {}
            }
        }
    }
    let threads = cpuinfo
        .lines()
        .filter(|line| line.starts_with("processor"))
        .count();
    // What this process may use, which a cgroup CPU quota (a systemd slice,
    // a container) can make less than the machine has.
    let usable = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(0);
    let ram_mb = std::fs::read_to_string("/proc/meminfo")
        .ok()
        .and_then(|text| {
            text.lines()
                .find(|line| line.starts_with("MemTotal:"))?
                .split_whitespace()
                .nth(1)?
                .parse::<u64>()
                .ok()
        })
        .map(|kib| kib / 1024)
        .unwrap_or(0);
    let mut text = String::new();
    kv(&mut text, "model", &model);
    kv(&mut text, "cores", &cores.len().to_string());
    kv(&mut text, "threads", &threads.to_string());
    kv(&mut text, "usable_threads", &usable.to_string());
    kv(&mut text, "ram_mb", &ram_mb.to_string());
    text.trim_start().to_string()
}

fn ffmpeg_line() -> String {
    fn version(packed: u32) -> String {
        format!(
            "{}.{}.{}",
            packed >> 16,
            (packed >> 8) & 0xff,
            packed & 0xff
        )
    }
    // The libraries this process loaded, not the headers it was built against:
    // the two differ when a package runs on another distribution's FFmpeg.
    format!(
        "avcodec={} avformat={} avutil={} swscale={} swresample={}",
        version(ffmpeg_next::codec::version()),
        version(ffmpeg_next::format::version()),
        version(ffmpeg_next::util::version()),
        version(ffmpeg_next::software::scaling::version()),
        version(ffmpeg_next::software::resampling::version()),
    )
}

/// The kernel module's version, e.g. `580.95.05`.
fn nvidia_driver() -> Option<String> {
    let text = std::fs::read_to_string("/proc/driver/nvidia/version").ok()?;
    let first = text.lines().next()?;
    // "NVRM version: NVIDIA UNIX x86_64 Kernel Module  580.95.05  Release Build …"
    first
        .split_whitespace()
        .find(|word| word.chars().next().is_some_and(|c| c.is_ascii_digit()) && word.contains('.'))
        .map(str::to_string)
}

fn settings_line() -> String {
    let settings = crate::modules::workspace::Settings::load();
    format!(
        "decode={:?} proxies={:?} preview_scale={} preview_max_edge={} ml_fast={}",
        settings.decode,
        settings.proxy_policy,
        settings.preview_scale,
        settings.preview_max_edge,
        settings.ml_fast
    )
}

/// What local transcription can run on: whisper.cpp on the CPU in this
/// process (the `local-whisper` feature) and the CUDA helper when it is
/// installed and an NVIDIA driver is loaded (decision 0036). Not probed here,
/// which would start CUDA at every launch; each transcription logs the device
/// it used, and a fallback logs why.
fn transcription_line() -> String {
    use crate::modules::speech::{helper, local};
    let cpu = if local::AVAILABLE {
        "in-process"
    } else {
        "none"
    };
    let gpu = helper::worth_trying()
        .map(|path| format!("cuda-helper={}", path.display()))
        .unwrap_or_else(|| "cuda-helper=none".to_string());
    format!("engine=whisper.cpp cpu={cpu} {gpu}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_are_quoted_only_when_they_must_be() {
        let mut text = String::new();
        kv(&mut text, "backend", "Vulkan");
        kv(&mut text, "adapter", "NVIDIA GeForce RTX 3060");
        kv(&mut text, "empty", "");
        assert_eq!(
            text,
            " backend=Vulkan adapter=\"NVIDIA GeForce RTX 3060\" empty=\"\""
        );
    }

    #[test]
    fn refusals_are_grouped_by_their_reason() {
        let vaapi = "VAAPI is present but this driver could not encode a test frame with it \
                     (this machine has no usable VAAPI device). Software encoding still works.";
        let grouped = group_refused(
            [
                ("h264_vaapi".to_string(), vaapi.to_string()),
                ("hevc_vaapi".to_string(), vaapi.to_string()),
                (
                    "av1_nvenc".to_string(),
                    "the card has no AV1 encoder".to_string(),
                ),
            ]
            .into_iter(),
        );
        assert_eq!(
            grouped,
            "h264_vaapi hevc_vaapi: this machine has no usable VAAPI device; \
             av1_nvenc: the card has no AV1 encoder"
        );
    }

    #[test]
    fn the_cheap_lines_describe_this_machine() {
        assert!(cpu_line().contains("threads="));
        assert!(os_line().contains("display="));
        let ffmpeg = ffmpeg_line();
        assert!(
            ffmpeg.starts_with("avcodec=") && !ffmpeg.contains("avcodec=0.0.0"),
            "{ffmpeg}"
        );
    }
}
