//! Self-update end to end against a folder laid out like a release.

use std::io::Cursor;
use std::path::Path;

use swarmux_application::dist::{DistError, UpdateOutcome, UpdateService};
use swarmux_domain::dist::{Target, Version};

use super::archive::make_archive;
use super::{ArchiveUnpacker, DirReleases, ExeReplacer, MinisignVerifier};

const TARGET: Target = Target::LinuxX64;

fn sign(pair: &minisign::KeyPair, data: &[u8]) -> String {
    minisign::sign(Some(&pair.pk), &pair.sk, Cursor::new(data), None, None)
        .unwrap()
        .to_string()
}

fn key_text(pair: &minisign::KeyPair) -> String {
    pair.pk.to_box().unwrap().to_string()
}

/// A release folder holding version `tag`, its archive of `exe` and a
/// signature made by `signer`.
fn release(dir: &Path, tag: &str, exe: &[u8], signer: &minisign::KeyPair) {
    let archive = make_archive(TARGET, exe);
    std::fs::write(dir.join("latest.txt"), tag).unwrap();
    std::fs::write(dir.join(TARGET.signature_name()), sign(signer, &archive)).unwrap();
    std::fs::write(dir.join(TARGET.asset_name()), archive).unwrap();
}

fn run(
    dir: &Path,
    key: &minisign::KeyPair,
    exe: &Path,
    current: &str,
    check_only: bool,
) -> Result<UpdateOutcome, DistError> {
    let source = DirReleases::new(dir.to_path_buf());
    let verifier = MinisignVerifier::new(&key_text(key)).unwrap();
    let replacer = ExeReplacer::new(exe.to_path_buf());
    let service = UpdateService {
        source: &source,
        verifier: &verifier,
        unpacker: &ArchiveUnpacker,
        replacer: &replacer,
        target: TARGET,
    };
    service.run(&Version::parse(current).unwrap(), check_only)
}

struct World {
    release: tempfile::TempDir,
    bin: tempfile::TempDir,
    exe: std::path::PathBuf,
    key: minisign::KeyPair,
}

fn world() -> World {
    let release = tempfile::tempdir().unwrap();
    let bin = tempfile::tempdir().unwrap();
    let exe = bin.path().join("swarmux");
    std::fs::write(&exe, b"v1 binary").unwrap();
    World {
        release,
        bin,
        exe,
        key: minisign::KeyPair::generate_unencrypted_keypair().unwrap(),
    }
}

#[test]
fn a_signed_newer_release_replaces_the_executable() {
    let w = world();
    release(w.release.path(), "v0.2.0", b"v2 binary", &w.key);
    let out = run(w.release.path(), &w.key, &w.exe, "0.1.0", false).unwrap();
    assert!(matches!(out, UpdateOutcome::Updated { .. }));
    assert_eq!(std::fs::read(&w.exe).unwrap(), b"v2 binary");
    assert_eq!(std::fs::read_dir(w.bin.path()).unwrap().count(), 1);
}

#[test]
fn a_signature_by_another_key_is_rejected_and_nothing_changes() {
    let w = world();
    let attacker = minisign::KeyPair::generate_unencrypted_keypair().unwrap();
    release(w.release.path(), "v0.2.0", b"evil", &attacker);
    let err = run(w.release.path(), &w.key, &w.exe, "0.1.0", false).unwrap_err();
    assert!(matches!(err, DistError::BadSignature(_)), "{err}");
    assert_eq!(std::fs::read(&w.exe).unwrap(), b"v1 binary");
    assert_eq!(std::fs::read_dir(w.bin.path()).unwrap().count(), 1);
}

#[test]
fn a_tampered_archive_or_a_garbled_signature_is_rejected() {
    let w = world();
    release(w.release.path(), "v0.2.0", b"v2 binary", &w.key);
    let archive = w.release.path().join(TARGET.asset_name());
    std::fs::write(&archive, make_archive(TARGET, b"swapped")).unwrap();
    assert!(matches!(
        run(w.release.path(), &w.key, &w.exe, "0.1.0", false),
        Err(DistError::BadSignature(_))
    ));

    std::fs::write(w.release.path().join(TARGET.signature_name()), "garbage").unwrap();
    assert!(matches!(
        run(w.release.path(), &w.key, &w.exe, "0.1.0", false),
        Err(DistError::BadSignature(_))
    ));
    assert_eq!(std::fs::read(&w.exe).unwrap(), b"v1 binary");
}

#[test]
fn up_to_date_and_check_only_download_nothing() {
    let w = world();
    release(w.release.path(), "v0.2.0", b"v2 binary", &w.key);
    // Break the archive: neither path may look at it.
    std::fs::remove_file(w.release.path().join(TARGET.asset_name())).unwrap();
    let same = run(w.release.path(), &w.key, &w.exe, "0.2.0", false).unwrap();
    assert!(matches!(same, UpdateOutcome::UpToDate(_)));
    let check = run(w.release.path(), &w.key, &w.exe, "0.1.0", true).unwrap();
    assert_eq!(
        check,
        UpdateOutcome::Available(Version::parse("0.2.0").unwrap())
    );
    assert_eq!(std::fs::read(&w.exe).unwrap(), b"v1 binary");
}
