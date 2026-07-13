# tangents — Core Concepts

A ground-up tour for someone new to **Rust** and to **terminal (TUI/CLI) apps**.
It explains the ideas you need to read this codebase, using real snippets from
it. Read it top-to-bottom once; after that it's a reference.

Two separate things are going on in this project, and it helps to keep them apart:

1. **The Rust language** — how the code is written (ownership, `Result`, traits…).
2. **The terminal domain** — what the code is *doing* (PTYs, escape codes,
   event loops, rendering).

We'll do Rust first (Part A), the terminal world second (Part B), then show how
the two meet in tangents (Part C).

---

## Part A — Rust, the subset this project uses

You don't need all of Rust. Here's the ~20% that covers ~95% of this code.

### A.1 Cargo: the build tool and package manager

- A Rust project is a **crate**. `Cargo.toml` is its manifest (name, version,
  dependencies). `Cargo.lock` pins exact versions.
- `src/main.rs` is the entry point of a binary crate; it has `fn main()`.
- Other `.rs` files in `src/` are **modules**, pulled in with `mod name;`.
- Commands you'll use:
  - `cargo build` — compile (add `--release` for an optimized build).
  - `cargo run -- <args>` — build then run; args after `--` go to the program.
  - `cargo test` — run every `#[test]` function.
  - `cargo clippy` — a linter that catches non-idiomatic code.
  - `cargo fmt` — auto-format to the standard style.

Dependencies (from our `Cargo.toml`) are libraries from crates.io — `ratatui`
(TUI), `portable-pty` (PTYs), `vt100` (terminal emulator), etc.

### A.2 Values, variables, and mutability

```rust
let x = 5;          // immutable by default
let mut y = 5;      // opt in to mutation with `mut`
y += 1;             // ok; `x += 1` would NOT compile
```

Rust is immutable-by-default. If something can change, it must say `mut`. This
is a running theme: the compiler wants to know exactly who can change what.

### A.3 Ownership & borrowing — the one genuinely new idea

This is the concept that makes Rust *Rust*. Most languages let any number of
references point at the same data and hope you don't misuse it. Rust enforces
rules at compile time so there are no data races and no use-after-free.

Three rules:

1. Every value has exactly one **owner** (a variable).
2. When the owner goes out of scope, the value is dropped (freed) automatically.
   No garbage collector, no manual `free`.
3. You can **borrow** a value instead of moving it:
   - `&T` — a *shared* (read-only) borrow. You can have many at once.
   - `&mut T` — an *exclusive* (mutable) borrow. Only one at a time, and no
     shared borrows may coexist with it.

You'll see this everywhere. Example from `session.rs`:

```rust
pub fn scan(project_dir: &Path) -> Vec<SessionInfo> { … }
```

`&Path` means "I'm *borrowing* the path to look at it; I won't take ownership or
modify it." The caller keeps its `Path`. Returning `Vec<SessionInfo>` *moves*
a freshly-built vector out to the caller (ownership transfers).

Why you sometimes see `.clone()`: when the borrow rules make sharing awkward,
you make an owned copy. E.g. in `app.rs` the draw code does
`let active = self.state.active.clone();` so the render closure owns a copy and
doesn't hold a borrow of `self` while other fields are borrowed mutably.

> **Mental model:** think of `&` as "lending a book to read" and `&mut` as
> "lending it to someone who'll write in it — so nobody else can hold it
> meanwhile." `clone()` is "photocopy it so we each have our own."

### A.4 `Option` and `Result` — no null, no exceptions

Rust has no `null` and no exceptions. Two enums stand in:

- `Option<T>` = `Some(T)` or `None`. "A value that might be absent."
- `Result<T, E>` = `Ok(T)` or `Err(E)`. "An operation that might fail."

You must handle both cases; the compiler won't let you forget. From `cli.rs`:

```rust
pub current_session_id: Option<String>,   // maybe we know it, maybe not
```

The `?` operator is the ergonomic bit. `something?` means "if this is
`Ok`/`Some`, unwrap it and continue; if it's `Err`/`None`, return it from the
current function immediately." From `pty.rs`:

```rust
let child = pair.slave.spawn_command(cmd)
    .with_context(|| format!("failed to spawn `{program}`"))?;   // <- the ?
```

If spawning fails, the function returns the error right there. This is how you
get concise error handling without try/catch. We use the **`anyhow`** crate for
`Result` types where we don't care about a specific error enum — `anyhow::Result<T>`
is "succeeds with `T` or fails with some error that carries a message."

### A.5 Structs and enums

A **struct** groups related data (like a record/object without inheritance):

