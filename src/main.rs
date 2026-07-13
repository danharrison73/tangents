mod app;
mod cli;
mod config;
mod event;
mod keys;
mod pty;
mod session;
mod state;
mod tree;
mod watcher;

use anyhow::Result;
use cli::Config;

fn main() -> Result<()> {
    let cfg = Config::from_args(std::env::args().skip(1))?;

    if cfg.print_argv {
        println!("claude binary : {}", cfg.claude_bin.display());
        println!("project dir   : {}", cfg.project_dir.display());
        println!("tangents dir  : {}", cfg.tangents_dir.display());
        println!("no_tree       : {}", cfg.no_tree);
        println!("spawn argv    : {:?}", cfg.spawn_argv());
        return Ok(());
    }

    app::run(cfg)
}
