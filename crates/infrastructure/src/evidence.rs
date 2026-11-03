//! Evidence bundles as files: each sealed bundle is also written into its
//! repository (`.swarmux/evidence/<slug>.json`), where it can be read,
//! committed and handed to `swarmux verify`.

use std::fs;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use swarmux_application::{AppError, AppResult, EvidenceStore, Stored};
use swarmux_domain::Millis;
use swarmux_domain::evidence::{Bundle, Label};

/// Where a bundle's file is written in its repository.
pub const EVIDENCE_DIR: &str = ".swarmux/evidence";

/// Where the file of `slug` lives under `root`.
pub fn bundle_path(root: &Path, slug: &str) -> PathBuf {
    root.join(EVIDENCE_DIR).join(format!("{slug}.json"))
}

/// Reads a bundle file.
pub fn read_bundle(path: &Path) -> AppResult<Bundle> {
    let text = fs::read_to_string(path)
        .map_err(|e| AppError::Storage(format!("cannot read {}: {e}", path.display())))?;
    Bundle::from_json(&text)
        .map_err(|e| AppError::Storage(format!("{} is not a bundle: {e}", path.display())))
}

/// Keeps the bundle in the database and writes its file.
pub struct EvidenceFiles {
    inner: Rc<dyn EvidenceStore>,
}

impl EvidenceFiles {
    pub fn new(inner: Rc<dyn EvidenceStore>) -> EvidenceFiles {
        EvidenceFiles { inner }
    }
}

impl EvidenceStore for EvidenceFiles {
    fn save(&self, bundle: &Bundle, seal_seq: Option<u64>, now: Millis) -> AppResult<()> {
        self.inner.save(bundle, seal_seq, now)?;
        // The database holds the authoritative copy; a folder that is gone
        // or read-only only loses the convenience file.
        let root = Path::new(&bundle.contract.root);
        if root.is_dir() {
            let path = bundle_path(root, &bundle.contract.slug);
            let _ = path
                .parent()
                .map(fs::create_dir_all)
                .transpose()
                .and_then(|_| fs::write(&path, bundle.to_json()));
        }
        Ok(())
    }

    fn get(&self, slug: &str) -> AppResult<Option<Stored>> {
        self.inner.get(slug)
    }

    fn list(&self) -> AppResult<Vec<Stored>> {
        self.inner.list()
    }

    fn set_labels(&self, slug: &str, labels: &[Label]) -> AppResult<()> {
        self.inner.set_labels(slug, labels)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SqliteStore;
    use swarmux_domain::evidence::{BUNDLE_SCHEMA, Signature};

    #[test]
    fn a_sealed_bundle_is_written_and_read_back() {
        let dir = tempfile::tempdir().unwrap();
        let store = Rc::new(SqliteStore::open_in_memory().unwrap());
        let files = EvidenceFiles::new(store.clone());
        let mut b = Bundle {
            schema: BUNDLE_SCHEMA,
            head_sha: "abc".into(),
            ..Bundle::default()
        };
        b.contract.slug = "fast".into();
        b.contract.root = dir.path().display().to_string();
        b.signature = Some(Signature {
            alg: "sha256".into(),
            value: b.digest_hex(),
        });
        files.save(&b, Some(4), 1).unwrap();
        let back = read_bundle(&bundle_path(dir.path(), "fast")).unwrap();
        assert_eq!(back, b);
        assert_eq!(files.get("fast").unwrap().unwrap().bundle, b);
        // A root that is not a folder only keeps the database copy.
        b.contract.root = "/no/such/folder".into();
        files.save(&b, None, 2).unwrap();
        assert!(read_bundle(&dir.path().join("nope.json")).is_err());
    }
}
