//! `SessionTerminal` adapter: a new Windows Terminal tab in the current window.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use swarmux_application::{AppError, AppResult, SessionTerminal};

use crate::process::hide_std;

/// Opens sessions with `wt -w 0 nt -d <cwd> <claude> --resume <id>`.
pub struct WindowsTerminal {
    wt: PathBuf,
    claude: PathBuf,
}

impl WindowsTerminal {
    pub fn new(claude: PathBuf) -> WindowsTerminal {
        WindowsTerminal {
            wt: "wt".into(),
            claude,
        }
    }

    /// The arguments after `wt`. Every one is its own argument: no shell
    /// ever parses the working folder or the session id.
    pub fn args(cwd: &Path, claude: &Path, session: &str) -> Vec<OsString> {
        let mut args: Vec<OsString> = ["-w", "0", "nt", "-d"].map(OsString::from).into();
        args.push(cwd.as_os_str().to_owned());
        args.push(claude.as_os_str().to_owned());
        args.push("--resume".into());
        args.push(session.into());
        args
    }
}

impl SessionTerminal for WindowsTerminal {
    fn open_resume(&self, cwd: &Path, session: &str) -> AppResult<()> {
        let mut cmd = Command::new(&self.wt);
        cmd.args(WindowsTerminal::args(cwd, &self.claude, session))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        hide_std(&mut cmd);
        cmd.spawn()
            .map(|_| ())
            .map_err(|e| AppError::Workspace(format!("wt: {e} (is Windows Terminal installed?)")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arguments_stay_separate() {
        let args = WindowsTerminal::args(
            Path::new(r"C:\my work\app; calc"),
            Path::new("claude"),
            "abc-123",
        );
        let args: Vec<_> = args
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            args,
            [
                "-w",
                "0",
                "nt",
                "-d",
                r"C:\my work\app; calc",
                "claude",
                "--resume",
                "abc-123"
            ]
        );
    }
}
