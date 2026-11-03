//! Distribution use cases: putting swarmux on a machine and taking it off
//! again, and updating it from a signed release.
//!
//! The ports here are plain traits; the adapters (files, the registry,
//! HTTP, minisign, archives) live in the infrastructure crate.

use std::path::{Path, PathBuf};

use swarmux_domain::dist::{Manifest, Target, Version};

#[derive(Debug, thiserror::Error)]
pub enum DistError {
    #[error("{0}")]
    Io(String),
    #[error("could not reach the release: {0}")]
    Network(String),
    #[error("the signature does not match: {0}")]
    BadSignature(String),
    #[error("this build has no release signing key, so it cannot check an update")]
    NoKey,
    #[error("the release archive is unusable: {0}")]
    Archive(String),
    #[error("{0}")]
    Unsupported(String),
}

pub type DistResult<T> = Result<T, DistError>;

// ----------------------------------------------------------------------
// install / uninstall

/// What the machine offers to install and remove things.
pub trait InstallHost {
    /// The running executable.
    fn current_exe(&self) -> DistResult<PathBuf>;
    /// swarmux's data folder (`~/.swarmux`).
    fn data_dir(&self) -> PathBuf;
    fn exists(&self, path: &Path) -> bool;
    fn create_dir_all(&self, path: &Path) -> DistResult<()>;
    /// Copies `from` to `to`, replacing a file there even when it is running.
    fn install_file(&self, from: &Path, to: &Path) -> DistResult<()>;
    /// Removes a file, even when it is the running executable. Returns a
    /// note when something had to be left for the next restart.
    fn remove_file(&self, path: &Path) -> DistResult<Option<String>>;
    /// Removes a folder only when it holds nothing. True when it is gone.
    fn remove_dir_if_empty(&self, path: &Path) -> DistResult<bool>;
    fn remove_dir_all(&self, path: &Path) -> DistResult<()>;
    fn path_has(&self, dir: &Path) -> bool;
    /// Adds a folder to the user's `PATH`. False when it cannot be done
    /// here (the caller says how to do it by hand) or was there already.
    fn path_add(&self, dir: &Path) -> DistResult<bool>;
    fn path_remove(&self, dir: &Path) -> DistResult<bool>;
    fn read_manifest(&self) -> DistResult<Option<Manifest>>;
    fn write_manifest(&self, manifest: &Manifest) -> DistResult<()>;
    fn remove_manifest(&self) -> DistResult<()>;
}

#[derive(Debug, Clone)]
pub struct InstallOptions {
    /// Where the executable goes.
    pub dir: PathBuf,
    /// Add `dir` to the user's `PATH` when it is not on it.
    pub edit_path: bool,
}

/// What an install or uninstall did, one line each, for the user.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Report {
    pub done: Vec<String>,
    pub notes: Vec<String>,
}

pub struct InstallService<H: InstallHost> {
    host: H,
}

impl<H: InstallHost> InstallService<H> {
    pub fn new(host: H) -> Self {
        Self { host }
    }

    pub fn install(&self, opts: &InstallOptions) -> DistResult<Report> {
        let mut report = Report::default();
        let exe = self.host.current_exe()?;
        let mut manifest = self.host.read_manifest()?.unwrap_or_default();

        // The outermost folder that does not exist yet, so that uninstall
        // can take back exactly the folders this made.
        if !self.host.exists(&opts.dir) && manifest.created_root.is_none() {
            manifest.created_root = opts
                .dir
                .ancestors()
                .take_while(|a| !a.as_os_str().is_empty() && !self.host.exists(a))
                .last()
                .map(Path::to_path_buf);
        }
        self.host.create_dir_all(&opts.dir)?;

        let name = exe
            .file_name()
            .ok_or_else(|| DistError::Io("the executable has no file name".into()))?;
        let target = opts.dir.join(name);
        if target != exe {
            self.host.install_file(&exe, &target)?;
            report.done.push(format!("installed {}", target.display()));
        } else {
            report
                .notes
                .push(format!("{} is already in place", target.display()));
        }
        manifest.binary = target;

        let data = self.host.data_dir();
        if !self.host.exists(&data) {
            self.host.create_dir_all(&data)?;
            manifest.data_dir_created = true;
            report.done.push(format!("created {}", data.display()));
        }

        if self.host.path_has(&opts.dir) {
            report
                .notes
                .push(format!("{} is already on your PATH", opts.dir.display()));
        } else if opts.edit_path && self.host.path_add(&opts.dir)? {
            manifest.path_entry = Some(opts.dir.display().to_string());
            report
                .done
                .push(format!("added {} to your PATH", opts.dir.display()));
            report
                .notes
                .push("open a new terminal for the PATH change to apply".into());
        } else {
            report.notes.push(format!(
                "add {} to your PATH to run `swarmux` from anywhere",
                opts.dir.display()
            ));
        }
        self.host.write_manifest(&manifest)?;
        Ok(report)
    }

