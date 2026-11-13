//! The allow rules the user added while swarmux ran, and where calls are
//! made.
//!
//! Rules live in `guard-rules.toml` in the state folder, one `[[rule]]` table
//! each, with where they apply, who made them and when they stop. The file
//! is read line by line: an entry that doesn't make sense is skipped (and
//! named by [`TomlRuleStore::problems`]), never a reason to refuse the rest.

use std::collections::HashMap;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};

use swarmux_application::ports::{RepoRoots, RuleStore};
use swarmux_application::{AppError, AppResult};
use swarmux_domain::{JobId, Pattern, RuleScope, RuleSource, StoredRule};
use toml::{Table, Value};

use crate::git::git_command;

/// The format of the rules file.
const SCHEMA_VERSION: i64 = 1;

/// Where the rules of the state folder `data_dir` are kept.
pub fn rules_file(data_dir: &Path) -> PathBuf {
    data_dir.join("guard-rules.toml")
}

/// `RuleStore` over `guard-rules.toml`.
pub struct TomlRuleStore {
    path: PathBuf,
}

impl TomlRuleStore {
    pub fn new(path: PathBuf) -> TomlRuleStore {
        TomlRuleStore { path }
    }

    fn fail(&self, message: impl std::fmt::Display) -> AppError {
        AppError::Config(format!("{}: {message}", self.path.display()))
    }

    /// The entries of the file that were skipped, one message each.
    pub fn problems(&self) -> Vec<String> {
        self.read()
            .map(|(_, problems)| problems)
            .unwrap_or_default()
    }

    fn read(&self) -> AppResult<(Vec<StoredRule>, Vec<String>)> {
        let text = match std::fs::read_to_string(&self.path) {
            Ok(t) => t,
            Err(e) if e.kind() == ErrorKind::NotFound => return Ok((Vec::new(), Vec::new())),
            Err(e) => return Err(self.fail(e)),
        };
        let table: Table = match toml::from_str(&text) {
            Ok(t) => t,
            // A file that doesn't parse grants nothing; its rules are skipped.
            Err(e) => {
                return Ok((
                    Vec::new(),
                    vec![format!(
                        "{} can't be read, so none of its rules apply: {}",
                        self.path.display(),
                        e.message().trim()
                    )],
                ));
            }
        };
        let mut rules = Vec::new();
        let mut problems = Vec::new();
        for (i, entry) in table
            .get("rule")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default()
            .iter()
            .enumerate()
        {
            match parse_entry(entry) {
                Ok(rule) => rules.push(rule),
                Err(why) => problems.push(format!("rule {} skipped: {why}", i + 1)),
            }
        }
        Ok((rules, problems))
    }

    fn write(&self, rules: &[StoredRule]) -> AppResult<()> {
        let mut out = String::from(
            "# Allow rules swarmux added when you pressed \"always allow\" or took an offered rule.\n\
             # List them with `swarmux guard rules list`; remove one with `swarmux guard rules remove <id>`.\n",
        );
        out.push_str(&format!("schema_version = {SCHEMA_VERSION}\n"));
        for r in rules {
            out.push_str("\n[[rule]]\n");
            out.push_str(&format!("id = {}\n", quote(&r.id)));
            out.push_str(&format!("rule = {}\n", quote(&r.text)));
            out.push_str(&format!("scope = {}\n", quote(r.scope.as_str())));
            if let Some(root) = &r.root {
                out.push_str(&format!("root = {}\n", quote(root)));
            }
            out.push_str(&format!("source = {}\n", quote(r.source.as_str())));
            out.push_str(&format!("created_at = {}\n", r.created_at));
            if let Some(at) = r.expires_at {
                out.push_str(&format!("expires_at = {at}\n"));
            }
        }
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| self.fail(e))?;
        }
        let tmp = self.path.with_extension("toml.tmp");
        std::fs::write(&tmp, out).map_err(|e| self.fail(e))?;
        std::fs::rename(&tmp, &self.path).map_err(|e| self.fail(e))
    }
}

/// A TOML basic string.
fn quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
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

fn parse_entry(entry: &Value) -> Result<StoredRule, String> {
    let text = entry.get("rule").and_then(Value::as_str).ok_or("no rule")?;
    Pattern::parse(text).map_err(|e| e.to_string())?;
    let scope = entry
        .get("scope")
        .and_then(Value::as_str)
        .and_then(RuleScope::parse)
        .ok_or("scope is not repo, workspace or global")?;
    let root = entry.get("root").and_then(Value::as_str);
    if scope != RuleScope::Global && root.is_none() {
        return Err("a repo or workspace rule needs a root".into());
    }
    let source = entry
        .get("source")
        .and_then(Value::as_str)
        .and_then(RuleSource::parse)
        .unwrap_or(RuleSource::AlwaysAllow);
    let created_at = entry
        .get("created_at")
        .and_then(Value::as_integer)
        .ok_or("no created_at")?;
    let expires_at = match entry.get("expires_at") {
        None => None,
        Some(v) => Some(v.as_integer().ok_or("expires_at is not a number")?),
    };
    let mut rule = StoredRule::new(text, scope, root, source, created_at, expires_at);
    // The id as written, so a rule keeps its name.
    if let Some(id) = entry.get("id").and_then(Value::as_str) {
        rule.id = id.to_string();
    }
    Ok(rule)
}

impl RuleStore for TomlRuleStore {
    fn list(&self) -> AppResult<Vec<StoredRule>> {
        self.read().map(|(rules, _)| rules)
    }

    fn add(&self, rule: &StoredRule) -> AppResult<()> {
        let (mut rules, _) = self.read()?;
        rules.retain(|r| r.id != rule.id);
        rules.push(rule.clone());
        self.write(&rules)
    }

