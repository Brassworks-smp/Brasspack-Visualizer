
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone)]
pub struct SavedFile {
    pub path: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub mode: String,
}

fn default_true() -> bool {
    true
}

#[derive(Serialize, Deserialize)]
pub struct Settings {
    #[serde(default)]
    pub files: Vec<SavedFile>,
    #[serde(default = "default_zoom")]
    pub zoom: f32,
    #[serde(default)]
    pub mode: String,
}

fn default_zoom() -> f32 {
    34.0
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            files: Vec::new(),
            zoom: 34.0,
            mode: "Auto".into(),
        }
    }
}

fn path() -> std::path::PathBuf {
    if let Some(home) = std::env::var_os("HOME") {
        return std::path::Path::new(&home).join(".backpack_infiltrator.json");
    }
    std::path::PathBuf::from(".backpack_infiltrator.json")
}

impl Settings {
    pub fn load() -> Settings {
        std::fs::read_to_string(path())
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) {
        if let Ok(json) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(path(), json);
        }
    }
}
