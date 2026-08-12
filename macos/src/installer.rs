use crate::config::AppPaths;
use crate::hooks;
use anyhow::{Context, Result, bail};
use serde::Serialize;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

pub const LAUNCH_LABEL: &str = "cloud.zhengru.codex-beacon";
pub const MENU_LAUNCH_LABEL: &str = "cloud.zhengru.codex-beacon.menu";

#[derive(Serialize)]
#[serde(rename_all = "PascalCase")]
struct LaunchAgent {
    label: String,
    program_arguments: Vec<String>,
    run_at_load: bool,
    keep_alive: bool,
    process_type: String,
    standard_out_path: String,
    standard_error_path: String,
}

pub fn install(binary: &Path, paths: &AppPaths, start_now: bool) -> Result<PathBuf> {
    paths.ensure()?;
    let binary = binary
        .canonicalize()
        .with_context(|| format!("resolve {}", binary.display()))?;
    hooks::install(&binary)?;
    let agent_path = launch_agent_path()?;
    if let Some(parent) = agent_path.parent() {
        fs::create_dir_all(parent)?;
    }
    let agent = LaunchAgent {
        label: LAUNCH_LABEL.to_owned(),
        program_arguments: vec![binary.display().to_string(), "run".to_owned()],
        run_at_load: true,
        keep_alive: true,
        process_type: "Background".to_owned(),
        standard_out_path: paths.log.display().to_string(),
        standard_error_path: paths.log.display().to_string(),
    };
    plist::to_file_xml(&agent_path, &agent)?;
    let menu_agent_path = menu_launch_agent_path()?;
    let menu_agent = LaunchAgent {
        label: MENU_LAUNCH_LABEL.to_owned(),
        program_arguments: vec![binary.display().to_string(), "menu".to_owned()],
        run_at_load: true,
        keep_alive: false,
        process_type: "Interactive".to_owned(),
        standard_out_path: paths.log.display().to_string(),
        standard_error_path: paths.log.display().to_string(),
    };
    plist::to_file_xml(&menu_agent_path, &menu_agent)?;
    if start_now {
        start_launch_agent(&agent_path)?;
    }
    Ok(agent_path)
}

pub fn uninstall() -> Result<()> {
    stop_service()?;
    let domain = launch_domain()?;
    let _ = Command::new("launchctl")
        .args(["bootout", &format!("{domain}/{MENU_LAUNCH_LABEL}")])
        .status();
    let path = launch_agent_path()?;
    if path.exists() {
        fs::remove_file(path)?;
    }
    let menu_path = menu_launch_agent_path()?;
    if menu_path.exists() {
        fs::remove_file(menu_path)?;
    }
    hooks::uninstall()?;
    Ok(())
}

pub fn start_service() -> Result<()> {
    let path = launch_agent_path()?;
    if !path.exists() {
        bail!("后台服务尚未安装")
    }
    start_launch_agent(&path)
}

pub fn stop_service() -> Result<()> {
    let domain = launch_domain()?;
    let status = Command::new("launchctl")
        .args(["bootout", &format!("{domain}/{LAUNCH_LABEL}")])
        .status()?;
    if !status.success() {
        // Stopping an already stopped service is intentionally idempotent.
        return Ok(());
    }
    Ok(())
}

pub fn restart_service() -> Result<()> {
    stop_service()?;
    start_service()
}

pub fn is_installed() -> bool {
    launch_agent_path().is_ok_and(|path| path.exists())
}

fn start_launch_agent(path: &Path) -> Result<()> {
    let domain = launch_domain()?;
    let _ = Command::new("launchctl")
        .args(["bootout", &format!("{domain}/{LAUNCH_LABEL}")])
        .status();
    let status = Command::new("launchctl")
        .args(["bootstrap", &domain])
        .arg(path)
        .status()
        .context("launchctl bootstrap")?;
    if !status.success() {
        bail!("无法启动后台服务；请运行 codex-ble-bridge doctor")
    }
    Ok(())
}

fn launch_agent_path() -> Result<PathBuf> {
    let home = dirs::home_dir().context("cannot find home directory")?;
    Ok(home
        .join("Library/LaunchAgents")
        .join(format!("{LAUNCH_LABEL}.plist")))
}

fn menu_launch_agent_path() -> Result<PathBuf> {
    let home = dirs::home_dir().context("cannot find home directory")?;
    Ok(home
        .join("Library/LaunchAgents")
        .join(format!("{MENU_LAUNCH_LABEL}.plist")))
}

fn launch_domain() -> Result<String> {
    let output = Command::new("id").arg("-u").output()?;
    if !output.status.success() {
        bail!("id -u failed")
    }
    Ok(format!(
        "gui/{}",
        String::from_utf8_lossy(&output.stdout).trim()
    ))
}

pub fn notify(message: &str) {
    let script = format!(
        "display notification {:?} with title {:?}",
        message, "Codex Beacon"
    );
    let _ = Command::new("osascript").args(["-e", &script]).status();
}
