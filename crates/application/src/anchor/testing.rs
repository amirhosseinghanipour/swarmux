//! Fakes for tests of anchoring.

use std::cell::RefCell;

use swarmux_domain::anchor::Checkpoint;
use swarmux_domain::ledger::{hex, sha256};

use super::{AnchorStore, CheckpointSigner, KeyProtection};
use crate::AppResult;

/// Checkpoints kept in memory.
#[derive(Default)]
pub struct MemoryAnchors {
    rows: RefCell<Vec<Checkpoint>>,
}

impl MemoryAnchors {
    /// Replaces what is kept (a checkpoint file edited by hand).
    pub fn replace(&self, rows: Vec<Checkpoint>) {
        *self.rows.borrow_mut() = rows;
    }
}

impl AnchorStore for MemoryAnchors {
    fn save(&self, checkpoint: &Checkpoint) -> AppResult<()> {
        self.rows.borrow_mut().push(checkpoint.clone());
        Ok(())
    }

    fn list(&self) -> AppResult<Vec<Checkpoint>> {
        Ok(self.rows.borrow().clone())
    }
}

/// A signer whose "signature" is a hash of the key and the message: enough
/// to tell a checkpoint that was edited from one that was not.
pub struct HashingSigner(pub String);

impl HashingSigner {
    fn mac(key: &str, message: &[u8]) -> String {
        let mut bytes = key.as_bytes().to_vec();
        bytes.extend_from_slice(message);
        hex(&sha256(&bytes))
    }
}

impl CheckpointSigner for HashingSigner {
    fn public_key(&self) -> AppResult<String> {
        Ok(self.0.clone())
    }

    fn sign(&self, message: &[u8]) -> AppResult<String> {
        Ok(HashingSigner::mac(&self.0, message))
    }

    fn verify(&self, public_key: &str, message: &[u8], signature: &str) -> bool {
        HashingSigner::mac(public_key, message) == signature
    }

    fn protection(&self) -> KeyProtection {
        KeyProtection::Memory
    }
}
