//! `swarmux doctor`: checks Claude Code and swarmux's own data.

use std::path::{Path, PathBuf};
use std::process::Command;

use swarmux_infrastructure::{Paths, SqliteStore};

/// The `claude` executable: `SWARMUX_CLAUDE_BIN` or `claude` on PATH.
pub fn claude_bin() -> PathBuf {
    std::env::var_os("SWARMUX_CLAUDE_BIN")
        .map(Into::into)
        .unwrap_or_else(|| "claude".into())
}

pub fn run() -> anyhow::Result<()> {
    let bin = claude_bin();
    match claude_version(&bin) {
        Some(v) => println!("ok   claude {v}"),
        None => println!(
            "fail claude not found ({}). Install Claude Code or set SWARMUX_CLAUDE_BIN.",
            bin.display()
        ),
    }

    let paths = Paths::default_home()?;
    println!("ok   data dir {}", paths.data_dir.display());
    std::fs::create_dir_all(&paths.data_dir)?;
    match SqliteStore::open(&paths.db).and_then(|s| s.schema_version()) {
        Ok(v) => println!("ok   database {} (schema v{v})", paths.db.display()),
        Err(e) => println!("fail database: {e}"),
    }
    Ok(())
}

fn claude_version(bin: &Path) -> Option<String> {
    let output = Command::new(bin).arg("--version").output().ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
}
