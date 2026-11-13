//! What a change touched, file by file.
//!
//! A [`DiffFile`] is one line of `git diff --numstat` (plus the kind of
//! change when git says it). Only what git counted is kept: a binary file
//! has no line counts, and a change whose per-file data was never kept (a
//! bundle sealed before C3) has no list at all, only the totals.

use crate::json::Json;
use crate::ledger::Payload;

/// How a file changed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChangeKind {
    Added,
    Modified,
    Deleted,
}

impl ChangeKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ChangeKind::Added => "added",
            ChangeKind::Modified => "modified",
            ChangeKind::Deleted => "deleted",
        }
    }

    pub fn parse(s: &str) -> Option<ChangeKind> {
        [ChangeKind::Added, ChangeKind::Modified, ChangeKind::Deleted]
            .into_iter()
            .find(|k| k.as_str() == s)
    }

    /// The status letter of `git diff --raw` (`A`, `M`, `D`, `T` for a
    /// changed type); any other letter says nothing the console shows.
    pub fn from_status(letter: char) -> Option<ChangeKind> {
        match letter {
            'A' => Some(ChangeKind::Added),
            'M' | 'T' => Some(ChangeKind::Modified),
            'D' => Some(ChangeKind::Deleted),
            _ => None,
        }
    }
}

/// One file of a change, with the lines git counted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiffFile {
    /// Relative to the repository, with `/`.
    pub path: String,
    pub insertions: u32,
    pub deletions: u32,
    /// git counts no lines of a binary file: the counts are 0 and are not
    /// to be read as "nothing changed".
    pub binary: bool,
    /// `None` when git did not say.
    pub kind: Option<ChangeKind>,
}

impl DiffFile {
    /// A file with line counts.
    pub fn text(path: &str, insertions: u32, deletions: u32) -> DiffFile {
        DiffFile {
            path: path.to_string(),
            insertions,
            deletions,
            binary: false,
            kind: None,
        }
    }

    pub fn with_kind(mut self, kind: ChangeKind) -> DiffFile {
        self.kind = Some(kind);
        self
    }

    pub(crate) fn payload(&self) -> Payload {
        Payload::new()
            .str("path", &self.path)
            .int("insertions", i64::from(self.insertions))
            .int("deletions", i64::from(self.deletions))
            .bool("binary", self.binary)
            .opt_str("kind", self.kind.map(ChangeKind::as_str))
    }

    /// The file as JSON, for keeping.
    pub fn to_json(&self) -> String {
        self.payload().build()
    }

    pub fn from_json(v: &Json) -> Option<DiffFile> {
        let count = |k: &str| {
            v.get(k)
                .and_then(Json::as_u64)
                .map(|n| u32::try_from(n).unwrap_or(u32::MAX))
        };
        Some(DiffFile {
            path: v.get("path")?.as_str()?.to_string(),
            insertions: count("insertions")?,
            deletions: count("deletions")?,
            binary: v.get("binary").and_then(Json::as_bool).unwrap_or(false),
            kind: v
                .get("kind")
                .and_then(Json::as_str)
                .and_then(ChangeKind::parse),
        })
    }
}

/// The files as a JSON array.
pub fn files_json(files: &[DiffFile]) -> String {
    format!(
        "[{}]",
        files
            .iter()
            .map(DiffFile::to_json)
            .collect::<Vec<_>>()
            .join(",")
    )
}

/// Reads an array written by [`files_json`]; entries that are not files
/// are left out.
pub fn files_from_json(v: Option<&Json>) -> Vec<DiffFile> {
    v.and_then(Json::as_array)
        .unwrap_or_default()
        .iter()
        .filter_map(DiffFile::from_json)
        .collect()
}

/// Insertions and deletions summed over `files`.
pub fn totals(files: &[DiffFile]) -> (u32, u32) {
    files.iter().fold((0u32, 0u32), |(i, d), f| {
        (
            i.saturating_add(f.insertions),
            d.saturating_add(f.deletions),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_round_trips_through_json() {
        let f = DiffFile::text("src/a \"b\".rs", 12, 3).with_kind(ChangeKind::Modified);
        let json = Json::parse(&f.to_json()).unwrap();
        assert_eq!(DiffFile::from_json(&json), Some(f));
        let bin = DiffFile {
            path: "logo.png".into(),
            insertions: 0,
            deletions: 0,
            binary: true,
            kind: None,
        };
        let json = Json::parse(&bin.to_json()).unwrap();
        assert_eq!(DiffFile::from_json(&json), Some(bin));
    }

    #[test]
    fn a_list_round_trips_and_skips_what_is_not_a_file() {
        let files = vec![
            DiffFile::text("a.rs", 1, 0).with_kind(ChangeKind::Added),
            DiffFile::text("b.rs", 0, 4).with_kind(ChangeKind::Deleted),
        ];
        let parsed = Json::parse(&files_json(&files)).unwrap();
        assert_eq!(files_from_json(Some(&parsed)), files);
        let mixed =
            Json::parse(r#"[{"path":"a.rs","insertions":1,"deletions":2},7,{"x":1}]"#).unwrap();
        assert_eq!(
            files_from_json(Some(&mixed)),
            vec![DiffFile::text("a.rs", 1, 2)]
        );
        assert_eq!(files_from_json(None), Vec::new());
    }

    #[test]
    fn totals_add_up_and_ignore_nothing_but_binaries_carry_zero() {
        let files = vec![DiffFile::text("a", 3, 1), DiffFile::text("b", 4, 0)];
        assert_eq!(totals(&files), (7, 1));
        assert_eq!(totals(&[]), (0, 0));
    }

    #[test]
    fn status_letters_map_to_kinds() {
        assert_eq!(ChangeKind::from_status('A'), Some(ChangeKind::Added));
        assert_eq!(ChangeKind::from_status('M'), Some(ChangeKind::Modified));
        assert_eq!(ChangeKind::from_status('T'), Some(ChangeKind::Modified));
        assert_eq!(ChangeKind::from_status('D'), Some(ChangeKind::Deleted));
        assert_eq!(ChangeKind::from_status('R'), None);
        for k in [ChangeKind::Added, ChangeKind::Modified, ChangeKind::Deleted] {
            assert_eq!(ChangeKind::parse(k.as_str()), Some(k));
        }
    }
}
