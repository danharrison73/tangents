# Arbor — Implementation Plan

> Working plan derived from `project-spec.md`, reconciled against the **actual**
> `claude` binary (v2.1.207) and real session JSONL on this machine.
> Where the spec and reality disagree, reality wins — see **§0 Spec Corrections**.

---

## 0. Spec Corrections (read this first)

I inspected `claude --help` and `~/.claude/projects/<hash>/*.jsonl` before
planning. Three load-bearing claims in `project-spec.md` are inaccurate and the
architecture must change accordingly.

### 0.1 There is no `/branch` slash command ❌
The spec's Phase 3 hinges on `Ctrl+B` sending `claude /branch` to the PTY. **No
such command exists.** The real primitive for forking a conversation is a
**launch flag**:

```
--fork-session      When resuming, create a new session ID (with --resume/--continue)
--session-id <uuid> Use a specific session ID for the session
-r, --resume [value]  Resume a conversation by session ID
--from-pr / -c / --continue  related resume modes
```

**Consequence:** Branching is not an in-band keystroke to a running `claude`.
It is: *spawn a new `claude` process* with `--resume <current> --fork-session`
(optionally `--session-id <uuid-we-choose>` so we know the child's ID up front).
This collapses Phase 3 and Phase 4 into essentially the same mechanism —
"branch" and "switch" are both "tear down the current PTY, spawn a new one with
different resume args." That's a simplification, but it means every branch/switch
incurs the PTY-swap latency the spec flags as Open Question #1.

Caveat: `--fork-session` forks from the **leaf** (end) of the session, not from
an arbitrary earlier message. "Branch from current message" mid-scroll is *not*
a CLI primitive; the interactive `/rewind` feature is the only earlier-point
mechanism and it isn't scriptable via flags. v1 should define "branch" as
"fork from where we are now."

### 0.2 Sessions do not store a parent-session reference ❌
The spec says forked sessions "reference parent session" and that Arbor "does
not maintain its own parallel state." Reality from the JSONL:

- `parentUuid` links **messages within one session** (a message DAG) — it does
  **not** point across session files.
- `parentSessionId`: **0 occurrences.** There is no parent-session field.
- Cross-session hints that *do* exist: `leafUuid` (last-prompt marker),
  `bridgeSessionId`, `sessionId`, plus every message carries a `uuid`.

When you `--fork-session`, Claude Code copies the parent's messages (with their
original `uuid`s) into the new file, then diverges. So parentage is
**recoverable heuristically** by detecting a shared UUID prefix between two
session files — but it is *not* a field you can just read.

**Consequence:** Arbor **must** own its tree metadata. `~/.arbor/branches.json`
is not optional polish — it is the source of truth for parent links, recorded at
fork time when we already know both IDs. The JSONL watch is for *discovery*
(new sessions, message counts, titles), not for parentage. (Optional stretch:
reconstruct parentage for pre-existing sessions via UUID-prefix overlap.)

### 0.3 Session schema is richer than the spec's struct
Real per-line fields worth using: `sessionId`, `cwd`, `gitBranch`, `timestamp`,
`messageCount`, `lastPrompt`, `aiTitle` (auto-generated title!), `isSidechain`
(sub-agent traffic — must be filtered out of the tree), `version`, `type`
(`user`/`assistant`/`system`/`summary`/`ai-title`/`bridge-session`/…).
`aiTitle`/`lastPrompt` answer Open Question #3 (auto-naming) for free.

### 0.4 Keybinding conflict risk
`Ctrl+B`, `Ctrl+R` (reverse-search), `Ctrl+W` (delete-word), `Ctrl+T`, `Ctrl+Z`
(SIGTSTP) are all meaningful to the shell/readline/Claude's own input. Grabbing
them globally will surprise users. Plan adopts a **tmux-style prefix** (default
`Ctrl+G`, then a key) so exactly one keystroke is reserved and everything else
passes through untouched. The spec's raw bindings become the *post-prefix* keys.

---

## 1. Scope for v1

**In:** transparent PTY wrapper around `claude`; live tree sidebar built from
JSONL + `branches.json`; fork-a-branch; switch-branch; toggle/navigate sidebar;
rename; status bar. Linux + macOS.

**Out (v1):** Windows (Open Q #2 — defer), merge/summarise (Open Q #4 — keep
metadata forward-compatible), branching from an arbitrary past message,
multi-PTY warm standby.

---

## 2. Architecture