    /// Removes what [`Self::install`] set up. The data folder goes only
    /// with `purge`, or when the install made it and nothing is in it.
    pub fn uninstall(&self, purge: bool) -> DistResult<Report> {
        let mut report = Report::default();
        let Some(manifest) = self.host.read_manifest()? else {
            report
                .notes
                .push("nothing was installed by `swarmux install`".into());
            return Ok(report);
        };

        if self.host.exists(&manifest.binary) {
            let note = self.host.remove_file(&manifest.binary)?;
            report
                .done
                .push(format!("removed {}", manifest.binary.display()));
            report.notes.extend(note);
        }
        if let Some(entry) = &manifest.path_entry
            && self.host.path_remove(Path::new(entry))?
        {
            report.done.push(format!("removed {entry} from your PATH"));
        }
        if let Some(root) = &manifest.created_root {
            // Empty folders from the binary's own folder up to the first one
            // the install created.
            let mut dir = manifest.binary.parent();
            while let Some(d) = dir {
                if !d.starts_with(root) {
                    break;
                }
                if self.host.exists(d) && self.host.remove_dir_if_empty(d)? {
                    report.done.push(format!("removed {}", d.display()));
                }
                if d == root {
                    break;
                }
                dir = d.parent();
            }
        }
        self.host.remove_manifest()?;

        let data = self.host.data_dir();
        if self.host.exists(&data) {
            if purge {
                self.host.remove_dir_all(&data)?;
                report.done.push(format!("removed {}", data.display()));
            } else if manifest.data_dir_created && self.host.remove_dir_if_empty(&data)? {
                report.done.push(format!("removed {}", data.display()));
            } else {
                report.notes.push(format!(
                    "kept {} (your data); `swarmux uninstall --purge` deletes it",
                    data.display()
                ));
            }
        }
        Ok(report)
    }
}

// ----------------------------------------------------------------------
// self-update

/// Where releases come from.
pub trait ReleaseSource {
    /// The newest release.
    fn latest(&self) -> DistResult<Version>;
    /// One file of that release.
    fn fetch(&self, version: &Version, asset: &str) -> DistResult<Vec<u8>>;
}

/// Checks a detached signature against the key built into the binary.
pub trait SignatureVerifier {
    fn verify(&self, data: &[u8], signature: &str) -> DistResult<()>;
}

/// Takes the executable out of a release archive.
pub trait Unpacker {
    fn executable(&self, archive: &[u8], target: Target) -> DistResult<Vec<u8>>;
}

/// Swaps the running executable for a new one.
pub trait BinaryReplacer {
    fn replace(&self, new_binary: &[u8]) -> DistResult<()>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateOutcome {
    UpToDate(Version),
    /// A newer release exists (check only).
    Available(Version),
    Updated {
        from: Version,
        to: Version,
    },
}

pub struct UpdateService<'a> {
    pub source: &'a dyn ReleaseSource,
    pub verifier: &'a dyn SignatureVerifier,
    pub unpacker: &'a dyn Unpacker,
    pub replacer: &'a dyn BinaryReplacer,
    pub target: Target,
}

impl UpdateService<'_> {
    /// Looks for a newer release; with `check_only` stops there. Otherwise
    /// downloads it, verifies the signature of the archive before anything
    /// is unpacked, and only then replaces the running executable.
    pub fn run(&self, current: &Version, check_only: bool) -> DistResult<UpdateOutcome> {
        let latest = self.source.latest()?;
        if latest <= *current {
            return Ok(UpdateOutcome::UpToDate(*current));
        }
        if check_only {
            return Ok(UpdateOutcome::Available(latest));
        }
        let archive = self.source.fetch(&latest, &self.target.asset_name())?;
        let sig = self.source.fetch(&latest, &self.target.signature_name())?;
        let sig = String::from_utf8(sig)
            .map_err(|_| DistError::BadSignature("the signature file is not text".into()))?;
        self.verifier.verify(&archive, &sig)?;
        let binary = self.unpacker.executable(&archive, self.target)?;
        self.replacer.replace(&binary)?;
        Ok(UpdateOutcome::Updated {
            from: *current,
            to: latest,
        })
    }
}
