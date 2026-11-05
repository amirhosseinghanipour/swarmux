//! Starting the background service so that it outlives the terminal that
//! started it.
//!
//! On Windows it gets no console (`DETACHED_PROCESS`), a process group of
//! its own (`CREATE_NEW_PROCESS_GROUP`, so the terminal's Ctrl+C and close
//! never reach it) and leaves any Job Object the terminal put this process
//! in (`CREATE_BREAKAWAY_FROM_JOB`, when the job allows it), so closing the
//! terminal kills neither it nor the agents it runs: their Job Objects
//! belong to the service. Elsewhere it starts a session of its own
//! (`setsid`), which leaves the terminal and its hang-up behind. Its
//! output goes to the log file in either case.

use std::fs::OpenOptions;
use std::io;
use std::path::Path;
use std::process::{Command, Stdio};

/// Starts `exe args…` detached in the folder `cwd` (so it never holds a
/// project folder open), with its output appended to `log`. Returns its
/// process id.
pub fn spawn_detached(exe: &Path, args: &[&str], cwd: &Path, log: &Path) -> io::Result<u32> {
    let out = OpenOptions::new().create(true).append(true).open(log)?;
    let err = out.try_clone()?;
    let mut cmd = Command::new(exe);
    cmd.args(args)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::from(out))
        .stderr(Stdio::from(err));
    imp::spawn(cmd)
}

#[cfg(windows)]
mod imp {
    use super::*;
    use std::os::windows::process::CommandExt;

    const DETACHED_PROCESS: u32 = 0x0000_0008;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;
    const ERROR_ACCESS_DENIED: i32 = 5;

    /// This process's own standard handles stop being inheritable. They
    /// may be pipes of whoever started it (a shell capturing the output of
    /// `swarmux daemon start`); a service holding them would keep that
    /// reader waiting for as long as it runs.
    fn keep_own_stdio() {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Foundation::{HANDLE_FLAG_INHERIT, SetHandleInformation};
        let handles = [
            io::stdin().as_raw_handle(),
            io::stdout().as_raw_handle(),
            io::stderr().as_raw_handle(),
        ];
        for h in handles {
            if !h.is_null() {
                // SAFETY: a handle of this process; only its flag changes.
                unsafe { SetHandleInformation(h.cast(), HANDLE_FLAG_INHERIT, 0) };
            }
        }
    }

    pub fn spawn(mut cmd: Command) -> io::Result<u32> {
        keep_own_stdio();
        let flags = DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP | crate::process::CREATE_NO_WINDOW;
        cmd.creation_flags(flags | CREATE_BREAKAWAY_FROM_JOB);
        match cmd.spawn() {
            Ok(child) => Ok(child.id()),
            // The job this process is in forbids breaking away: start
            // inside it rather than not at all.
            Err(e) if e.raw_os_error() == Some(ERROR_ACCESS_DENIED) => {
                cmd.creation_flags(flags);
                cmd.spawn().map(|c| c.id())
            }
            Err(e) => Err(e),
        }
    }
}

#[cfg(not(windows))]
mod imp {
    use super::*;
    use std::os::unix::process::CommandExt;

    pub fn spawn(mut cmd: Command) -> io::Result<u32> {
        // SAFETY: setsid is async-signal-safe and touches nothing of the
        // parent; it runs in the child between fork and exec.
        unsafe {
            cmd.pre_exec(|| {
                if libc::setsid() == -1 {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let mut child = cmd.spawn()?;
        let pid = child.id();
        // Reaped when it ends, so it never lingers as a zombie of this
        // process.
        std::thread::spawn(move || {
            let _ = child.wait();
        });
        Ok(pid)
    }
}
