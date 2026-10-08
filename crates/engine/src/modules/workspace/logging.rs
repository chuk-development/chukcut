//! Where the log goes when nobody is watching a terminal.
//!
//! The app used to log to stdout and nowhere else. That is fine while it is
//! started from a shell and useless the moment it is started from a launcher,
//! which is how everyone but us starts it: a user reports a broken export and
//! there is no record of which encoder ran, which frame path was taken or what
//! failed. So every run also writes a file, and [`init`] is the only place a
//! subscriber is installed.
//!
//! ## The two layers are filtered differently, on purpose
//!
//! stdout keeps the old behaviour exactly — `RUST_LOG` still works and still
//! defaults to `chukcut=debug,warn` — because that is the knob a developer
//! reaches for. The **file** is fixed at [`FILE_FILTER`] and deliberately does
//! *not* read `RUST_LOG`: a log somebody mails us should have the same shape
//! whatever their environment happens to say, and `RUST_LOG=error` in a user's
//! profile silently emptying the file is the exact failure this module exists
//! to prevent.
//!
//! ## Why the rotation is written here rather than taken from a crate
//!
//! It is a date comparison, a byte count and a directory listing, and owning it
//! means the current file's *path* is something we can hand to the UI without
//! guessing at another crate's naming scheme.
//!
//! ## The size bound
//!
//! The log is always on, so it must never be the thing that fills a disk. Two
//! hard limits hold whatever the app does, even when something logs in a loop
//! for hours:
//!
//! - **Per file, [`MAX_FILE_BYTES`].** A day starts in
//!   `chukcut-YYYY-MM-DD.log`. When the next line would take it past the cap,
//!   the line goes into a new part, `chukcut-YYYY-MM-DD.2.log`, then `.3.log`.
//! - **For the directory, [`MAX_TOTAL_BYTES`]**, and at most [`MAX_FILES`]
//!   files. After every roll-over the oldest files go until the others plus a
//!   full current file fit in the total, so the directory never holds more
//!   than the total, not even for a moment. The current file is never deleted.
//!
//! Rate limits in `modules::diag` keep a misbehaving loop from pushing a
//! week of history out; the caps are what holds when those are not enough.
//!
//! Nothing is buffered. Every event is one `write` to the file, so a log ends
//! at the last thing that happened rather than a few kilobytes before it, which
//! is the only property that matters when the process died.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use tracing_subscriber::fmt::writer::MakeWriter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, Layer};

use super::paths;

/// What stdout logs when `RUST_LOG` says nothing. Unchanged from before this
/// module existed.
const STDOUT_DEFAULT: &str = "chukcut=debug,warn";

/// What the file logs, always. Our own crate from INFO up, everything else from
/// WARN up: a user's log should contain what the app decided, not wgpu's
/// adapter enumeration.
///
/// The `chukcut` prefix matches `chukcut_engine::…` too — `EnvFilter` compares
/// target prefixes, and the library half of this crate is where nearly every
/// event is raised.
const FILE_FILTER: &str = "chukcut=info,warn";

/// The largest one file may grow. A bigger file is hard to open in an editor
/// and hard to attach to a bug report; 20 MB is several days of normal use.
pub const MAX_FILE_BYTES: u64 = 20 * 1024 * 1024;

/// The most the whole directory may hold.
pub const MAX_TOTAL_BYTES: u64 = 100 * 1024 * 1024;

/// The most files the directory may hold, whatever their size. About a month
/// of daily use: "it broke last week" is still in there.
pub const MAX_FILES: usize = 30;

/// The caps a [`DailyFile`] holds. The app uses [`Limits::default`]; tests use
/// small numbers to reach the caps quickly.
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    pub max_file_bytes: u64,
    pub max_total_bytes: u64,
    pub max_files: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_file_bytes: MAX_FILE_BYTES,
            max_total_bytes: MAX_TOTAL_BYTES,
            max_files: MAX_FILES,
        }
    }
}

const PREFIX: &str = "chukcut-";
const SUFFIX: &str = ".log";

