//! Claude Code transcripts (`~/.claude/projects/**/*.jsonl`) read
//! incrementally into a [`UsageBook`].
//!
//! Each file is read from where the previous pass stopped, up to its last
//! complete line; a file that shrank is read again from the start (messages
//! are deduplicated by id, so nothing counts twice).

mod cache;
mod line;

use std::collections::HashMap;
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Instant, UNIX_EPOCH};

use swarmux_domain::telemetry::UsageBook;

pub use cache::{FileMark, UsageCache};
pub use line::{UsageLine, parse_usage_line};

/// Read size per chunk.
const CHUNK: usize = 4 << 20;
/// Most files read at once.
const MAX_WORKERS: usize = 8;

/// What one [`TranscriptIndex::refresh`] did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RefreshStats {
    pub files: usize,
    /// Files that had new bytes.
    pub files_read: usize,
    pub bytes_read: u64,
    /// Messages new or updated in this pass.
    pub messages: usize,
    pub took_ms: u64,
}

pub struct TranscriptIndex {
    root: PathBuf,
    cache: Option<UsageCache>,
    book: UsageBook,
    /// Message key → index in the book.
    keys: HashMap<u64, usize>,
    /// Index in the book → message key.
    entry_keys: Vec<u64>,
    marks: HashMap<String, FileMark>,
    /// Why the cache couldn't be used, if it couldn't.
    pub cache_error: Option<String>,
}

impl TranscriptIndex {
    /// An index of `root`, starting from what `cache` (a file path) remembers.
    /// Without a cache, or if it fails to open, everything is read again.
    pub fn open(root: &Path, cache: Option<&Path>) -> TranscriptIndex {
        let mut index = TranscriptIndex {
            root: root.to_path_buf(),
            cache: None,
            book: UsageBook::default(),
            keys: HashMap::new(),
            entry_keys: Vec::new(),
            marks: HashMap::new(),
            cache_error: None,
        };
        let Some(path) = cache else {
            return index;
        };
        let loaded = UsageCache::open(path).and_then(|c| {
            let marks = c.marks()?;
            let entries = c.entries()?;
            Ok((c, marks, entries))
        });
        match loaded {
            Ok((c, marks, mut entries)) => {
                entries.sort_by_key(|l| l.ts);
                for l in &entries {
                    index.upsert(l);
                }
                index.marks = marks;
                index.cache = Some(c);
            }
            Err(e) => index.cache_error = Some(format!("usage cache {}: {e}", path.display())),
        }
        index
    }

