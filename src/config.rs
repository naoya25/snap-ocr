use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

pub const DEFAULT_MODEL: &str = "gemini-2.5-flash";
pub const FALLBACK_MODELS: &[&str] = &["gemini-2.5-flash", "gpt-5.4-nano", "claude-4-5-haiku"];

/// Persisted, non-secret app configuration.
///
/// NOTE: the API key is never written here — it only ever comes from the
/// `JAPANAI_API_KEY` env var or the macOS Keychain.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Config {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

fn config_dir() -> Result<PathBuf> {
    let base = dirs::config_dir().context("could not resolve Application Support directory")?;
    Ok(base.join("snap-ocr"))
}

fn config_path() -> Result<PathBuf> {
    Ok(config_dir()?.join("config.json"))
}

pub fn cache_dir() -> Result<PathBuf> {
    let base = dirs::cache_dir().context("could not resolve Caches directory")?;
    Ok(base.join("snap-ocr"))
}

impl Config {
    pub fn load() -> Result<Self> {
        let path = config_path()?;
        if !path.exists() {
            return Ok(Self::default());
        }
        let raw = std::fs::read_to_string(&path)
            .with_context(|| format!("failed to read {}", path.display()))?;
        let cfg: Config = serde_json::from_str(&raw)
            .with_context(|| format!("failed to parse {}", path.display()))?;
        Ok(cfg)
    }

    pub fn save(&self) -> Result<()> {
        let dir = config_dir()?;
        std::fs::create_dir_all(&dir)
            .with_context(|| format!("failed to create {}", dir.display()))?;
        let path = config_path()?;
        let raw = serde_json::to_string_pretty(self)?;
        std::fs::write(&path, raw)
            .with_context(|| format!("failed to write {}", path.display()))?;
        Ok(())
    }

    pub fn set_model(&mut self, model: &str) -> Result<()> {
        self.model = Some(model.to_string());
        self.save()
    }

    pub fn set_user_id(&mut self, user_id: &str) -> Result<()> {
        self.user_id = Some(user_id.to_string());
        self.save()
    }
}

/// Resolve the JAPAN AI API key: env var first, then macOS Keychain.
/// Returns `None` if neither is available (never logs/prints the value).
pub fn resolve_api_key() -> Option<String> {
    if let Ok(key) = std::env::var("JAPANAI_API_KEY") {
        if !key.trim().is_empty() {
            return Some(key);
        }
    }

    let output = std::process::Command::new("security")
        .args(["find-generic-password", "-s", "snap-ocr", "-a", "api-key", "-w"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let key = String::from_utf8(output.stdout).ok()?;
    let key = key.trim().to_string();
    if key.is_empty() {
        None
    } else {
        Some(key)
    }
}

/// Resolve the userId: env var first, then config.json.
/// If resolved via env and different from what's stored, persist it for
/// convenience on future runs (userId is not a secret).
pub fn resolve_user_id(config: &mut Config) -> Option<String> {
    if let Ok(user_id) = std::env::var("JAPANAI_USER_ID") {
        let user_id = user_id.trim().to_string();
        if !user_id.is_empty() {
            if config.user_id.as_deref() != Some(user_id.as_str()) {
                let _ = config.set_user_id(&user_id);
            }
            return Some(user_id);
        }
    }
    config.user_id.clone()
}

pub const SETUP_HELP: &str = "\
snap-ocr のセットアップが未完了です。

1. APIキーを設定してください（どちらか一方）:
   環境変数: export JAPANAI_API_KEY=\"sk-...\"
   または macOS Keychain に保存:
     security add-generic-password -s snap-ocr -a api-key -w \"sk-...\"

2. userId（マイページのメールアドレス）を設定してください（どちらか一方）:
   環境変数: export JAPANAI_USER_ID=\"you@example.com\"
   または ~/Library/Application Support/snap-ocr/config.json に \"user_id\" を記載
";
