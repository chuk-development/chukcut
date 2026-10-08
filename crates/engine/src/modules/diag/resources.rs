//! One line every 30 seconds about what the process and the machine are
//! doing.
//!
//! ```text
//! INFO chukcut_engine::modules::diag::resources: resources rss_mb=812 cpu_pct=34.2 threads=57 \
//!   sys_cpu_pct=41.0 load1=3.21 mem_avail_mb=7012 mem_total_mb=31842 \
//!   children="ml-worker 41207 rss_mb=1210 cpu_pct=88.0" gpu="NVIDIA GeForce RTX 3060" \
//!   gpu_pct=41 vram_used_mb=2210 vram_total_mb=12288 nvenc_pct=0 nvdec_pct=12 sample_us=180
//! ```
//!
//! (One line in the file.) `cpu_pct` is this process over the last interval,
//! where 100 is one core busy the whole time, so it can exceed 100.
//! `sys_cpu_pct` is the whole machine, where 100 is every core busy: a slow
//! frame on a machine at 95 % is a different problem from one on an idle
//! machine. The `gpu*`, `vram*`, `nvenc_pct` and `nvdec_pct` fields appear
//! only on a machine with an NVIDIA driver, and describe the whole card, not
//! only this process. `sample_us` is what taking the sample cost.
//!
//! ## When a line is written
//!
//! Every [`INTERVAL`] while the app is busy, which is when this process or one
//! of its helpers used at least [`BUSY_CPU_PCT`] of a core over the interval.
//! An idle editor uses a few per cent (its 8 ms UI tick; 3.8 % measured in a
//! debug build), playback and export use far more, so an idle app writes one
//! line every [`IDLE_INTERVAL`] instead: enough to show it was alive and how
//! its memory went, at 12 lines an hour.
//!
//! ## Cost
//!
//! Five small reads from `/proc` and, on NVIDIA, five NVML calls. NVML is
//! `libnvidia-ml.so.1`, loaded with `dlopen` at run time, never linked: a
//! machine without it simply has no `gpu` fields. NVML is the driver's
//! management library; it does not create a CUDA context or a Vulkan device,
//! so it does not break the one-device rule of `modules::gpu`.

use std::fmt::Write as _;
use std::time::{Duration, Instant};

use super::child;

/// How often a sample is taken.
pub const INTERVAL: Duration = Duration::from_secs(30);

/// How often a line is written while nothing happens.
pub const IDLE_INTERVAL: Duration = Duration::from_secs(300);

/// A process using less than this share of one core is idle.
pub const BUSY_CPU_PCT: f64 = 10.0;

/// Start the sampler thread. Once per process; later calls do nothing.
pub fn start() {
    static STARTED: std::sync::Once = std::sync::Once::new();
    STARTED.call_once(|| {
        let spawned = std::thread::Builder::new()
            .name("diag-sampler".into())
            .spawn(|| {
                let mut sampler = Sampler::new();
                let mut gpu_opened = false;
                loop {
                    std::thread::sleep(INTERVAL);
                    // After the first interval, by which time the startup
                    // report knows which adapter the render device picked.
                    if !gpu_opened {
                        sampler.open_gpu();
                        gpu_opened = true;
                    }
                    // Over-budget lines the rate limit held back, now that
                    // their window has passed.
                    super::budget::flush();
                    let sample = sampler.sample();
                    if sampler.should_write(&sample, Instant::now()) {
                        tracing::info!("{}", sample.line());
                    }
                }
            });
        if let Err(error) = spawned {
            tracing::warn!(%error, "could not start the resource sampler");
        }
    });
}

