//! User config in `~/.config/daw/config.json` (or `$XDG_CONFIG_HOME/daw`).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Key chords per action id, e.g. `"focus-left": ["alt+h", "alt+left"]`.
    pub keys: BTreeMap<String, Vec<String>>,
    pub autosave_minutes: u64,
    pub midi_input: Option<String>,
}

impl Default for Config {
    fn default() -> Self { Self { keys: BTreeMap::new(), autosave_minutes: 2, midi_input: None } }
}

pub fn path() -> PathBuf {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| dirs::home_dir().map(|home| home.join(".config")))
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("daw").join("config.json")
}

/// Read the config. A missing file gives the defaults; an unreadable one
/// gives the defaults and an error to show.
pub fn load(path: &Path) -> (Config, Option<String>) {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return (Config::default(), None),
        Err(error) => return (Config::default(), Some(format!("could not read {}: {error}", path.display()))),
    };
    match serde_json::from_str(&text) {
        Ok(config) => (config, None),
        Err(error) => (Config::default(), Some(format!("could not parse {}: {error}", path.display()))),
    }
}

/// Write through a temporary file so a crash never leaves half a config.
pub fn save(path: &Path, config: &Config) -> Result<(), String> {
    let dir = path.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let text = serde_json::to_string_pretty(config).map_err(|e| e.to_string())?;
    let temp = path.with_extension("json.tmp");
    std::fs::write(&temp, text + "\n").map_err(|e| e.to_string())?;
    std::fs::rename(&temp, path).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_file_is_default_and_save_round_trips() {
        let dir = std::env::temp_dir().join(format!("daw-config-test-{}", std::process::id()));
        let path = dir.join("nested").join("config.json");
        assert_eq!(load(&path), (Config::default(), None));
        let mut config = Config::default();
        config.keys.insert("zoom".into(), vec!["alt+z".into()]);
        save(&path, &config).unwrap();
        assert_eq!(load(&path), (config, None));
        std::fs::write(&path, "{ not json").unwrap();
        let (loaded, error) = load(&path);
        assert_eq!(loaded, Config::default());
        assert!(error.is_some());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
