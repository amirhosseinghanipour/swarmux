//! `Workspace` adapter backed by the `git` CLI.

use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use swarmux_application::ports::Workspace;
use swarmux_application::{AppError, AppResult};

/// Largest diff handed to a prompt, in bytes.
pub const DIFF_CAP: usize = 64 * 1024;

pub struct GitCli {
    bin: PathBuf,
}

impl GitCli {
    pub fn new(bin: PathBuf) -> GitCli {
        GitCli { bin }
    }
}

impl Default for GitCli {
    fn default() -> GitCli {
        GitCli::new("git".into())
    }
}

impl Workspace for GitCli {
    fn git_diff(&self, cwd: &Path) -> AppResult<String> {
        let mut cmd = Command::new(&self.bin);
        cmd.arg("-C")
            .arg(cwd)
            .args(["diff", "--no-color", "--no-ext-diff"])
            .stdin(Stdio::null())
            .stderr(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        }
        let output = match cmd.output() {
            Ok(output) => output,
            // No git installed: same as no repository.
            Err(e) if e.kind() == ErrorKind::NotFound => return Ok(String::new()),
            Err(e) => return Err(AppError::Workspace(format!("git: {e}"))),
        };
        if !output.status.success() {
            return Ok(String::new());
        }
        Ok(cap(String::from_utf8_lossy(&output.stdout).into_owned()))
    }
}

fn cap(mut text: String) -> String {
    if text.len() > DIFF_CAP {
        let mut end = DIFF_CAP;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    fn git(dir: &Path, args: &[&str]) -> bool {
        Command::new("git")
            .arg("-C")
            .arg(dir)
            .args([
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "-c",
                "commit.gpgsign=false",
            ])
            .args(args)
            .output()
            .is_ok_and(|o| o.status.success())
    }

    #[test]
    fn diff_of_a_modified_repo() {
        let dir = tempfile::tempdir().unwrap();
        if !git(dir.path(), &["init", "-q"]) {
            eprintln!("git not available; skipping");
            return;
        }
        let file = dir.path().join("a.txt");
        std::fs::write(&file, "one\n").unwrap();
        assert!(git(dir.path(), &["add", "a.txt"]));
        assert!(git(dir.path(), &["commit", "-q", "-m", "init"]));
        assert_eq!(GitCli::default().git_diff(dir.path()).unwrap(), "");

        std::fs::write(&file, "one\ntwo é\n").unwrap();
        let diff = GitCli::default().git_diff(dir.path()).unwrap();
        assert!(diff.contains("+two é"), "{diff}");
    }

    #[test]
    fn not_a_repo_or_no_git_gives_empty() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(GitCli::default().git_diff(dir.path()).unwrap(), "");
        let missing = GitCli::new(dir.path().join("no-such-git"));
        assert_eq!(missing.git_diff(dir.path()).unwrap(), "");
    }

    #[test]
    fn cap_cuts_on_a_char_boundary() {
        let text = "é".repeat(DIFF_CAP);
        let capped = cap(text);
        assert!(capped.len() <= DIFF_CAP);
        assert!(capped.chars().all(|c| c == 'é'));
        assert_eq!(cap("short".into()), "short");
    }
}
