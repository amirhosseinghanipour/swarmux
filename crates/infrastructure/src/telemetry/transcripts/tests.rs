use std::io::Write;

use swarmux_domain::telemetry::PriceTable;

use super::*;

const NOW: i64 = 1_793_613_600_000; // 2026-11-02 10:00 UTC

/// A synthetic assistant line (no real transcript data).
fn assistant(msg: &str, session: &str, minute: u32, output: u64) -> String {
    format!(
        r#"{{"type":"assistant","isSidechain":false,"cwd":"/work/demo","sessionId":"{session}","message":{{"id":"{msg}","model":"claude-sonnet-4-6","content":[{{"type":"text","text":"hello"}}],"usage":{{"input_tokens":10,"output_tokens":{output},"cache_read_input_tokens":100,"cache_creation_input_tokens":0}}}},"requestId":"req_{msg}","timestamp":"2026-11-02T09:{minute:02}:00.000Z"}}"#
    )
}

fn user(text: &str) -> String {
    format!(
        r#"{{"type":"user","message":{{"role":"user","content":"{text}"}},"timestamp":"2026-11-02T09:00:00Z"}}"#
    )
}

fn append(path: &Path, text: &str) {
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .unwrap();
    f.write_all(text.as_bytes()).unwrap();
}

fn never() -> AtomicBool {
    AtomicBool::new(false)
}

fn outputs(index: &TranscriptIndex) -> u64 {
    index.book().entries.iter().map(|e| e.usage.output).sum()
}

#[test]
fn reads_nested_project_folders_and_skips_other_lines() {
    let dir = tempfile::tempdir().unwrap();
    let a = dir.path().join("C--work-demo");
    let sub = a.join("sess-1").join("subagents");
    std::fs::create_dir_all(&sub).unwrap();
    append(
        &a.join("sess-1.jsonl"),
        &format!("{}\n{}\n", user("hi"), assistant("m1", "sess-1", 1, 5)),
    );
    append(
        &sub.join("agent-x.jsonl"),
        &format!("{}\n", assistant("m2", "sess-1", 2, 7)),
    );
    append(&a.join("notes.txt"), &assistant("m3", "sess-1", 3, 100));
    let mut index = TranscriptIndex::open(dir.path(), None);
    let stats = index.refresh(&never()).unwrap();
    assert_eq!((stats.files, stats.files_read, stats.messages), (2, 2, 2));
    assert_eq!(outputs(&index), 12);
    let report = index.book().report(&PriceTable::builtin(), NOW, 0);
    assert_eq!(report.sessions.len(), 1);
    assert_eq!(report.today.messages, 2);
}

#[test]
fn incremental_reads_wait_for_complete_lines() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("s.jsonl");
    let line = assistant("m1", "s", 1, 5);
    let (head, tail) = line.split_at(40);
    append(&file, head);
    let mut index = TranscriptIndex::open(dir.path(), None);
    assert_eq!(index.refresh(&never()).unwrap().messages, 0);
    append(&file, &format!("{tail}\n"));
    let stats = index.refresh(&never()).unwrap();
    assert_eq!(stats.messages, 1);
    assert_eq!(stats.bytes_read, line.len() as u64 + 1);
    // Nothing new: nothing read.
    assert_eq!(index.refresh(&never()).unwrap().files_read, 0);
    append(&file, &format!("{}\r\n", assistant("m2", "s", 2, 6)));
    assert_eq!(index.refresh(&never()).unwrap().messages, 1);
    assert_eq!(outputs(&index), 11);
}

#[test]
fn repeated_messages_count_once_with_the_fullest_numbers() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("s.jsonl");
    // Streamed blocks repeat a message; a resumed session copies it to a new file.
    append(
        &file,
        &format!(
            "{}\n{}\n",
            assistant("m1", "s", 1, 3),
            assistant("m1", "s", 1, 9)
        ),
    );
    append(
        &dir.path().join("t.jsonl"),
        &format!("{}\n", assistant("m1", "s", 1, 9)),
    );
    let mut index = TranscriptIndex::open(dir.path(), None);
    index.refresh(&never()).unwrap();
    assert_eq!(index.book().entries.len(), 1);
    assert_eq!(outputs(&index), 9);
}

