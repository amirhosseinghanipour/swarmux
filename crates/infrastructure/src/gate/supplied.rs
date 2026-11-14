//! What a repository supplies that needs the owner's trust: the checks it
//! ships in `.swarmux/gates.toml`, the commands swarmux guesses from its
//! files, the keys of its `.swarmux/*.toml` that loosen the gate, and the
//! hash of every file those come from.

use std::fs;
use std::path::Path;

use swarmux_application::LoadedGate;
use swarmux_domain::gate::GateOrigin;
use swarmux_domain::trust::{CommandSource, Supplied, TrustEntry, WantedCommand};

use super::config::{RepoFilesStamp, declared_check_count, npm_test_script};

/// The `.swarmux/*.toml` files directly in `root`, with their hashes.
/// Folders below (`missions`, `evidence`) are swarmux's own and not read.
fn config_files(root: &Path) -> Vec<TrustEntry> {
    let mut out = Vec::new();
    let Ok(entries) = fs::read_dir(root.join(".swarmux")) else {
        return out;
    };
    for entry in entries.filter_map(Result::ok) {
        let name = entry.file_name().to_string_lossy().into_owned();
        let is_file = entry.file_type().is_ok_and(|t| t.is_file());
        if is_file
            && name.ends_with(".toml")
            && let Ok(bytes) = fs::read(entry.path())
        {
            out.push(TrustEntry::of_bytes(&format!(".swarmux/{name}"), &bytes));
        }
    }
    out.sort();
    out
}

/// What `gate` (just loaded from `root`) supplies. Nothing when it only
/// restricts: a file of protected globs asks for no trust.
pub(super) fn compute(root: &Path, gate: &LoadedGate, repo_id: String) -> Supplied {
    let spec = &gate.spec;
    let loosens = spec.loosening();
    let from_file = gate.origin == GateOrigin::File
        && fs::read_to_string(root.join(super::config::GATES_FILE))
            .map(|text| declared_check_count(&text) > 0)
            .unwrap_or(false);
    if spec.checks.is_empty() && loosens.is_empty() {
        return Supplied::default();
    }
    let source = if from_file {
        CommandSource::File
    } else {
        CommandSource::Detected
    };
    let commands: Vec<WantedCommand> = spec
        .checks
        .iter()
        .map(|c| WantedCommand {
            name: c.name.clone(),
            line: c.command_line(),
            source,
        })
        .collect();
    let mut entries = Vec::new();
    if from_file || !loosens.is_empty() {
        entries.extend(config_files(root));
    }
    if !from_file {
        let script = npm_test_script(root);
        for c in &commands {
            let script = c
                .line
                .starts_with("npm ")
                .then_some(script.as_deref())
                .flatten();
            entries.push(TrustEntry::detected(&c.line, script));
        }
    }
    entries.sort();
    entries.dedup();
    Supplied {
        repo_id,
        root: root.display().to_string(),
        entries,
        commands,
        loosens,
    }
}

/// When the files `compute` reads last changed: a cheap check that its
/// answer still holds.
pub(super) fn stamp(root: &Path) -> RepoFilesStamp {
    let modified = |p: &Path| fs::metadata(p).and_then(|m| m.modified()).ok();
    let dir = root.join(".swarmux");
    [
        modified(root),
        modified(&dir),
        modified(&dir.join("gates.toml")),
        modified(&dir.join("config.toml")),
        modified(&root.join("package.json")),
    ]
}
