//! Process trees as process groups.

use std::io;
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};

/// The group the child leads.
pub struct Tree {
    pgid: u32,
}

pub fn prepare_std(cmd: &mut std::process::Command) {
    cmd.process_group(0);
}

pub fn prepare_tokio(cmd: &mut tokio::process::Command) {
    cmd.process_group(0);
}

pub fn adopt_std(child: &mut std::process::Child) -> io::Result<Tree> {
    Ok(Tree { pgid: child.id() })
}

pub fn adopt_tokio(child: &mut tokio::process::Child) -> io::Result<Tree> {
    let pgid = child
        .id()
        .ok_or_else(|| io::Error::other("the child has already been reaped"))?;
    Ok(Tree { pgid })
}

impl Tree {
    pub fn kill(&self) {
        // Already gone is fine.
        let _ = Command::new("kill")
            .args(["-KILL", &format!("-{}", self.pgid)])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}

impl Drop for Tree {
    fn drop(&mut self) {
        self.kill();
    }
}
