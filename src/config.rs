//! Optional user config at `~/.tangents/config.toml`.
//!
//! Everything is optional; a missing or malformed file yields defaults.
//!
//! ```toml
//! prefix = "g"       # the command prefix key, used as Ctrl+<prefix>
//! no_tree = false    # start with the tree hidden
//! tree_width = 30    # sidebar width in columns
//! ```

use serde::Deserialize;
use std::path::Path;

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct TangentsConfig {
    /// The command prefix key (used as Ctrl+<prefix>). Default `g`.
    pub prefix: char,
    /// Start with the tree hidden.
    pub no_tree: bool,
    /// Preferred sidebar width in columns (clamped at render time).
    pub tree_width: u16,
}

impl Default for TangentsConfig {
    fn default() -> Self {
        Self {
            prefix: 'g',
            no_tree: false,
            tree_width: 30,
        }
    }
}

impl TangentsConfig {
    /// Load `<tangents_dir>/config.toml`, falling back to defaults.
    pub fn load(tangents_dir: &Path) -> Self {
        let path = tangents_dir.join("config.toml");
        match std::fs::read_to_string(&path) {
            Ok(text) => toml::from_str(&text).unwrap_or_default(),
            Err(_) => Self::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_when_missing() {
        let cfg = TangentsConfig::load(Path::new("/nonexistent/tangents"));
        assert_eq!(cfg.prefix, 'g');
        assert!(!cfg.no_tree);
    }

    #[test]
    fn parses_partial_toml() {
        let cfg: TangentsConfig = toml::from_str("prefix = \"a\"\n").unwrap();
        assert_eq!(cfg.prefix, 'a');
        assert_eq!(cfg.tree_width, 30); // default preserved
    }
}
