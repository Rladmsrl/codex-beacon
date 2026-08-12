use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[repr(u8)]
pub enum TaskState {
    Thinking = 1,
    Editing = 2,
    Running = 3,
    Testing = 4,
    Waiting = 5,
    Done = 6,
    Error = 7,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskView {
    pub id: String,
    pub title: String,
    pub cwd: Option<String>,
    pub state: TaskState,
    pub attention: bool,
    pub updated_at: u64,
    pub started_at: u64,
}

impl TaskView {
    pub fn elapsed(&self, now: u64) -> Duration {
        Duration::from_secs(now.saturating_sub(self.started_at))
    }
}

#[derive(Debug, Default)]
pub struct TaskStore {
    tasks: HashMap<String, TaskView>,
}

impl TaskStore {
    pub fn apply_hook(&mut self, event: &serde_json::Value) {
        let Some(id) = event.get("session_id").and_then(|v| v.as_str()) else {
            return;
        };
        let now = unix_now();
        let event_name = event
            .get("hook_event_name")
            .or_else(|| event.get("event"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let tool_name = event
            .get("tool_name")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let cwd = event.get("cwd").and_then(|v| v.as_str()).map(str::to_owned);

        let (state, attention) = match event_name {
            "PermissionRequest" => (TaskState::Waiting, true),
            "Stop" | "SessionEnd" => (TaskState::Done, false),
            "PreToolUse" => (state_for_tool(tool_name, event), false),
            "PostToolUseFailure" => (TaskState::Error, true),
            "PostToolUse" => (TaskState::Thinking, false),
            "SessionStart" | "UserPromptSubmit" | "SubagentStart" | "SubagentStop" => {
                (TaskState::Thinking, false)
            }
            _ => (TaskState::Thinking, false),
        };

        let fallback_title = cwd
            .as_deref()
            .and_then(|p| std::path::Path::new(p).file_name())
            .and_then(|v| v.to_str())
            .unwrap_or("Codex task")
            .to_owned();
        let task = self.tasks.entry(id.to_owned()).or_insert(TaskView {
            id: id.to_owned(),
            title: fallback_title,
            cwd: cwd.clone(),
            state,
            attention,
            updated_at: now,
            started_at: now,
        });
        task.state = state;
        task.attention = attention;
        task.updated_at = now;
        if cwd.is_some() {
            task.cwd = cwd;
        }
    }

    pub fn merge_metadata(&mut self, items: &[TaskMetadata]) {
        let now = unix_now();
        for item in items {
            if let Some(task) = self.tasks.get_mut(&item.id) {
                if !item.title.trim().is_empty() {
                    task.title.clone_from(&item.title);
                }
                if item.cwd.is_some() {
                    task.cwd.clone_from(&item.cwd);
                }
            } else if item.status != "notLoaded" {
                self.tasks.insert(
                    item.id.clone(),
                    TaskView {
                        id: item.id.clone(),
                        title: item.title.clone(),
                        cwd: item.cwd.clone(),
                        state: state_from_app_server(&item.status),
                        attention: false,
                        updated_at: item.updated_at.unwrap_or(now),
                        started_at: item.updated_at.unwrap_or(now),
                    },
                );
            }
        }
        self.prune(now);
    }

    pub fn snapshot(&self) -> Vec<TaskView> {
        let now = unix_now();
        let mut values: Vec<_> = self
            .tasks
            .values()
            .filter(|t| t.state != TaskState::Done || now.saturating_sub(t.updated_at) < 20)
            .cloned()
            .collect();
        values.sort_by_key(|t| (t.state == TaskState::Done, std::cmp::Reverse(t.updated_at)));
        values
    }

    fn prune(&mut self, now: u64) {
        self.tasks.retain(|_, task| {
            let ttl = if task.state == TaskState::Done {
                60
            } else {
                86_400
            };
            now.saturating_sub(task.updated_at) < ttl
        });
    }
}

#[derive(Debug, Clone)]
pub struct TaskMetadata {
    pub id: String,
    pub title: String,
    pub cwd: Option<String>,
    pub status: String,
    pub updated_at: Option<u64>,
}

fn state_for_tool(tool_name: &str, event: &serde_json::Value) -> TaskState {
    let lower = tool_name.to_ascii_lowercase();
    let input = event
        .get("tool_input")
        .map(serde_json::Value::to_string)
        .unwrap_or_default()
        .to_ascii_lowercase();
    if lower.contains("patch") || lower.contains("write") || lower.contains("edit") {
        TaskState::Editing
    } else if input.contains(" test")
        || input.contains("cargo test")
        || input.contains("pio run")
        || input.contains("npm test")
        || input.contains("pytest")
    {
        TaskState::Testing
    } else {
        TaskState::Running
    }
}

fn state_from_app_server(status: &str) -> TaskState {
    match status {
        "active" | "running" => TaskState::Thinking,
        "error" | "failed" => TaskState::Error,
        _ => TaskState::Done,
    }
}

pub fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn hook_events_drive_visible_states() {
        let mut store = TaskStore::default();
        store.apply_hook(&json!({
            "session_id": "task-1",
            "hook_event_name": "PreToolUse",
            "tool_name": "apply_patch",
            "cwd": "/tmp/demo"
        }));
        assert_eq!(store.snapshot()[0].state, TaskState::Editing);

        store.apply_hook(&json!({
            "session_id": "task-1",
            "hook_event_name": "PermissionRequest"
        }));
        assert_eq!(store.snapshot()[0].state, TaskState::Waiting);
        assert!(store.snapshot()[0].attention);
    }
}