```rust
pub struct SessionInfo {
    pub session_id: String,
    pub message_count: usize,
    pub last_active: Option<DateTime<Utc>>,
    …
}
```

An **enum** is a type that's *one of several variants* — and variants can hold
data. This is far more powerful than enums in most languages. Our event bus:

```rust
pub enum Event {
    PtyOutput(Generation, Vec<u8>),  // bytes from claude, tagged with a generation
    PtyExit(Generation),             // claude closed
    Input(CtEvent),                  // a keystroke/paste/resize from the user
    SessionsChanged,                 // a JSONL file changed on disk
}
```

One `Event` value is *exactly one* of these. To use it you must `match` (below),
which forces you to consider every variant.

### A.6 `impl` blocks and methods

Behaviour is attached to types via `impl`:

```rust
impl SessionInfo {
    pub fn derived_name(&self) -> String { … }   // a method: takes &self
}
```

- `&self` — a method that reads the struct (shared borrow).
- `&mut self` — a method that mutates it (exclusive borrow).
- `self` — consumes it.
- No `self` — an "associated function", i.e. a constructor-like thing, called as
  `SessionInfo::something(...)`. `App::new(...)` is one.

### A.7 Pattern matching with `match`

`match` is a switch on steroids — it destructures enums and must be exhaustive.
The heart of our app loop (`app.rs`):

```rust
match ev {
    Event::PtyOutput(generation, bytes) => {
        if generation == self.pty_generation {
            self.parser.process(&bytes);   // feed bytes to the emulator
            return true;                    // needs redraw
        }
        false                               // stale output — ignore
    }
    Event::PtyExit(generation) => { … }
    Event::Input(ct) => self.handle_input(ct),
    Event::SessionsChanged => { self.rescan(); true }
}
```

Each arm binds the data inside the variant (`generation`, `bytes`) to names you
use on the right. `if let Some(x) = maybe { … }` is the shorthand for matching
just one case.

### A.8 Traits — Rust's "interfaces"

A **trait** is a set of methods a type promises to provide. Code can be generic
over "any type implementing trait X." You mostly *use* traits here rather than
define them. Two you'll notice:

- `serde::Deserialize` / `Serialize` — "this struct can be turned to/from JSON or
  TOML." We `#[derive(Deserialize)]` on `SessionInfo` so `serde_json` can fill it
  from a JSONL line automatically.
- `tui_term::widget::Screen` — a trait the `PseudoTerminal` widget renders. The
  `vt100::Screen` type implements it, which is *why* we can hand our emulator's
  screen straight to the widget.

`#[derive(...)]` auto-generates a trait implementation. `#[derive(Debug, Clone)]`
on a struct gives you "printable with `{:?}`" and "`.clone()`able" for free.

### A.9 Closures

A **closure** is an anonymous function that can capture variables from around it.
Written `|args| body`. Our filesystem watcher hands one to the `notify` library:

```rust
let mut debouncer = new_debouncer(
    Duration::from_millis(200),
    None,
    move |res| {                       // <- closure; runs on fs events
        if res.is_ok() {
            let _ = tx.send(Event::SessionsChanged);
        }
    },
)?;
```

`move` means the closure *takes ownership* of what it captures (here, `tx`, the
channel sender) — needed because the closure outlives the current function (it
runs later, on another thread).

### A.10 Generics & lifetimes (just enough to not be scared)

- **Generics**: `Vec<T>` is "a vector of some type `T`." `Option<String>` is an
  option of string. You'll read these constantly; you rarely write new ones here.
- **Lifetimes**: occasionally you'll see `'a` or `'static`, e.g.
  `TreeItem<'static, String>`. A lifetime is the compiler tracking "how long a
  borrow is valid." `'static` means "lives for the whole program / owns its
  data." You don't need to master this to follow the code — read `'static` as
  "self-contained, no borrowed strings inside."

### A.11 Modules & visibility

- `mod foo;` in `main.rs` includes `src/foo.rs`.
- `pub` makes an item visible outside its module. No `pub` = private.
- `use crate::foo::Bar;` imports `Bar` so you can write `Bar` instead of the full
  path. `crate::` = "from the root of this crate."

That's the whole language surface you need here. Now the domain.

---

## Part B — How terminals actually work

This is the part with no prior analog if you've only built web/desktop apps. It's
worth understanding because tangents is *entirely* about manipulating a terminal.

### B.1 A terminal is a byte stream, both ways

A terminal program (your shell, vim, claude, tangents) talks to the terminal
**emulator** (the window: iTerm, Windows Terminal, GNOME Terminal…) through two
byte streams:

