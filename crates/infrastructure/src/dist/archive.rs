//! Taking the executable out of a release archive.

use std::io::{Cursor, Read};
use std::path::Path;

use swarmux_application::dist::{DistError, DistResult, Unpacker};
use swarmux_domain::dist::Target;

/// No release executable is anywhere near this large.
const MAX_BINARY: u64 = 256 * 1024 * 1024;

pub struct ArchiveUnpacker;

impl Unpacker for ArchiveUnpacker {
    fn executable(&self, archive: &[u8], target: Target) -> DistResult<Vec<u8>> {
        let name = target.exe_name();
        let found = if target.archive_ext() == "zip" {
            from_zip(archive, name)?
        } else {
            from_tar_gz(archive, name)?
        };
        found.ok_or_else(|| DistError::Archive(format!("no {name} inside")))
    }
}

fn is_named(path: &Path, name: &str) -> bool {
    path.file_name().is_some_and(|n| n == name)
}

fn read_capped(mut from: impl Read) -> DistResult<Vec<u8>> {
    let mut out = Vec::new();
    from.by_ref()
        .take(MAX_BINARY + 1)
        .read_to_end(&mut out)
        .map_err(|e| DistError::Archive(e.to_string()))?;
    if out.len() as u64 > MAX_BINARY {
        return Err(DistError::Archive("the executable is too large".into()));
    }
    Ok(out)
}

fn from_zip(archive: &[u8], name: &str) -> DistResult<Option<Vec<u8>>> {
    let mut zip = zip::ZipArchive::new(Cursor::new(archive))
        .map_err(|e| DistError::Archive(e.to_string()))?;
    for i in 0..zip.len() {
        let file = zip
            .by_index(i)
            .map_err(|e| DistError::Archive(e.to_string()))?;
        let entry = file.name().map(|n| n.into_owned()).unwrap_or_default();
        if file.is_file() && is_named(Path::new(&entry.replace('\\', "/")), name) {
            return read_capped(file).map(Some);
        }
    }
    Ok(None)
}

fn from_tar_gz(archive: &[u8], name: &str) -> DistResult<Option<Vec<u8>>> {
    let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(Cursor::new(archive)));
    let entries = tar
        .entries()
        .map_err(|e| DistError::Archive(e.to_string()))?;
    for entry in entries {
        let entry = entry.map_err(|e| DistError::Archive(e.to_string()))?;
        let path = entry
            .path()
            .map_err(|e| DistError::Archive(e.to_string()))?
            .into_owned();
        if entry.header().entry_type().is_file() && is_named(&path, name) {
            return read_capped(entry).map(Some);
        }
    }
    Ok(None)
}

/// Builds a release archive holding one executable, as the release
/// workflow does, for tests.
#[cfg(test)]
pub(crate) fn make_archive(target: Target, exe: &[u8]) -> Vec<u8> {
    use std::io::Write;
    let inner = format!("swarmux-{}/{}", target.triple(), target.exe_name());
    if target.archive_ext() == "zip" {
        let mut out = Cursor::new(Vec::new());
        {
            let mut zip = zip::ZipWriter::new(&mut out);
            let opts = zip::write::SimpleFileOptions::default();
            zip.start_file(inner, opts).unwrap();
            zip.write_all(exe).unwrap();
            zip.finish().unwrap();
        }
        out.into_inner()
    } else {
        let gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        let mut tar = tar::Builder::new(gz);
        let mut header = tar::Header::new_gnu();
        header.set_size(exe.len() as u64);
        header.set_mode(0o755);
        header.set_cksum();
        tar.append_data(&mut header, inner, exe).unwrap();
        tar.into_inner().unwrap().finish().unwrap()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_executable_in_every_archive_kind() {
        for target in Target::ALL {
            let archive = make_archive(target, b"binary-bytes");
            let got = ArchiveUnpacker.executable(&archive, target).unwrap();
            assert_eq!(got, b"binary-bytes", "{target:?}");
        }
    }

    #[test]
    fn rejects_garbage_and_archives_without_it() {
        assert!(
            ArchiveUnpacker
                .executable(b"junk", Target::LinuxX64)
                .is_err()
        );
        let archive = make_archive(Target::WindowsX64, b"x");
        // A Windows archive has no `swarmux`.
        assert!(from_zip(&archive, "swarmux").unwrap().is_none());
    }
}
