use crate::config::{AppPaths, SavedDevice, Settings, load, save};
use crate::model::unix_now;
use crate::protocol::encode_hello;
use anyhow::{Context, Result, bail};
use btleplug::api::{
    Central, Characteristic, Manager as _, Peripheral as _, ScanFilter, WriteType,
};
use btleplug::platform::{Adapter, Manager, Peripheral};
use std::collections::{HashMap, HashSet};
use tokio::sync::watch;
use tokio::time::{Duration, Instant, sleep};
use uuid::Uuid;

pub const SERVICE_UUID: Uuid = Uuid::from_u128(0x7a5c10004e6f4f70656e4149436f6465);
pub const SNAPSHOT_UUID: Uuid = Uuid::from_u128(0x7a5c10014e6f4f70656e4149436f6465);
pub const CONTROL_UUID: Uuid = Uuid::from_u128(0x7a5c10024e6f4f70656e4149436f6465);

pub async fn pair_visible(paths: &AppPaths, seconds: u64) -> Result<Vec<SavedDevice>> {
    let adapter = adapter().await?;
    let candidates = discover(&adapter, Duration::from_secs(seconds)).await?;
    if candidates.is_empty() {
        bail!("未发现处于配对模式的 Codex Beacon")
    }
    let mut settings = load(paths)?;
    let mut paired = Vec::new();
    for peripheral in candidates {
        match connect_and_hello(&peripheral).await {
            Ok(name) => {
                let saved = SavedDevice {
                    id: peripheral.id().to_string(),
                    name,
                    last_seen: unix_now(),
                };
                upsert_device(&mut settings, saved.clone());
                paired.push(saved);
            }
            Err(error) => eprintln!("跳过 {}: {error:#}", peripheral.id()),
        }
    }
    save(paths, &settings)?;
    Ok(paired)
}

pub async fn broadcast_loop(
    paths: AppPaths,
    mut snapshots: watch::Receiver<Vec<u8>>,
) -> Result<()> {
    let adapter = adapter().await?;
    let mut connected: HashMap<String, (Peripheral, Characteristic)> = HashMap::new();
    let mut last_sent: HashMap<String, (Vec<u8>, Instant)> = HashMap::new();
    let mut refresh = tokio::time::interval(Duration::from_secs(8));
    let mut push = tokio::time::interval(Duration::from_secs(1));

    loop {
        tokio::select! {
            _ = refresh.tick() => {
                let settings = load(&paths).unwrap_or_default();
                let candidates = discover(&adapter, Duration::from_secs(2)).await.unwrap_or_default();
                for peripheral in candidates {
                    let id = peripheral.id().to_string();
                    let known = settings.devices.iter().any(|d| d.id == id);
                    if !known && !(settings.auto_pair || settings.devices.is_empty()) {
                        continue;
                    }
                    if connected.contains_key(&id) {
                        continue;
                    }
                    match connect(&peripheral).await {
                        Ok((name, characteristic)) => {
                            if peripheral.write(&characteristic, &encode_hello(), WriteType::WithResponse).await.is_ok() {
                                remember(&paths, id.clone(), name).ok();
                                connected.insert(id, (peripheral, characteristic));
                            }
                        }
                        Err(error) => eprintln!("BLE connect {id}: {error:#}"),
                    }
                }
            }
            _ = push.tick() => {
                let payload = snapshots.borrow_and_update().clone();
                let mut failed = Vec::new();
                let now = Instant::now();
                for (id, (peripheral, characteristic)) in &connected {
                    let should_send = last_sent.get(id).is_none_or(|(previous, sent_at)| {
                        !same_snapshot(previous, &payload)
                            || now.duration_since(*sent_at) >= Duration::from_secs(15)
                    });
                    if !should_send {
                        continue;
                    }
                    if peripheral.write(characteristic, &payload, WriteType::WithoutResponse).await.is_err() {
                        failed.push(id.clone());
                    } else {
                        last_sent.insert(id.clone(), (payload.clone(), now));
                    }
                }
                for id in failed {
                    connected.remove(&id);
                    last_sent.remove(&id);
                }
            }
            result = snapshots.changed() => {
                if result.is_err() {
                    return Ok(());
                }
            }
        }
    }
}

fn same_snapshot(left: &[u8], right: &[u8]) -> bool {
    left.len() == right.len()
        && left
            .iter()
            .zip(right)
            .enumerate()
            .all(|(index, (left, right))| index == 3 || left == right)
}

async fn adapter() -> Result<Adapter> {
    let manager = Manager::new().await.context("initialize CoreBluetooth")?;
    manager
        .adapters()
        .await?
        .into_iter()
        .next()
        .context("这台 Mac 没有可用的蓝牙适配器")
}

async fn discover(adapter: &Adapter, duration: Duration) -> Result<Vec<Peripheral>> {
    adapter
        .start_scan(ScanFilter {
            services: vec![SERVICE_UUID],
        })
        .await?;
    sleep(duration).await;
    adapter.stop_scan().await?;
    let mut found = Vec::new();
    for peripheral in adapter.peripherals().await? {
        let properties = peripheral.properties().await?;
        let matches = properties.as_ref().is_some_and(|p| {
            p.services.contains(&SERVICE_UUID)
                || p.local_name.as_deref().is_some_and(|n| {
                    n.starts_with("Codex Beacon") || n.starts_with("Codex Monitor")
                })
        });
        if matches {
            found.push(peripheral);
        }
    }
    Ok(found)
}

async fn connect_and_hello(peripheral: &Peripheral) -> Result<String> {
    let (name, characteristic) = connect(peripheral).await?;
    peripheral
        .write(&characteristic, &encode_hello(), WriteType::WithResponse)
        .await
        .context("secure pairing handshake")?;
    Ok(name)
}

async fn connect(peripheral: &Peripheral) -> Result<(String, Characteristic)> {
    if !peripheral.is_connected().await? {
        peripheral.connect().await?;
    }
    peripheral.discover_services().await?;
    let characteristic = peripheral
        .characteristics()
        .into_iter()
        .find(|c| c.uuid == SNAPSHOT_UUID)
        .context("设备缺少 Codex snapshot characteristic")?;
    let name = peripheral
        .properties()
        .await?
        .and_then(|p| p.local_name)
        .unwrap_or_else(|| format!("Codex Beacon {}", peripheral.id()));
    Ok((name, characteristic))
}

fn remember(paths: &AppPaths, id: String, name: String) -> Result<()> {
    let mut settings = load(paths)?;
    upsert_device(
        &mut settings,
        SavedDevice {
            id,
            name,
            last_seen: unix_now(),
        },
    );
    save(paths, &settings)
}

fn upsert_device(settings: &mut Settings, device: SavedDevice) {
    if let Some(old) = settings.devices.iter_mut().find(|d| d.id == device.id) {
        *old = device;
    } else {
        settings.devices.push(device);
    }
    let mut seen = HashSet::new();
    settings.devices.retain(|d| seen.insert(d.id.clone()));
}

#[cfg(test)]
mod tests {
    use super::same_snapshot;

    #[test]
    fn sequence_byte_does_not_make_snapshot_dirty() {
        assert!(same_snapshot(b"CX\x01\x01\x00\x00", b"CX\x01\x99\x00\x00"));
        assert!(!same_snapshot(b"CX\x01\x01\x00\x00", b"CX\x01\x02\x01\x00"));
    }
}
