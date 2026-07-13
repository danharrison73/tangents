# Arbor — Plan v0.1

> Tree-structured conversation TUI for Claude Code.  
> "Every tangent deserves its own branch."

---

## Problem

Claude Code sessions are linear. When a conversation diverges into a sub-topic,
you either pollute the main thread or lose context by starting fresh. The mental
model of a conversation is a tree, not a list — Arbor makes that literal.

---

## Core Idea

A thin Rust TUI that wraps the real `claude` binary in a PTY. Claude Code runs
completely unmodified inside the terminal pane. Arbor adds a sidebar showing
the session tree, and a small set of keybindings to branch, switch, and
navigate — without replacing or reimplementing anything Claude Code already
does.

---

## Command Compatibility

**Short answer: yes, full compatibility, zero integration work.**

The PTY approach means `claude` runs inside a real pseudo-terminal owned by
Arbor. From Claude's perspective it is talking to a terminal. Every slash
command (`/branch`, `/clear`, `/model`, `/resume`, etc.), every flag
(`--dangerously-skip-permissions`, `--model`, `--add-dir`, etc.), every
permission prompt and interactive element passes through unmodified.

Arbor intercepts nothing in the I/O stream — it only reads the session JSONL
files that Claude Code writes to `~/.claude/` to build the tree view. This is
the same approach used by `claude-tui` (Python/Textual) and `claude-pty-wrapper`
(Rust), both of which achieve full command passthrough this way.

The one known limitation: some highly decorative terminal sequences (kitty
keyboard protocol, synchronized updates) may render slightly differently
depending on the VT100 emulation layer. Claude Code's core output is
unaffected.

---

## Architecture

```
┌─────────────────────────────────────────────────────┐
│  Arbor (Ratatui TUI)                                │
│                                                     │
│  ┌──────────────┐  ┌─────────────────────────────┐ │
│  │  Tree Panel  │  │  Terminal Pane (PTY)        │ │
│  │              │  │                             │ │
│  │  main ───────│  │  > claude running here      │ │
│  │  ├─ branch-1 │  │  > all commands work        │ │
│  │  │  └─ b-1a  │  │  > all flags work           │ │
│  │  └─ branch-2 │  │  > all prompts work         │ │
│  │  (active)    │  │                             │ │
│  └──────────────┘  └─────────────────────────────┘ │
│                                                     │
│  Status bar: branch name | session id | keybinds   │
└─────────────────────────────────────────────────────┘
```

**Data flow:**

1. Arbor spawns `claude [args]` in a PTY via `portable-pty` or `nix::pty`
2. PTY bytes flow bidirectionally — keystrokes in, rendered output out
3. Arbor reads `~/.claude/projects/<hash>/*.jsonl` to build the session tree
4. Tree panel re-renders on file change (via `notify` crate for fs events)
5. Arbor's own keybindings (e.g. `Ctrl+B` to branch) are intercepted before
   being forwarded to the PTY — everything else passes through untouched

---

## Layout

```
[Tree 25%] | [Terminal 75%]
```

- Tree panel is toggleable (`Ctrl+T`) to give full width back to the terminal
- Status bar at bottom: current branch name, session ID, active keybinds
- Tree panel supports keyboard navigation independent of the terminal pane

---

## Keybindings (Arbor-specific)

All bindings use `Ctrl+` prefix to avoid conflicting with Claude Code's own
input handling.

| Binding       | Action                                              |
|---------------|-----------------------------------------------------|
| `Ctrl+B`      | Branch from current message (calls `claude /branch` then registers in tree) |
| `Ctrl+T`      | Toggle tree panel visibility                        |
| `Ctrl+↑ / ↓`  | Navigate tree (move focus to tree panel)            |
| `Ctrl+Enter`  | Switch to selected branch in tree panel             |
| `Ctrl+R`      | Rename current branch                               |
| `Ctrl+W`      | Close / archive current branch                      |
| `Ctrl+Z`      | Go to parent branch                                 |
| `?`           | Show Arbor help overlay (only when tree has focus)  |