/// The file the current process is writing to, once [`init`] has run.
///
/// `None` when the log directory could not be created or opened, which is not
/// fatal: the app runs, stdout still works, and the UI says file logging is off
/// rather than offering a button that reveals nothing.
static CURRENT: OnceLock<Option<Arc<DailyFile>>> = OnceLock::new();

/// Install the process-wide subscriber: stdout as before, plus a file.
///
/// Panics if called twice, exactly as the `tracing_subscriber` call it replaced
/// did — two subscribers would mean events going to whichever won the race.
pub fn init() {
    let stdout = tracing_subscriber::fmt::layer()
        .with_writer(std::io::stdout)
        .with_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(STDOUT_DEFAULT)),
        );

    // Opened before the subscriber exists, so the failure cannot be logged
    // here; it is reported a few lines down, once there is somewhere to report
    // it to.
    let opened = DailyFile::new(paths::logs_dir(), Limits::default());
    let (file, failure) = match opened {
        Ok(file) => (Some(Arc::new(file)), None),
        Err(error) => (None, Some(error)),
    };

    let file_layer = file.clone().map(|file| {
        tracing_subscriber::fmt::layer()
            // No escape codes in a file. Colour in a log somebody opens in an
            // editor is noise, and in a log they paste into an issue it is
            // worse than noise.
            .with_ansi(false)
            .with_writer(LogWriter(file))
            .with_filter(EnvFilter::new(FILE_FILTER))
    });

    let _ = CURRENT.set(file);

    tracing_subscriber::registry()
        .with(stdout)
        .with(file_layer)
        .init();

    match (&failure, log_file()) {
        (Some(error), _) => tracing::error!(
            directory = %paths::logs_dir().display(),
            %error,
            "could not open a log file; this run is logging to stdout only"
        ),
        (None, Some(path)) => tracing::info!(
            path = %path.display(),
            max_file_mb = MAX_FILE_BYTES / (1024 * 1024),
            max_total_mb = MAX_TOTAL_BYTES / (1024 * 1024),
            "logging to file"
        ),
        (None, None) => {}
    }
}

/// Append `text` to the log file directly, not through `tracing`.
///
/// For a panic report: it must reach the file whether or not a subscriber
/// with a file layer is installed — the CLI logs to stderr only, and its
/// panics belong in the same file the app's do. Opens the file on first use
/// when [`init`] did not. One write, so a report is never interleaved with
/// another thread's line. False when there is no file to write to.
pub fn write_raw(text: &str) -> bool {
    static FALLBACK: OnceLock<Option<Arc<DailyFile>>> = OnceLock::new();
    let file = match CURRENT.get() {
        Some(file) => file.clone(),
        None => FALLBACK
            .get_or_init(|| {
                DailyFile::new(paths::logs_dir(), Limits::default())
                    .ok()
                    .map(Arc::new)
            })
            .clone(),
    };
    let Some(file) = file else {
        return false;
    };
    file.append(text.as_bytes()).is_ok()
}

/// The file this run is writing to, for the UI and for anyone about to ask a
/// user to send it.
pub fn log_file() -> Option<PathBuf> {
    CURRENT.get()?.as_ref().map(|file| file.path())
}

// ---------------------------------------------------------------------------
// The appender
// ---------------------------------------------------------------------------

/// An append-only file that rolls over at midnight UTC and at a size cap, and
/// prunes its predecessors to a total size.
pub struct DailyFile {
    dir: PathBuf,
    limits: Limits,
    open: Mutex<Open>,
}

struct Open {
    /// Days since the Unix epoch, UTC. A change is a roll-over.
    day: i64,
    /// Which part of the day this is, from 1.
    part: u32,
    /// Bytes in the file: its length at open plus what this process wrote.
    /// Another process appending to the same file (the CLI's panic report) is
    /// not counted; the cap is then off by that report, which is small.
    written: u64,
    file: File,
    path: PathBuf,
}

