use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::macro_def::{Macro, MAX_MACROS};

pub const FILE_NAME: &str = ".tapmatic.json";

/// Refuse config files larger than this before parsing: the file is the
/// untrusted-input boundary, and all clamping runs only after `serde_json`
/// has fully materialized the document.
const MAX_CONFIG_BYTES: u64 = 64 * 1024 * 1024;

/// Top-level config file: settings + macros in one file.
#[derive(Serialize, Deserialize)]
pub struct AppConfig {
    #[serde(default)]
    pub audio_enabled: bool,
    #[serde(default = "default_true")]
    pub macros_enabled: bool,
    #[serde(default = "default_f64_one")]
    pub speed_multiplier: f64,
    #[serde(default)]
    pub macros: Vec<Macro>,
}

fn default_true() -> bool { true }
fn default_f64_one() -> f64 { 1.0 }

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            audio_enabled: true,
            macros_enabled: true,
            speed_multiplier: 1.0,
            macros: Vec::new(),
        }
    }
}

fn config_path() -> PathBuf {
    dirs_fallback().join(FILE_NAME)
}

fn exe_dir() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf))
}

/// Resolve the config directory: the platform's home variable, falling back
/// to the executable's own directory.
///
/// The fallback is deliberately not the working directory: which file the app
/// reads and overwrites must not depend on where it was launched from.
fn dirs_fallback() -> PathBuf {
    #[cfg(windows)]
    let home = std::env::var("USERPROFILE").or_else(|_| std::env::var("HOME"));
    #[cfg(not(windows))]
    let home = std::env::var("HOME");

    home.map(PathBuf::from)
        .ok()
        .or_else(exe_dir)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Write the config to disk atomically.
///
/// Serializes to a sibling `.tmp` file and renames it over the target, so an
/// interrupted write cannot truncate an existing config. The temp file is
/// created fresh with owner-only permissions (macros can hold typed text) and
/// removed if the rename fails.
pub fn save(config: &AppConfig) -> io::Result<()> {
    save_to(&config_path(), config)
}

fn save_to(path: &Path, config: &AppConfig) -> io::Result<()> {
    let json = serde_json::to_string_pretty(config)
        .map_err(io::Error::other)?;
    let tmp = path.with_extension("json.tmp");

    // A leftover tmp from an earlier failed save must not break create_new.
    let _ = fs::remove_file(&tmp);
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);

    {
        use io::Write;
        let mut file = options.open(&tmp)?;
        file.write_all(json.as_bytes())?;
    }
    fs::rename(&tmp, path).inspect_err(|_| {
        let _ = fs::remove_file(&tmp);
    })
}

/// Outcome of reading the config file, distinguishing a fresh start from a
/// failure that must not be silently overwritten.
pub enum LoadOutcome {
    Loaded(AppConfig),
    /// No config file yet: a first run, or a migration from the legacy format.
    Fresh(AppConfig),
    /// The file exists but could not be read or parsed. Carries the reason so
    /// the caller can warn the user; `macros` is empty, so saving over it would
    /// destroy whatever is still on disk.
    Corrupt(String),
}

/// Read the config from the resolved config directory (see `dirs_fallback`).
///
/// Never panics and never returns an error type: a missing file yields
/// `Fresh`, and an unreadable or malformed one yields `Corrupt` rather than a
/// default config, so callers can tell "nothing saved yet" from "something is
/// there but we could not read it".
pub fn load() -> LoadOutcome {
    let path = config_path();
    let legacy = exe_dir().map(|dir| dir.join("macros.json"));
    load_from(&path, legacy.as_deref())
}

fn load_from(path: &Path, legacy: Option<&Path>) -> LoadOutcome {
    let data = match read_config_file(path) {
        Ok(data) => data,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            return LoadOutcome::Fresh(migrate_old_format(path, legacy));
        }
        Err(e) => return LoadOutcome::Corrupt(format!("could not read {}: {e}", path.display())),
    };
    match serde_json::from_str::<AppConfig>(&data) {
        Ok(mut config) => {
            sanitize_macros(&mut config.macros);
            LoadOutcome::Loaded(config)
        }
        Err(e) => LoadOutcome::Corrupt(format!("{} is not valid JSON: {e}", path.display())),
    }
}

/// Read a config file, refusing oversized ones before any bytes are parsed.
fn read_config_file(path: &Path) -> io::Result<String> {
    let size = fs::metadata(path)?.len();
    if size > MAX_CONFIG_BYTES {
        return Err(io::Error::other(format!(
            "config file is {size} bytes, larger than the {MAX_CONFIG_BYTES}-byte limit"
        )));
    }
    fs::read_to_string(path)
}

