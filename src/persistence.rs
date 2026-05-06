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
    // Always in user home directory
    dirs_fallback().join(FILE_NAME)
}

fn dirs_fallback() -> PathBuf {
    // USERPROFILE on Windows, HOME on Unix
    std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("HOME"))
        .map(PathBuf::from)
        .unwrap_or_else(|_| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
}

pub fn save(config: &AppConfig) -> io::Result<()> {
    let path = config_path();
    let json = serde_json::to_string_pretty(config)
        .map_err(io::Error::other)?;
    fs::write(&path, json)
}

pub fn load() -> AppConfig {
    let path = config_path();
    if !path.exists() {
        // Try migrating old macros.json
        return migrate_old_format();
    }
    let Ok(data) = fs::read_to_string(&path) else {
        return AppConfig::default();
    };
    serde_json::from_str(&data).unwrap_or_default()
}

/// Migrate from the old macros.json format (just a Vec<Macro>).
fn migrate_old_format() -> AppConfig {
    // Check exe dir and current dir for old macros.json
    let candidates = [
        std::env::current_exe().ok().and_then(|p| Some(p.parent()?.join("macros.json"))),
        Some(std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")).join("macros.json")),
    ];
    for candidate in candidates.into_iter().flatten() {
        if candidate.exists() {
            if let Ok(data) = fs::read_to_string(&candidate) {
                if let Ok(macros) = serde_json::from_str::<Vec<Macro>>(&data) {
                    let config = AppConfig { macros, ..AppConfig::default() };
                    // Save in new format and remove old file
                    let _ = save(&config);
                    let _ = fs::remove_file(&candidate);
                    return config;
                }
            }
        }
    }
    AppConfig::default()
}
