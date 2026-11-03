//! How swarmux starts and stops child processes.
//!
//! Every child is started without a console window. A child that has to be
//! cancellable is started through [`spawn_std`] or [`spawn_tokio`], which put
//! it and everything it starts into one [`ProcessTree`]: a Job Object on
//! Windows, a process group elsewhere. Killing (or dropping) the tree ends
//! all of them, even when the program is a `.cmd` wrapper that starts
//! `node` which starts the real work.

mod stagger;
#[cfg(unix)]
mod unix;
#[cfg(windows)]
mod windows;

use std::io;

pub use stagger::{Stagger, random_below};
#[cfg(unix)]
use unix as imp;
#[cfg(windows)]
use windows as imp;

/// `CREATE_NO_WINDOW`: no console window flashes for a child.
#[cfg(windows)]
pub(crate) const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// A child and its descendants. Dropping the tree kills what is left of it.
pub struct ProcessTree(imp::Tree);

impl ProcessTree {
    /// Ends the child and everything it started.
    pub fn kill(&self) {
        self.0.kill();
    }
}

/// Keeps a console window from appearing for `cmd`.
pub fn hide_std(cmd: &mut std::process::Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    #[cfg(not(windows))]
    let _ = cmd;
}

/// Starts `cmd` hidden, as the first process of a new [`ProcessTree`].
pub fn spawn_std(
    cmd: &mut std::process::Command,
) -> io::Result<(std::process::Child, ProcessTree)> {
    imp::prepare_std(cmd);
    let mut child = cmd.spawn()?;
    let tree = imp::adopt_std(&mut child)?;
    Ok((child, ProcessTree(tree)))
}

/// Starts `cmd` hidden, as the first process of a new [`ProcessTree`].
/// The tree is also killed when the returned guard is dropped, so a task
/// that is aborted or times out leaves nothing behind.
pub fn spawn_tokio(
    cmd: &mut tokio::process::Command,
) -> io::Result<(tokio::process::Child, ProcessTree)> {
    imp::prepare_tokio(cmd);
    let mut child = cmd.spawn()?;
    let tree = imp::adopt_tokio(&mut child)?;
    Ok((child, ProcessTree(tree)))
}

#[cfg(test)]
mod tests;