- **stdout**: bytes the program *writes* → the emulator *displays*.
- **stdin**: keystrokes the user types → bytes the program *reads*.

Crucially, "display" isn't just letters. Interleaved in the output are **escape
sequences** (also called ANSI/VT sequences): invisible control codes starting
with the ESC byte (`0x1b`, written `\x1b` or `\e`). Examples:

- `\x1b[31m` → "draw following text in red"
- `\x1b[2J` → "clear the screen"
- `\x1b[10;5H` → "move the cursor to row 10, column 5"
- `\x1b[?1049h` → "switch to the alternate screen buffer" (full-screen apps)

So a full-screen app like claude doesn't "have widgets" — it just prints a
carefully-ordered stream of text and escape codes that *paint* a UI. Our
`keys.rs` does the reverse for input: when crossterm tells us "user pressed
Up-arrow," we must send the bytes `\x1b[A` to claude, because that's what an
arrow key physically sends down stdin.

### B.2 Cooked mode vs raw mode

By default a terminal is in **cooked** (canonical) mode: the OS line-buffers
input, echoes typed characters, and handles backspace/Ctrl+C for you. Great for
`cat`, useless for a UI — you want each keystroke *immediately* and you want to
draw the screen yourself.

**Raw mode** turns all that off: no echo, no line buffering, no signal
interception — every keystroke arrives as raw bytes the moment it's pressed.
Every TUI enables raw mode on entry and *must* restore the terminal on exit,
otherwise the user's shell is left broken. In `app.rs`:

```rust
struct TerminalGuard { … }
impl TerminalGuard {
    fn enter() -> Result<Self> {
        enable_raw_mode()?;                              // go raw
        execute!(stdout, EnterAlternateScreen, EnableBracketedPaste)?;
        …
    }
}
impl Drop for TerminalGuard {
    fn drop(&mut self) {                                 // runs automatically on exit
        let _ = disable_raw_mode();                      // ALWAYS restore
        let _ = execute!(io::stdout(), LeaveAlternateScreen, …, Show);
    }
}
```

That `Drop` impl is a key Rust idiom: `Drop::drop` runs automatically when the
value goes out of scope — *even if the program panics*. So the terminal is
restored no matter how we exit. This is Rust's version of "try/finally," tied to
ownership. (`PtySession` uses the same trick to guarantee the claude process is
killed.)

**Alternate screen** (`EnterAlternateScreen`) is the secondary buffer full-screen
apps draw into, so that when you quit, your original scrollback reappears
untouched — like how vim leaves your prompt as it was.

### B.3 What a PTY is, and why tangents needs one

Normally when a program runs, it inherits *your* terminal. But tangents needs to
run claude and sit *between* claude and the real terminal — capturing everything
claude draws so it can put it in a sub-pane next to the tree.

A **pseudo-terminal (PTY)** is an OS-provided fake terminal: a pair of endpoints.

- The **master** end is held by the *controlling* program (tangents).
- The **slave** end is handed to the *child* (claude) as its stdin/stdout.

From claude's perspective it's talking to a perfectly normal terminal — so *every*
slash command, flag, and prompt works, with zero integration. But actually,
everything claude writes comes out of the master end into tangents' hands, and
anything tangents writes to the master is delivered to claude as if typed. That's
the whole trick that makes the wrapper transparent. `pty.rs`:

```rust
let pair = pty_system.openpty(PtySize { rows, cols, … })?;  // master + slave
let child = pair.slave.spawn_command(cmd)?;                 // claude runs on the slave
let mut reader = pair.master.try_clone_reader()?;           // we read claude's output
let writer = pair.master.take_writer()?;                    // we write claude's input
```

The PTY has a **size** (rows × columns). If tangents shrinks claude's pane (say,
to show the tree), it must tell the PTY the new size (`master.resize(...)`), or
claude will draw as if it still had the whole screen and everything smears. Pane
size and PTY size must stay in lockstep — a classic source of TUI bugs.

### B.4 Terminal emulation: why `vt100`

Here's the subtle bit the spec glossed over. Claude's output is a stream of text
+ escape codes meant to paint a *whole terminal screen*. We can't just forward
those bytes to a 75%-width sub-rectangle — the cursor-move codes would reference
absolute screen positions and it'd be chaos.

So tangents runs a **terminal emulator in software**: the `vt100` crate. You feed
it claude's raw bytes; it maintains a virtual grid of cells (each with a
character, colour, style) exactly as a real terminal would — the "screen" claude
*thinks* it's drawing to:

```rust
self.parser.process(&bytes);   // apply claude's output to the virtual screen
let screen = self.parser.screen();  // the resulting grid of cells
```

