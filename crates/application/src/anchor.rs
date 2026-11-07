//! Ledger anchoring: signed checkpoints of the audit ledger's head, and the
//! audit that compares the ledger with every checkpoint ever made.
//!
//! A checkpoint is taken on request (`swarmux audit checkpoint`) and
//! whenever a mission is delivered, just before its evidence bundle is
//! sealed, so the bundle carries the checkpoint that covers its entries.
//! The ports live here: [`AnchorStore`] keeps the checkpoints,
//! [`CheckpointSigner`] holds the key and signs.

use std::rc::Rc;

use swarmux_domain::anchor::{Checkpoint, Standing, stand};
use swarmux_domain::ledger::{ChainBreak, Hash};

use crate::ledger::{Ledger, verify_all};
use crate::ports::Clock;
use crate::{AppError, AppResult};

/// Where the signing key is kept.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyProtection {
    /// The OS keychain: another program running as the same user can ask
    /// for it, but it is not a file lying next to the database.
    Keychain,
    /// A file in swarmux's folder: anything running as the same user,
    /// agents included, can read it. Better than nothing; exported
    /// checkpoints kept elsewhere are what make up for it.
    File,
    /// A key given for a test.
    Memory,
}

impl KeyProtection {
    pub fn text(self) -> &'static str {
        match self {
            KeyProtection::Keychain => "OS keychain",
            KeyProtection::File => "a file in ~/.swarmux (readable by anything running as you)",
            KeyProtection::Memory => "memory",
        }
    }
}

/// Holds the signing key and signs with it.
pub trait CheckpointSigner {
    /// The public key as text; makes the key pair on first use.
    fn public_key(&self) -> AppResult<String>;
    /// Signs `message`; the signature as text.
    fn sign(&self, message: &[u8]) -> AppResult<String>;
    /// Whether `signature` is `public_key`'s signature of `message`.
    fn verify(&self, public_key: &str, message: &[u8], signature: &str) -> bool;
    /// Where the private key lives.
    fn protection(&self) -> KeyProtection;
}

/// Where checkpoints are kept: append only.
pub trait AnchorStore {
    fn save(&self, checkpoint: &Checkpoint) -> AppResult<()>;
    /// Every checkpoint, oldest first.
    fn list(&self) -> AppResult<Vec<Checkpoint>>;
}

/// How one checkpoint fares.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Judged {
    pub checkpoint: Checkpoint,
    /// The signature is valid for the checkpoint's own key.
    pub signed: bool,
    /// The key is the one this machine signs with.
    pub own_key: bool,
    pub standing: Standing,
}

impl Judged {
    pub fn ok(&self) -> bool {
        self.signed && self.standing == Standing::Intact
    }

    pub fn problem(&self) -> Option<String> {
        let c = &self.checkpoint;
        if !self.signed {
            return Some(format!(
                "checkpoint #{}: the signature does not match",
                c.seq
            ));
        }
        match self.standing {
            Standing::Intact => None,
            Standing::Truncated { ledger_head } => Some(format!(
                "ledger cut short: checkpoint #{} exists, the ledger ends at {}",
                c.seq,
                ledger_head.map_or_else(|| "nothing".to_string(), |h| format!("#{h}"))
            )),
            Standing::Rewritten => Some(format!(
                "ledger rewritten: entry #{} no longer has the hash the checkpoint signed",
                c.seq
            )),
        }
    }
}

/// What an audit found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuditReport {
    pub entries: u64,
    pub head: Option<(u64, Hash)>,
    pub chain: Option<ChainBreak>,
    pub checkpoints: Vec<Judged>,
    /// Entries after the newest checkpoint: not anchored yet.
    pub unanchored: u64,
}

impl AuditReport {
    /// The chain is whole and every checkpoint holds.
    pub fn is_ok(&self) -> bool {
        self.chain.is_none() && self.checkpoints.iter().all(Judged::ok)
    }

    pub fn problems(&self) -> Vec<String> {
        let mut out = Vec::new();
        if let Some(b) = &self.chain {
            out.push(format!("chain broken at #{}: {}", b.seq, b.reason));
        }
        out.extend(self.checkpoints.iter().filter_map(Judged::problem));
        out
    }
}

