//! Which instruction files (CLAUDE.md, rules) a session has loaded.

use crate::Millis;

/// Files kept per session.
pub const FILES_PER_SESSION: usize = 64;

/// One instruction file Claude Code loaded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InstructionFile {
    pub path: String,
    /// `User`, `Project`, `Local`, `Managed`, …
    pub memory_type: String,
    /// `session_start`, `nested_traversal`, `path_glob_match`, `include`, `compact`.
    pub load_reason: String,
    pub loaded_at: Millis,
}

impl InstructionFile {
    /// Separator between the load reason and the memory type in a hook
    /// notice's detail.
    pub const SEP: char = ';';

    /// The notice detail for this file's reason and type.
    pub fn detail(load_reason: &str, memory_type: &str) -> String {
        format!("{load_reason}{}{memory_type}", Self::SEP)
    }

    /// Rebuilds a file from a notice's path and detail.
    pub fn from_notice(path: &str, detail: &str, loaded_at: Millis) -> InstructionFile {
        let (reason, kind) = detail.split_once(Self::SEP).unwrap_or((detail, ""));
        InstructionFile {
            path: path.to_string(),
            memory_type: kind.to_string(),
            load_reason: reason.to_string(),
            loaded_at,
        }
    }
}

/// Adds `file` to a session's list: a file loaded again moves to the end
/// with its newest reason; the oldest entries go when the list is full.
pub fn remember(files: &mut Vec<InstructionFile>, file: InstructionFile) {
    files.retain(|f| f.path != file.path);
    files.push(file);
    if files.len() > FILES_PER_SESSION {
        files.remove(0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detail_round_trips() {
        let d = InstructionFile::detail("session_start", "Project");
        let f = InstructionFile::from_notice("/w/CLAUDE.md", &d, 5);
        assert_eq!(f.load_reason, "session_start");
        assert_eq!(f.memory_type, "Project");
        assert_eq!(
            InstructionFile::from_notice("p", "include", 0).memory_type,
            ""
        );
    }

    #[test]
    fn reloading_replaces_and_the_list_is_bounded() {
        let mut files = Vec::new();
        remember(
            &mut files,
            InstructionFile::from_notice("a", "session_start;User", 1),
        );
        remember(
            &mut files,
            InstructionFile::from_notice("b", "session_start;User", 1),
        );
        remember(
            &mut files,
            InstructionFile::from_notice("a", "compact;User", 2),
        );
        let order: Vec<_> = files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(order, ["b", "a"]);
        assert_eq!(files[1].load_reason, "compact");
        for i in 0..100 {
            remember(
                &mut files,
                InstructionFile::from_notice(&format!("f{i}"), "", i),
            );
        }
        assert_eq!(files.len(), FILES_PER_SESSION);
    }
}
