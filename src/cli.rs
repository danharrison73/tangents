//! Command-line parsing and environment resolution.
//!
//! tangents is a *transparent wrapper*: it recognises only its own handful of
//! flags and forwards everything else — verbatim, in order — to the `claude`
//! binary. Using clap's derive here would make it reject claude's own flags
//! (`--model`, `--resume`, ...), so we hand-roll a minimal splitter instead.

use anyhow::{Result, anyhow, bail};
use std::env;
use std::path::{Path, PathBuf};

/// Fully-resolved runtime configuration.
#[derive(Debug, Clone)]
pub struct Config {
    /// Start with the tree panel hidden.
    pub no_tree: bool,
    /// Print the argv we would hand to `claude`, then exit (debug aid).
    pub print_argv: bool,
    /// Absolute path to the `claude` binary we will spawn.
    pub claude_bin: PathBuf,
    /// Arguments forwarded to `claude`, in original order.
    pub claude_args: Vec<String>,
    /// The directory tangents was launched from.
    pub cwd: PathBuf,
    /// `~/.claude/projects/<slug>` for `cwd` — where session JSONL lives.
    pub project_dir: PathBuf,
    /// `~/.tangents` — where tangents keeps its own metadata.
    pub tangents_dir: PathBuf,
}

impl Config {
    /// Parse process arguments (excluding argv[0]).
    pub fn from_args<I: IntoIterator<Item = String>>(args: I) -> Result<Config> {
        let mut no_tree = false;
        let mut print_argv = false;
        let mut claude_bin_override: Option<PathBuf> = None;
        let mut claude_args: Vec<String> = Vec::new();

        let mut iter = args.into_iter();
        while let Some(arg) = iter.next() {
            match arg.as_str() {
                "--no-tree" => no_tree = true,
                "--print-argv" => print_argv = true,
                "--claude-bin" => {
                    let p = iter
                        .next()
                        .ok_or_else(|| anyhow!("--claude-bin requires a path argument"))?;
                    claude_bin_override = Some(PathBuf::from(p));
                }
                s if s.starts_with("--claude-bin=") => {
                    claude_bin_override = Some(PathBuf::from(&s["--claude-bin=".len()..]));
                }
                // Everything else — including --help, --model, --resume, and
                // bare prompts — belongs to claude.
                _ => claude_args.push(arg),
            }
        }

        let cwd = env::current_dir()?;
        let home = dirs::home_dir().ok_or_else(|| anyhow!("cannot determine home directory"))?;
        let project_dir = home
            .join(".claude")
            .join("projects")
            .join(project_slug(&cwd));
        let tangents_dir = home.join(".tangents");
        let claude_bin = resolve_claude_bin(claude_bin_override)?;

        Ok(Config {
            no_tree,
            print_argv,
            claude_bin,
            claude_args,
            cwd,
            project_dir,
            tangents_dir,
        })
    }

    /// The full argv (program + args) that will be spawned.
    pub fn spawn_argv(&self) -> Vec<String> {
        let mut v = Vec::with_capacity(self.claude_args.len() + 1);
        v.push(self.claude_bin.to_string_lossy().into_owned());
        v.extend(self.claude_args.iter().cloned());
        v
    }
}

/// Remove any session-referencing flags, leaving "base" flags (model,
/// permission-mode, add-dir, ...) that should be carried into every respawn.
pub fn strip_session_flags(args: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        match a.as_str() {
            "--fork-session" => i += 1,
            "-c" | "--continue" => i += 1,
            "-r" | "--resume" => {
                i += 1;
                if i < args.len() && !args[i].starts_with('-') {
                    i += 1; // skip its value
                }
            }
            "--session-id" => i += 2,
            "--from-pr" => {
                i += 1;
                if i < args.len() && !args[i].starts_with('-') {
                    i += 1;
                }
            }
            s if s.starts_with("--session-id=")
                || s.starts_with("--resume=")
                || s.starts_with("--from-pr=") =>
            {
                i += 1;
            }
            _ => {
                out.push(a.clone());
                i += 1;
            }
        }
    }
    out
}

/// Find the value following any of `names` (`--flag value` or `--flag=value`).
fn find_flag_value(args: &[String], names: &[&str]) -> Option<String> {
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        if names.contains(&a.as_str())
            && let Some(v) = args.get(i + 1)
                && !v.starts_with('-') {
                    return Some(v.clone());
                }
        for n in names {
            let pref = format!("{n}=");
            if let Some(rest) = a.strip_prefix(&pref) {
                return Some(rest.to_string());
            }
        }
        i += 1;
    }
    None
}