impl DailyFile {
    /// Create `dir` if it is missing, open today's newest part for appending
    /// (or a new part when that one is full), and prune to `limits`.
    pub fn new(dir: impl Into<PathBuf>, limits: Limits) -> io::Result<Self> {
        let dir = dir.into();
        fs::create_dir_all(&dir)?;
        let day = today();
        let part = newest_part(&dir, day);
        let open = open_part(&dir, day, part, limits.max_file_bytes)?;
        let this = Self {
            dir,
            limits,
            open: Mutex::new(open),
        };
        this.prune();
        Ok(this)
    }

    /// The file being written to right now.
    pub fn path(&self) -> PathBuf {
        self.lock().path.clone()
    }

    /// A poisoned lock here means a thread panicked mid-write. The log is the
    /// last thing that should stop working while the app is falling over, so
    /// the guard is taken anyway.
    fn lock(&self) -> std::sync::MutexGuard<'_, Open> {
        self.open
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn append(&self, buf: &[u8]) -> io::Result<usize> {
        let mut rolled = false;
        let written = {
            let mut open = self.lock();
            let day = today();
            let full =
                open.written > 0 && open.written + buf.len() as u64 > self.limits.max_file_bytes;
            if day != open.day || full {
                let part = if day == open.day { open.part + 1 } else { 1 };
                // A failed roll-over keeps writing to the current file rather
                // than losing the line: a log in the wrong file is
                // recoverable, a dropped one is not.
                if let Ok(next) = open_part(&self.dir, day, part, self.limits.max_file_bytes) {
                    *open = next;
                    rolled = true;
                }
            }
            let written = open.file.write(buf)?;
            open.written += written as u64;
            written
        };
        if rolled {
            self.prune();
        }
        Ok(written)
    }

    /// Delete the oldest files until at most `max_files` remain and the
    /// others plus a *full* current file fit in `max_total_bytes`.
    ///
    /// Reserving the current file's whole cap is what makes the total a hard
    /// bound: the current file grows between prunes, and nothing is deleted
    /// while it does. The current file itself is never deleted.
    fn prune(&self) {
        let current = self.path();
        let Ok(entries) = fs::read_dir(&self.dir) else {
            return;
        };
        let mut others: Vec<((i64, u32), PathBuf, u64)> = entries
            .flatten()
            .filter_map(|entry| {
                let path = entry.path();
                let key = parse_name(path.file_name()?.to_str()?)?;
                let size = entry.metadata().ok()?.len();
                Some((key, path, size))
            })
            .filter(|(_, path, _)| *path != current)
            .collect();
        others.sort_by_key(|(key, _, _)| *key);
        let mut total: u64 = others.iter().map(|(_, _, size)| size).sum();
        let budget = self
            .limits
            .max_total_bytes
            .saturating_sub(self.limits.max_file_bytes);
        let keep_others = self.limits.max_files.saturating_sub(1);
        let mut count = others.len();
        for (_, stale, size) in &others {
            if total <= budget && count <= keep_others {
                break;
            }
            // A file we cannot delete is not worth failing a log write over,
            // and the next roll-over will try again.
            if fs::remove_file(stale).is_ok() {
                total -= size;
                count -= 1;
            }
        }
    }
}

/// The per-event handle the `fmt` layer writes through.
///
/// Cloned rather than borrowed because `MakeWriter` hands one out per event and
/// the events come from several threads; the `Mutex` inside [`DailyFile`] is
/// what actually serialises them.
#[derive(Clone)]
pub struct LogWriter(Arc<DailyFile>);

impl LogWriter {
    pub fn new(file: Arc<DailyFile>) -> Self {
        Self(file)
    }
}

