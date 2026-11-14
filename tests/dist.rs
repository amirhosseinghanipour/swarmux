//! `swarmux install`, `uninstall` and `self-update` as a user runs them:
//! the real binary, with a temporary home so nothing outside it is touched.

use std::path::Path;
use std::process::{Command, Output};

use swarmux_domain::dist::Target;

const EXE: &str = env!("CARGO_BIN_EXE_swarmux");

fn swarmux(exe: &Path, home: &Path, args: &[&str]) -> Output {
    Command::new(exe)
        .args(args)
        .env("HOME", home)
        .env("USERPROFILE", home)
        .env("LOCALAPPDATA", home.join("local"))
        .env_remove("XDG_STATE_HOME")
        .env_remove("XDG_CONFIG_HOME")
        .env_remove("SWARMUX_STATE_DIR")
        .env_remove("SWARMUX_CONFIG_DIR")
        .output()
        .expect("run swarmux")
}

/// Where a home with no `~/.swarmux` keeps its data: the platform's state folder.
fn state_dir(home: &Path) -> std::path::PathBuf {
    if cfg!(windows) {
        home.join("local").join("swarmux")
    } else if cfg!(target_os = "macos") {
        home.join("Library")
            .join("Application Support")
            .join("swarmux")
            .join("state")
    } else {
        home.join(".local").join("state").join("swarmux")
    }
}

fn text(out: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

fn entries(dir: &Path) -> Vec<String> {
    std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect()
}

#[test]
fn clean_install_then_uninstall_leaves_no_trace() {
    let home = tempfile::tempdir().unwrap();
    let bin = home.path().join("tools").join("bin");
    let bin_arg = bin.to_str().unwrap();

    let out = swarmux(
        Path::new(EXE),
        home.path(),
        &["install", "--dir", bin_arg, "--no-path", "--statusline"],
    );
    assert!(out.status.success(), "{}", text(&out));
    let said = text(&out);
    assert!(said.contains("installed"), "{said}");
    assert!(said.contains("statusLine"), "{said}");
    let installed = bin.join(Path::new(EXE).file_name().unwrap());
    assert!(installed.is_file());
    assert!(state_dir(home.path()).is_dir());
    // Claude Code's settings are never touched.
    assert!(!home.path().join(".claude").exists());

    let out = swarmux(Path::new(EXE), home.path(), &["uninstall", "--no-path"]);
    assert!(out.status.success(), "{}", text(&out));
    let said = text(&out);
    assert!(said.contains("removed"), "{said}");
    assert!(said.contains(installed.to_str().unwrap()), "{said}");
    assert_eq!(entries(home.path()), Vec::<String>::new(), "{said}");
}

#[test]
fn uninstall_keeps_data_unless_asked() {
    let home = tempfile::tempdir().unwrap();
    let bin = home.path().join("bin");
    let bin_arg = bin.to_str().unwrap();
    swarmux(
        Path::new(EXE),
        home.path(),
        &["install", "--dir", bin_arg, "--no-path"],
    );
    std::fs::write(state_dir(home.path()).join("keep.txt"), "x = 1").unwrap();

    let out = swarmux(Path::new(EXE), home.path(), &["uninstall", "--no-path"]);
    assert!(out.status.success(), "{}", text(&out));
    assert!(text(&out).contains("kept"));
    assert!(state_dir(home.path()).join("keep.txt").is_file());
    assert!(!bin.exists());

    swarmux(
        Path::new(EXE),
        home.path(),
        &["install", "--dir", bin_arg, "--no-path"],
    );
    let out = swarmux(
        Path::new(EXE),
        home.path(),
        &["uninstall", "--purge", "--no-path"],
    );
    assert!(out.status.success(), "{}", text(&out));
    assert_eq!(entries(home.path()), Vec::<String>::new());
}

#[test]
fn self_update_refuses_a_bad_signature_and_keeps_the_binary() {
    let home = tempfile::tempdir().unwrap();
    let copy = home.path().join(Path::new(EXE).file_name().unwrap());
    std::fs::copy(EXE, &copy).unwrap();
    let before = std::fs::read(&copy).unwrap();

    let target = Target::from_parts(std::env::consts::OS, std::env::consts::ARCH)
        .expect("a supported test platform");
    let release = home.path().join("release");
    std::fs::create_dir(&release).unwrap();
    std::fs::write(release.join("latest.txt"), "v99.0.0").unwrap();
    std::fs::write(release.join(target.asset_name()), b"not really an archive").unwrap();
    std::fs::write(
        release.join(target.signature_name()),
        "untrusted comment: forged\nRWQAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA==\ntrusted comment: forged\nAAAA\n",
    )
    .unwrap();

    let from = release.to_str().unwrap();
    let out = swarmux(&copy, home.path(), &["self-update", "--from", from]);
    assert!(!out.status.success(), "{}", text(&out));
    assert!(text(&out).contains("update refused"), "{}", text(&out));
    assert_eq!(std::fs::read(&copy).unwrap(), before);
    assert!(
        !entries(home.path())
            .iter()
            .any(|n| n.ends_with(".old") || n.ends_with(".new")),
        "{:?}",
        entries(home.path())
    );
}

#[test]
fn self_update_check_reports_a_newer_release_without_touching_anything() {
    let home = tempfile::tempdir().unwrap();
    let release = home.path().join("release");
    std::fs::create_dir(&release).unwrap();
    std::fs::write(release.join("latest.txt"), "v99.0.0").unwrap();
    let from = release.to_str().unwrap();

    let out = swarmux(
        Path::new(EXE),
        home.path(),
        &["self-update", "--check", "--from", from],
    );
    assert!(out.status.success(), "{}", text(&out));
    assert!(text(&out).contains("99.0.0 is available"), "{}", text(&out));

    std::fs::write(release.join("latest.txt"), "v0.0.1").unwrap();
    let out = swarmux(
        Path::new(EXE),
        home.path(),
        &["self-update", "--check", "--from", from],
    );
    assert!(text(&out).contains("newest"), "{}", text(&out));
}