/// One sample, already reduced to numbers.
#[derive(Debug, Clone, Default)]
pub struct Sample {
    pub rss_mb: u64,
    /// This process, per cent of one core, over the interval.
    pub cpu_pct: f64,
    pub threads: u64,
    /// The machine, per cent of all cores, over the interval.
    pub sys_cpu_pct: f64,
    pub load1: f64,
    pub mem_avail_mb: u64,
    pub mem_total_mb: u64,
    /// `(name, pid, rss_mb, cpu_pct)` per helper process.
    pub children: Vec<(&'static str, u32, u64, f64)>,
    pub gpu: Option<GpuSample>,
    pub sample_us: u64,
}

#[derive(Debug, Clone, Default)]
pub struct GpuSample {
    pub name: String,
    pub util_pct: u32,
    pub vram_used_mb: u64,
    pub vram_total_mb: u64,
    pub enc_pct: u32,
    pub dec_pct: u32,
}

impl Sample {
    /// The log line.
    pub fn line(&self) -> String {
        let mut line = format!(
            "resources rss_mb={} cpu_pct={:.1} threads={} sys_cpu_pct={:.1} load1={:.2} \
             mem_avail_mb={} mem_total_mb={}",
            self.rss_mb,
            self.cpu_pct,
            self.threads,
            self.sys_cpu_pct,
            self.load1,
            self.mem_avail_mb,
            self.mem_total_mb
        );
        if !self.children.is_empty() {
            let children: Vec<String> = self
                .children
                .iter()
                .map(|(name, pid, rss, cpu)| format!("{name} {pid} rss_mb={rss} cpu_pct={cpu:.1}"))
                .collect();
            let _ = write!(line, " children=\"{}\"", children.join("; "));
        }
        if let Some(gpu) = &self.gpu {
            let _ = write!(
                line,
                " gpu=\"{}\" gpu_pct={} vram_used_mb={} vram_total_mb={} nvenc_pct={} nvdec_pct={}",
                gpu.name,
                gpu.util_pct,
                gpu.vram_used_mb,
                gpu.vram_total_mb,
                gpu.enc_pct,
                gpu.dec_pct
            );
        }
        let _ = write!(line, " sample_us={}", self.sample_us);
        line
    }

    fn busy(&self) -> bool {
        self.cpu_pct >= BUSY_CPU_PCT
            || self
                .children
                .iter()
                .any(|(_, _, _, cpu)| *cpu >= BUSY_CPU_PCT)
    }
}

/// What the sampler remembers between samples, to turn counters into rates.
pub struct Sampler {
    clock_ticks: f64,
    page_bytes: u64,
    last_at: Instant,
    last_self_ticks: Option<u64>,
    last_children: Vec<(u32, u64)>,
    last_system: Option<(u64, u64)>,
    last_written: Option<Instant>,
    #[cfg(target_os = "linux")]
    nvml: Option<nvml::Nvml>,
}

impl Default for Sampler {
    fn default() -> Self {
        Self::new()
    }
}

impl Sampler {
    pub fn new() -> Self {
        let mut sampler = Self {
            clock_ticks: clock_ticks(),
            page_bytes: page_bytes(),
            last_at: Instant::now(),
            last_self_ticks: None,
            last_children: Vec::new(),
            last_system: None,
            last_written: None,
            #[cfg(target_os = "linux")]
            nvml: None,
        };
        // A first reading, so the first real sample has a rate to report.
        let _ = sampler.sample();
        sampler
    }

    /// Load NVML for the `gpu` fields, on a machine with an NVIDIA driver.
    pub fn open_gpu(&mut self) {
        #[cfg(target_os = "linux")]
        {
            self.nvml = nvml::Nvml::open();
        }
    }

    /// Take a sample now.
    pub fn sample(&mut self) -> Sample {
        let started = Instant::now();
        let elapsed = started
            .saturating_duration_since(self.last_at)
            .as_secs_f64();
        self.last_at = started;
        let mut sample = Sample::default();

        if let Some(stat) = read_stat("/proc/self/stat") {
            sample.rss_mb = stat.rss_pages * self.page_bytes / (1024 * 1024);
            sample.threads = stat.threads;
            sample.cpu_pct = rate(self.last_self_ticks, stat.ticks, elapsed, self.clock_ticks);
            self.last_self_ticks = Some(stat.ticks);
        }

        let mut children = Vec::new();
        for (pid, name) in child::registered() {
            let Some(stat) = read_stat(&format!("/proc/{pid}/stat")) else {
                // Gone without its stderr closing yet; the forwarder will
                // unregister it, and until then it has nothing to report.
                continue;
            };
            let before = self
                .last_children
                .iter()
                .find(|(p, _)| *p == pid)
                .map(|(_, ticks)| *ticks);
            let cpu = rate(before, stat.ticks, elapsed, self.clock_ticks);
            sample.children.push((
                name,
                pid,
                stat.rss_pages * self.page_bytes / (1024 * 1024),
                cpu,
            ));
            children.push((pid, stat.ticks));
        }
        self.last_children = children;

        if let Some((busy, total)) = read_system_ticks() {
            if let Some((last_busy, last_total)) = self.last_system {
                let total_delta = total.saturating_sub(last_total);
                if total_delta > 0 {
                    sample.sys_cpu_pct =
                        busy.saturating_sub(last_busy) as f64 * 100.0 / total_delta as f64;
                }
            }
            self.last_system = Some((busy, total));
        }

        sample.load1 = std::fs::read_to_string("/proc/loadavg")
            .ok()
            .and_then(|text| text.split_whitespace().next()?.parse().ok())
            .unwrap_or(0.0);
        if let Some((avail, total)) = read_meminfo() {
            sample.mem_avail_mb = avail / 1024;
            sample.mem_total_mb = total / 1024;
        }

        #[cfg(target_os = "linux")]
        if let Some(nvml) = &self.nvml {
            sample.gpu = nvml.sample();
        }

        sample.sample_us = started.elapsed().as_micros() as u64;
        sample
    }

