//! `HoldoutStore` adapter: hidden checks kept as files outside every
//! repository, at `~/.swarmux/holdout/<repo>/<check>/`.
//!
//! A repository is known by its folder name and what it is (its remote and
//! its root commit), like `shop.id-3fa9c1d2e4b7`, so two repositories whose
//! folders share a name do not share hidden checks. Checks kept under the
//! plain folder name by an earlier version move to the identity key the first
//! time that repository is looked at (see `FsHoldoutStore::migrate`).
//!
//! Each check has a `check.toml` (its command, timeout and the SHA-256 of
//! each of its files, taken when it was added) and a `files/` folder that
//! mirrors where those files go in the repository. Before a file is placed
//! into a clean copy its hash is compared with the pin.

use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use swarmux_application::{AppError, AppResult, HoldoutStore};
use swarmux_domain::gate::{DEFAULT_CHECK_TIMEOUT_SECS, GateCheck, Pin};
use swarmux_domain::ledger::{hex, sha256};
use swarmux_domain::verifier::{HoldoutCheck, holdout_path, legacy_name, valid_name};

/// A check's description, in its folder.
pub const CHECK_FILE: &str = "check.toml";
/// Where a check's files are kept, in its folder.
const FILES_DIR: &str = "files";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawCheck {
    name: String,
    run: Vec<String>,
    timeout_secs: Option<u64>,
    #[serde(default)]
    file: Vec<RawFile>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawFile {
    path: String,
    sha256: String,
}

/// Told when hidden checks move from a folder-name key to an identity key:
/// the old key, the new one.
pub type MigrationLog = Box<dyn Fn(&str, &str) + Send + Sync>;

/// Keeps hidden checks below one folder.
pub struct FsHoldoutStore {
    root: PathBuf,
    migrated: Option<MigrationLog>,
}

/// Which folder names were handed to which repository, one `name<TAB>key`
/// per line, so a second repository of the same name is not given the first
/// one's hidden checks.
const MIGRATED_FILE: &str = ".migrated";

impl FsHoldoutStore {
    pub fn new(root: PathBuf) -> FsHoldoutStore {
        FsHoldoutStore {
            root,
            migrated: None,
        }
    }

    /// Calls `log` whenever hidden checks move to an identity key.
    pub fn with_migration_log(mut self, log: MigrationLog) -> FsHoldoutStore {
        self.migrated = Some(log);
        self
    }

    fn repo_dir(&self, repo: &str) -> AppResult<PathBuf> {
        valid_name(repo).map_err(|e| AppError::Workspace(format!("repository name {e}")))?;
        let dir = self.root.join(repo);
        if !dir.exists()
            && let Some(legacy) = legacy_name(repo)
        {
            self.migrate(legacy, repo, &dir);
        }
        Ok(dir)
    }

    /// An earlier version kept a repository's hidden checks under its
    /// folder name alone. The first repository of that name to be looked at
    /// by its identity takes them over: the folder is renamed to its key
    /// and the move is written to `.migrated`. A name already handed to
    /// another repository is not handed out twice (the later repository
    /// simply has no hidden checks of its own yet).
    fn migrate(&self, legacy: &str, key: &str, to: &Path) {
        let from = self.root.join(legacy);
        if !from.is_dir() {
            return;
        }
        let marker = self.root.join(MIGRATED_FILE);
        let taken = fs::read_to_string(&marker).unwrap_or_default();
        if taken.lines().any(|l| l.split('\t').next() == Some(legacy)) {
            return;
        }
        if fs::rename(&from, to).is_err() {
            return;
        }
        let line = format!("{legacy}\t{key}\n");
        let _ = fs::write(&marker, format!("{taken}{line}"));
        if let Some(log) = &self.migrated {
            log(legacy, key);
        }
    }

    fn check_dir(&self, repo: &str, name: &str) -> AppResult<PathBuf> {
        valid_name(name).map_err(|e| AppError::Workspace(format!("check name {e}")))?;
        Ok(self.repo_dir(repo)?.join(name))
    }
}

fn io(what: &Path, e: std::io::Error) -> AppError {
    AppError::Workspace(format!("{}: {e}", what.display()))
}

/// A TOML basic string.
pub(crate) fn quoted(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if c.is_control() => out.push_str(&format!("\\u{:04X}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn render(check: &HoldoutCheck) -> String {
    let c = &check.check;
    let run: Vec<String> = std::iter::once(&c.program)
        .chain(&c.args)
        .map(|a| quoted(a))
        .collect();
    let mut out = format!(
        "# A hidden check: swarmux runs it in clean copies, agents never see it.\nname = {}\nrun = [{}]\ntimeout_secs = {}\n",
        quoted(&c.name),
        run.join(", "),
        c.timeout_secs
    );
    for f in &check.files {
        out.push_str(&format!(
            "\n[[file]]\npath = {}\nsha256 = {}\n",
            quoted(&f.path),
            quoted(&f.sha256)
        ));
    }
    out
}

fn parse(text: &str, folder: &str) -> Result<HoldoutCheck, String> {
    let raw: RawCheck = toml::from_str(text).map_err(|e| e.message().to_string())?;
    if raw.name != folder {
        return Err(format!("its name {:?} is not its folder's", raw.name));
    }
    let Some((program, args)) = raw.run.split_first() else {
        return Err("run is empty".into());
    };
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let mut check = GateCheck::new(&raw.name, program, &args);
    check.timeout_secs = match raw.timeout_secs {
        Some(0) => return Err("timeout_secs must be above 0".into()),
        Some(t) => t,
        None => DEFAULT_CHECK_TIMEOUT_SECS,
    };
    let files = raw
        .file
        .into_iter()
        .map(|f| {
            Ok(Pin {
                path: holdout_path(&f.path)?,
                sha256: f.sha256,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok(HoldoutCheck { check, files })
}

impl HoldoutStore for FsHoldoutStore {
    fn list(&self, repo: &str) -> AppResult<Vec<HoldoutCheck>> {
        let dir = self.repo_dir(repo)?;
        let entries = match fs::read_dir(&dir) {
            Ok(e) => e,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(io(&dir, e)),
        };
        let mut out = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|e| io(&dir, e))?;
            let file = entry.path().join(CHECK_FILE);
            if !file.is_file() {
                continue;
            }
            let folder = entry.file_name().to_string_lossy().into_owned();
            let text = fs::read_to_string(&file).map_err(|e| io(&file, e))?;
            let check = parse(&text, &folder)
                .map_err(|e| AppError::Workspace(format!("{}: {e}", file.display())))?;
            out.push(check);
        }
        out.sort_by(|a, b| a.name().cmp(b.name()));
        Ok(out)
    }

    fn add(
        &self,
        repo: &str,
        check: &GateCheck,
        files: &[(String, Vec<u8>)],
    ) -> AppResult<HoldoutCheck> {
        let dir = self.check_dir(repo, &check.name)?;
        if check.program.trim().is_empty() {
            return Err(AppError::Workspace("a check needs a program to run".into()));
        }
        let mut pins = Vec::new();
        for (path, _) in files {
            let path = holdout_path(path).map_err(AppError::Workspace)?;
            if pins.iter().any(|p: &Pin| p.path == path) {
                return Err(AppError::Workspace(format!("{path} is given twice")));
            }
            pins.push(Pin {
                path,
                sha256: String::new(),
            });
        }
        if dir.exists() {
            fs::remove_dir_all(&dir).map_err(|e| io(&dir, e))?;
        }
        let store = dir.join(FILES_DIR);
        fs::create_dir_all(&store).map_err(|e| io(&store, e))?;
        for (pin, (_, bytes)) in pins.iter_mut().zip(files) {
            let to = store.join(&pin.path);
            if let Some(parent) = to.parent() {
                fs::create_dir_all(parent).map_err(|e| io(parent, e))?;
            }
            fs::write(&to, bytes).map_err(|e| io(&to, e))?;
            pin.sha256 = hex(&sha256(bytes));
        }
        let held = HoldoutCheck {
            check: check.clone(),
            files: pins,
        };
        let file = dir.join(CHECK_FILE);
        fs::write(&file, render(&held)).map_err(|e| io(&file, e))?;
        Ok(held)
    }

    fn remove(&self, repo: &str, name: &str) -> AppResult<bool> {
        let dir = self.check_dir(repo, name)?;
        if !dir.join(CHECK_FILE).is_file() {
            return Ok(false);
        }
        fs::remove_dir_all(&dir).map_err(|e| io(&dir, e))?;
        Ok(true)
    }

    fn place(&self, repo: &str, check: &HoldoutCheck, dest: &Path) -> AppResult<()> {
        let store = self.check_dir(repo, check.name())?.join(FILES_DIR);
        for pin in &check.files {
            let path = holdout_path(&pin.path).map_err(AppError::Workspace)?;
            let from = store.join(&path);
            let bytes = fs::read(&from).map_err(|e| io(&from, e))?;
            if hex(&sha256(&bytes)) != pin.sha256 {
                return Err(AppError::Workspace(format!(
                    "{path} no longer matches the hash it was added with"
                )));
            }
            let to = dest.join(&path);
            if let Some(parent) = to.parent() {
                fs::create_dir_all(parent).map_err(|e| io(parent, e))?;
            }
            fs::write(&to, bytes).map_err(|e| io(&to, e))?;
        }
        Ok(())
    }

    fn root(&self) -> PathBuf {
        self.root.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> (tempfile::TempDir, FsHoldoutStore) {
        let dir = tempfile::tempdir().unwrap();
        let store = FsHoldoutStore::new(dir.path().join("holdout"));
        (dir, store)
    }

    fn hidden() -> GateCheck {
        let mut c = GateCheck::new("bigger", "cargo", &["test", "--test", "hidden \"x\""]);
        c.timeout_secs = 90;
        c
    }

    #[test]
    fn checks_round_trip_with_their_files_pinned() {
        let (_d, store) = store();
        assert!(store.list("shop").unwrap().is_empty());
        let held = store
            .add(
                "shop",
                &hidden(),
                &[("tests\\hidden.rs".into(), b"#[test] fn t() {}".to_vec())],
            )
            .unwrap();
        assert_eq!(held.files[0].path, "tests/hidden.rs");
        assert_eq!(held.files[0].sha256, hex(&sha256(b"#[test] fn t() {}")));
        let listed = store.list("shop").unwrap();
        assert_eq!(listed, std::slice::from_ref(&held));
        assert_eq!(listed[0].check.timeout_secs, 90);
        assert_eq!(listed[0].check.args[2], "hidden \"x\"");

        let dest = tempfile::tempdir().unwrap();
        store.place("shop", &held, dest.path()).unwrap();
        assert_eq!(
            fs::read(dest.path().join("tests/hidden.rs")).unwrap(),
            b"#[test] fn t() {}"
        );

        assert!(store.remove("shop", "bigger").unwrap());
        assert!(!store.remove("shop", "bigger").unwrap());
        assert!(store.list("shop").unwrap().is_empty());
    }

    #[test]
    fn a_file_edited_after_it_was_added_is_not_placed() {
        let (_d, store) = store();
        let held = store
            .add(
                "shop",
                &hidden(),
                &[("tests/h.rs".into(), b"strict".to_vec())],
            )
            .unwrap();
        let kept = store.root().join("shop/bigger/files/tests/h.rs");
        fs::write(&kept, "lenient").unwrap();
        let dest = tempfile::tempdir().unwrap();
        let err = store.place("shop", &held, dest.path()).unwrap_err();
        assert!(err.to_string().contains("no longer matches"), "{err}");
        assert!(!dest.path().join("tests/h.rs").exists());
    }

    #[test]
    fn bad_names_and_paths_are_refused() {
        let (_d, store) = store();
        assert!(store.add("../x", &hidden(), &[]).is_err());
        let mut bad = hidden();
        bad.name = "a/b".into();
        assert!(store.add("shop", &bad, &[]).is_err());
        for path in [
            "../x",
            "/etc/x",
            ".git/hooks/pre-commit",
            ".swarmux/gates.toml",
        ] {
            assert!(
                store
                    .add("shop", &hidden(), &[(path.into(), vec![])])
                    .is_err(),
                "{path}"
            );
        }
        let twice = [("a.rs".to_string(), vec![]), ("./a.rs".to_string(), vec![])];
        assert!(store.add("shop", &hidden(), &twice).is_err());
    }

    #[test]
    fn checks_kept_under_a_folder_name_move_to_the_identity_key_once() {
        let (_d, store) = store();
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let log = seen.clone();
        let store = store.with_migration_log(Box::new(move |from, to| {
            log.lock().unwrap().push((from.to_string(), to.to_string()));
        }));
        store
            .add("shop", &hidden(), &[("t/h.rs".into(), b"x".to_vec())])
            .unwrap();

        let first = "shop.id-0123456789ab";
        let held = store.list(first).unwrap();
        assert_eq!(
            held.len(),
            1,
            "the first repository of that name takes them over"
        );
        assert!(!store.root().join("shop").exists(), "moved, not copied");
        assert!(store.root().join(first).join("bigger/check.toml").is_file());
        let dest = tempfile::tempdir().unwrap();
        store.place(first, &held[0], dest.path()).unwrap();
        assert!(dest.path().join("t/h.rs").is_file());
        assert_eq!(
            *seen.lock().unwrap(),
            [("shop".to_string(), first.to_string())]
        );

        // A second repository in a folder of the same name is not given
        // the first one's hidden checks, and a name is handed out once.
        store
            .add("shop", &hidden(), &[("t/h.rs".into(), b"y".to_vec())])
            .unwrap();
        assert!(store.list("shop.id-ba9876543210").unwrap().is_empty());
        assert_eq!(store.list(first).unwrap().len(), 1, "still the first one's");
        assert_eq!(seen.lock().unwrap().len(), 1);
    }

    #[test]
    fn a_broken_check_file_is_an_error_not_a_silent_skip() {
        let (_d, store) = store();
        let dir = store.root().join("shop/oops");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(CHECK_FILE), "name = \"oops\"\nrun = []\n").unwrap();
        assert!(store.list("shop").is_err());
        fs::write(dir.join(CHECK_FILE), "name = \"other\"\nrun = [\"x\"]\n").unwrap();
        assert!(store.list("shop").is_err());
    }
}