    fn remove(&self, id: &str) -> AppResult<bool> {
        let (mut rules, _) = self.read()?;
        let before = rules.len();
        rules.retain(|r| r.id != id);
        if rules.len() == before {
            return Ok(false);
        }
        self.write(&rules)?;
        Ok(true)
    }
}

/// The top of a working tree and the folder of its repository.
type Roots = (Option<String>, Option<String>);

/// `RepoRoots` from `git`, remembering the answer per folder.
pub struct GitRepoRoots {
    bin: PathBuf,
    /// The top of the working tree, and the folder of the repository a
    /// worktree belongs to, by the folder a call was made in.
    cache: Mutex<HashMap<String, Roots>>,
    /// The workspace folder of a job, when it runs in one.
    workspaces: Option<WorkspaceOf>,
}

/// Which workspace a job belongs to.
pub type WorkspaceOf = Box<dyn Fn(JobId) -> Option<String> + Send + Sync>;

impl GitRepoRoots {
    pub fn new(bin: PathBuf) -> GitRepoRoots {
        GitRepoRoots {
            bin,
            cache: Mutex::default(),
            workspaces: None,
        }
    }

    /// Where a job's workspace is, as `of` says.
    pub fn with_workspaces(mut self, of: WorkspaceOf) -> GitRepoRoots {
        self.workspaces = Some(of);
        self
    }

    fn lookup(&self, cwd: &str) -> Roots {
        if cwd.trim().is_empty() {
            return (None, None);
        }
        if let Some(hit) = self
            .cache
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(cwd)
        {
            return hit.clone();
        }
        let ask = |args: &[&str]| -> Option<String> {
            let out = git_command(&self.bin, Path::new(cwd))
                .args(args)
                .output()
                .ok()?;
            out.status.success().then(|| {
                String::from_utf8_lossy(&out.stdout)
                    .trim()
                    .replace('\\', "/")
            })
        };
        let tree = ask(&["rev-parse", "--show-toplevel"]).filter(|t| !t.is_empty());
        // `--git-common-dir` is the main repository's `.git`, also from a worktree.
        let main = ask(&["rev-parse", "--path-format=absolute", "--git-common-dir"])
            .filter(|c| !c.is_empty())
            .map(|c| c.strip_suffix("/.git").map(str::to_string).unwrap_or(c))
            .or_else(|| tree.clone());
        self.cache
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(cwd.to_string(), (tree.clone(), main.clone()));
        (tree, main)
    }
}

impl RepoRoots for GitRepoRoots {
    fn repo_root(&self, _job: Option<JobId>, cwd: &str) -> Option<String> {
        self.lookup(cwd).1
    }

    fn workspace_root(&self, job: Option<JobId>, _cwd: &str) -> Option<String> {
        self.workspaces.as_ref().and_then(|of| of(job?))
    }

    fn tree_root(&self, cwd: &str) -> Option<String> {
        self.lookup(cwd).0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(text: &str, scope: RuleScope, root: Option<&str>, at: i64) -> StoredRule {
        StoredRule::new(
            text,
            scope,
            root,
            RuleSource::AlwaysAllow,
            at,
            Some(at + 1000),
        )
    }

    #[test]
    fn rules_round_trip_with_their_provenance() {
        let dir = tempfile::tempdir().unwrap();
        let store = TomlRuleStore::new(rules_file(dir.path()));
        assert_eq!(store.list().unwrap(), []);
        let a = rule("Bash(make all)", RuleScope::Repo, Some(r"C:\Proj\app"), 10);
        let b = StoredRule {
            expires_at: None,
            source: RuleSource::Suggestion,
            ..rule("Bash(cargo test:*)", RuleScope::Global, None, 20)
        };
        store.add(&a).unwrap();
        store.add(&b).unwrap();
        store.add(&a).unwrap();
        assert_eq!(store.list().unwrap(), vec![b.clone(), a.clone()]);
        assert!(store.remove(&a.id).unwrap());
        assert!(!store.remove(&a.id).unwrap());
        assert_eq!(store.list().unwrap(), [b]);
        let text = std::fs::read_to_string(rules_file(dir.path())).unwrap();
        assert!(text.contains("schema_version = 1"), "{text}");
        assert!(
            !text.contains("expires_at"),
            "never has no expiry line: {text}"
        );
    }

    #[test]
    fn bad_entries_are_skipped_and_named() {
        let dir = tempfile::tempdir().unwrap();
        let path = rules_file(dir.path());
        std::fs::write(
            &path,
            "schema_version = 1\n\n[[rule]]\nrule = \"Bash(\"\nscope = \"global\"\ncreated_at = 1\n\n[[rule]]\nrule = \"Bash(make)\"\nscope = \"galaxy\"\ncreated_at = 1\n\n[[rule]]\nrule = \"Bash(make)\"\nscope = \"repo\"\ncreated_at = 1\n\n[[rule]]\nrule = \"Bash(ls)\"\nscope = \"global\"\ncreated_at = 1\n",
        )
        .unwrap();
        let store = TomlRuleStore::new(path);
        let rules = store.list().unwrap();
        assert_eq!(rules.len(), 1, "{rules:?}");
        assert_eq!(rules[0].text, "Bash(ls)");
        assert_eq!(store.problems().len(), 3);
    }

    #[test]
    fn a_file_that_does_not_parse_grants_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let path = rules_file(dir.path());
        std::fs::write(&path, "this is [not toml").unwrap();
        let store = TomlRuleStore::new(path);
        assert_eq!(store.list().unwrap(), []);
        assert_eq!(store.problems().len(), 1);
    }
}