    /// Whether `sample` is worth a line: always while busy, else once per
    /// [`IDLE_INTERVAL`].
    pub fn should_write(&mut self, sample: &Sample, now: Instant) -> bool {
        let due = self
            .last_written
            .is_none_or(|last| now.saturating_duration_since(last) >= IDLE_INTERVAL);
        if sample.busy() || due {
            self.last_written = Some(now);
            true
        } else {
            false
        }
    }
}

/// Per cent of one core from two tick readings `elapsed` seconds apart.
fn rate(before: Option<u64>, now: u64, elapsed: f64, clock_ticks: f64) -> f64 {
    match before {
        Some(before) if elapsed > 0.0 => {
            let pct = now.saturating_sub(before) as f64 / clock_ticks / elapsed * 100.0;
            (pct * 10.0).round() / 10.0
        }
        _ => 0.0,
    }
}

struct Stat {
    ticks: u64,
    threads: u64,
    rss_pages: u64,
}

/// `utime + stime`, the thread count and the resident set from a
/// `/proc/<pid>/stat`.
fn read_stat(path: &str) -> Option<Stat> {
    parse_stat(&std::fs::read_to_string(path).ok()?)
}

fn parse_stat(text: &str) -> Option<Stat> {
    // The command name is in parentheses and may itself hold spaces or
    // parentheses, so the fields are counted from the *last* ')'. After it,
    // field 3 (state) is index 0, so field N is index N - 3.
    let rest = &text[text.rfind(')')? + 1..];
    let fields: Vec<&str> = rest.split_whitespace().collect();
    let field = |n: usize| -> Option<u64> { fields.get(n - 3)?.parse().ok() };
    Some(Stat {
        ticks: field(14)? + field(15)?,
        threads: field(20)?,
        rss_pages: field(24)?,
    })
}

/// `(busy, total)` jiffies of all cores together, from `/proc/stat`.
fn read_system_ticks() -> Option<(u64, u64)> {
    let text = std::fs::read_to_string("/proc/stat").ok()?;
    let first = text.lines().next()?;
    let values: Vec<u64> = first
        .split_whitespace()
        .skip(1)
        .filter_map(|v| v.parse().ok())
        .collect();
    // user nice system idle iowait irq softirq steal …
    let total: u64 = values.iter().take(8).sum();
    let idle = values.get(3)? + values.get(4).copied().unwrap_or(0);
    Some((total.saturating_sub(idle), total))
}

/// `(MemAvailable, MemTotal)` in KiB.
fn read_meminfo() -> Option<(u64, u64)> {
    let text = std::fs::read_to_string("/proc/meminfo").ok()?;
    let value = |key: &str| -> Option<u64> {
        text.lines()
            .find(|line| line.starts_with(key))?
            .split_whitespace()
            .nth(1)?
            .parse()
            .ok()
    };
    Some((value("MemAvailable:")?, value("MemTotal:")?))
}

#[cfg(unix)]
fn clock_ticks() -> f64 {
    // SAFETY: `sysconf` only reads a configuration value.
    let ticks = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
    if ticks > 0 {
        ticks as f64
    } else {
        100.0
    }
}

#[cfg(not(unix))]
fn clock_ticks() -> f64 {
    100.0
}

#[cfg(unix)]
fn page_bytes() -> u64 {
    // SAFETY: `sysconf` only reads a configuration value.
    let size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    if size > 0 {
        size as u64
    } else {
        4096
    }
}

#[cfg(not(unix))]
fn page_bytes() -> u64 {
    4096
}

/// NVIDIA's management library, loaded at run time.
#[cfg(target_os = "linux")]
mod nvml {
    use std::ffi::{c_char, c_int, c_uint, c_void, CStr};