    pub fn book(&self) -> &UsageBook {
        &self.book
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Reads what was added to the transcripts since the last pass. Stops
    /// early (keeping what it read) once `stop` is set.
    pub fn refresh(&mut self, stop: &AtomicBool) -> io::Result<RefreshStats> {
        let started = Instant::now();
        let mut stats = RefreshStats::default();
        let files = list_jsonl(&self.root)?;
        stats.files = files.len();
        let mut todo: Vec<(String, PathBuf, FileMark, FileMark)> = Vec::new();
        for (path, size, mtime) in &files {
            let key = path.to_string_lossy().into_owned();
            let old = self.marks.get(&key).copied().unwrap_or_default();
            let known = self.marks.contains_key(&key);
            if known && old.size == *size && old.mtime == *mtime {
                continue;
            }
            // A file that shrank was replaced: read it from the start.
            let from = if *size < old.offset { 0 } else { old.offset };
            let start = FileMark {
                offset: from,
                ..old
            };
            todo.push((
                key,
                path.clone(),
                start,
                FileMark {
                    offset: from,
                    size: *size,
                    mtime: *mtime,
                },
            ));
        }
        let gone: Vec<String> = {
            let present: std::collections::HashSet<String> = files
                .iter()
                .map(|(p, _, _)| p.to_string_lossy().into_owned())
                .collect();
            self.marks
                .keys()
                .filter(|k| !present.contains(*k))
                .cloned()
                .collect()
        };

        let results = read_all(&todo, stop);
        let mut new_marks = Vec::new();
        let mut changed: Vec<usize> = Vec::new();
        for ((key, _, _, target), result) in todo.iter().zip(results) {
            let Some((lines, consumed)) = result else {
                continue;
            };
            stats.files_read += usize::from(consumed > 0);
            stats.bytes_read += consumed;
            let mark = FileMark {
                offset: target.offset + consumed,
                size: target.size,
                mtime: target.mtime,
            };
            for l in &lines {
                if let Some(i) = self.upsert(l) {
                    changed.push(i);
                }
            }
            self.marks.insert(key.clone(), mark);
            new_marks.push((key.clone(), mark));
        }
        for k in &gone {
            self.marks.remove(k);
        }
        changed.sort_unstable();
        changed.dedup();
        stats.messages = changed.len();
        if self.cache.is_some() {
            let lines: Vec<UsageLine> = changed.iter().map(|&i| self.line_at(i)).collect();
            let refs: Vec<&UsageLine> = lines.iter().collect();
            if let Some(Err(e)) = self
                .cache
                .as_mut()
                .map(|c| c.save(&refs, &new_marks, &gone))
            {
                self.cache_error = Some(format!("usage cache: {e}"));
            }
        }
        stats.took_ms = started.elapsed().as_millis() as u64;
        Ok(stats)
    }

    /// Adds a message or grows a known one. Returns its index if it changed.
    fn upsert(&mut self, l: &UsageLine) -> Option<usize> {
        match self.keys.get(&l.key) {
            Some(&i) => {
                let e = &mut self.book.entries[i];
                // Repeats of a streamed message: keep the most complete numbers.
                (l.usage.total() > e.usage.total()).then(|| {
                    e.usage = l.usage;
                    i
                })
            }
            None => {
                let i = self
                    .book
                    .push(l.ts, &l.session, &l.cwd, &l.model, l.sidechain, l.usage);
                self.keys.insert(l.key, i);
                self.entry_keys.push(l.key);
                Some(i)
            }
        }
    }

    fn line_at(&self, i: usize) -> UsageLine {
        let e = &self.book.entries[i];
        let key = self.entry_keys.get(i).copied().unwrap_or_default();
        UsageLine {
            key,
            ts: e.ts,
            session: self.book.sessions.name(e.session).to_string(),
            cwd: self.book.projects.name(e.project).to_string(),
            model: self.book.models.name(e.model).to_string(),
            sidechain: e.sidechain,
            usage: e.usage,
        }
    }
}

/// Every `*.jsonl` under `root` with its size and modification time (ms).
fn list_jsonl(root: &Path) -> io::Result<Vec<(PathBuf, u64, i64)>> {
    let mut out = Vec::new();
    let mut dirs = vec![root.to_path_buf()];
    let mut first = true;
    while let Some(dir) = dirs.pop() {
        let entries = match std::fs::read_dir(&dir) {
            Ok(e) => e,
            // A missing root is an error; a vanished subfolder is not.
            Err(e) if first => return Err(e),
            Err(_) => continue,
        };
        first = false;
        for entry in entries.flatten() {
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            let path = entry.path();
            if kind.is_dir() {
                dirs.push(path);
                continue;
            }
            if !path.extension().is_some_and(|e| e == "jsonl") {
                continue;
            }
            if let Ok(meta) = entry.metadata() {
                let mtime = meta
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                    .map_or(0, |d| d.as_millis() as i64);
                out.push((path, meta.len(), mtime));
            }
        }
    }
    Ok(out)
}

type ReadResult = Option<(Vec<UsageLine>, u64)>;

/// Reads every file's new lines, several files at a time.
fn read_all(todo: &[(String, PathBuf, FileMark, FileMark)], stop: &AtomicBool) -> Vec<ReadResult> {
    let workers = std::thread::available_parallelism()
        .map_or(2, |n| n.get())
        .clamp(1, MAX_WORKERS)
        .min(todo.len().max(1));
    let mut results: Vec<ReadResult> = vec![None; todo.len()];
    let next = std::sync::atomic::AtomicUsize::new(0);
    let slots = std::sync::Mutex::new(&mut results);
    std::thread::scope(|s| {
        for _ in 0..workers {
            s.spawn(|| {
                loop {
                    if stop.load(Ordering::Relaxed) {
                        return;
                    }
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    let Some((_, path, _, target)) = todo.get(i) else {
                        return;
                    };
                    let r = read_new_lines(path, target.offset, target.size).ok();
                    if let Ok(mut slots) = slots.lock() {
                        slots[i] = r;
                    }
                }
            });
        }
    });
    results
}

/// Usage lines in `path` between `from` and `to`, and how many bytes up to
/// the last complete line were consumed.
pub fn read_new_lines(path: &Path, from: u64, to: u64) -> io::Result<(Vec<UsageLine>, u64)> {
    let mut file = File::open(path)?;
    file.seek(SeekFrom::Start(from))?;
    let mut left = to.saturating_sub(from);
    let mut lines = Vec::new();
    let mut carry: Vec<u8> = Vec::new();
    let mut consumed = 0u64;
    let mut buf = vec![0u8; CHUNK];
    while left > 0 {
        let want = (left as usize).min(CHUNK);
        let n = file.read(&mut buf[..want])?;
        if n == 0 {
            break;
        }
        left -= n as u64;
        carry.extend_from_slice(&buf[..n]);
        let Some(last_nl) = carry.iter().rposition(|&b| b == b'\n') else {
            continue;
        };
        for raw in carry[..last_nl].split(|&b| b == b'\n') {
            let raw = raw.strip_suffix(b"\r").unwrap_or(raw);
            if let Some(l) = parse_usage_line(raw) {
                lines.push(l);
            }
        }
        consumed += last_nl as u64 + 1;
        carry.drain(..=last_nl);
    }
    Ok((lines, consumed))
}

#[cfg(test)]
mod tests;
