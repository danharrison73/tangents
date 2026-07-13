//! The single event bus. Producer threads (PTY reader, input reader, fs watcher)
//! all funnel into one channel that the main loop drains.

use crossterm::event::Event as CtEvent;

/// Identifies which PTY generation an event came from. When we respawn claude
/// (fork/switch), we bump the generation so stale output/exit from the killed
/// child is ignored instead of quitting tangents.
pub type Generation = u64;

/// Everything the main loop reacts to.
pub enum Event {
    /// Raw bytes read from a claude PTY of the given generation.
    PtyOutput(Generation, Vec<u8>),
    /// A PTY generation closed (claude exited).
    PtyExit(Generation),
    /// A terminal input event from the user (key, paste, resize, ...).
    Input(CtEvent),
    /// The session tree changed on disk.
    SessionsChanged,
}

pub type EventTx = std::sync::mpsc::Sender<Event>;