Then a widget (`tui-term`'s `PseudoTerminal`) copies that virtual grid into
whatever rectangle we want on the *real* screen. So the flow is:

```
claude → (PTY bytes) → vt100 emulator → virtual screen grid → tui-term widget → real terminal pane
```

That indirection is what lets claude live inside a pane beside the tree.

### B.5 The event loop and immediate-mode rendering

A TUI is a loop:

```
forever:
    wait for an event (keypress, child output, timer, fs change)
    update in-memory state based on it
    redraw the screen from that state
```

Two flavours of UI exist: *retained-mode* (you create widget objects once and
mutate them — like the DOM) and **immediate-mode** (each frame you describe the
*entire* UI from scratch and the library diffs it against the last frame). Ratatui
is immediate-mode. Every draw, we build the widgets fresh:

```rust
terminal.draw(|f| {
    f.render_widget(PseudoTerminal::new(screen), term_rect);   // the claude pane
    if let Some(tr) = tree_rect {
        tree.render(f, tr, focus == Focus::Tree, active.as_deref());  // the sidebar
    }
    f.render_widget(Paragraph::new(status), status_rect);      // the status bar
})?;
```

`f.area()` is the whole screen; we carve it into rectangles (`Rect`) with a
`Layout` and render a widget into each. Ratatui figures out the minimal escape
codes to turn last frame into this one. (Fun consequence you saw during testing:
if a cell didn't change, ratatui doesn't redraw it — it just moves the cursor
past it, which is why raw-output scraping "lost" spaces.)

---

## Part C — How it all fits together in tangents

Now the two halves meet. The architecture is a **single event loop fed by several
threads over one channel**.

### C.1 Threads and channels (the concurrency model)

Rust threads are OS threads. We can't have every part of the program reading from
different sources in one loop (a blocking read on the PTY would freeze input), so
we spin up **producer threads** that all send into one **channel** — a thread-safe
queue. The main thread is the single **consumer**.

- Channel type: `std::sync::mpsc` = *multi-producer, single-consumer*. Many
  senders (`tx.clone()`), one receiver (`rx`).
- Producers:
  1. **PTY reader thread** (`pty.rs`) — blocks on reading claude's output, sends
     `Event::PtyOutput`.
  2. **Input thread** (`app.rs::run`) — blocks on `crossterm::event::read()`,
     sends `Event::Input`.
  3. **FS watcher** (`watcher.rs`) — fires `Event::SessionsChanged` on JSONL
     changes.
- Consumer: the main loop calls `rx.recv()`, matches the event, updates state,
  redraws.

```rust
while let Ok(ev) = rx.recv() {          // block until an event arrives
    let mut dirty = app.handle(ev);     // update state; did anything change?
    while let Ok(ev) = rx.try_recv() {  // drain any others waiting (coalesce)
        dirty |= app.handle(ev);
    }
    if app.should_quit { break; }
    if dirty { app.draw(&mut terminal)?; }   // one redraw per burst
}
```

Because everything funnels through `App` on the main thread, there are no locks
and no shared-mutable-state headaches — the borrow checker's favourite shape.

### C.2 The data flow, end to end

Two independent flows:

**Terminal passthrough (the wrapper):**

```
you type ─▶ input thread ─▶ Event::Input ─▶ app.handle_input
             │
             ├─ if it's the prefix (Ctrl+G) or a tangents command → handle internally
             └─ else → keys.rs re-encodes it to bytes → PtySession.write → claude
claude draws ─▶ PTY reader thread ─▶ Event::PtyOutput ─▶ vt100 parser ─▶ redraw pane
```

**The tree (the added value):**

```
claude writes JSONL to ~/.claude/projects/<hash>/  ─▶ notify watcher ─▶ Event::SessionsChanged
   ─▶ session.rs re-scans the files ─▶ tree.rs builds the forest (using parent links
      from ~/.tangents/branches.json) ─▶ redraw sidebar
```

### C.3 The prefix state machine (why Ctrl+G)

Claude uses tons of Ctrl-key combos, so tangents can't just grab `Ctrl+B` etc.
Instead it reserves **one** key, `Ctrl+G` (tmux-style). Logic in
`handle_input`:

- If we're waiting after a prefix (`pending_prefix`), interpret this key as a
  *command* (`t` = toggle tree, `b` = branch, `r` = rename…).
- Else if this key *is* the prefix, set `pending_prefix = true` and swallow it.
- Else, forward it to claude untouched.

So exactly one keystroke is intercepted; everything else is claude's. Pressing
`Ctrl+G` twice sends a literal `Ctrl+G` through, tmux-style.

### C.4 Branching: the mechanism (and a spec correction)

There is **no `/branch` command** in Claude Code (the original spec assumed one).
The real primitive is a launch flag: `claude --resume <id> --fork-session
--session-id <new>` creates a new session id forked from an existing one. So in
tangents, "branch" and "switch" are the *same operation* — kill the current PTY,
spawn a new claude with different resume args:

```rust
fn respawn(&mut self, args: Vec<String>, new_id: Option<String>) {
    self.pty.kill();                 // stop the current claude
    self.pty_generation += 1;        // ← bump the generation (see below)
    self.parser = vt100::Parser::new(self.rows, self.cols, 0);  // fresh screen
    self.pty = PtySession::spawn(self.pty_generation, …, &args, …)?;
    self.current_session_id = new_id;
}
```

**The generation counter** is a subtle but essential detail. When we `kill()` the
old PTY, its reader thread hits end-of-file and sends `Event::PtyExit` — which
would normally quit tangents! So every PTY (and its events) carries a
`generation` number. When we respawn we bump it; the main loop ignores any
`PtyExit`/`PtyOutput` whose generation isn't the current one. Stale death from
the old child is discarded.

Also note: Claude's JSONL records message parentage *within* a session but has no
"this session was forked from that session" field. So tangents records that edge
itself, in `~/.tangents/branches.json`, at the moment it forks — that file is the
*source of truth* for the tree's shape. (See `state.rs`.)

### C.5 File-by-file map

| File | What it owns | Key concepts from above |
|------|--------------|-------------------------|
| `main.rs` | Entry point; wires modules | modules, `Result`, `?` |
| `cli.rs` | Arg parsing, claude/session resolution | borrowing, `Option`, iterators |
| `pty.rs` | The claude child in a PTY | PTY, threads, `Drop` cleanup |
| `keys.rs` | Keypress → terminal bytes | escape sequences, `match` |
| `event.rs` | The `Event` enum + channel types | enums, generics |
| `app.rs` | Event loop, rendering, all commands | event loop, immediate-mode UI, state machine |
| `session.rs` | Reading Claude's JSONL | structs, `serde` deserialize, tolerant parsing |
| `state.rs` | `branches.json` (parent links, names) | serialize, atomic file writes |
| `tree.rs` | Building & drawing the forest | recursion, ratatui widgets, styling |
| `watcher.rs` | Filesystem change notifications | closures, `move`, channels |
| `config.rs` | `~/.tangents/config.toml` | `serde`, defaults |

---

## Part D — How to poke at it (learning by doing)

Small experiments that teach the most:

1. **See the args without launching:** `cargo run -- --print-argv --model sonnet`
   — shows how `cli.rs` splits tangents flags from claude flags.
2. **Read one module end-to-end.** Start with `keys.rs` — it's self-contained,
   pure logic, and has unit tests you can run: `cargo test keys`.
3. **Run the tests and read them.** `cargo test` — the `#[cfg(test)] mod tests`
   blocks are worked examples of every module's behaviour.
4. **Change a keybinding.** In `app.rs::handle_prefix_command`, add a new
   `KeyCode::Char('x') => { … }` arm and rebuild. Fast feedback on the event loop.
5. **Break the borrow checker on purpose.** Add a second `&mut self.tree` next to
   an existing one and read the compiler error — the messages are the best Rust
   teacher you'll get.
6. **Watch the tree update live.** Run `cargo run`, and in another terminal touch
   a file in `~/.claude/projects/<hash>/` — you'll see the watcher fire.

## Glossary

- **Crate** — a Rust package/library.
- **Trait** — an interface; a set of methods a type implements.
- **Borrow** — a temporary reference (`&T` shared, `&mut T` exclusive).
- **Move** — transferring ownership of a value.
- **PTY (pseudo-terminal)** — an OS fake terminal (master/slave pair) that lets
  one program host another as if it were a real terminal.
- **Raw mode** — terminal setting where keystrokes arrive unbuffered, unechoed.
- **Alternate screen** — the secondary buffer full-screen apps draw into.
- **Escape/ANSI/VT sequence** — control codes (starting `\x1b`) that colour text,
  move the cursor, clear the screen, etc.
- **Terminal emulator** — software that interprets those codes into a grid of
  cells. `vt100` is one, running *inside* tangents.
- **Immediate-mode UI** — you re-describe the whole UI each frame; the library
  diffs it. Ratatui works this way.
- **Event loop** — the wait-for-event → update-state → redraw cycle.
- **Channel (mpsc)** — a thread-safe queue; many senders, one receiver.

---

*Companion docs: `PLAN.md` (architecture + the three spec corrections) and
`README.md` (usage + keybindings).*
