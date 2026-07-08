use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};

use crate::config::cache_dir;

/// Runs `screencapture -i -x` (interactive selection, no shutter sound) and
/// returns the path to the resulting PNG, or `Ok(None)` if the user cancelled
/// (Esc) and no file was produced.
pub fn capture_screenshot() -> Result<Option<PathBuf>> {
    let dir = cache_dir()?;
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("failed to create {}", dir.display()))?;

    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let path = dir.join(format!("capture-{nanos}.png"));

    let status = std::process::Command::new("screencapture")
        .args(["-i", "-x"])
        .arg(&path)
        .status()
        .context("failed to spawn screencapture")?;

    if !status.success() || !path.exists() {
        // User pressed Esc / cancelled the selection: no file was written.
        cleanup(&path);
        return Ok(None);
    }

    Ok(Some(path))
}

pub fn cleanup(path: &Path) {
    let _ = std::fs::remove_file(path);
}
