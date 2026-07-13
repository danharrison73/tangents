//! tangents' own metadata: `~/.tangents/branches.json`.
//!
//! Claude Code's JSONL does *not* record which session a fork came from, so this
//! file is our source of truth for parent links (written at fork time), plus
//! user-assigned names/colours and the active-branch pointer.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

const STATE_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TangentsState {
    pub version: u32,
    /// session id of the active branch, if any.
    #[serde(default)]
    pub active: Option<String>,
    /// Per-session metadata, keyed by session id.
    #[serde(default)]
    pub branches: HashMap<String, BranchMeta>,
    /// Where this state was loaded from (not serialised).
    #[serde(skip)]
    path: PathBuf,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BranchMeta {
    /// Parent session id — the only reliable parent link, set when we fork.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    /// User-assigned display name (overrides the derived title).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Optional colour index.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<u8>,
    /// Archived branches can be hidden from the tree.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub archived: bool,
}

impl TangentsState {
    /// Load state from `<tangents_dir>/branches.json`, or a fresh empty state.
    pub fn load(tangents_dir: &Path) -> Self {
        let path = tangents_dir.join("branches.json");
        if let Ok(content) = std::fs::read_to_string(&path)
            && let Ok(mut st) = serde_json::from_str::<TangentsState>(&content) {
                st.path = path;
                return st;
            }
        TangentsState {
            version: STATE_VERSION,
            active: None,
            branches: HashMap::new(),
            path,
        }
    }

    /// Atomically persist to disk (write temp + rename) so a crash mid-write
    /// never corrupts the file.
    pub fn save(&self) -> Result<()> {
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        }
        let json = serde_json::to_string_pretty(self)?;
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, json.as_bytes())
            .with_context(|| format!("writing {}", tmp.display()))?;
        std::fs::rename(&tmp, &self.path)
            .with_context(|| format!("renaming into {}", self.path.display()))?;
        Ok(())
    }

    pub fn meta(&self, session_id: &str) -> Option<&BranchMeta> {
        self.branches.get(session_id)
    }

    pub fn meta_mut(&mut self, session_id: &str) -> &mut BranchMeta {
        self.branches.entry(session_id.to_string()).or_default()
    }

    /// Record a fork edge child <- parent.
    pub fn record_fork(&mut self, child: &str, parent: &str) {
        self.meta_mut(child).parent = Some(parent.to_string());
    }

    pub fn set_active(&mut self, session_id: Option<String>) {
        self.active = session_id;
    }

    pub fn set_name(&mut self, session_id: &str, name: Option<String>) {
        self.meta_mut(session_id).name = name;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_disk() {
        let dir = std::env::temp_dir().join(format!("tangents-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut st = TangentsState::load(&dir);
        assert!(st.branches.is_empty());
        st.record_fork("child", "parent");
        st.set_name("child", Some("my branch".into()));
        st.set_active(Some("child".into()));
        st.save().unwrap();

        let reloaded = TangentsState::load(&dir);
        assert_eq!(reloaded.active.as_deref(), Some("child"));
        assert_eq!(
            reloaded.meta("child").unwrap().parent.as_deref(),
            Some("parent")
        );
        assert_eq!(
            reloaded.meta("child").unwrap().name.as_deref(),
            Some("my branch")
        );
        std::fs::remove_dir_all(&dir).ok();
    }
}