/// Decide the initial launch args and the session id we should track.
///
/// - explicit `--session-id X`  → track X, launch unchanged
/// - `--resume X` (no fork)     → track X, launch unchanged
/// - continue / fork / resume-picker / from-pr → unknown (detected at runtime)
/// - fresh session              → inject a `--session-id` we generate, track it
pub fn prepare_initial(claude_args: &[String]) -> (Vec<String>, Option<String>) {
    if let Some(id) = find_flag_value(claude_args, &["--session-id"]) {
        return (claude_args.to_vec(), Some(id));
    }
    let has_fork = claude_args.iter().any(|a| a == "--fork-session");
    let has_continue = claude_args.iter().any(|a| a == "-c" || a == "--continue");
    let has_from_pr = claude_args
        .iter()
        .any(|a| a == "--from-pr" || a.starts_with("--from-pr="));
    let has_resume = claude_args
        .iter()
        .any(|a| a == "-r" || a == "--resume" || a.starts_with("--resume="));

    if !has_fork
        && let Some(id) = find_flag_value(claude_args, &["-r", "--resume"]) {
            return (claude_args.to_vec(), Some(id));
        }
    if has_fork || has_continue || has_from_pr || has_resume {
        return (claude_args.to_vec(), None); // ambiguous; resolve later
    }

    let id = uuid::Uuid::new_v4().to_string();
    let mut args = claude_args.to_vec();
    args.push("--session-id".to_string());
    args.push(id.clone());
    (args, Some(id))
}

/// Claude Code slugifies a project path by replacing `/` and `.` with `-`.
/// e.g. `/home/dan/code/tangents` -> `-home-dan-code-tangents`
/// and  `/a/.claude-worktrees/b`  -> `-a--claude-worktrees-b`
pub fn project_slug(cwd: &Path) -> String {
    cwd.to_string_lossy()
        .chars()
        .map(|c| if c == '/' || c == '.' { '-' } else { c })
        .collect()
}

/// Resolve the claude binary: explicit override -> $TANGENTS_CLAUDE_BIN -> PATH
/// search -> bare "claude" (resolved by the OS at spawn time).
fn resolve_claude_bin(override_path: Option<PathBuf>) -> Result<PathBuf> {
    if let Some(p) = override_path {
        if !p.exists() {
            bail!("--claude-bin path does not exist: {}", p.display());
        }
        return Ok(p);
    }
    if let Ok(p) = env::var("TANGENTS_CLAUDE_BIN") {
        let p = PathBuf::from(p);
        if p.exists() {
            return Ok(p);
        }
    }
    if let Some(p) = search_path("claude") {
        return Ok(p);
    }
    // Last resort: let the OS resolve it. Spawn will error clearly if missing.
    Ok(PathBuf::from("claude"))
}

/// Minimal PATH search for an executable name.
fn search_path(name: &str) -> Option<PathBuf> {
    let paths = env::var_os("PATH")?;
    for dir in env::split_paths(&paths) {
        let candidate = dir.join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slug_matches_claude_convention() {
        assert_eq!(
            project_slug(Path::new("/home/dan/code/tangents")),
            "-home-dan-code-tangents"
        );
        assert_eq!(
            project_slug(Path::new("/a/.claude-worktrees/b")),
            "-a--claude-worktrees-b"
        );
    }

    #[test]
    fn tangents_flags_are_consumed_rest_forwarded() {
        let cfg = Config::from_args(
            [
                "--no-tree",
                "--model",
                "sonnet",
                "--dangerously-skip-permissions",
            ]
            .map(String::from),
        )
        .unwrap();
        assert!(cfg.no_tree);
        assert_eq!(
            cfg.claude_args,
            vec!["--model", "sonnet", "--dangerously-skip-permissions"]
        );
    }

    #[test]
    fn strip_removes_session_flags_keeps_base() {
        let args = [
            "--model",
            "sonnet",
            "--resume",
            "abc",
            "--fork-session",
            "--dangerously-skip-permissions",
        ]
        .map(String::from);
        assert_eq!(
            strip_session_flags(&args),
            vec!["--model", "sonnet", "--dangerously-skip-permissions"]
        );
    }

    #[test]
    fn prepare_injects_session_id_for_fresh_launch() {
        let (args, id) = prepare_initial(&["--model".into(), "sonnet".into()]);
        let id = id.expect("fresh launch should have a known id");
        assert!(
            args.windows(2)
                .any(|w| w[0] == "--session-id" && w[1] == id)
        );
    }

    #[test]
    fn prepare_tracks_explicit_resume() {
        let (args, id) = prepare_initial(&["--resume".into(), "xyz".into()]);
        assert_eq!(id.as_deref(), Some("xyz"));
        assert_eq!(args, vec!["--resume", "xyz"]); // unchanged
    }

    #[test]
    fn prepare_is_unknown_for_continue() {
        let (_args, id) = prepare_initial(&["--continue".into()]);
        assert_eq!(id, None);
    }

    #[test]
    fn claude_bin_flag_is_not_forwarded() {
        // Use a path that exists so resolution succeeds.
        let cfg =
            Config::from_args(["--claude-bin", "/bin/sh", "--resume"].map(String::from)).unwrap();
        assert_eq!(cfg.claude_bin, PathBuf::from("/bin/sh"));
        assert_eq!(cfg.claude_args, vec!["--resume"]);
    }
}