impl Write for LogWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.append(buf)
    }

    /// Nothing is held: `File::write` is a syscall, not a buffer.
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl<'a> MakeWriter<'a> for LogWriter {
    type Writer = LogWriter;

    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

/// `(day, part)` for one of our file names, `None` for anything else.
///
/// The prune sorts these keys, not the names: as a string,
/// `chukcut-2026-10-09.2.log` sorts *before* `chukcut-2026-10-09.log`
/// ('2' < 'l'), and `.10.log` before `.2.log`.
fn parse_name(name: &str) -> Option<(i64, u32)> {
    let stem = name.strip_prefix(PREFIX)?.strip_suffix(SUFFIX)?;
    let (date, part) = match stem.split_once('.') {
        Some((date, part)) => (date, part.parse::<u32>().ok().filter(|p| *p >= 2)?),
        None => (stem, 1),
    };
    if date.len() != 10 {
        return None;
    }
    let mut fields = date.splitn(3, '-');
    let year: i64 = fields.next()?.parse().ok()?;
    let month: u32 = fields.next()?.parse().ok()?;
    let day: u32 = fields.next()?.parse().ok()?;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    Some((days_from_civil(year, month, day), part))
}

/// The highest part that exists for `day`, or 1 when none does.
fn newest_part(dir: &Path, day: i64) -> u32 {
    fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| parse_name(entry.file_name().to_str()?))
        .filter(|(d, _)| *d == day)
        .map(|(_, part)| part)
        .max()
        .unwrap_or(1)
}

/// Open `part` of `day` for appending, moving on to the next part while the
/// one asked for is already full: a restart must not append to a file that
/// is at its cap.
fn open_part(dir: &Path, day: i64, mut part: u32, max_file_bytes: u64) -> io::Result<Open> {
    loop {
        let path = dir.join(part_name(day, part));
        let written = fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        if written >= max_file_bytes && part < u32::MAX {
            part += 1;
            continue;
        }
        let file = OpenOptions::new().create(true).append(true).open(&path)?;
        return Ok(Open {
            day,
            part,
            written,
            file,
            path,
        });
    }
}

/// The first file of `day`.
pub fn file_name(day: i64) -> String {
    part_name(day, 1)
}

/// `chukcut-YYYY-MM-DD.log` for part 1, `chukcut-YYYY-MM-DD.N.log` after.
pub fn part_name(day: i64, part: u32) -> String {
    let (year, month, dom) = civil_from_days(day);
    if part <= 1 {
        format!("{PREFIX}{year:04}-{month:02}-{dom:02}{SUFFIX}")
    } else {
        format!("{PREFIX}{year:04}-{month:02}-{dom:02}.{part}{SUFFIX}")
    }
}

/// Days since the Unix epoch, UTC.
///
/// UTC rather than local time so a machine that travels — or one whose timezone
/// database updates mid-run — cannot roll the file backwards and interleave two
/// days in one file.
fn today() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since| (since.as_secs() as i64).div_euclid(86_400))
        .unwrap_or(0)
}

/// Days since the Unix epoch to a civil (year, month, day).
///
/// Howard Hinnant's `civil_from_days`, which is the standard branch-free
/// version of this and is why no date crate is in the dependency list for one
/// filename a day.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    // Shift the epoch to 0000-03-01, so leap days land at the end of the cycle
    // and every era is exactly 146097 days.
    let shifted = days + 719_468;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted.rem_euclid(146_097); // [0, 146096]
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365; // [0, 399]
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100); // [0, 365]
    let month_prime = (5 * day_of_year + 2) / 153; // [0, 11], March is 0
    let day = (day_of_year - (153 * month_prime + 2) / 5 + 1) as u32; // [1, 31]
    let month = if month_prime < 10 {
        month_prime + 3
    } else {
        month_prime - 9
    } as u32;
    (if month <= 2 { year + 1 } else { year }, month, day)
}