    use super::GpuSample;

    type Device = *mut c_void;

    #[repr(C)]
    #[derive(Default)]
    struct Utilization {
        gpu: c_uint,
        memory: c_uint,
    }

    #[repr(C)]
    #[derive(Default)]
    struct Memory {
        total: u64,
        free: u64,
        used: u64,
    }

    type Init = unsafe extern "C" fn() -> c_int;
    type Count = unsafe extern "C" fn(*mut c_uint) -> c_int;
    type HandleByIndex = unsafe extern "C" fn(c_uint, *mut Device) -> c_int;
    type Name = unsafe extern "C" fn(Device, *mut c_char, c_uint) -> c_int;
    type Utilizations = unsafe extern "C" fn(Device, *mut Utilization) -> c_int;
    type MemoryInfo = unsafe extern "C" fn(Device, *mut Memory) -> c_int;
    type CodecUtilization = unsafe extern "C" fn(Device, *mut c_uint, *mut c_uint) -> c_int;

    pub struct Nvml {
        // Held so the function pointers below stay valid.
        _lib: libloading::Library,
        device: Device,
        name: String,
        utilization: Utilizations,
        memory: MemoryInfo,
        encoder: CodecUtilization,
        decoder: CodecUtilization,
    }

    // SAFETY: an NVML device handle is an opaque pointer that NVML documents
    // as usable from any thread; it is only read by the sampler thread.
    unsafe impl Send for Nvml {}

    impl Nvml {
        /// Load NVML and pick the device to report: the one whose name the
        /// render device has, else the first. `None` without an NVIDIA driver,
        /// which is the normal answer on Intel and AMD.
        pub fn open() -> Option<Nvml> {
            if !std::path::Path::new("/proc/driver/nvidia/version").exists() {
                return None;
            }
            // SAFETY: loading the driver's own library, whose initialisers are
            // written to be loaded into any process.
            let lib = unsafe { libloading::Library::new("libnvidia-ml.so.1") }.ok()?;
            // SAFETY: the signatures match nvml.h for these symbols.
            unsafe {
                let init: Init = *lib.get::<Init>(b"nvmlInit_v2\0").ok()?;
                let count: Count = *lib.get::<Count>(b"nvmlDeviceGetCount_v2\0").ok()?;
                let by_index: HandleByIndex = *lib
                    .get::<HandleByIndex>(b"nvmlDeviceGetHandleByIndex_v2\0")
                    .ok()?;
                let name_of: Name = *lib.get::<Name>(b"nvmlDeviceGetName\0").ok()?;
                let utilization: Utilizations = *lib
                    .get::<Utilizations>(b"nvmlDeviceGetUtilizationRates\0")
                    .ok()?;
                let memory: MemoryInfo =
                    *lib.get::<MemoryInfo>(b"nvmlDeviceGetMemoryInfo\0").ok()?;
                let encoder: CodecUtilization = *lib
                    .get::<CodecUtilization>(b"nvmlDeviceGetEncoderUtilization\0")
                    .ok()?;
                let decoder: CodecUtilization = *lib
                    .get::<CodecUtilization>(b"nvmlDeviceGetDecoderUtilization\0")
                    .ok()?;
                if init() != 0 {
                    return None;
                }
                let mut n: c_uint = 0;
                if count(&mut n) != 0 || n == 0 {
                    return None;
                }
                let wanted = super::super::startup::adapter_name();
                let mut chosen: Option<(Device, String)> = None;
                for index in 0..n {
                    let mut device: Device = std::ptr::null_mut();
                    if by_index(index, &mut device) != 0 {
                        continue;
                    }
                    let mut buf = [0 as c_char; 96];
                    let name = if name_of(device, buf.as_mut_ptr(), buf.len() as c_uint) == 0 {
                        CStr::from_ptr(buf.as_ptr()).to_string_lossy().into_owned()
                    } else {
                        String::from("NVIDIA")
                    };
                    let matches = wanted.as_deref().is_some_and(|w| w == name);
                    if chosen.is_none() || matches {
                        chosen = Some((device, name));
                    }
                    if matches {
                        break;
                    }
                }
                let (device, name) = chosen?;
                Some(Nvml {
                    _lib: lib,
                    device,
                    name,
                    utilization,
                    memory,
                    encoder,
                    decoder,
                })
            }
        }

