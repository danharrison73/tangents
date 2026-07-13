//! Watches the Claude project directory for JSONL changes and pings the event
//! bus (debounced, so a burst of writes collapses into one refresh).

use std::path::Path;
use std::time::Duration;

use anyhow::Result;
use notify_debouncer_full::notify::{RecommendedWatcher, RecursiveMode};
use notify_debouncer_full::{Debouncer, RecommendedCache, new_debouncer};

use crate::event::{Event, EventTx};

/// Keeps the debouncer alive; dropping it stops watching.
pub struct SessionWatcher {
    _debouncer: Debouncer<RecommendedWatcher, RecommendedCache>,
}

/// Begin watching `project_dir`. Any change emits [`Event::SessionsChanged`].
pub fn watch(project_dir: &Path, tx: EventTx) -> Result<SessionWatcher> {
    // Ensure the directory exists so watch() has something to attach to; it is
    // created lazily by claude on first session otherwise.
    std::fs::create_dir_all(project_dir).ok();

    let mut debouncer = new_debouncer(
        Duration::from_millis(200),
        None,
        move |res: notify_debouncer_full::DebounceEventResult| {
            if res.is_ok() {
                let _ = tx.send(Event::SessionsChanged);
            }
        },
    )?;
    debouncer.watch(project_dir, RecursiveMode::NonRecursive)?;
    Ok(SessionWatcher {
        _debouncer: debouncer,
    })
}
