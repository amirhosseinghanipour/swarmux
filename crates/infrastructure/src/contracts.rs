//! Lists and reads the source files contracts are extracted from.
//!
//! Build output, dependency folders and VCS metadata are never entered;
//! symbolic links are not followed; files over a megabyte are skipped
//! (generated bundles, dumps of data). A real `.env` is never opened: the
//! extractors only want `.env.example` and its siblings.

use std::fs;
use std::path::Path;
use std::time::UNIX_EPOCH;

use swarmux_application::{SourceFile, SourceTree};

/// Folders no extractor looks into.
const SKIP_DIRS: [&str; 22] = [
    ".git",
    ".hg",
    ".svn",
    "node_modules",
    ".dart_tool",
    "build",
    "dist",
    "out",
    "target",
    ".next",
    ".nuxt",
    ".svelte-kit",
    ".turbo",
    ".gradle",
    "Pods",
    "vendor",
    "venv",
    ".venv",
    "__pycache__",
    ".idea",
    ".claude",
    ".swarmux",
];
/// Files over this size are not read.
const MAX_FILE_BYTES: u64 = 1_000_000;
/// Deeper folders are not entered.
const MAX_DEPTH: usize = 24;

/// Reads the real file system.
#[derive(Clone, Copy, Debug, Default)]
pub struct FsSourceTree;

fn walk(
    dir: &Path,
    rel: &str,
    depth: usize,
    wanted: &dyn Fn(&str) -> bool,
    out: &mut Vec<SourceFile>,
) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<_> = entries.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let name = e.file_name().to_string_lossy().into_owned();
        let Ok(kind) = e.file_type() else {
            continue;
        };
        let path = if rel.is_empty() {
            name.clone()
        } else {
            format!("{rel}/{name}")
        };
        if kind.is_dir() {
            if depth < MAX_DEPTH && !SKIP_DIRS.contains(&name.as_str()) {
                walk(&e.path(), &path, depth + 1, wanted, out);
            }
        } else if kind.is_file()
            && wanted(&path)
            && let Ok(meta) = e.metadata()
            && meta.len() <= MAX_FILE_BYTES
        {
            let mtime = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX));
            out.push(SourceFile {
                rel: path,
                size: meta.len(),
                mtime,
            });
        }
    }
}

impl SourceTree for FsSourceTree {
    fn files(&self, dir: &Path, wanted: &dyn Fn(&str) -> bool) -> Vec<SourceFile> {
        let mut out = Vec::new();
        walk(dir, "", 0, wanted, &mut out);
        out
    }

    fn read(&self, dir: &Path, rel: &str) -> Option<String> {
        let path = rel.split('/').fold(dir.to_path_buf(), |p, c| p.join(c));
        let bytes = fs::read(path).ok()?;
        Some(String::from_utf8_lossy(&bytes).into_owned())
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("swarmux-contracts-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn lists_wanted_files_and_skips_dependency_folders_and_big_files() {
        let dir = temp("walk");
        fs::create_dir_all(dir.join("lib/src")).unwrap();
        fs::create_dir_all(dir.join("node_modules/x")).unwrap();
        fs::create_dir_all(dir.join(".git")).unwrap();
        fs::write(dir.join("lib/src/a.dart"), "x").unwrap();
        fs::write(dir.join("lib/readme.md"), "x").unwrap();
        fs::write(dir.join("node_modules/x/b.dart"), "x").unwrap();
        fs::write(dir.join(".git/c.dart"), "x").unwrap();
        fs::write(dir.join("big.dart"), vec![b'x'; 1_100_000]).unwrap();
        let t = FsSourceTree;
        let files = t.files(&dir, &|p| p.ends_with(".dart"));
        let names: Vec<&str> = files.iter().map(|f| f.rel.as_str()).collect();
        assert_eq!(names, vec!["lib/src/a.dart"]);
        assert_eq!(t.read(&dir, "lib/src/a.dart").as_deref(), Some("x"));
        assert!(t.read(&dir, "missing.dart").is_none());
        let _ = fs::remove_dir_all(&dir);
    }
}
