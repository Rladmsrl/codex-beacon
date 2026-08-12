use crate::model::{TaskView, unix_now};

pub const PROTOCOL_VERSION: u8 = 1;
pub const MAX_VISIBLE_TASKS: usize = 4;
pub const MAX_TITLE_BYTES: usize = 32;

pub fn encode_snapshot(sequence: u8, tasks: &[TaskView]) -> Vec<u8> {
    let visible = tasks.len().min(MAX_VISIBLE_TASKS);
    let total = tasks.len().min(u8::MAX as usize) as u8;
    let mut out = Vec::with_capacity(180);
    out.extend_from_slice(b"CX");
    out.extend_from_slice(&[PROTOCOL_VERSION, sequence, visible as u8, total]);
    let now = unix_now();

    for task in tasks.iter().take(visible) {
        out.extend_from_slice(&fnv1a32(task.id.as_bytes()).to_le_bytes());
        out.push(task.state as u8);
        out.push(u8::from(task.attention));
        let elapsed = task.elapsed(now).as_secs().min(u16::MAX as u64) as u16;
        out.extend_from_slice(&elapsed.to_le_bytes());
        let title = utf8_prefix(task.title.trim(), MAX_TITLE_BYTES);
        out.push(title.len() as u8);
        out.extend_from_slice(title.as_bytes());
    }
    out
}

pub fn encode_hello() -> [u8; 4] {
    [b'C', b'X', PROTOCOL_VERSION, 0]
}

fn utf8_prefix(value: &str, max_bytes: usize) -> &str {
    if value.len() <= max_bytes {
        return value;
    }
    let mut end = max_bytes;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    &value[..end]
}

fn fnv1a32(bytes: &[u8]) -> u32 {
    let mut hash = 0x811c9dc5u32;
    for byte in bytes {
        hash ^= u32::from(*byte);
        hash = hash.wrapping_mul(0x01000193);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::TaskState;

    #[test]
    fn packet_stays_inside_default_ble_mtu_payload() {
        let tasks: Vec<_> = (0..8)
            .map(|i| TaskView {
                id: format!("task-{i}"),
                title: "这是一个很长的 Codex 会话标题 abcdefghijklmnopqrstuvwxyz".into(),
                cwd: None,
                state: TaskState::Running,
                attention: false,
                updated_at: 1,
                started_at: 1,
            })
            .collect();
        let packet = encode_snapshot(1, &tasks);
        assert!(packet.len() <= 182);
        assert_eq!(packet[4], 4);
        assert_eq!(packet[5], 8);
    }
}
