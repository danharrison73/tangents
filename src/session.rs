//! Reads Claude Code session JSONL files into a lightweight in-memory summary.
//!
//! We only pull the handful of fields the tree needs and are deliberately
//! tolerant: unknown fields are ignored and any line that fails to parse (e.g. a
//! half-written trailing line while claude is mid-flush) is skipped.

use chrono::{DateTime, Utc};
use serde::Deserialize;
use std::path::Path;

/// One line of a session JSONL, partially deserialised.
#[derive(Deserialize)]
struct Line {
    #[serde(rename = "type")]
    kind: Option<String>,
    #[serde(rename = "isSidechain")]
    is_sidechain: Option<bool>,
    #[serde(rename = "isMeta")]
    is_meta: Option<bool>,
    #[serde(rename = "aiTitle")]
    ai_title: Option<String>,
    #[serde(rename = "lastPrompt")]
    last_prompt: Option<String>,
    #[serde(rename = "leafUuid")]
    leaf_uuid: Option<String>,
    uuid: Option<String>,
    timestamp: Option<String>,
    #[serde(rename = "gitBranch")]
    git_branch: Option<String>,
    version: Option<String>,
    message: Option<serde_json::Value>,
}

/// A summarised session (one JSONL file = one node in the tree).
#[derive(Debug, Clone)]
pub struct SessionInfo {
    pub session_id: String,
    pub ai_title: Option<String>,
    pub last_prompt: Option<String>,
    pub first_prompt: Option<String>,
    pub message_count: usize,
    pub created_at: Option<DateTime<Utc>>,
    pub last_active: Option<DateTime<Utc>>,
    pub git_branch: Option<String>,
    pub version: Option<String>,
    /// First message uuid — anchor for the fork-overlap heuristic (Phase 5).
    pub first_uuid: Option<String>,
    /// The leaf the session currently points at.
    pub leaf_uuid: Option<String>,
}

impl SessionInfo {
    /// Best available human label, ignoring any user-assigned override
    /// (which lives in tangents state, merged later).
    pub fn derived_name(&self) -> String {
        if let Some(t) = non_empty(&self.ai_title) {
            return t;
        }
        if let Some(p) = non_empty(&self.first_prompt) {
            return truncate(&p, 40);
        }
        if let Some(p) = non_empty(&self.last_prompt) {
            return truncate(&p, 40);
        }
        format!("session {}", short_id(&self.session_id))
    }
}

/// Scan a project directory, returning one [`SessionInfo`] per parseable file.
pub fn scan(project_dir: &Path) -> Vec<SessionInfo> {
    let mut out = Vec::new();
    let entries = match std::fs::read_dir(project_dir) {
        Ok(e) => e,
        Err(_) => return out, // project dir may not exist yet
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
            continue;
        }
        if let Some(info) = parse_session(&path) {
            out.push(info);
        }
    }
    out
}

/// Parse a single session file. Returns `None` if it holds no real content.
pub fn parse_session(path: &Path) -> Option<SessionInfo> {
    let session_id = path.file_stem()?.to_string_lossy().into_owned();
    let content = std::fs::read_to_string(path).ok()?;

    let mut info = SessionInfo {
        session_id,
        ai_title: None,
        last_prompt: None,
        first_prompt: None,
        message_count: 0,
        created_at: None,
        last_active: None,
        git_branch: None,
        version: None,
        first_uuid: None,
        leaf_uuid: None,
    };
    let mut saw_line = false;

    for raw in content.lines() {
        let line: Line = match serde_json::from_str(raw) {
            Ok(l) => l,
            Err(_) => continue, // tolerate partial/foreign lines
        };
        saw_line = true;
        let kind = line.kind.as_deref().unwrap_or("");

        if let Some(t) = line.ai_title {
            info.ai_title = Some(t);
        }
        if let Some(p) = line.last_prompt {
            info.last_prompt = Some(p);
        }
        if line.git_branch.is_some() {
            info.git_branch = line.git_branch;
        }
        if line.version.is_some() {
            info.version = line.version;
        }
        if kind == "last-prompt"
            && let Some(l) = line.leaf_uuid {
                info.leaf_uuid = Some(l);
            }

        let is_sidechain = line.is_sidechain.unwrap_or(false);
        let is_meta = line.is_meta.unwrap_or(false);
        let is_real_message = matches!(kind, "user" | "assistant") && !is_sidechain && !is_meta;

        if is_real_message {
            info.message_count += 1;
            if info.first_uuid.is_none() {
                info.first_uuid = line.uuid.clone();
            }
            if info.first_prompt.is_none() && kind == "user"
                && let Some(text) = line.message.as_ref().and_then(extract_user_text) {
                    info.first_prompt = Some(text);
                }
        }

        if let Some(ts) = line.timestamp.as_deref().and_then(parse_ts) {
            info.created_at = Some(match info.created_at {
                Some(c) if c <= ts => c,
                _ => ts,
            });
            info.last_active = Some(match info.last_active {
                Some(l) if l >= ts => l,
                _ => ts,
            });
        }
    }

    if !saw_line {
        return None;
    }
    // Fall back to file mtime for last_active if no timestamps were present.
    if info.last_active.is_none()
        && let Ok(meta) = std::fs::metadata(path)
            && let Ok(modified) = meta.modified() {
                info.last_active = Some(DateTime::<Utc>::from(modified));
            }
    Some(info)
}

/// Pull human-typed text out of a user message's `content`.
fn extract_user_text(message: &serde_json::Value) -> Option<String> {
    let content = message.get("content")?;
    match content {
        serde_json::Value::String(s) => Some(s.clone()),
        serde_json::Value::Array(blocks) => {
            for b in blocks {
                if b.get("type").and_then(|t| t.as_str()) == Some("text")
                    && let Some(t) = b.get("text").and_then(|t| t.as_str()) {
                        return Some(t.to_string());
                    }
            }
            None
        }
        _ => None,
    }
    .map(|s| s.trim().to_string())
    .filter(|s| !s.is_empty())
}

fn parse_ts(s: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|dt| dt.with_timezone(&Utc))
}

fn non_empty(s: &Option<String>) -> Option<String> {
    s.as_ref()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Truncate to `max` characters (not bytes), adding an ellipsis if cut.
pub fn truncate(s: &str, max: usize) -> String {
    let trimmed: String = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if trimmed.chars().count() <= max {
        return trimmed;
    }
    let mut out: String = trimmed.chars().take(max.saturating_sub(1)).collect();
    out.push('\u{2026}');
    out
}

/// First segment of a UUID, for compact display.
pub fn short_id(id: &str) -> String {
    id.split('-').next().unwrap_or(id).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_respects_char_count() {
        assert_eq!(truncate("hello world", 40), "hello world");
        assert_eq!(truncate("  fix   the  bug ", 40), "fix the bug");
        assert_eq!(truncate("abcdefghij", 5), "abcd\u{2026}");
    }

    #[test]
    fn extract_string_and_block_content() {
        let s = serde_json::json!({"content": "hi there"});
        assert_eq!(extract_user_text(&s).as_deref(), Some("hi there"));
        let b = serde_json::json!({"content": [{"type":"text","text":"blocked"}]});
        assert_eq!(extract_user_text(&b).as_deref(), Some("blocked"));
        let tool = serde_json::json!({"content": [{"type":"tool_result","x":1}]});
        assert_eq!(extract_user_text(&tool), None);
    }

    #[test]
    fn short_id_takes_first_segment() {
        assert_eq!(short_id("86931d84-a611-4e86"), "86931d84");
    }
}
