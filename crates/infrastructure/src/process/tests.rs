use std::collections::HashSet;
use std::path::Path;
use std::process::Stdio;
use std::time::{Duration, Instant};

use sysinfo::{Pid, ProcessesToUpdate, System};

use super::*;

/// Every live process below `root`, by pid.
fn descendants(root: u32) -> HashSet<u32> {
    let mut sys = System::new().unwrap();
    sys.refresh_processes(ProcessesToUpdate::All, true);
    let mut found = HashSet::new();
    loop {
        let before = found.len();
        for (pid, p) in sys.processes() {
            let parent = p.parent().map(Pid::as_u32);
            if parent == Some(root) || parent.is_some_and(|q| found.contains(&q)) {
                found.insert(pid.as_u32());
            }
        }
        if found.len() == before {
            return found;
        }
    }
}

/// Still running. A zombie has exited and only waits for its parent to
/// reap it, so it doesn't count.
fn alive(pid: u32) -> bool {
    let mut sys = System::new().unwrap();
    sys.refresh_processes(ProcessesToUpdate::Some(&[Pid::from_u32(pid)]), true);
    sys.process(Pid::from_u32(pid))
        .is_some_and(|p| p.status() != sysinfo::ProcessStatus::Zombie)
}

/// Waits until `f` holds, for at most 30 s.
fn eventually(f: impl Fn() -> bool) -> bool {
    let deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline {
        if f() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    f()
}

/// A program that starts two long-lived grandchildren and then waits.
fn parent_with_grandchildren(dir: &Path) -> std::process::Command {
    if cfg!(windows) {
        let script = dir.join("claude.cmd");
        std::fs::write(
            &script,
            "@echo off\r\nstart /b ping -n 120 127.0.0.1 >nul\r\nstart /b ping -n 120 127.0.0.2 >nul\r\nping -n 120 127.0.0.3 >nul\r\n",
        )
        .unwrap();
        std::process::Command::new(script)
    } else {
        let mut cmd = std::process::Command::new("sh");
        cmd.args(["-c", "sleep 120 & sleep 120 & sleep 120; wait"]);
        cmd
    }
}

/// Waits until the child has started its grandchildren.
fn grandchildren_of(pid: u32) -> HashSet<u32> {
    assert!(
        eventually(|| descendants(pid).len() >= 3),
        "the grandchildren did not start: {:?}",
        descendants(pid)
    );
    descendants(pid)
}

#[test]
fn killing_the_tree_ends_the_child_and_every_descendant() {
    let dir = tempfile::tempdir().unwrap();
    let mut cmd = parent_with_grandchildren(dir.path());
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let (mut child, tree) = spawn_std(&mut cmd).unwrap();
    let pid = child.id();
    let mut all = grandchildren_of(pid);
    all.insert(pid);

    tree.kill();
    let _ = child.wait();
    assert!(
        eventually(|| all.iter().all(|p| !alive(*p))),
        "left running: {:?}",
        all.iter().filter(|p| alive(**p)).collect::<Vec<_>>()
    );
}

#[test]
fn dropping_the_tree_ends_the_descendants_too() {
    let dir = tempfile::tempdir().unwrap();
    let mut cmd = parent_with_grandchildren(dir.path());
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let (child, tree) = spawn_std(&mut cmd).unwrap();
    let pid = child.id();
    let mut all = grandchildren_of(pid);
    all.insert(pid);

    drop(tree);
    assert!(
        eventually(|| all.iter().all(|p| !alive(*p))),
        "left running: {:?}",
        all.iter().filter(|p| alive(**p)).collect::<Vec<_>>()
    );
    drop(child);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn aborting_a_tokio_task_leaves_no_process_behind() {
    let dir = tempfile::tempdir().unwrap();
    let mut std_cmd = parent_with_grandchildren(dir.path());
    let program = std_cmd.get_program().to_owned();
    let args: Vec<_> = std_cmd.get_args().map(|a| a.to_owned()).collect();
    std_cmd.stdin(Stdio::null());
    let (pid_tx, pid_rx) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(async move {
        let mut cmd = tokio::process::Command::new(program);
        cmd.args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        let (mut child, _tree) = spawn_tokio(&mut cmd).unwrap();
        let _ = pid_tx.send(child.id().unwrap());
        let _ = child.wait().await;
    });
    let pid = pid_rx.await.unwrap();
    let mut all = tokio::task::spawn_blocking(move || {
        let mut all = grandchildren_of(pid);
        all.insert(pid);
        all
    })
    .await
    .unwrap();
    task.abort();
    let _ = task.await;
    all.retain(|p| *p != 0);
    let left = tokio::task::spawn_blocking(move || eventually(|| all.iter().all(|p| !alive(*p))))
        .await
        .unwrap();
    assert!(left, "processes survived the abort");
}

#[test]
fn a_hidden_child_still_runs_and_reports() {
    let mut cmd = if cfg!(windows) {
        let mut c = std::process::Command::new("cmd");
        c.args(["/C", "echo hi"]);
        c
    } else {
        let mut c = std::process::Command::new("echo");
        c.arg("hi");
        c
    };
    cmd.stdout(Stdio::piped());
    hide_std(&mut cmd);
    let out = cmd.output().unwrap();
    assert!(String::from_utf8_lossy(&out.stdout).contains("hi"));
    let (child, _tree) = spawn_std(&mut cmd).unwrap();
    assert!(child.wait_with_output().unwrap().status.success());
}