/// Takes, keeps and checks checkpoints.
pub struct AnchorService {
    ledger: Rc<dyn Ledger>,
    store: Rc<dyn AnchorStore>,
    signer: Rc<dyn CheckpointSigner>,
    clock: Rc<dyn Clock>,
}

impl AnchorService {
    pub fn new(
        ledger: Rc<dyn Ledger>,
        store: Rc<dyn AnchorStore>,
        signer: Rc<dyn CheckpointSigner>,
        clock: Rc<dyn Clock>,
    ) -> AnchorService {
        AnchorService {
            ledger,
            store,
            signer,
            clock,
        }
    }

    /// Signs the ledger's head as it is now and keeps the checkpoint. An
    /// error on an empty ledger: there is nothing to anchor.
    pub fn checkpoint(&self) -> AppResult<Checkpoint> {
        let head = self
            .ledger
            .head()?
            .ok_or_else(|| AppError::Storage("the ledger is empty: nothing to anchor".into()))?;
        let at = self.clock.now();
        let message = Checkpoint::message(head.seq, &head.hash, at);
        let checkpoint = Checkpoint {
            seq: head.seq,
            head: head.hash,
            at,
            key: self.signer.public_key()?,
            signature: self.signer.sign(&message)?,
        };
        self.store.save(&checkpoint)?;
        Ok(checkpoint)
    }

    /// Every checkpoint kept, oldest first.
    pub fn list(&self) -> AppResult<Vec<Checkpoint>> {
        self.store.list()
    }

    /// The newest checkpoint that covers entry `seq`, if there is one.
    pub fn covering(&self, seq: u64) -> AppResult<Option<Checkpoint>> {
        Ok(self
            .store
            .list()?
            .into_iter()
            .filter(|c| c.seq >= seq)
            .max_by_key(|c| c.seq))
    }

    /// The signer's public key and where the private key is kept.
    pub fn key(&self) -> AppResult<(String, KeyProtection)> {
        Ok((self.signer.public_key()?, self.signer.protection()))
    }

    /// How the ledger stands against `checkpoint`: its signature, and the
    /// entry at its position.
    pub fn judge(&self, checkpoint: Checkpoint, ledger_head: Option<u64>) -> AppResult<Judged> {
        let c = checkpoint;
        let hash_at = self
            .ledger
            .range(c.seq, 1)?
            .into_iter()
            .next()
            .and_then(|e| (e.seq == c.seq).then_some(e.hash));
        let own = self.signer.public_key().ok();
        Ok(Judged {
            signed: self
                .signer
                .verify(&c.key, &c.signed_message(), &c.signature),
            own_key: own.as_deref() == Some(c.key.as_str()),
            standing: stand(&c, ledger_head, hash_at),
            checkpoint: c,
        })
    }

    /// [`AnchorService::judge`] against the ledger's current head.
    pub fn check(&self, checkpoint: &Checkpoint) -> AppResult<Judged> {
        let head = self.ledger.head()?.map(|e| e.seq);
        self.judge(checkpoint.clone(), head)
    }

    /// Walks the chain and compares the ledger with every kept checkpoint
    /// and with `extra` ones (from an export kept elsewhere).
    pub fn audit(&self, extra: &[Checkpoint]) -> AppResult<AuditReport> {
        let chain = verify_all(&*self.ledger)?;
        let mut all = self.store.list()?;
        for c in extra {
            if !all.contains(c) {
                all.push(c.clone());
            }
        }
        all.sort_by_key(|c| (c.seq, c.at));
        let head_seq = chain.head.map(|(s, _)| s);
        let mut judged = Vec::new();
        for c in all {
            judged.push(self.judge(c, head_seq)?);
        }
        let newest = judged.iter().map(|j| j.checkpoint.seq).max().unwrap_or(0);
        Ok(AuditReport {
            entries: chain.entries,
            unanchored: head_seq.map_or(0, |h| h.saturating_sub(newest)),
            head: chain.head,
            chain: chain.broken,
            checkpoints: judged,
        })
    }
}

#[cfg(any(test, feature = "test-support"))]
pub mod testing;
#[cfg(test)]
mod tests;