#[test]
fn a_file_that_shrank_is_read_again_without_double_counting() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("s.jsonl");
    append(
        &file,
        &format!(
            "{}\n{}\n",
            assistant("m1", "s", 1, 5),
            assistant("m2", "s", 2, 5)
        ),
    );
    let mut index = TranscriptIndex::open(dir.path(), None);
    index.refresh(&never()).unwrap();
    std::fs::write(&file, format!("{}\n", assistant("m3", "s", 3, 1))).unwrap();
    index.refresh(&never()).unwrap();
    assert_eq!(index.book().entries.len(), 3);
    assert_eq!(outputs(&index), 11);
}

#[test]
fn the_cache_remembers_messages_and_positions_across_restarts() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("projects");
    std::fs::create_dir_all(&root).unwrap();
    let cache = dir.path().join("usage.db");
    let file = root.join("s.jsonl");
    append(&file, &format!("{}\n", assistant("m1", "s", 1, 5)));
    let mut index = TranscriptIndex::open(&root, Some(&cache));
    index.refresh(&never()).unwrap();
    drop(index);

    append(&file, &format!("{}\n", assistant("m2", "s", 2, 6)));
    let mut index = TranscriptIndex::open(&root, Some(&cache));
    assert_eq!(index.cache_error, None);
    assert_eq!(outputs(&index), 5);
    let stats = index.refresh(&never()).unwrap();
    // Only the new line was read.
    assert_eq!(stats.messages, 1);
    assert_eq!(
        stats.bytes_read,
        assistant("m2", "s", 2, 6).len() as u64 + 1
    );
    assert_eq!(outputs(&index), 11);
}

#[test]
fn missing_root_is_an_error_and_stop_stops() {
    let dir = tempfile::tempdir().unwrap();
    let mut index = TranscriptIndex::open(&dir.path().join("nope"), None);
    assert!(index.refresh(&never()).is_err());
    let file = dir.path().join("s.jsonl");
    append(&file, &format!("{}\n", assistant("m1", "s", 1, 5)));
    let mut index = TranscriptIndex::open(dir.path(), None);
    let stats = index.refresh(&AtomicBool::new(true)).unwrap();
    assert_eq!(stats.messages, 0);
    // Unread files are tried again next time.
    assert_eq!(index.refresh(&never()).unwrap().messages, 1);
}

/// Indexes the real `~/.claude/projects` and prints sizes and timings (only
/// counts, never content). `cargo test --release -- --ignored real_transcripts`
#[test]
#[ignore = "reads the real transcripts of this machine"]
fn real_transcripts_timing() {
    let Some(home) = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }) else {
        return;
    };
    let root = Path::new(&home).join(".claude").join("projects");
    let dir = tempfile::tempdir().unwrap();
    let cache = dir.path().join("usage.db");
    let started = Instant::now();
    let mut index = TranscriptIndex::open(&root, Some(&cache));
    let first = index.refresh(&never()).unwrap();
    let cold = started.elapsed();
    let report_started = Instant::now();
    let report = index.book().report(&PriceTable::builtin(), NOW, 0);
    let report_ms = report_started.elapsed().as_millis();
    let second = index.refresh(&never()).unwrap();
    drop(index);
    let started = Instant::now();
    let mut index = TranscriptIndex::open(&root, Some(&cache));
    let loaded = started.elapsed();
    let third = index.refresh(&never()).unwrap();
    let walk_started = Instant::now();
    let walked = list_jsonl(&root).unwrap().len();
    eprintln!("walk {walked} files {:?}", walk_started.elapsed());
    eprintln!(
        "files {} bytes {} messages {} sessions {}\ncold index {:?}\nreport {} ms\nwarm refresh {} ms\nrestart load {:?} + refresh {} ms (read {} bytes)",
        first.files,
        first.bytes_read,
        first.messages,
        report.sessions_total,
        cold,
        report_ms,
        second.took_ms,
        loaded,
        third.took_ms,
        third.bytes_read
    );
}