        pub fn sample(&self) -> Option<GpuSample> {
            let mut util = Utilization::default();
            let mut mem = Memory::default();
            let (mut enc, mut dec, mut period) = (0, 0, 0);
            // SAFETY: valid device handle and out-pointers to locals.
            unsafe {
                if (self.utilization)(self.device, &mut util) != 0 {
                    return None;
                }
                let _ = (self.memory)(self.device, &mut mem);
                let _ = (self.encoder)(self.device, &mut enc, &mut period);
                let _ = (self.decoder)(self.device, &mut dec, &mut period);
            }
            let _ = util.memory;
            let _ = mem.free;
            Some(GpuSample {
                name: self.name.clone(),
                util_pct: util.gpu,
                vram_used_mb: mem.used / (1024 * 1024),
                vram_total_mb: mem.total / (1024 * 1024),
                enc_pct: enc,
                dec_pct: dec,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stat_fields_are_counted_from_the_last_parenthesis() {
        // A command name with a space and a ')' in it, as a thread name can be.
        let text = "4242 (pre view) x) S 1 4242 4242 0 -1 4194560 100 0 0 0 \
                    250 50 0 0 20 0 33 0 1000 123456789 2048 18446744073709551615";
        let stat = parse_stat(text).expect("parses");
        assert_eq!(stat.ticks, 300, "utime 250 + stime 50");
        assert_eq!(stat.threads, 33);
        assert_eq!(stat.rss_pages, 2048);
    }

    #[test]
    fn this_process_can_be_sampled_and_cheaply() {
        let mut sampler = Sampler::new();
        // Burn a little CPU so the rate is not zero.
        let started = Instant::now();
        let mut x = 0u64;
        while started.elapsed() < Duration::from_millis(50) {
            x = x.wrapping_mul(6364136223846793005).wrapping_add(1);
        }
        std::hint::black_box(x);
        let sample = sampler.sample();
        assert!(sample.rss_mb > 0, "{sample:?}");
        assert!(sample.threads >= 1, "{sample:?}");
        assert!(sample.mem_total_mb > 0, "{sample:?}");
        let line = sample.line();
        assert!(line.starts_with("resources rss_mb="), "{line}");
        assert!(line.contains(" sample_us="), "{line}");
        // The budget is 1 ms of work per sample; a loaded CI machine gets
        // ten times that before this fails.
        assert!(
            sample.sample_us < 10_000,
            "a sample cost {} µs",
            sample.sample_us
        );
    }

    #[test]
    fn an_idle_app_writes_rarely_and_a_busy_one_every_time() {
        let mut sampler = Sampler::new();
        let now = Instant::now();
        let idle = Sample::default();
        let busy = Sample {
            cpu_pct: 35.0,
            ..Sample::default()
        };
        assert!(sampler.should_write(&idle, now), "the first line always");
        assert!(!sampler.should_write(&idle, now + INTERVAL));
        assert!(sampler.should_write(&busy, now + INTERVAL * 2));
        assert!(sampler.should_write(&busy, now + INTERVAL * 3));
        assert!(!sampler.should_write(&idle, now + INTERVAL * 4));
        assert!(sampler.should_write(&idle, now + INTERVAL * 3 + IDLE_INTERVAL));
        let helper_busy = Sample {
            children: vec![("ml-worker", 1, 900, 80.0)],
            ..Sample::default()
        };
        assert!(
            sampler.should_write(&helper_busy, now + INTERVAL * 3 + IDLE_INTERVAL + INTERVAL),
            "a busy helper makes the app busy"
        );
    }
}