Everything else — all typing, `Esc`, `Enter`, `/commands` — goes straight
to the PTY.

---

## Session Tree Model

Claude Code already stores sessions as JSONL files with parent references.
Arbor reads these to construct the tree — it does not maintain its own
parallel state.

```
~/.claude/projects/<project-hash>/
  <session-id>.jsonl        # each session is a node
  <session-id>.jsonl        # forked sessions reference parent session
```

Arbor adds a lightweight metadata file alongside:

```
~/.arbor/
  branches.json             # display names, colours, active node pointer
```

This means Arbor's tree view survives restarts and can be shared/committed
if desired.

**Node structure (in memory):**

```rust
struct BranchNode {
    session_id: String,
    parent_id: Option<String>,
    name: String,           // user-assigned or auto ("branch-1")
    created_at: DateTime,
    last_active: DateTime,
    message_count: usize,
    children: Vec<String>,  // session_ids
}
```

---

## Crate Stack

| Concern              | Crate                          |
|----------------------|--------------------------------|
| TUI framework        | `ratatui`                      |
| Terminal backend     | `crossterm`                    |
| PTY management       | `portable-pty` (cross-platform) |
| Tree widget          | `tui-tree-widget`              |
| File system events   | `notify`                       |
| JSONL parsing        | `serde_json` + `serde`         |
| Async runtime        | `tokio`                        |
| CLI arg parsing      | `clap`                         |
| Date/time            | `chrono`                       |
| Config/state         | `serde_json` (branches.json)   |

---

## CLI Interface

Arbor is a drop-in wrapper. Any flags not recognised by Arbor are passed
directly to `claude`:

```bash
# Basic usage — identical to calling claude
arbor

# Pass any claude flag through
arbor --model sonnet
arbor --dangerously-skip-permissions
arbor --resume <session-id>
arbor --add-dir ~/other-project

# Arbor-specific flags
arbor --no-tree          # start with tree panel hidden
arbor --claude-bin <path> # override claude binary location
```

---

## Phased Build Plan

### Phase 1 — PTY Shell (foundation)
- Spawn `claude` in a PTY, relay bytes to/from terminal
- Confirm 100% command/flag compatibility
- No tree UI yet — just a working transparent wrapper

### Phase 2 — Tree Panel
- Read `~/.claude/` JSONL files on startup and on fs change
- Render tree panel with `tui-tree-widget`
- Toggle panel with `Ctrl+T`
- Navigate tree with keyboard (no switching yet)

### Phase 3 — Branching
- `Ctrl+B` sends `/branch` to the PTY, captures new session ID
- Register branch in `~/.arbor/branches.json`
- Update tree panel live

### Phase 4 — Branch Switching
- `Ctrl+Enter` on a tree node resumes that session
- Gracefully suspends current PTY, spawns new one with `--resume`
- Active branch highlighted in tree panel

### Phase 5 — Polish
- Branch naming (`Ctrl+R`)
- Status bar with breadcrumb (main > branch-1 > b-1a)
- Colour coding by branch depth
- Search/filter in tree panel
- Arbor config file (`~/.arbor/config.toml`)

---

## Open Questions

1. **PTY switching latency** — suspending one PTY and resuming another will
   have a small delay. Acceptable? Or should we keep multiple PTYs alive
   in the background?

2. **Windows support** — `portable-pty` supports Windows but Claude Code's
   own Windows support is limited. Deprioritise for now?

3. **Branch naming UX** — auto-name from first message content (e.g. first 40
   chars) vs sequential numbering? Probably auto-name with rename option.

4. **Merge/summarise** — out of scope for v1 but the most requested feature
   in the Claude Code issue tracker. Keep the architecture compatible.

5. **Project name** — "Arbor" (a tree structure / also a bower, fitting the
   British sensibility). Open to alternatives.

---

## What Arbor Is Not

- Not a reimplementation of Claude Code
- Not a replacement for the `claude` binary
- Not a web app or Electron wrapper
- Not managing API keys or model selection — Claude Code does all of that

---

*Last updated: 2026-07-12*
