use std::fs;
use std::io;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::macro_def::Macro;

const FILE_NAME: &str = ".tapmatic.json";

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

/// Resolve the config directory: `USERPROFILE` on Windows, `HOME` on Unix.
///
/// With neither set the config lands in the working directory, so the file
/// read depends on where the binary was launched from.
fn dirs_fallback() -> PathBuf {
    std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("HOME"))
        .map(PathBuf::from)
        .unwrap_or_else(|_| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
}

/// Write the config to disk atomically.
///
/// Serializes to a sibling `.tmp` file and renames it over the target, so an
/// interrupted write cannot truncate an existing config.
pub fn save(config: &AppConfig) -> io::Result<()> {
    let path = config_path();
    let json = serde_json::to_string_pretty(config)
        .map_err(io::Error::other)?;
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, json)?;
    fs::rename(&tmp, &path)
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

/// Read the config from the user's home directory.
///
/// Never panics and never returns an error type: a missing file yields
/// `Fresh`, and an unreadable or malformed one yields `Corrupt` rather than a
/// default config, so callers can tell "nothing saved yet" from "something is
/// there but we could not read it".
pub fn load() -> LoadOutcome {
    let path = config_path();
    if !path.exists() {
        return LoadOutcome::Fresh(migrate_old_format());
    }
    let data = match fs::read_to_string(&path) {
        Ok(data) => data,
        Err(e) => return LoadOutcome::Corrupt(format!("could not read {}: {e}", path.display())),
    };
    match serde_json::from_str::<AppConfig>(&data) {
        Ok(mut config) => {
            for mac in &mut config.macros {
                mac.clamp_to_valid_ranges();
            }
            LoadOutcome::Loaded(config)
        }
        Err(e) => LoadOutcome::Corrupt(format!("{} is not valid JSON: {e}", path.display())),
    }
}

/// Adopt macros from the pre-`AppConfig` `macros.json` format, if one exists.
///
/// Deletes the legacy file only once the new config has been written
/// successfully; if the write fails the original is left untouched. Returns a
/// default config when there is nothing to migrate.
fn migrate_old_format() -> AppConfig {
    let candidates = [
        std::env::current_exe().ok().and_then(|p| Some(p.parent()?.join("macros.json"))),
        Some(std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")).join("macros.json")),
    ];
    for candidate in candidates.into_iter().flatten() {
        if candidate.exists()
            && let Ok(data) = fs::read_to_string(&candidate)
                && let Ok(mut macros) = serde_json::from_str::<Vec<Macro>>(&data) {
                    for mac in &mut macros {
                        mac.clamp_to_valid_ranges();
                    }
                    let config = AppConfig { macros, ..AppConfig::default() };
                    if save(&config).is_ok() {
                        let _ = fs::remove_file(&candidate);
                    }
                    return config;
                }
    }
    AppConfig::default()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Round-trip through the on-disk envelope the binary actually writes,
    /// rather than a bare Vec<Macro>.
    #[test]
    fn app_config_round_trip_preserves_settings_and_macros() {
        let config = AppConfig {
            audio_enabled: false,
            macros_enabled: false,
            speed_multiplier: 2.5,
            macros: vec![Macro::new("first".into()), Macro::new("second".into())],
        };

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
}