/// The inverse of [`civil_from_days`], from the same source.
fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400; // [0, 399]
    let month_prime = if month > 2 { month - 3 } else { month + 9 } as i64; // [0, 11]
    let day_of_year = (153 * month_prime + 2) / 5 + day as i64 - 1; // [0, 365]
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    /// A directory of this test's own. `temp_dir` plus the process id is the
    /// pattern the rest of this codebase uses; the counter keeps two tests in
    /// one process apart.
    fn scratch(name: &str) -> PathBuf {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "chukcut-log-{name}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn dates_are_the_ones_a_calendar_agrees_with() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(19_723), (2024, 1, 1));
        // 2024 was a leap year; day 59 of it is the 29th of February.
        assert_eq!(civil_from_days(19_723 + 59), (2024, 2, 29));
        assert_eq!(file_name(19_723 + 59), "chukcut-2024-02-29.log");
    }

    /// The whole point of the module: an event raised through a subscriber
    /// reaches a file on disk, and it is the file the app would tell the user
    /// about.
    #[test]
    fn events_reach_the_file_the_ui_would_point_at() {
        let dir = scratch("writes");
        let file = Arc::new(DailyFile::new(&dir, Limits::default()).expect("open the log file"));
        let path = file.path();
        assert!(path.exists(), "the file exists before anything is logged");

        let subscriber = tracing_subscriber::registry().with(
            tracing_subscriber::fmt::layer()
                .with_ansi(false)
                .with_writer(LogWriter::new(Arc::clone(&file)))
                .with_filter(EnvFilter::new(FILE_FILTER)),
        );
        tracing::subscriber::with_default(subscriber, || {
            tracing::info!(frames = 240, "export finished for the test");
            tracing::debug!("a debug line the file filter drops");
        });

        let written = fs::read_to_string(&path).expect("read the log back");
        assert!(
            written.contains("export finished for the test"),
            "the INFO event is in the file: {written:?}"
        );
        assert!(
            written.contains("frames=240"),
            "structured fields survive: {written:?}"
        );
        assert!(
            !written.contains("a debug line"),
            "the file filter is INFO and above: {written:?}"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    /// A second handle on the same day appends rather than truncating — two
    /// windows, or a restart, must not eat the first run's log.
    #[test]
    fn a_second_run_on_the_same_day_appends() {
        let dir = scratch("append");
        {
            let file = DailyFile::new(&dir, Limits::default()).expect("first run");
            LogWriter::new(Arc::new(file))
                .write_all(b"first\n")
                .unwrap();
        }
        let file = DailyFile::new(&dir, Limits::default()).expect("second run");
        let path = file.path();
        LogWriter::new(Arc::new(file))
            .write_all(b"second\n")
            .unwrap();

        assert_eq!(fs::read_to_string(&path).unwrap(), "first\nsecond\n");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn only_the_newest_files_survive() {
        let dir = scratch("prune");
        fs::create_dir_all(&dir).unwrap();
        for day in ["2026-07-01", "2026-07-02", "2026-07-03", "2026-07-04"] {
            fs::write(dir.join(format!("{PREFIX}{day}{SUFFIX}")), b"old\n").unwrap();
        }
        // Something else's file in the same directory is left alone.
        fs::write(dir.join("notes.txt"), b"keep me\n").unwrap();

        let limits = Limits {
            max_files: 2,
            ..Limits::default()
        };
        let file = DailyFile::new(&dir, limits).expect("open with a two-file limit");
        let surviving: Vec<String> = fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|name| name.starts_with(PREFIX))
            .collect();

        assert_eq!(
            surviving.len(),
            2,
            "the newest two of five, one of which is today's: {surviving:?}"
        );
        assert!(
            surviving.contains(
                &file
                    .path()
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .to_string()
            ),
            "today's file is never pruned: {surviving:?}"
        );
        assert!(
            surviving.contains(&format!("{PREFIX}2026-07-04{SUFFIX}")),
            "the newest old file is the one kept: {surviving:?}"
        );
        assert!(
            dir.join("notes.txt").exists(),
            "foreign files are not ours to delete"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    fn ours_in(dir: &Path) -> Vec<(String, u64)> {
        let mut files: Vec<(String, u64)> = fs::read_dir(dir)
            .unwrap()
            .flatten()
            .filter_map(|e| {
                let name = e.file_name().to_string_lossy().to_string();
                parse_name(&name)?;
                Some((name, e.metadata().unwrap().len()))
            })
            .collect();
        files.sort();
        files
    }

    #[test]
    fn names_parse_and_sort_by_day_then_part() {
        let day = 19_723 + 59;
        assert_eq!(part_name(day, 1), "chukcut-2024-02-29.log");
        assert_eq!(part_name(day, 2), "chukcut-2024-02-29.2.log");
        assert_eq!(parse_name("chukcut-2024-02-29.log"), Some((day, 1)));
        assert_eq!(parse_name("chukcut-2024-02-29.10.log"), Some((day, 10)));
        // As strings these sort the wrong way round; as keys they do not.
        assert!(parse_name("chukcut-2024-02-29.2.log") > parse_name("chukcut-2024-02-29.log"));
        assert!(parse_name("chukcut-2024-02-29.10.log") > parse_name("chukcut-2024-02-29.2.log"));
        assert!(parse_name("chukcut-2024-03-01.log") > parse_name("chukcut-2024-02-29.10.log"));
        for foreign in [
            "notes.txt",
            "chukcut-2024-02-29.txt",
            "chukcut-2024-13-01.log",
            "chukcut-2024-02-29.1.log",
            "chukcut-2024-02-29.x.log",
            "chukcut-24-02-29.log",
        ] {
            assert_eq!(parse_name(foreign), None, "{foreign} is not ours");
        }
        for days in [-1000, 0, 19_723, 19_782, 20_735, 100_000] {
            let (y, m, d) = civil_from_days(days);
            assert_eq!(days_from_civil(y, m, d), days);
        }
    }

    /// A file at its cap rolls over to the next part, and a restart appends
    /// to the newest part, not to the full first one.
    #[test]
    fn a_full_file_rolls_over_to_the_next_part() {
        let dir = scratch("roll");
        let limits = Limits {
            max_file_bytes: 100,
            max_total_bytes: 10_000,
            max_files: 100,
        };
        let file = Arc::new(DailyFile::new(&dir, limits).unwrap());
        let first = file.path();
        let mut writer = LogWriter::new(Arc::clone(&file));
        // One write per line, as the `fmt` layer does it.
        let mut line = vec![b'x'; 39];
        line.push(b'\n');
        for _ in 0..5 {
            writer.write_all(&line).unwrap();
        }
        let second = file.path();
        assert_ne!(first, second, "200 bytes do not fit in one 100-byte file");
        assert!(second.to_string_lossy().ends_with(".3.log"), "{second:?}");
        for (name, size) in ours_in(&dir) {
            assert!(size <= 100, "{name} is {size} bytes, over the cap");
        }
        // Whole lines only: the cap rolls *before* a write, never inside one.
        assert_eq!(fs::read_to_string(&first).unwrap().len(), 80);

        drop(writer);
        drop(file);
        let again = DailyFile::new(&dir, limits).unwrap();
        assert_eq!(again.path(), second, "a restart continues the newest part");
        let _ = fs::remove_dir_all(&dir);
    }

    /// The directory never holds more than the total, however much is
    /// written, and the oldest files are the ones that go.
    #[test]
    fn the_directory_stays_under_its_total() {
        let dir = scratch("total");
        fs::create_dir_all(&dir).unwrap();
        // A week of old logs, 120 bytes each.
        for day in 1..=7 {
            fs::write(
                dir.join(format!("{PREFIX}2026-07-0{day}{SUFFIX}")),
                [b'o'; 120],
            )
            .unwrap();
        }
        let limits = Limits {
            max_file_bytes: 200,
            max_total_bytes: 1000,
            max_files: 100,
        };
        let file = Arc::new(DailyFile::new(&dir, limits).unwrap());
        let mut writer = LogWriter::new(Arc::clone(&file));
        let mut line = vec![b'n'; 49];
        line.push(b'\n');
        for _ in 0..400 {
            writer.write_all(&line).unwrap();
            let total: u64 = ours_in(&dir).iter().map(|(_, size)| size).sum();
            assert!(total <= 1000, "the directory holds {total} bytes");
        }
        let left = ours_in(&dir);
        assert!(
            left.iter().all(|(name, _)| !name.contains("2026-07")),
            "20 KB of new log pushed every old file out: {left:?}"
        );
        assert!(
            left.len() >= 4,
            "and kept as many new parts as fit: {left:?}"
        );
        let _ = fs::remove_dir_all(&dir);
    }
}