/// Bring macros deserialized from disk into the ranges the app assumes.
fn sanitize_macros(macros: &mut Vec<Macro>) {
    macros.truncate(MAX_MACROS);
    for mac in macros {
        mac.clamp_to_valid_ranges();
    }
}

/// Adopt macros from the pre-`AppConfig` `macros.json` next to the executable,
/// if one exists.
///
/// Only the executable's own directory is searched: adopting (and deleting) a
/// `macros.json` from the working directory would let any directory the app is
/// launched from plant macros. Deletes the legacy file only once the new
/// config has been written successfully, and never overwrites a config that
/// appeared at `target` in the meantime; in both failure cases the original is
/// left untouched.
fn migrate_old_format(target: &Path, legacy: Option<&Path>) -> AppConfig {
    let Some(candidate) = legacy else {
        return AppConfig::default();
    };
    let Ok(data) = read_config_file(candidate) else {
        return AppConfig::default();
    };
    let Ok(mut macros) = serde_json::from_str::<Vec<Macro>>(&data) else {
        return AppConfig::default();
    };

    sanitize_macros(&mut macros);
    let config = AppConfig { macros, ..AppConfig::default() };
    if !target.exists() && save_to(target, &config).is_ok() {
        let _ = fs::remove_file(candidate);
    }
    config
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("tapmatic-test-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    fn sample_config() -> AppConfig {
        AppConfig {
            audio_enabled: false,
            macros_enabled: false,
            speed_multiplier: 2.5,
            macros: vec![Macro::new("first".into()), Macro::new("second".into())],
        }
    }

    #[test]
    fn oversized_config_is_corrupt_not_parsed() {
        let dir = temp_dir("oversized");
        let path = dir.join(FILE_NAME);
        fs::write(&path, "x").expect("write");
        // Grow the file past the cap without materializing 64 MB in memory.
        let f = fs::OpenOptions::new().write(true).open(&path).expect("open");
        f.set_len(MAX_CONFIG_BYTES + 1).expect("set_len");
        drop(f);

        let outcome = load_from(&path, None);

        assert!(matches!(outcome, LoadOutcome::Corrupt(ref e) if e.contains("limit")));
        let _ = fs::remove_dir_all(&dir);
    }

    /// Round-trip through the on-disk envelope the binary actually writes,
    /// rather than a bare Vec<Macro>.
    #[test]
    fn app_config_round_trip_preserves_settings_and_macros() {
        let config = sample_config();

        let json = serde_json::to_string_pretty(&config).expect("serialize");
        let parsed: AppConfig = serde_json::from_str(&json).expect("deserialize");

        assert!(!parsed.audio_enabled);
        assert!(!parsed.macros_enabled);
        assert_eq!(parsed.speed_multiplier, 2.5);
        assert_eq!(parsed.macros.len(), 2);
        assert_eq!(parsed.macros[1].name, "second");
    }

    #[test]
    fn missing_settings_fall_back_to_defaults() {
        let parsed: AppConfig = serde_json::from_str(r#"{"macros":[]}"#).expect("deserialize");

        assert!(parsed.macros_enabled, "macros_enabled defaults to true");
        assert_eq!(parsed.speed_multiplier, 1.0);
        assert!(parsed.macros.is_empty());
    }

    #[test]
    fn legacy_bare_array_is_not_a_valid_app_config() {
        let legacy = r#"[{"name":"old","steps":[],"hotkey_vk":65,"repetition":"Toggle","use_recorded_delays":true,"fixed_interval_ms":50}]"#;

        assert!(
            serde_json::from_str::<AppConfig>(legacy).is_err(),
            "legacy files must go through migrate_old_format, not parse as AppConfig"
        );
        assert!(serde_json::from_str::<Vec<Macro>>(legacy).is_ok());
    }

    #[test]
    fn malformed_json_is_rejected_rather_than_defaulted() {
        assert!(serde_json::from_str::<AppConfig>("{ not json").is_err());
        assert!(serde_json::from_str::<AppConfig>("").is_err());
    }

    #[test]
    fn save_then_load_round_trips_through_disk() {
        let dir = temp_dir("round-trip");
        let path = dir.join(FILE_NAME);

        save_to(&path, &sample_config()).expect("save");

        match load_from(&path, None) {
            LoadOutcome::Loaded(config) => {
                assert_eq!(config.speed_multiplier, 2.5);
                assert_eq!(config.macros.len(), 2);
            }
            _ => panic!("expected Loaded"),
        }
        assert!(!dir.join(".tapmatic.json.tmp").exists(), "no tmp left behind");
    }

    #[test]
    fn missing_file_is_fresh_not_corrupt() {
        let dir = temp_dir("fresh");

        match load_from(&dir.join(FILE_NAME), None) {
            LoadOutcome::Fresh(config) => assert!(config.macros.is_empty()),
            _ => panic!("expected Fresh"),
        }
    }

    #[test]
    fn corrupt_file_is_reported_not_defaulted() {
        let dir = temp_dir("corrupt");
        let path = dir.join(FILE_NAME);
        fs::write(&path, "{ not json").expect("write");

        match load_from(&path, None) {
            LoadOutcome::Corrupt(reason) => assert!(reason.contains("not valid JSON")),
            _ => panic!("expected Corrupt"),
        }
    }

    #[test]
    fn out_of_range_fields_are_clamped_on_load() {
        let dir = temp_dir("clamp");
        let path = dir.join(FILE_NAME);
        let mut config = AppConfig::default();
        let mut mac = Macro::new("m".into());
        mac.fixed_interval_ms = u64::MAX;
        mac.hotkey_vk = 9999;
        config.macros.push(mac);
        // Bypass save-side sanity by writing the raw JSON directly.
        fs::write(&path, serde_json::to_string(&config).unwrap()).expect("write");

        match load_from(&path, None) {
            LoadOutcome::Loaded(config) => {
                assert_eq!(config.macros[0].fixed_interval_ms, 60 * 60 * 1000);
                assert_eq!(config.macros[0].hotkey_vk, 0);
            }
            _ => panic!("expected Loaded"),
        }
    }

    #[test]
    fn migration_adopts_legacy_and_deletes_it_after_saving() {
        let dir = temp_dir("migrate-ok");
        let target = dir.join(FILE_NAME);
        let legacy = dir.join("macros.json");
        fs::write(&legacy, r#"[{"name":"old","steps":[],"hotkey_vk":65,"repetition":"Toggle","use_recorded_delays":true,"fixed_interval_ms":50}]"#)
            .expect("write legacy");

        match load_from(&target, Some(&legacy)) {
            LoadOutcome::Fresh(config) => assert_eq!(config.macros[0].name, "old"),
            _ => panic!("expected Fresh"),
        }
        assert!(target.exists(), "migrated config written");
        assert!(!legacy.exists(), "legacy file deleted after successful save");
    }

    #[test]
    fn migration_keeps_legacy_when_save_fails() {
        let dir = temp_dir("migrate-fail");
        let legacy = dir.join("macros.json");
        fs::write(&legacy, r#"[{"name":"old","steps":[],"hotkey_vk":65,"repetition":"Toggle","use_recorded_delays":true,"fixed_interval_ms":50}]"#)
            .expect("write legacy");
        // A target whose parent directory does not exist makes save_to fail.
        let target = dir.join("no-such-dir").join(FILE_NAME);

        match load_from(&target, Some(&legacy)) {
            LoadOutcome::Fresh(config) => assert_eq!(config.macros[0].name, "old"),
            _ => panic!("expected Fresh"),
        }
        assert!(legacy.exists(), "legacy file must survive a failed save");
    }

    #[test]
    fn migration_never_overwrites_a_config_that_appeared() {
        let dir = temp_dir("migrate-race");
        let target = dir.join(FILE_NAME);
        let legacy = dir.join("macros.json");
        fs::write(&legacy, r#"[{"name":"old","steps":[],"hotkey_vk":65,"repetition":"Toggle","use_recorded_delays":true,"fixed_interval_ms":50}]"#)
            .expect("write legacy");
        save_to(&target, &sample_config()).expect("pre-existing config");

        migrate_old_format(&target, Some(&legacy));

        match load_from(&target, None) {
            LoadOutcome::Loaded(config) => {
                assert_eq!(config.macros.len(), 2, "existing config left untouched");
            }
            _ => panic!("expected Loaded"),
        }
        assert!(legacy.exists(), "legacy file not deleted when nothing was migrated");
    }

    #[cfg(unix)]
    #[test]
    fn saved_config_is_owner_only_on_unix() {
        use std::os::unix::fs::PermissionsExt;
        let dir = temp_dir("perms");
        let path = dir.join(FILE_NAME);

        save_to(&path, &sample_config()).expect("save");

        let mode = fs::metadata(&path).expect("metadata").permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "TypeText steps can hold sensitive text");
    }
}
