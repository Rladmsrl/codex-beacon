use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

pub const PRODUCT_NAME: &str = "Codex Beacon";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedDevice {
    pub id: String,
    pub name: String,
    pub last_seen: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    #[serde(default)]
    pub auto_pair: bool,
    #[serde(default)]
    pub devices: Vec<SavedDevice>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            auto_pair: false,
            devices: Vec::new(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct AppPaths {
    pub support: PathBuf,
    pub settings: PathBuf,
    pub socket: PathBuf,
    pub lock: PathBuf,
    pub log: PathBuf,
}

impl AppPaths {
    pub fn discover() -> Result<Self> {
        let base = dirs::data_dir().context("cannot find macOS Application Support directory")?;
        let support = base.join(PRODUCT_NAME);
        Ok(Self {
            settings: support.join("settings.json"),
            socket: support.join("events.sock"),
            lock: support.join("service.lock"),
            log: support.join("bridge.log"),
            support,
        })
    }

    pub fn ensure(&self) -> Result<()> {
        fs::create_dir_all(&self.support)
            .with_context(|| format!("create {}", self.support.display()))
    }
}

pub fn load(paths: &AppPaths) -> Result<Settings> {
    if !paths.settings.exists() {
        return Ok(Settings::default());
    }
    let bytes = fs::read(&paths.settings)?;
    serde_json::from_slice(&bytes).context("parse settings.json")
}

pub fn save(paths: &AppPaths, settings: &Settings) -> Result<()> {
    paths.ensure()?;
    let temp = paths.settings.with_extension("json.tmp");
    let result = (|| -> Result<()> {
        fs::write(&temp, serde_json::to_vec_pretty(settings)?)?;
        fs::rename(&temp, &paths.settings)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

pub fn find_codex_binary() -> Result<PathBuf> {
    let mut candidates = vec![
        PathBuf::from("/Applications/ChatGPT.app/Contents/Resources/codex"),
        PathBuf::from("/Applications/Codex.app/Contents/Resources/codex"),
    ];
    if let Some(home) = dirs::home_dir() {
        candidates.push(home.join("Applications/ChatGPT.app/Contents/Resources/codex"));
        candidates.push(home.join("Applications/Codex.app/Contents/Resources/codex"));
    }
    if let Some(path) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path) {
            candidates.push(dir.join("codex"));
        }
    }
    for candidate in candidates {
        if candidate.is_file() {
            return Ok(candidate);
        }
    }
    bail!("未找到 Codex。请先安装默认位置的 Codex/ChatGPT Mac App")
}

pub fn shell_quote(path: &Path) -> String {
    format!("'{}'", path.display().to_string().replace('\'', "'\\''"))
}