```
                 ┌────────────────────── Arbor process ──────────────────────┐
  stdin (raw) ──▶│  Input router ── prefix? ──yes──▶ Arbor command handler    │
                 │        │                              │ (fork/switch/…)     │
                 │        └──no──▶ PTY master.write ──────┘                     │
                 │                                                             │
  terminal   ◀───│  Renderer (ratatui) ◀── VT screen buffer ◀── PTY read loop │
                 │            ▲                                                │
                 │            └── Tree model ◀── SessionStore ◀── notify(fs)   │
                 │                     ▲                            watches    │
                 │                     └── branches.json (parent links)       │
                 └───────────────────────────────────────────────────────────┘
```

Threads/tasks (tokio):
1. **PTY read loop** — read master → feed a VT parser → emit screen state.
2. **Input loop** — read stdin (crossterm raw events); prefix state machine
   decides Arbor-intercept vs forward-to-PTY.
3. **FS watch** — `notify` on `~/.claude/projects/<hash>/`; debounce; reparse
   changed files → update `SessionStore` → mark tree dirty.
4. **Render loop** — on any dirty flag (screen or tree), redraw ratatui frame.

### 2.1 The hard problem: rendering `claude`'s output inside a ratatui pane
The spec hand-waves this. `claude` emits a full-screen interactive TUI with
colors, cursor moves, and alt-screen usage. To show it inside a *sub-rectangle*
of Arbor's own ratatui frame, Arbor must run a **terminal emulator**: feed PTY
bytes to a VT100 parser that maintains a virtual screen grid, then blit that grid
into the ratatui terminal-pane widget each frame, and translate cursor position.

