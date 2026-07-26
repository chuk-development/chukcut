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
//! It is a date comparison and a directory listing, and owning it means the
//! current file's *path* is something we can hand to the UI without guessing at
//! another crate's naming scheme. Files are named `chukcut-YYYY-MM-DD.log`, one
//! per day the app is used, and the newest [`KEEP_FILES`] survive — a log
//! directory that grows without bound is its own bug report.
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
/// The `chukcut` prefix matches `chukcut_lib::…` too — `EnvFilter` compares
/// target prefixes, and the library half of this crate is where nearly every
/// event is raised.
const FILE_FILTER: &str = "chukcut=info,warn";

/// How many daily files to keep. A week of use is enough to cover "it broke on
/// Friday" reported on Monday, and costs a few hundred kilobytes.
const KEEP_FILES: usize = 7;

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
    let opened = DailyFile::new(paths::logs_dir(), KEEP_FILES);
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
        (None, Some(path)) => tracing::info!(path = %path.display(), "logging to file"),
        (None, None) => {}
    }
}

/// The file this run is writing to, for the UI and for anyone about to ask a
/// user to send it.
pub fn log_file() -> Option<PathBuf> {
    CURRENT.get()?.as_ref().map(|file| file.path())
}

// ---------------------------------------------------------------------------
// The appender
// ---------------------------------------------------------------------------

/// An append-only file that rolls over at midnight UTC and prunes its
/// predecessors.
pub struct DailyFile {
    dir: PathBuf,
    keep: usize,
    open: Mutex<Open>,
}

struct Open {
    /// Days since the Unix epoch, UTC. The whole rotation rule is "has this
    /// changed".
    day: i64,
    file: File,
    path: PathBuf,
}

impl DailyFile {
    /// Create `dir` if it is missing, open today's file for appending, and drop
    /// everything but the newest `keep` files.
    pub fn new(dir: impl Into<PathBuf>, keep: usize) -> io::Result<Self> {
        let dir = dir.into();
        fs::create_dir_all(&dir)?;
        let day = today();
        let (file, path) = open_for(&dir, day)?;
        let this = Self {
            dir,
            keep,
            open: Mutex::new(Open { day, file, path }),
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
        self.open.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn append(&self, buf: &[u8]) -> io::Result<usize> {
        let mut rolled = false;
        let written = {
            let mut open = self.lock();
            let day = today();
            if day != open.day {
                // A failed rollover keeps yesterday's file rather than losing
                // the line: a log in the wrong file is recoverable, a dropped
                // one is not.
                if let Ok((file, path)) = open_for(&self.dir, day) {
                    *open = Open { day, file, path };
                    rolled = true;
                }
            }
            open.file.write(buf)?
        };
        if rolled {
            self.prune();
        }
        Ok(written)
    }

    /// Delete all but the newest `keep` files. Names sort as dates do, which is
    /// the reason for the `YYYY-MM-DD` spelling.
    fn prune(&self) {
        if self.keep == 0 {
            return;
        }
        let Ok(entries) = fs::read_dir(&self.dir) else {
            return;
        };
        let mut ours: Vec<PathBuf> = entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| is_ours(path))
            .collect();
        if ours.len() <= self.keep {
            return;
        }
        ours.sort();
        for stale in &ours[..ours.len() - self.keep] {
            // A file we cannot delete is not worth failing a log write over,
            // and the next run will try again.
            let _ = fs::remove_file(stale);
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

fn is_ours(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.starts_with(PREFIX) && name.ends_with(SUFFIX))
}

fn open_for(dir: &Path, day: i64) -> io::Result<(File, PathBuf)> {
    let path = dir.join(file_name(day));
    let file = OpenOptions::new().create(true).append(true).open(&path)?;
    Ok((file, path))
}

pub fn file_name(day: i64) -> String {
    let (year, month, dom) = civil_from_days(day);
    format!("{PREFIX}{year:04}-{month:02}-{dom:02}{SUFFIX}")
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
        let file = Arc::new(DailyFile::new(&dir, KEEP_FILES).expect("open the log file"));
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
            let file = DailyFile::new(&dir, KEEP_FILES).expect("first run");
            LogWriter::new(Arc::new(file)).write_all(b"first\n").unwrap();
        }
        let file = DailyFile::new(&dir, KEEP_FILES).expect("second run");
        let path = file.path();
        LogWriter::new(Arc::new(file)).write_all(b"second\n").unwrap();

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

        let file = DailyFile::new(&dir, 2).expect("open with a two-file limit");
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
            surviving.contains(&file.path().file_name().unwrap().to_string_lossy().to_string()),
            "today's file is never pruned: {surviving:?}"
        );
        assert!(dir.join("notes.txt").exists(), "foreign files are not ours to delete");

        let _ = fs::remove_dir_all(&dir);
    }
}
