//! Evidence bundles in the database: the bundle JSON as sealed, its
//! digest, the ledger entry that sealed it and the labels earned since.

use rusqlite::{Row, params};
use serde_json::{Value, json};
use swarmux_application::{AppResult, EvidenceStore, Stored};
use swarmux_domain::Millis;
use swarmux_domain::evidence::{Bundle, Label, LabelKind};

use super::{OrStorage, SqliteStore};

const COLS: &str = "bundle, digest, seal_seq, labels";

fn labels_to_json(labels: &[Label]) -> String {
    Value::Array(
        labels
            .iter()
            .map(|l| json!({"kind": l.kind.as_str(), "pr": l.pr, "at": l.at}))
            .collect(),
    )
    .to_string()
}

fn labels_from_json(text: &str) -> Vec<Label> {
    let Ok(Value::Array(items)) = serde_json::from_str::<Value>(text) else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|v| {
            Some(Label {
                kind: LabelKind::parse(v.get("kind")?.as_str()?)?,
                pr: v.get("pr")?.as_u64()?,
                at: v.get("at")?.as_i64()?,
            })
        })
        .collect()
}

fn from_row(r: &Row) -> rusqlite::Result<Stored> {
    let text: String = r.get(0)?;
    let bundle = Bundle::from_json(&text).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, e.into())
    })?;
    let labels: String = r.get(3)?;
    Ok(Stored {
        bundle,
        digest: r.get(1)?,
        seal_seq: r
            .get::<_, Option<i64>>(2)?
            .and_then(|n| u64::try_from(n).ok()),
        labels: labels_from_json(&labels),
    })
}

impl EvidenceStore for SqliteStore {
    fn save(&self, b: &Bundle, seal_seq: Option<u64>, now: Millis) -> AppResult<()> {
        self.conn
            .execute(
                "INSERT INTO evidence (slug, job_id, digest, seal_seq, bundle, labels, sealed_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, '[]', ?6)
                 ON CONFLICT(slug) DO UPDATE SET
                    job_id = excluded.job_id,
                    digest = excluded.digest,
                    seal_seq = excluded.seal_seq,
                    bundle = excluded.bundle,
                    sealed_at = excluded.sealed_at",
                params![
                    b.contract.slug,
                    b.job,
                    b.digest_hex(),
                    seal_seq.and_then(|n| i64::try_from(n).ok()),
                    b.to_json(),
                    now,
                ],
            )
            .or_storage()?;
        Ok(())
    }

    fn get(&self, slug: &str) -> AppResult<Option<Stored>> {
        let mut stmt = self
            .conn
            .prepare(&format!("SELECT {COLS} FROM evidence WHERE slug = ?1"))
            .or_storage()?;
        let mut rows = stmt.query_map([slug], from_row).or_storage()?;
        rows.next().transpose().or_storage()
    }

    fn list(&self) -> AppResult<Vec<Stored>> {
        let mut stmt = self
            .conn
            .prepare(&format!(
                "SELECT {COLS} FROM evidence ORDER BY sealed_at, slug"
            ))
            .or_storage()?;
        let rows = stmt.query_map([], from_row).or_storage()?;
        rows.collect::<rusqlite::Result<Vec<_>>>().or_storage()
    }

    fn set_labels(&self, slug: &str, labels: &[Label]) -> AppResult<()> {
        self.conn
            .execute(
                "UPDATE evidence SET labels = ?2 WHERE slug = ?1",
                params![slug, labels_to_json(labels)],
            )
            .or_storage()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use swarmux_domain::evidence::{BUNDLE_SCHEMA, Signature};

    fn bundle(slug: &str, head: &str) -> Bundle {
        let mut b = Bundle {
            schema: BUNDLE_SCHEMA,
            head_sha: head.into(),
            job: 3,
            ..Bundle::default()
        };
        b.contract.slug = slug.into();
        b.signature = Some(Signature {
            alg: "sha256".into(),
            value: b.digest_hex(),
        });
        b
    }

    #[test]
    fn bundles_labels_and_replacement() {
        let s = SqliteStore::open_in_memory().unwrap();
        assert!(s.get("a").unwrap().is_none());
        s.save(&bundle("b", "h2"), Some(9), 20).unwrap();
        s.save(&bundle("a", "h1"), None, 10).unwrap();
        let all = s.list().unwrap();
        assert_eq!(all[0].bundle.contract.slug, "a", "oldest first");
        assert_eq!(all[1].seal_seq, Some(9));
        assert_eq!(all[0].digest, all[0].bundle.digest_hex());

        let labels = [
            Label {
                kind: LabelKind::PrOpen,
                pr: 4,
                at: 5,
            },
            Label {
                kind: LabelKind::Merged,
                pr: 4,
                at: 6,
            },
        ];
        s.set_labels("a", &labels).unwrap();
        // Sealing again keeps the labels and swaps the bundle.
        s.save(&bundle("a", "h3"), Some(11), 30).unwrap();
        let a = s.get("a").unwrap().unwrap();
        assert_eq!(a.bundle.head_sha, "h3");
        assert_eq!(a.labels, labels);
        assert_eq!(a.seal_seq, Some(11));
        assert_eq!(a.bundle.signature_ok(), Ok(true));
    }
}
