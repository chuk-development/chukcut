//! The key store: `secrets.toml`, owner-only.
//!
//! One table, account id → key. Nothing else lives here, so the file holds
//! exactly what must be protected and the account settings beside it
//! (`accounts.toml`) can be read, shared and diffed freely.
//!
//! - Written to a temporary file created with mode 0600, synced, then renamed
//!   over the old one: a crash cannot leave a half-written or world-readable
//!   key file. The directory is set to 0700.
//! - At load, a file that the group or others can read is tightened and a
//!   warning logged. Loading never fails the app.
//! - Never in a project file, the cache or a log. `Debug` prints key names
//!   only.
//! - `CHUKCUT_KEY_<ACCOUNT>` in the environment overrides the file and is
//!   never written back — for the CLI and MCP shells, and for CI.

use std::collections::BTreeMap;
use std::io::Write as _;
use std::path::Path;

use serde::{Deserialize, Serialize};

#[derive(Default, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Secrets {
    #[serde(default)]
    keys: BTreeMap<String, String>,
}

impl std::fmt::Debug for Secrets {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Secrets")
            .field("accounts", &self.keys.keys().collect::<Vec<_>>())
            .finish()
    }
}

impl Secrets {
    pub fn load(path: &Path) -> Self {
        let Ok(raw) = std::fs::read_to_string(path) else {
            return Self::default();
        };
        tighten(path);
        toml::from_str(&raw).unwrap_or_else(|error| {
            // The message names the file, never its content.
            tracing::warn!(file = %path.display(), %error, "the key store does not parse; starting empty");
            Self::default()
        })
    }

    pub fn save(&self, path: &Path) -> Result<(), String> {
        let text = toml::to_string(self).map_err(|e| e.to_string())?;
        write_private(path, text.as_bytes())
    }

    /// The key for `account`: the environment first, then the file.
    pub fn key(&self, account: &str) -> Option<String> {
        if let Ok(value) = std::env::var(env_name(account)) {
            if !value.is_empty() {
                return Some(value);
            }
        }
        self.keys.get(account).filter(|k| !k.is_empty()).cloned()
    }

    pub fn set(&mut self, account: &str, key: Option<String>) {
        match key.filter(|k| !k.trim().is_empty()) {
            Some(key) => {
                self.keys
                    .insert(account.to_string(), key.trim().to_string());
            }
            None => {
                self.keys.remove(account);
            }
        }
    }

    pub fn has(&self, account: &str) -> bool {
        self.key(account).is_some()
    }
}

/// `CHUKCUT_KEY_` and the account id, upper case, with anything that is not
/// a letter or digit as `_`.
pub fn env_name(account: &str) -> String {
    let id: String = account
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_uppercase()
            } else {
                '_'
            }
        })
        .collect();
    format!("CHUKCUT_KEY_{id}")
}

/// The last four characters, for the settings UI.
pub fn masked(key: &str) -> String {
    let tail: String = key
        .chars()
        .rev()
        .take(4)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    if key.chars().count() <= 4 {
        "••••".to_string()
    } else {
        format!("••••{tail}")
    }
}

/// Write `bytes` to `path` so that only the owner can ever read it.
pub(crate) fn write_private(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let parent = path.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(parent)
        .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let _ = std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700));
    }

    let temp = path.with_extension("toml.part");
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    let mut file = options
        .open(&temp)
        .map_err(|e| format!("cannot write {}: {e}", temp.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        // `mode` applies only when the file is created; a `.part` left by a
        // crash keeps whatever it had.
        file.set_permissions(std::fs::Permissions::from_mode(0o600))
            .map_err(|e| e.to_string())?;
    }
    file.write_all(bytes)
        .and_then(|_| file.sync_all())
        .map_err(|e| format!("cannot write {}: {e}", temp.display()))?;
    drop(file);
    std::fs::rename(&temp, path).map_err(|e| format!("cannot write {}: {e}", path.display()))
}

/// Make a key file that others can read owner-only again.
fn tighten(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let Ok(meta) = std::fs::metadata(path) else {
            return;
        };
        if meta.permissions().mode() & 0o077 != 0 {
            tracing::warn!(file = %path.display(), "the key store was readable by others; making it owner-only");
            let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    pub(crate) fn scratch(name: &str) -> PathBuf {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/test-scratch/cloud")
            .join(name);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[cfg(unix)]
    fn mode(path: &Path) -> u32 {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    #[test]
    fn keys_are_written_owner_only_and_read_back() {
        let dir = scratch("secrets-roundtrip");
        let path = dir.join("secrets.toml");
        let mut secrets = Secrets::default();
        secrets.set("acct-1", Some("sk-one ".into()));
        secrets.save(&path).unwrap();
        #[cfg(unix)]
        {
            assert_eq!(mode(&path), 0o600);
            assert_eq!(mode(&dir), 0o700);
        }
        let back = Secrets::load(&path);
        assert_eq!(back.key("acct-1").as_deref(), Some("sk-one"));
        assert_eq!(back, secrets);

        let mut cleared = back;
        cleared.set("acct-1", None);
        assert!(!cleared.has("acct-1"));
    }

    #[test]
    fn a_readable_file_is_tightened_at_load() {
        let dir = scratch("secrets-tighten");
        let path = dir.join("secrets.toml");
        std::fs::write(&path, "[keys]\na = \"b\"\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
            assert_eq!(Secrets::load(&path).key("a").as_deref(), Some("b"));
            assert_eq!(mode(&path), 0o600);
        }
    }

    #[test]
    fn debug_never_prints_a_key() {
        let mut secrets = Secrets::default();
        secrets.set("acct-x", Some("sk-very-secret".into()));
        let printed = format!("{secrets:?}");
        assert!(!printed.contains("sk-very-secret"), "{printed}");
        assert!(printed.contains("acct-x"));
    }

    #[test]
    fn the_environment_overrides_the_file() {
        let mut secrets = Secrets::default();
        secrets.set("env-test-acct", Some("from-file".into()));
        assert_eq!(env_name("env-test-acct"), "CHUKCUT_KEY_ENV_TEST_ACCT");
        // A name no other test uses, so setting it cannot race with them.
        std::env::set_var("CHUKCUT_KEY_ENV_TEST_ACCT", "from-env");
        assert_eq!(secrets.key("env-test-acct").as_deref(), Some("from-env"));
        std::env::remove_var("CHUKCUT_KEY_ENV_TEST_ACCT");
        assert_eq!(secrets.key("env-test-acct").as_deref(), Some("from-file"));
    }

    #[test]
    fn masking_shows_the_last_four_only() {
        assert_eq!(masked("sk-1234567890abcd"), "••••abcd");
        assert_eq!(masked("abc"), "••••");
    }
}
