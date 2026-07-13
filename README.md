# tangents

> Tree-structured conversation TUI for Claude Code.
> *Every tangent deserves its own branch.*

`tangents` is a thin Rust TUI that wraps the real `claude` binary in a
pseudo-terminal. Claude Code runs **completely unmodified** inside the terminal
pane; tangents adds a sidebar showing your session tree and a few keybindings to
branch, switch, and navigate — without reimplementing anything Claude Code does.

```
┌─ tangents ◂ ──┬──────────────────────────────────────┐
│ ● main   ·227 │  > claude running here, unmodified    │
│  ├ fix-bug ·12│  > all slash commands work            │
│  └ refactor ·8│  > all flags work                     │
│               │  > all prompts work                   │
├───────────────┴──────────────────────────────────────┤
│ main › fix-bug          ^G: t tree · b branch · ? help │
└────────────────────────────────────────────────────────┘
```

## How it works

- Spawns `claude [your args]` in a PTY (`portable-pty`); every keystroke,
  slash command, flag, and permission prompt passes straight through.
- Renders claude inside a ratatui pane via a `vt100` terminal emulator
  (`tui-term`) — the terminal pane is a faithful re-render of claude's own TUI.
- Reads `~/.claude/projects/<hash>/*.jsonl` (with a `notify` file-watch) to build
  the session tree live.
- Keeps its own metadata in `~/.tangents/branches.json` — **parent links**,
  names, and the active branch. (Claude Code's JSONL does not record which
  session a fork came from, so tangents owns that.)

## Branching model

There is no `/branch` command in Claude Code. tangents branches by **forking**:
it spawns a new `claude --resume <current> --fork-session --session-id <new>`,
records the parent edge, and swaps the terminal to the child. Switching is the
same machinery with a plain `--resume`. A fork starts from the *end* of the
current conversation (the only fork point the CLI exposes).

## Usage

```bash
# Drop-in replacement for `claude` — any claude flag is forwarded:
tangents
tangents --model sonnet
tangents --dangerously-skip-permissions
tangents --resume <session-id>

# tangents-specific flags:
tangents --no-tree            # start with the tree hidden
tangents --claude-bin <path>  # override the claude binary location
tangents --print-argv         # show the argv tangents would spawn, then exit
```

`claude` is located via `--claude-bin`, then `$TANGENTS_CLAUDE_BIN`, then `PATH`.

## Keybindings

All commands use a tmux-style **prefix** (default `Ctrl+G`), so exactly one
keystroke is reserved — everything else goes to claude untouched.

| Keys            | Action                                        |
|-----------------|-----------------------------------------------|
| `Ctrl+G` `t`    | Toggle the tree panel                         |
| `Ctrl+G` `b`    | Branch (fork the current session)             |
| `Ctrl+G` `⏎`    | Switch to the selected branch                 |
| `Ctrl+G` `r`    | Rename the selected/current branch            |
| `Ctrl+G` `j`/`k` (or `↑`/`↓`) | Focus the tree and navigate     |
| `Ctrl+G` `w`    | Toggle focus between terminal and tree        |
| `Ctrl+G` `?`    | Help overlay                                  |
| `Ctrl+G` `Ctrl+G` | Send a literal `Ctrl+G` to claude           |

In the tree: `⏎` switch · `space` expand/collapse · `h`/`l` collapse/open ·
`Esc` back to the terminal.

## Config

Optional `~/.tangents/config.toml`:

```toml
prefix = "g"      # command prefix key, used as Ctrl+<prefix>
no_tree = false   # start with the tree hidden
tree_width = 30   # sidebar width in columns
```

## Build

```bash
cargo build --release
./target/release/tangents
```

## Status

Phases 0–5 of [`PLAN.md`](PLAN.md) are implemented: transparent wrapper, live
tree, fork, switch, and polish (status bar/breadcrumb, rename, depth colour,
help, config). Deferred: tree search/filter, heuristic parent reconstruction for
pre-existing sessions, and a warm-PTY pool to hide switch latency.

See [`PLAN.md`](PLAN.md) for the architecture and the three spec corrections that
shaped it (no `/branch` command; no parent-session field in JSONL; in-pane
terminal emulation).
