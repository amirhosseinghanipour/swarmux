//! Process trees as Job Objects.
//!
//! A child is created suspended, put into a job whose handle closing kills
//! every process in it, and only then resumed. It cannot start a
//! grandchild before it is in the job, so none escapes.

use std::io;
use std::mem::size_of;
use std::os::windows::io::AsRawHandle;
use std::os::windows::process::CommandExt;
use std::process::{Command, Stdio};

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First, Thread32Next,
};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
    SetInformationJobObject, TerminateJobObject,
};
use windows_sys::Win32::System::Threading::{
    CREATE_SUSPENDED, OpenThread, ResumeThread, THREAD_SUSPEND_RESUME,
};

use super::CREATE_NO_WINDOW;

/// The job holding the child, when one could be made.
pub struct Tree {
    job: Option<HANDLE>,
    pid: u32,
}

// A job handle may be used from any thread.
unsafe impl Send for Tree {}
unsafe impl Sync for Tree {}

pub fn prepare_std(cmd: &mut std::process::Command) {
    cmd.creation_flags(CREATE_NO_WINDOW | CREATE_SUSPENDED);
}

pub fn prepare_tokio(cmd: &mut tokio::process::Command) {
    cmd.creation_flags(CREATE_NO_WINDOW | CREATE_SUSPENDED);
}

pub fn adopt_std(child: &mut std::process::Child) -> io::Result<Tree> {
    adopt(child.id(), child.as_raw_handle() as HANDLE, || child.kill())
}

pub fn adopt_tokio(child: &mut tokio::process::Child) -> io::Result<Tree> {
    let pid = child
        .id()
        .ok_or_else(|| io::Error::other("the child has already been reaped"))?;
    let handle = child
        .raw_handle()
        .ok_or_else(|| io::Error::other("the child has already been reaped"))?
        as HANDLE;
    adopt(pid, handle, || child.start_kill())
}

/// Puts the suspended process into a new job, then lets it run. If it can't
/// be resumed it is killed, so no suspended process is left behind.
fn adopt(pid: u32, process: HANDLE, kill: impl FnOnce() -> io::Result<()>) -> io::Result<Tree> {
    let job = new_job().filter(|job| {
        // SAFETY: both handles are valid; the process handle is owned by the child.
        let assigned = unsafe { AssignProcessToJobObject(*job, process) } != 0;
        if !assigned {
            // SAFETY: the job handle was just created and is not used again.
            unsafe { CloseHandle(*job) };
        }
        assigned
    });
    let tree = Tree { job, pid };
    if let Err(e) = resume(pid) {
        let _ = kill();
        return Err(e);
    }
    Ok(tree)
}

/// A job that kills its processes when its last handle closes.
fn new_job() -> Option<HANDLE> {
    // SAFETY: plain Win32 calls with valid arguments; the handle is closed on
    // every failing path.
    unsafe {
        let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
        if job.is_null() {
            return None;
        }
        let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        let set = SetInformationJobObject(
            job,
            JobObjectExtendedLimitInformation,
            std::ptr::from_ref(&info).cast(),
            size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        );
        if set == 0 {
            CloseHandle(job);
            return None;
        }
        Some(job)
    }
}

/// Resumes the single thread of a process created suspended.
fn resume(pid: u32) -> io::Result<()> {
    // SAFETY: the snapshot and thread handles are closed before returning;
    // the entry is initialised with its size as the API requires.
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0);
        if snapshot == INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error());
        }
        let mut entry: THREADENTRY32 = std::mem::zeroed();
        entry.dwSize = size_of::<THREADENTRY32>() as u32;
        let mut resumed = false;
        let mut more = Thread32First(snapshot, &mut entry) != 0;
        while more {
            if entry.th32OwnerProcessID == pid {
                let thread = OpenThread(THREAD_SUSPEND_RESUME, 0, entry.th32ThreadID);
                if !thread.is_null() {
                    resumed |= ResumeThread(thread) != u32::MAX;
                    CloseHandle(thread);
                }
            }
            more = Thread32Next(snapshot, &mut entry) != 0;
        }
        CloseHandle(snapshot);
        if resumed {
            Ok(())
        } else {
            Err(io::Error::other("could not resume the new process"))
        }
    }
}

impl Tree {
    pub fn kill(&self) {
        match self.job {
            // SAFETY: the handle stays open as long as the tree lives.
            Some(job) => unsafe {
                TerminateJobObject(job, 1);
            },
            None => taskkill(self.pid),
        }
    }
}

/// The fallback when no job could be made: walk the tree by pid.
fn taskkill(pid: u32) {
    let _ = Command::new("taskkill")
        .args(["/T", "/F", "/PID", &pid.to_string()])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .status();
}

impl Drop for Tree {
    fn drop(&mut self) {
        if let Some(job) = self.job {
            // SAFETY: closing the last handle kills the processes left in the job.
            unsafe { CloseHandle(job) };
        }
    }
}
