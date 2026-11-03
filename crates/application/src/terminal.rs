//! Opening a session in a terminal of its own.

use std::path::Path;

use crate::AppResult;

/// Opens `claude --resume <session>` in a new terminal tab, started in `cwd`.
pub trait SessionTerminal {
    fn open_resume(&self, cwd: &Path, session: &str) -> AppResult<()>;
}
