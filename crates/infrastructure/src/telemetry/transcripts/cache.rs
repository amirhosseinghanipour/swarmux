//! `~/.swarmux/usage.db`: what was already read from each transcript and the
//! numbers found, so a restart reads only new lines. It is a cache: a
//! version mismatch or a broken file just starts it over.

use std::collections::HashMap;
use std::path::Path;

use rusqlite::{Connection, params};
use swarmux_domain::Millis;
use swarmux_domain::telemetry::TokenUsage;

use super::line::UsageLine;

const VERSION: i64 = 1;

const SCHEMA: &str = "
CREATE TABLE files (
    path TEXT PRIMARY KEY,
    offset INTEGER NOT NULL,
    size INTEGER NOT NULL,
    mtime INTEGER NOT NULL
);
CREATE TABLE names (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL UNIQUE
);
CREATE TABLE entries (
    key INTEGER PRIMARY KEY,
    ts INTEGER NOT NULL,
    session INTEGER NOT NULL,
    project INTEGER NOT NULL,
    model INTEGER NOT NULL,
    sidechain INTEGER NOT NULL,
    input INTEGER NOT NULL,
    output INTEGER NOT NULL,
    cache_read INTEGER NOT NULL,
    cache_5m INTEGER NOT NULL,
    cache_1h INTEGER NOT NULL
);
";

/// How far a transcript was read.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FileMark {
    /// Bytes consumed (up to the last complete line).
    pub offset: u64,
    pub size: u64,
    pub mtime: i64,
}

pub struct UsageCache {
    conn: Connection,
    /// name → id of the `names` table.
    names: HashMap<String, i64>,
}

impl UsageCache {
    pub fn open(path: &Path) -> rusqlite::Result<UsageCache> {
        match UsageCache::try_open(path) {
            Ok(c) => Ok(c),
            Err(_) => {
                // Unreadable or foreign: rebuild from the transcripts.
                let _ = std::fs::remove_file(path);
                UsageCache::try_open(path)
            }
        }
    }

    pub fn open_in_memory() -> rusqlite::Result<UsageCache> {
        UsageCache::init(Connection::open_in_memory()?)
    }

