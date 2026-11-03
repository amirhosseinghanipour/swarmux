//! `swarmux install`, `uninstall` and `self-update`: wiring only.

use std::path::PathBuf;

use anyhow::{Context, bail};
use swarmux_application::dist::{
    InstallOptions, InstallService, Report, UpdateOutcome, UpdateService,
};
use swarmux_domain::dist::{Target, Version};
use swarmux_infrastructure::Paths;
use swarmux_infrastructure::dist::{
    ArchiveUnpacker, DirReleases, EmbeddedKey, ExeReplacer, FsInstallHost, GithubReleases,
    default_bin_dir,
};
use swarmux_infrastructure::hook;

fn host(no_path: bool) -> anyhow::Result<FsInstallHost> {
    let home = Paths::default_home()?.home;
    let host = FsInstallHost::new(home);
    Ok(if no_path { host.without_path() } else { host })
}

fn print(report: &Report) {
    for line in &report.done {
        println!("{line}");
    }
    for line in &report.notes {
        println!("note: {line}");
    }
}

/// Puts this executable in a per-user folder, makes `~/.swarmux` and puts
/// the folder on the user's PATH. Never touches Claude Code's settings:
/// with `statusline` it only prints the setting to add.
pub fn install(dir: Option<PathBuf>, no_path: bool, statusline: bool) -> anyhow::Result<()> {
    let home = Paths::default_home()?.home;
    let dir = dir.unwrap_or_else(|| {
        let local = std::env::var_os("LOCALAPPDATA").map(PathBuf::from);
        default_bin_dir(&home, local.as_deref())
    });
    let exe = std::env::current_exe().context("cannot find the swarmux executable")?;
    let installed = dir.join(exe.file_name().context("the executable has no file name")?);

    let service = InstallService::new(host(no_path)?);
    let report = service.install(&InstallOptions {
        dir,
        edit_path: !no_path,
    })?;
    print(&report);
    if statusline {
        println!();
        println!("To show swarmux in Claude Code's status line, add this to your settings.json:");
        println!("{}", hook::statusline_settings(&installed));
    } else {
        println!(
            "Run `swarmux install --statusline` to print the Claude Code status line setting."
        );
    }
    Ok(())
}

/// Removes what `install` set up, and says what it removed.
pub fn uninstall(purge: bool, no_path: bool) -> anyhow::Result<()> {
    let report = InstallService::new(host(no_path)?).uninstall(purge)?;
    print(&report);
    if !report.done.is_empty() {
        println!(
            "If you added the status line to Claude Code's settings.json, remove the `statusLine` entry yourself."
        );
    }
    Ok(())
}

/// Updates this executable from the newest signed release.
pub fn self_update(check: bool, from: Option<PathBuf>) -> anyhow::Result<()> {
    let target =
        Target::from_parts(std::env::consts::OS, std::env::consts::ARCH).with_context(|| {
            format!(
                "no release is built for {} {}",
                std::env::consts::OS,
                std::env::consts::ARCH
            )
        })?;
    let current =
        Version::parse(env!("CARGO_PKG_VERSION")).context("this build has no usable version")?;
    let exe = std::env::current_exe().context("cannot find the swarmux executable")?;

    let github;
    let dir;
    let source: &dyn swarmux_application::dist::ReleaseSource = match from {
        Some(path) => {
            dir = DirReleases::new(path);
            &dir
        }
        None => {
            github = GithubReleases::new(&format!("swarmux/{current}"))?;
            &github
        }
    };
    let replacer = ExeReplacer::new(exe);
    let service = UpdateService {
        source,
        verifier: &EmbeddedKey,
        unpacker: &ArchiveUnpacker,
        replacer: &replacer,
        target,
    };
    match service.run(&current, check) {
        Ok(UpdateOutcome::UpToDate(v)) => println!("swarmux {v} is the newest release."),
        Ok(UpdateOutcome::Available(v)) => {
            println!("swarmux {v} is available (this is {current}); run `swarmux self-update`.");
        }
        Ok(UpdateOutcome::Updated { from, to }) => println!("updated swarmux {from} -> {to}."),
        Err(e) => bail!("update refused: {e}"),
    }
    Ok(())
}
