//! Owns the `claude` child process running inside a pseudo-terminal.
//!
//! A dedicated reader thread pumps PTY output into the event bus. The writer and
//! master handle stay here so the main loop can send input and resize the PTY.

use anyhow::{Context, Result};
use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};
use std::io::{Read, Write};
use std::path::Path;

use crate::event::{Event, EventTx, Generation};

pub struct PtySession {
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    child: Box<dyn Child + Send + Sync>,
}

impl PtySession {
    /// Spawn `program args...` in a fresh PTY sized `rows`x`cols`, streaming its
    /// output to `tx` as [`Event::PtyOutput`], tagged with `generation`. Emits
    /// [`Event::PtyExit`] on EOF.
    pub fn spawn(
        generation: Generation,
        program: &str,
        args: &[String],
        cwd: &Path,
        rows: u16,
        cols: u16,
        tx: EventTx,
    ) -> Result<Self> {
        let pty_system = native_pty_system();
        let pair = pty_system
            .openpty(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .context("failed to open PTY")?;

        let mut cmd = CommandBuilder::new(program);
        cmd.args(args);
        cmd.cwd(cwd);
        // Advertise a capable-but-plain terminal; claude negotiates the rest.
        cmd.env("TERM", "xterm-256color");

        let child = pair
            .slave
            .spawn_command(cmd)
            .with_context(|| format!("failed to spawn `{program}`"))?;

        // The slave fd must be dropped in the parent so EOF propagates when the
        // child exits; `spawn_command` kept what it needs.
        drop(pair.slave);

        let mut reader = pair
            .master
            .try_clone_reader()
            .context("failed to clone PTY reader")?;
        let writer = pair
            .master
            .take_writer()
            .context("failed to take PTY writer")?;

        std::thread::spawn(move || {
            let mut buf = [0u8; 8192];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => break, // EOF: child closed the PTY
                    Ok(n) => {
                        if tx
                            .send(Event::PtyOutput(generation, buf[..n].to_vec()))
                            .is_err()
                        {
                            break; // main loop is gone
                        }
                    }
                    Err(_) => break,
                }
            }
            let _ = tx.send(Event::PtyExit(generation));
        });

        Ok(PtySession {
            master: pair.master,
            writer,
            child,
        })
    }

    /// Forward user input bytes to claude.
    pub fn write(&mut self, bytes: &[u8]) -> std::io::Result<()> {
        self.writer.write_all(bytes)?;
        self.writer.flush()
    }

    /// Resize the PTY (call whenever the terminal pane changes shape).
    pub fn resize(&self, rows: u16, cols: u16) -> Result<()> {
        self.master
            .resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .context("failed to resize PTY")
    }

    /// Terminate the child and reap it.
    pub fn kill(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for PtySession {
    fn drop(&mut self) {
        // Best-effort cleanup so we never leak a claude process.
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
