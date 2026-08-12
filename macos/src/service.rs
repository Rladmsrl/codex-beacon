use crate::app_server;
use crate::ble;
use crate::config::{AppPaths, find_codex_binary};
use crate::model::TaskStore;
use crate::protocol::encode_snapshot;
use anyhow::{Context, Result};
use fs2::FileExt;
use std::fs::{self, File, OpenOptions};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tokio::net::UnixDatagram;
use tokio::sync::{mpsc, watch};
use tokio::time::Duration;

pub async fn run(paths: AppPaths) -> Result<()> {
    let _runtime = RuntimeFiles::acquire(paths.clone())?;
    let socket = UnixDatagram::bind(&paths.socket)?;
    let store = Arc::new(Mutex::new(TaskStore::default()));
    let hook_store = Arc::clone(&store);
    tokio::spawn(async move {
        receive_hooks(socket, hook_store).await;
    });

    let (metadata_tx, mut metadata_rx) = mpsc::channel(4);
    let codex = find_codex_binary()?;
    tokio::spawn(async move {
        app_server::poll_forever(&codex, metadata_tx).await;
    });

    let (snapshot_tx, snapshot_rx) = watch::channel(encode_snapshot(0, &[]));
    let ble_paths = paths.clone();
    tokio::spawn(async move {
        if let Err(error) = ble::broadcast_loop(ble_paths, snapshot_rx).await {
            eprintln!("Bluetooth: {error:#}");
        }
    });

    let mut ticker = tokio::time::interval(Duration::from_millis(500));
    let mut sequence = 0u8;
    loop {
        tokio::select! {
            Some(metadata) = metadata_rx.recv() => {
                store.lock().unwrap().merge_metadata(&metadata);
            }
            _ = ticker.tick() => {
                sequence = sequence.wrapping_add(1);
                let tasks = store.lock().unwrap().snapshot();
                snapshot_tx.send_replace(encode_snapshot(sequence, &tasks));
            }
            _ = tokio::signal::ctrl_c() => break,
        }
    }
    Ok(())
}

/// Remove stale runtime files. Persistent settings and logs are kept unless
/// `all_data` is explicitly requested.
pub fn clean(paths: &AppPaths, all_data: bool) -> Result<Vec<PathBuf>> {
    if !paths.support.exists() {
        return Ok(Vec::new());
    }
    let lock = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .open(&paths.lock)?;
    lock.try_lock_exclusive()
        .context("后台服务仍在运行，请先执行 uninstall 或停止服务")?;

    let mut removed = Vec::new();
    remove_if_present(&paths.socket, &mut removed)?;
    remove_if_present(&paths.settings.with_extension("json.tmp"), &mut removed)?;
    if all_data {
        remove_if_present(&paths.settings, &mut removed)?;
        remove_if_present(&paths.log, &mut removed)?;
    }

    FileExt::unlock(&lock)?;
    drop(lock);
    remove_if_present(&paths.lock, &mut removed)?;
    if fs::remove_dir(&paths.support).is_ok() {
        removed.push(paths.support.clone());
    }
    Ok(removed)
}

pub fn is_running(paths: &AppPaths) -> bool {
    if !paths.lock.exists() {
        return false;
    }
    let Ok(lock) = OpenOptions::new().read(true).write(true).open(&paths.lock) else {
        return false;
    };
    match lock.try_lock_exclusive() {
        Ok(()) => {
            let _ = FileExt::unlock(&lock);
            false
        }
        Err(_) => true,
    }
}

struct RuntimeFiles {
    paths: AppPaths,
    lock: Option<File>,
}

impl RuntimeFiles {
    fn acquire(paths: AppPaths) -> Result<Self> {
        paths.ensure()?;
        let lock = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .open(&paths.lock)?;
        lock.try_lock_exclusive()
            .context("Codex BLE Bridge 已经在运行")?;
        // A socket or atomic-write temporary file can remain after a crash.
        let _ = fs::remove_file(&paths.socket);
        let _ = fs::remove_file(paths.settings.with_extension("json.tmp"));
        Ok(Self {
            paths,
            lock: Some(lock),
        })
    }
}

impl Drop for RuntimeFiles {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.paths.socket);
        if let Some(lock) = self.lock.take() {
            let _ = FileExt::unlock(&lock);
            drop(lock);
        }
        let _ = fs::remove_file(&self.paths.lock);
    }
}

fn remove_if_present(path: &std::path::Path, removed: &mut Vec<PathBuf>) -> Result<()> {
    if path.exists() {
        fs::remove_file(path)?;
        removed.push(path.to_path_buf());
    }
    Ok(())
}

async fn receive_hooks(socket: UnixDatagram, store: Arc<Mutex<TaskStore>>) {
    let mut buffer = vec![0u8; 64 * 1024];
    loop {
        let Ok(size) = socket.recv(&mut buffer).await else {
            break;
        };
        if let Ok(event) = serde_json::from_slice(&buffer[..size]) {
            store.lock().unwrap().apply_hook(&event);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths(root: &std::path::Path) -> AppPaths {
        AppPaths {
            support: root.to_path_buf(),
            settings: root.join("settings.json"),
            socket: root.join("events.sock"),
            lock: root.join("service.lock"),
            log: root.join("bridge.log"),
        }
    }

    #[test]
    fn clean_keeps_persistent_data_by_default() {
        let temp = tempfile::tempdir().unwrap();
        let paths = paths(temp.path());
        fs::write(&paths.settings, b"{}").unwrap();
        fs::write(paths.settings.with_extension("json.tmp"), b"stale").unwrap();
        fs::write(&paths.socket, b"stale").unwrap();

        clean(&paths, false).unwrap();
        assert!(paths.settings.exists());
        assert!(!paths.socket.exists());
        assert!(!paths.lock.exists());
        assert!(!paths.settings.with_extension("json.tmp").exists());
    }

    #[test]
    fn clean_all_data_removes_known_application_files() {
        let temp = tempfile::tempdir().unwrap();
        let paths = paths(temp.path());
        fs::write(&paths.settings, b"{}").unwrap();
        fs::write(&paths.log, b"log").unwrap();

        clean(&paths, true).unwrap();
        assert!(!paths.settings.exists());
        assert!(!paths.log.exists());
    }
}
