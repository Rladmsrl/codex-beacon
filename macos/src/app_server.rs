use crate::model::TaskMetadata;
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::path::Path;
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::process::{ChildStdout, Command};
use tokio::sync::mpsc;
use tokio::time::{Duration, sleep};

pub async fn poll_forever(codex: &Path, sender: mpsc::Sender<Vec<TaskMetadata>>) {
    loop {
        if let Err(error) = poll_process(codex, &sender).await {
            eprintln!("Codex App Server: {error:#}");
        }
        sleep(Duration::from_secs(5)).await;
    }
}

async fn poll_process(codex: &Path, sender: &mpsc::Sender<Vec<TaskMetadata>>) -> Result<()> {
    let mut child = Command::new(codex)
        .args(["app-server", "--stdio"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .with_context(|| format!("start {} app-server", codex.display()))?;
    let mut input = child.stdin.take().context("app-server stdin unavailable")?;
    let output = child
        .stdout
        .take()
        .context("app-server stdout unavailable")?;
    let mut lines = BufReader::new(output).lines();

    write_json(
        &mut input,
        &json!({
            "method": "initialize",
            "id": 1,
            "params": {
                "clientInfo": {
                    "name": "codex_ble_bridge",
                    "title": "Codex Beacon",
                    "version": env!("CARGO_PKG_VERSION")
                },
                "capabilities": {"experimentalApi": true}
            }
        }),
    )
    .await?;
    let _ = response_for(&mut lines, 1).await?;
    write_json(&mut input, &json!({"method": "initialized", "params": {}})).await?;

    let mut request_id = 2u64;
    loop {
        write_json(
            &mut input,
            &json!({
                "method": "thread/list",
                "id": request_id,
                "params": {
                    "limit": 100,
                    "sortKey": "updated_at",
                    "sortDirection": "desc",
                    "sourceKinds": []
                }
            }),
        )
        .await?;
        let response = response_for(&mut lines, request_id).await?;
        let tasks = parse_thread_list(&response);
        if sender.send(tasks).await.is_err() {
            return Ok(());
        }
        request_id += 1;
        sleep(Duration::from_secs(4)).await;
    }
}

async fn write_json(input: &mut tokio::process::ChildStdin, value: &Value) -> Result<()> {
    let mut bytes = serde_json::to_vec(value)?;
    bytes.push(b'\n');
    input.write_all(&bytes).await?;
    input.flush().await?;
    Ok(())
}

async fn response_for(lines: &mut Lines<BufReader<ChildStdout>>, id: u64) -> Result<Value> {
    while let Some(line) = lines.next_line().await? {
        let Ok(value) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if value.get("id").and_then(Value::as_u64) == Some(id) {
            if let Some(error) = value.get("error") {
                bail!("app-server request {id} failed: {error}");
            }
            return Ok(value);
        }
    }
    bail!("app-server closed stdout")
}

pub fn parse_thread_list(response: &Value) -> Vec<TaskMetadata> {
    let Some(items) = response
        .get("result")
        .and_then(|v| v.get("data"))
        .and_then(Value::as_array)
    else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|item| {
            let id = item.get("id")?.as_str()?.to_owned();
            let title = item
                .get("name")
                .or_else(|| item.get("title"))
                .or_else(|| item.get("preview"))
                .and_then(Value::as_str)
                .unwrap_or("Codex task")
                .lines()
                .next()
                .unwrap_or("Codex task")
                .to_owned();
            Some(TaskMetadata {
                id,
                title,
                cwd: item.get("cwd").and_then(Value::as_str).map(str::to_owned),
                status: item
                    .get("status")
                    .and_then(|status| {
                        status
                            .as_str()
                            .or_else(|| status.get("type").and_then(Value::as_str))
                    })
                    .unwrap_or("notLoaded")
                    .to_owned(),
                updated_at: item.get("updatedAt").and_then(Value::as_u64),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_real_thread_list_shape() {
        let response = json!({"result":{"data":[{
            "id":"abc", "name":"Build bridge", "cwd":"/tmp/demo",
            "status":{"type":"notLoaded"}, "updatedAt":42
        }]}});
        let tasks = parse_thread_list(&response);
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].title, "Build bridge");
        assert_eq!(tasks[0].status, "notLoaded");
    }
}
