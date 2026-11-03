//! Adapters for distribution: installing, signature checks, downloads,
//! unpacking and replacing the running executable.

mod archive;
mod host;
mod pathenv;
mod replace;
mod source;
mod verify;

pub use archive::ArchiveUnpacker;
pub use host::{FsInstallHost, default_bin_dir};
pub use replace::{ExeReplacer, replace_file};
pub use source::{DirReleases, GithubReleases, REPOSITORY};
pub use verify::{EmbeddedKey, MinisignVerifier};

#[cfg(test)]
mod update_tests;
