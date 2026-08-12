use crate::config::{AppPaths, shell_quote};
use anyhow::{Context, Result};
use serde_json::{Value, json};
use std::fs;
use std::io::{self, Read};
use std::os::unix::net::UnixDatagram;
use std::path::{Path, PathBuf};

const EVENTS: &[&str] = &[
    "SessionStart",
    "SessionEnd",
    "UserPromptSubmit",
    "PreToolUse",
    "PostToolUse",
    "PostToolUseFailure",
    "PermissionRequest",
    "Stop",
    "SubagentStart",
    "SubagentStop",
];
const MARKER: &str = "codex-ble-bridge";

pub fn forward_stdin(paths: &AppPaths) -> Result<()> {
    let mut input = Vec::new();
    io::stdin().read_to_end(&mut input)?;
    if input.is_empty() || !paths.socket.exists() {
        return Ok(());
    }
    let socket = UnixDatagram::unbound()?;
    // Hooks must never block or break a Codex task when the bridge is offline.
    let _ = socket.send_to(&input, &paths.socket);
    Ok(())
}

pub fn install(binary: &Path) -> Result<PathBuf> {
    let home = dirs::home_dir().context("cannot find home directory")?;
    let codex_dir = home.join(".codex");
    fs::create_dir_all(&codex_dir)?;
    let path = codex_dir.join("hooks.json");
    let mut root: Value = if path.exists() {
        serde_json::from_slice(&fs::read(&path)?)
            .with_context(|| format!("parse {}", path.display()))?
    } else {
        json!({"description": "Global Codex hooks"})
    };
    if !root.is_object() {
        anyhow::bail!("{} 必须是 JSON object", path.display());
    }
    let hooks = root
        .as_object_mut()
        .unwrap()
        .entry("hooks")
        .or_insert_with(|| json!({}));
    if !hooks.is_object() {
        *hooks = json!({});
    }
    let command = format!("{} hook", shell_quote(binary));
    for event in EVENTS {
        let entries = hooks
            .as_object_mut()
            .unwrap()
            .entry((*event).to_owned())
            .or_insert_with(|| json!([]));
        if !entries.is_array() {
            *entries = json!([]);
        }
        remove_ours(entries);
        entries.as_array_mut().unwrap().push(json!({
            "hooks": [{
                "type": "command",
                "command": command,
                // This command only forwards one datagram to the local service.
                // Codex versions that do not support async hooks skip entries
                // containing `async: true` entirely, so keep it synchronous.
                "timeout": 3
            }]
        }));
    }
    write_json_atomic(&path, &root)?;
    Ok(path)
}

pub fn uninstall() -> Result<Option<PathBuf>> {
    let Some(home) = dirs::home_dir() else {
        return Ok(None);
    };
    let path = home.join(".codex/hooks.json");
    if !path.exists() {
        return Ok(None);
    }
    let mut root: Value = serde_json::from_slice(&fs::read(&path)?)?;
    if let Some(hooks) = root.get_mut("hooks").and_then(Value::as_object_mut) {
        for entries in hooks.values_mut() {
            remove_ours(entries);
        }
        hooks.retain(|_, entries| entries.as_array().is_none_or(|a| !a.is_empty()));
    }
    write_json_atomic(&path, &root)?;
    Ok(Some(path))
}

fn remove_ours(entries: &mut Value) {
    let Some(array) = entries.as_array_mut() else {
        return;
    };
    array.retain(|entry| !entry.to_string().contains(MARKER));
}

fn write_json_atomic(path: &Path, value: &Value) -> Result<()> {
    let temp = path.with_extension("json.tmp");
    let result = (|| -> Result<()> {
        fs::write(&temp, serde_json::to_vec_pretty(value)?)?;
        fs::rename(&temp, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marker_is_detected_in_generated_command() {
        let command = format!("{} hook", shell_quote(Path::new("/tmp/codex-ble-bridge")));
        assert!(command.contains(MARKER));
        assert!(command.ends_with(" hook"));
    }
}
