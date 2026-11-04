//! Process trees as process groups.

use std::io;
use std::os::unix::process::CommandExt;

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
        let Ok(pgid) = libc::pid_t::try_from(self.pgid) else {
            return;
        };
        // SAFETY: killpg only sends a signal; a group that is already gone
        // makes it fail with ESRCH, which is fine.
        unsafe {
            libc::killpg(pgid, libc::SIGKILL);
        }
    }
}

impl Drop for Tree {
    fn drop(&mut self) {
        self.kill();
    }
}