- Crate: **`vt100`** (or `wezterm-term`/`termwiz` for higher fidelity). Pair with
  a ratatui widget like **`tui-term`** (built exactly for "render a `vt100::Parser`
  screen in a ratatui rect"). This replaces the spec's implicit "bytes just flow
  out" model — they don't; they must be parsed and re-rendered.
- The PTY's window size must be set to the pane's inner dimensions (rows×cols),
  and updated (`TIOCSWINSZ` via `portable-pty`'s `resize`) whenever the sidebar
  toggles or the terminal is resized. Getting this wrong is the #1 source of
  garbled `claude` output.
- **Fallback / de-risk option:** if in-pane emulation proves too janky for
  Claude's decorative sequences, fall back to **full-screen passthrough** — Arbor
  gives 100% of the terminal to `claude` and overlays the tree only transiently
  (prefix-triggered popup), never splitting the screen. Decide at end of Phase 2.

### 2.2 Branch/switch mechanics
- Each branch = a Claude session id + Arbor metadata. To create: choose a fresh
  `uuid`, spawn `claude --resume <parent> --fork-session --session-id <uuid>`
  (verify `--fork-session` honors `--session-id`; if not, scrape the new id from
  the JSONL that appears). Record `{child_uuid, parent: <parent>}` in
  `branches.json` **immediately** — this is our only reliable parent link.
- Switch = kill/detach current PTY child, spawn `claude --resume <target>`.
  A branch's process is not kept warm in v1 (accept the latency; Open Q #1).
  Structure the `PtySession` type so a warm-pool is a later drop-in.

---

## 3. Data Model

```rust
// ~/.arbor/branches.json  (Arbor-owned source of truth)
struct ArborState {
    version: u32,
    active: Option<String>,                 // active session id
    branches: HashMap<String, BranchMeta>,  // keyed by session id
}
struct BranchMeta {
    parent: Option<String>,   // set at fork time — the link JSONL can't give us
    name: Option<String>,     // user rename; else fall back to aiTitle/lastPrompt
    color: Option<u8>,
    archived: bool,
}

// Derived at runtime by merging branches.json + scanned JSONL:
struct BranchNode {
    session_id: String,
    parent_id: Option<String>,   // branches.json first, UUID-overlap heuristic 2nd
    display_name: String,        // name ?? aiTitle ?? lastPrompt[..40] ?? "branch-N"
    created_at: DateTime<Utc>,   // first timestamp in file
    last_active: DateTime<Utc>,  // mtime / last timestamp
    message_count: usize,        // count non-sidechain user/assistant lines
    children: Vec<String>,
}
```

`SessionStore` = the scanner: enumerate `*.jsonl` for the cwd's project hash,
stream-parse each (only fields we need), skip `isSidechain` traffic, and expose
`Vec<BranchNode>` as an adjacency list. Handles partial/last-line-truncated files
(Claude writes incrementally — a half-written final line is normal; ignore it).

---

## 4. Crate Stack (adjusted)

| Concern            | Spec crate        | Plan | Note |
|--------------------|-------------------|------|------|
| TUI                | ratatui           | ✅ keep | |
| Backend            | crossterm         | ✅ keep | raw mode, key events |
| PTY                | portable-pty      | ✅ keep | spawn, resize, kill |
| **Terminal emu**   | *(missing)*       | ➕ **add** `vt100` + `tui-term` | render claude inside a pane — the crux |
| Tree widget        | tui-tree-widget   | ✅ keep | |
| FS events          | notify            | ✅ keep | + debounce (`notify-debouncer-mini`) |
| JSONL              | serde_json/serde  | ✅ keep | stream/line parse |
| Async              | tokio             | ✅ keep | |
| CLI                | clap              | ✅ keep | `trailing_var_arg` to pass unknown flags → claude |
| Date/time          | chrono            | ✅ keep | |
| State              | serde_json        | ✅ keep | atomic write (tmp+rename) for branches.json |

---

## 5. Phased Build Plan (revised)

Each phase ends at a demoable, shippable state. Verify against the real `claude`
binary at every phase, not mocks.

### Phase 0 — Skeleton (½ day)
`cargo init`; clap arg model (`--no-tree`, `--claude-bin`, `trailing_var_arg`
passthrough); locate claude binary; resolve project hash for cwd
(`~/.claude/projects/<slugified-cwd>/`). No UI. **Done when:** `arbor --model sonnet`
prints the exact argv it would hand to `claude`.

### Phase 1 — Transparent PTY wrapper (the foundation)
Spawn `claude [passthrough args]` in a PTY; full-screen passthrough with a VT
emulator (`vt100`) driving a ratatui full-frame render; wire stdin→PTY,
PTY→screen; handle resize (`SIGWINCH` → `resize`); clean teardown on `claude`
exit. **Done when:** `arbor` is indistinguishable from `claude` — every slash
command, flag, permission prompt, and `Ctrl+C`/`Esc` works. **This is the
riskiest phase; timebox and decide split-pane vs overlay before Phase 2.**

### Phase 2 — Tree panel (read-only)
`SessionStore` scanner + `notify` watch + debounce; build `BranchNode` graph;
render `tui-tree-widget` sidebar at 25%; `Ctrl+G T` toggles it; resize PTY on
toggle; keyboard nav within tree (prefix-then-arrows). No switching yet.
**Done when:** opening two real claude sessions in this project shows both as
nodes with live message counts and auto-titles.

### Phase 3 — Fork a branch
Prefix `B`: spawn a forked session (`--resume cur --fork-session
--session-id <new>`), record parent in `branches.json`, swap the PTY to the
child. Tree updates live via the fs watch. **Done when:** forking creates a child
node under the current node and drops you into the forked conversation.

### Phase 4 — Switch branches
Prefix + select + `Enter`: tear down current PTY, spawn `claude --resume
<target>`, highlight active node, update `branches.json.active`. Confirm-on-switch
if the current child is mid-response. **Done when:** you can hop between any two
branches and land in the right conversation each time.

### Phase 5 — Polish
Rename (prefix `R`, writes `name`), status-bar breadcrumb (main › branch-1 ›
b-1a), depth-based color, tree filter/search, `~/.arbor/config.toml`
(prefix key, colors, default `--no-tree`). Optional: UUID-overlap parent
reconstruction for pre-existing sessions; warm-PTY pool if switch latency stings.

---

## 6. Top Risks

1. **In-pane rendering of `claude`** (§2.1) — highest risk. Mitigation: prove it
   in Phase 1; overlay-fallback ready.
2. **`--fork-session` + `--session-id` interaction** — verify empirically in
   Phase 3; fallback is to scrape the new id from the newest JSONL.
3. **PTY-swap latency on every branch/switch** (Open Q #1) — accept in v1; keep
   `PtySession` abstraction warm-pool-ready.
4. **Parent-link fragility** — mitigated by owning `branches.json`; heuristic
   only as backfill.
5. **Keybinding collisions** (§0.4) — mitigated by single prefix key.

## 7. Open Questions for the user
- **Prefix key**: adopt tmux-style `Ctrl+G <key>` (my recommendation) or insist
  on the spec's direct `Ctrl+*` bindings despite collision risk?
- **Split-pane vs overlay** sidebar — decide after Phase 1 spike, or commit to
  split-pane now?
- **Name**: keep "Arbor"? (Open Q #5 — no blocker either way.)
```

Everything else in `project-spec.md` (layout %, status bar, "what Arbor is not")
carries over unchanged.