    fn try_open(path: &Path) -> rusqlite::Result<UsageCache> {
        let conn = Connection::open(path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        UsageCache::init(conn)
    }

    fn init(conn: Connection) -> rusqlite::Result<UsageCache> {
        let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        if version != VERSION {
            conn.execute_batch(
                "DROP TABLE IF EXISTS files; DROP TABLE IF EXISTS names;
                 DROP TABLE IF EXISTS entries;",
            )?;
            conn.execute_batch(SCHEMA)?;
            conn.pragma_update(None, "user_version", VERSION)?;
        }
        let names = {
            let mut stmt = conn.prepare("SELECT name, id FROM names")?;
            let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
            rows.collect::<rusqlite::Result<HashMap<String, i64>>>()?
        };
        Ok(UsageCache { conn, names })
    }

    pub fn marks(&self) -> rusqlite::Result<HashMap<String, FileMark>> {
        let mut stmt = self
            .conn
            .prepare("SELECT path, offset, size, mtime FROM files")?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                FileMark {
                    offset: r.get::<_, i64>(1)? as u64,
                    size: r.get::<_, i64>(2)? as u64,
                    mtime: r.get(3)?,
                },
            ))
        })?;
        rows.collect()
    }

    /// Every stored message, in no particular order.
    pub fn entries(&self) -> rusqlite::Result<Vec<UsageLine>> {
        let by_id: HashMap<i64, &str> =
            self.names.iter().map(|(n, &id)| (id, n.as_str())).collect();
        let name = |id: i64| by_id.get(&id).copied().unwrap_or("").to_string();
        let mut stmt = self.conn.prepare(
            "SELECT key, ts, session, project, model, sidechain, input, output, cache_read,
                    cache_5m, cache_1h FROM entries",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(UsageLine {
                key: r.get::<_, i64>(0)? as u64,
                ts: r.get::<_, Millis>(1)?,
                session: name(r.get(2)?),
                cwd: name(r.get(3)?),
                model: name(r.get(4)?),
                sidechain: r.get(5)?,
                usage: TokenUsage {
                    input: r.get::<_, i64>(6)? as u64,
                    output: r.get::<_, i64>(7)? as u64,
                    cache_read: r.get::<_, i64>(8)? as u64,
                    cache_write_5m: r.get::<_, i64>(9)? as u64,
                    cache_write_1h: r.get::<_, i64>(10)? as u64,
                },
            })
        })?;
        rows.collect()
    }

    /// Stores new or grown messages and the new read positions, in one transaction.
    pub fn save(
        &mut self,
        lines: &[&UsageLine],
        marks: &[(String, FileMark)],
        gone: &[String],
    ) -> rusqlite::Result<()> {
        let tx = self.conn.transaction()?;
        {
            let mut add_name = tx.prepare_cached("INSERT INTO names (name) VALUES (?1)")?;
            let mut add = tx.prepare_cached(
                "INSERT INTO entries (key, ts, session, project, model, sidechain, input, output,
                                      cache_read, cache_5m, cache_1h)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
                 ON CONFLICT(key) DO UPDATE SET
                   input = excluded.input, output = excluded.output,
                   cache_read = excluded.cache_read, cache_5m = excluded.cache_5m,
                   cache_1h = excluded.cache_1h",
            )?;
            let mut id_of = |name: &str| -> rusqlite::Result<i64> {
                if let Some(&id) = self.names.get(name) {
                    return Ok(id);
                }
                add_name.execute([name])?;
                let id = tx.last_insert_rowid();
                self.names.insert(name.to_string(), id);
                Ok(id)
            };
            for l in lines {
                let ids = (id_of(&l.session)?, id_of(&l.cwd)?, id_of(&l.model)?);
                let u = &l.usage;
                add.execute(params![
                    l.key as i64,
                    l.ts,
                    ids.0,
                    ids.1,
                    ids.2,
                    l.sidechain,
                    u.input as i64,
                    u.output as i64,
                    u.cache_read as i64,
                    u.cache_write_5m as i64,
                    u.cache_write_1h as i64
                ])?;
            }
            let mut mark = tx.prepare_cached(
                "INSERT INTO files (path, offset, size, mtime) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(path) DO UPDATE SET
                   offset = excluded.offset, size = excluded.size, mtime = excluded.mtime",
            )?;
            for (path, m) in marks {
                mark.execute(params![path, m.offset as i64, m.size as i64, m.mtime])?;
            }
            let mut forget = tx.prepare_cached("DELETE FROM files WHERE path = ?1")?;
            for path in gone {
                forget.execute([path])?;
            }
        }
        tx.commit()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(key: u64, output: u64) -> UsageLine {
        UsageLine {
            key,
            ts: 1000 + key as i64,
            session: "s".into(),
            cwd: "/w".into(),
            model: "claude-opus-5-5".into(),
            sidechain: key.is_multiple_of(2),
            usage: TokenUsage {
                output,
                ..TokenUsage::default()
            },
        }
    }

    #[test]
    fn entries_and_marks_round_trip_through_a_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("usage.db");
        let mut c = UsageCache::open(&path).unwrap();
        let mark = FileMark {
            offset: 10,
            size: 12,
            mtime: 5,
        };
        c.save(
            &[&line(1, 5), &line(u64::MAX, 7)],
            &[("a.jsonl".into(), mark)],
            &[],
        )
        .unwrap();
        // The same message again with more output replaces the numbers.
        c.save(&[&line(1, 9)], &[], &[]).unwrap();
        drop(c);
        let c = UsageCache::open(&path).unwrap();
        let mut got = c.entries().unwrap();
        got.sort_by_key(|l| l.key);
        assert_eq!(got, [line(1, 9), line(u64::MAX, 7)]);
        assert_eq!(c.marks().unwrap()["a.jsonl"], mark);
    }

    #[test]
    fn gone_files_are_forgotten_and_garbage_is_rebuilt() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("usage.db");
        let mut c = UsageCache::open(&path).unwrap();
        c.save(&[], &[("a".into(), FileMark::default())], &[])
            .unwrap();
        c.save(&[], &[], &["a".into()]).unwrap();
        assert!(c.marks().unwrap().is_empty());
        drop(c);
        std::fs::write(&path, b"not a database").unwrap();
        let c = UsageCache::open(&path).unwrap();
        assert!(c.entries().unwrap().is_empty());
    }
}
