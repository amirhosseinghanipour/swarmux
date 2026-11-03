//! The ledger: an append-only, hash-chained log of everything swarmux
//! observed or decided.
//!
//! Every entry commits to the one before it, so editing, deleting or
//! reordering stored entries breaks the chain and [`verify`] names the first
//! entry where it breaks. The chain starts with a genesis entry whose
//! previous hash is all zeros.
//!
//! # Hash
//!
//! `hash = SHA-256(prev_hash ‖ canonical_bytes(seq, draft))`, where
//! `canonical_bytes` is, in this order:
//!
//! | field     | encoding                                            |
//! |-----------|-----------------------------------------------------|
//! | `seq`     | u64, big-endian                                     |
//! | `ts`      | i64, big-endian (Unix milliseconds)                 |
//! | `kind`    | u32 big-endian byte length, then UTF-8 ([`LedgerKind::as_str`]) |
//! | `actor`   | u32 length, then UTF-8                              |
//! | `job`     | u32 length, then the decimal job id as UTF-8 (`""` if none) |
//! | `session` | u32 length, then UTF-8 (`""` if none)               |
//! | `payload` | u32 length, then the payload JSON text, byte for byte |
//!
//! The payload is hashed as stored, never re-serialized, so any JSON text
//! (key order, spacing) verifies as long as it is unchanged. This format is
//! frozen: changing it invalidates every existing ledger.

use std::fmt;

use sha2::{Digest, Sha256};

use crate::json::quote;
use crate::{JobId, Millis};

/// A SHA-256 digest.
pub type Hash = [u8; 32];

/// The previous hash of the genesis entry.
pub const GENESIS_PREV: Hash = [0; 32];

/// Longest text field kept in a payload, in bytes. Longer text is cut and
/// its full SHA-256 and length are kept next to it.
pub const PAYLOAD_TEXT_MAX: usize = 8192;

/// What an entry records.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LedgerKind {
    /// The first entry; its payload names the swarmux version.
    Genesis,
    JobQueued,
    JobStarted,
    /// Something a running agent reported (stream-json).
    AgentEvent,
    /// A guard, loop-breaker, JEV or routing decision.
    GuardDecision,
    ReviewVerdict,
    JobFinished,
    /// The user acknowledged an attention item.
    Acknowledged,
    /// A label attached later to an earlier entry.
    Label,
    /// The done-gate judged a stop, or its checks were run by hand.
    GateVerdict,
}

impl LedgerKind {
    pub const ALL: [LedgerKind; 10] = [
        LedgerKind::Genesis,
        LedgerKind::JobQueued,
        LedgerKind::JobStarted,
        LedgerKind::AgentEvent,
        LedgerKind::GuardDecision,
        LedgerKind::ReviewVerdict,
        LedgerKind::JobFinished,
        LedgerKind::Acknowledged,
        LedgerKind::Label,
        LedgerKind::GateVerdict,
    ];

    /// The stable name stored and hashed.
    pub fn as_str(self) -> &'static str {
        match self {
            LedgerKind::Genesis => "genesis",
            LedgerKind::JobQueued => "job_queued",
            LedgerKind::JobStarted => "job_started",
            LedgerKind::AgentEvent => "agent_event",
            LedgerKind::GuardDecision => "guard_decision",
            LedgerKind::ReviewVerdict => "review_verdict",
            LedgerKind::JobFinished => "job_finished",
            LedgerKind::Acknowledged => "acknowledged",
            LedgerKind::Label => "label",
            LedgerKind::GateVerdict => "gate_verdict",
        }
    }

    pub fn parse(s: &str) -> Option<LedgerKind> {
        LedgerKind::ALL.into_iter().find(|k| k.as_str() == s)
    }
}

impl fmt::Display for LedgerKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// An entry before the ledger numbers and chains it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LedgerDraft {
    pub ts: Millis,
    pub kind: LedgerKind,
    /// Who did or reported it: "swarmux", "guard", "lead", a subagent id, …
    pub actor: String,
    pub job: Option<JobId>,
    /// The Claude Code session id. `Some("")` hashes like `None`.
    pub session: Option<String>,
    /// One JSON value, usually an object.
    pub payload_json: String,
}

impl LedgerDraft {
    /// The first entry of every ledger.
    pub fn genesis(ts: Millis, version: &str) -> LedgerDraft {
        LedgerDraft {
            ts,
            kind: LedgerKind::Genesis,
            actor: "swarmux".into(),
            job: None,
            session: None,
            payload_json: Payload::new().str("version", version).build(),
        }
    }
}

/// A stored, chained entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LedgerEntry {
    /// 1 for genesis, then +1 per entry with no gaps.
    pub seq: u64,
    pub prev_hash: Hash,
    pub hash: Hash,
    pub draft: LedgerDraft,
}

impl LedgerEntry {
    /// Chains `draft` after `head` (or starts the chain when there is none).
    pub fn next(head: Option<&LedgerEntry>, draft: LedgerDraft) -> LedgerEntry {
        let (seq, prev_hash) = head.map_or((1, GENESIS_PREV), |h| (h.seq + 1, h.hash));
        LedgerEntry {
            seq,
            prev_hash,
            hash: chain_hash(&prev_hash, seq, &draft),
            draft,
        }
    }
}

fn put_field(out: &mut Vec<u8>, bytes: &[u8]) {
    // SQLite caps text far below 4 GiB, so the length always fits.
    let len = u32::try_from(bytes.len()).unwrap_or(u32::MAX);
    out.extend_from_slice(&len.to_be_bytes());
    out.extend_from_slice(bytes);
}

/// The bytes hashed for an entry; see the module docs for the format.
pub fn canonical_bytes(seq: u64, draft: &LedgerDraft) -> Vec<u8> {
    let job = draft.job.map(|j| j.0.to_string()).unwrap_or_default();
    let session = draft.session.as_deref().unwrap_or("");
    let mut out = Vec::with_capacity(
        16 + 4 * 5
            + draft.kind.as_str().len()
            + draft.actor.len()
            + job.len()
            + session.len()
            + draft.payload_json.len(),
    );
    out.extend_from_slice(&seq.to_be_bytes());
    out.extend_from_slice(&draft.ts.to_be_bytes());
    put_field(&mut out, draft.kind.as_str().as_bytes());
    put_field(&mut out, draft.actor.as_bytes());
    put_field(&mut out, job.as_bytes());
    put_field(&mut out, session.as_bytes());
    put_field(&mut out, draft.payload_json.as_bytes());
    out
}

/// `SHA-256(prev ‖ canonical_bytes(seq, draft))`.
pub fn chain_hash(prev: &Hash, seq: u64, draft: &LedgerDraft) -> Hash {
    let mut h = Sha256::new();
    h.update(prev);
    h.update(canonical_bytes(seq, draft));
    h.finalize().into()
}

/// SHA-256 of arbitrary bytes.
pub fn sha256(bytes: &[u8]) -> Hash {
    Sha256::digest(bytes).into()
}

/// Lowercase hex of a hash.
pub fn hex(hash: &Hash) -> String {
    hash.iter().map(|b| format!("{b:02x}")).collect()
}

/// Why the chain is broken at an entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BreakReason {
    /// The first entry isn't a genesis entry chained to zeros, or a genesis
    /// entry appears later.
    BadGenesis,
    /// Entries are missing before this one.
    Gap { expected: u64 },
    /// The stored previous hash isn't the previous entry's hash.
    PrevMismatch,
    /// The entry's content doesn't hash to its stored hash: it was edited.
    HashMismatch,
}

impl fmt::Display for BreakReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BreakReason::BadGenesis => f.write_str("not a valid genesis entry"),
            BreakReason::Gap { expected } => write!(f, "missing entries from #{expected}"),
            BreakReason::PrevMismatch => f.write_str("previous hash doesn't match"),
            BreakReason::HashMismatch => f.write_str("content doesn't match its hash"),
        }
    }
}

/// The first entry where the chain breaks.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("ledger chain broken at #{seq}: {reason}")]
pub struct ChainBreak {
    pub seq: u64,
    pub reason: BreakReason,
}

/// Checks entries one at a time, in `seq` order, so long ledgers can be
/// verified page by page.
#[derive(Clone, Debug)]
pub struct ChainVerifier {
    next_seq: u64,
    prev: Hash,
}

impl Default for ChainVerifier {
    fn default() -> ChainVerifier {
        ChainVerifier {
            next_seq: 1,
            prev: GENESIS_PREV,
        }
    }
}

impl ChainVerifier {
    /// Entries checked so far.
    pub fn checked(&self) -> u64 {
        self.next_seq - 1
    }

    /// The hash of the last checked entry.
    pub fn head(&self) -> Option<Hash> {
        (self.next_seq > 1).then_some(self.prev)
    }

    /// Checks the next entry. After an error the verifier is unchanged.
    pub fn push(&mut self, e: &LedgerEntry) -> Result<(), ChainBreak> {
        let fail = |reason| Err(ChainBreak { seq: e.seq, reason });
        if e.seq != self.next_seq {
            return fail(if self.next_seq == 1 {
                BreakReason::BadGenesis
            } else {
                BreakReason::Gap {
                    expected: self.next_seq,
                }
            });
        }
        let genesis = e.draft.kind == LedgerKind::Genesis;
        if genesis != (e.seq == 1) {
            return fail(BreakReason::BadGenesis);
        }
        if e.prev_hash != self.prev {
            return fail(BreakReason::PrevMismatch);
        }
        let hash = chain_hash(&self.prev, e.seq, &e.draft);
        if hash != e.hash {
            return fail(BreakReason::HashMismatch);
        }
        self.prev = hash;
        self.next_seq += 1;
        Ok(())
    }
}

/// Verifies a whole ledger given in `seq` order.
pub fn verify(entries: &[LedgerEntry]) -> Result<(), ChainBreak> {
    let mut v = ChainVerifier::default();
    entries.iter().try_for_each(|e| v.push(e))
}

/// Builds a flat JSON object for a payload. Keys keep insertion order.
#[derive(Clone, Debug, Default)]
pub struct Payload {
    fields: Vec<String>,
}

impl Payload {
    pub fn new() -> Payload {
        Payload::default()
    }

    fn raw(mut self, key: &str, json: String) -> Payload {
        self.fields.push(format!("{}:{json}", quote(key)));
        self
    }

    pub fn str(self, key: &str, value: &str) -> Payload {
        self.raw(key, quote(value))
    }

    /// Like [`Payload::str`], but cut at [`PAYLOAD_TEXT_MAX`] bytes; a cut
    /// value also gets `<key>_sha256` and `<key>_len` of the full text.
    pub fn text(self, key: &str, value: &str) -> Payload {
        if value.len() <= PAYLOAD_TEXT_MAX {
            return self.str(key, value);
        }
        let mut end = PAYLOAD_TEXT_MAX;
        while !value.is_char_boundary(end) {
            end -= 1;
        }
        self.str(key, &value[..end])
            .str(&format!("{key}_sha256"), &hex(&sha256(value.as_bytes())))
            .int(&format!("{key}_len"), value.len() as i64)
    }

    pub fn opt_str(self, key: &str, value: Option<&str>) -> Payload {
        match value {
            Some(v) => self.str(key, v),
            None => self.raw(key, "null".into()),
        }
    }

    pub fn int(self, key: &str, value: i64) -> Payload {
        self.raw(key, value.to_string())
    }

    pub fn opt_int(self, key: &str, value: Option<i64>) -> Payload {
        self.raw(key, value.map_or("null".into(), |v| v.to_string()))
    }

    /// Non-finite numbers are written as `null`.
    pub fn num(self, key: &str, value: f64) -> Payload {
        let json = if value.is_finite() {
            value.to_string()
        } else {
            "null".into()
        };
        self.raw(key, json)
    }

    pub fn opt_num(self, key: &str, value: Option<f64>) -> Payload {
        match value {
            Some(v) => self.num(key, v),
            None => self.raw(key, "null".into()),
        }
    }

    pub fn bool(self, key: &str, value: bool) -> Payload {
        self.raw(key, value.to_string())
    }

    /// Embeds `json`, which must already be one valid JSON value.
    pub fn json(self, key: &str, json: &str) -> Payload {
        self.raw(key, json.to_string())
    }

    /// A JSON array of strings, each cut like [`Payload::text`] would.
    pub fn strs(self, key: &str, values: &[String]) -> Payload {
        let items: Vec<String> = values
            .iter()
            .map(|v| {
                let mut end = v.len().min(PAYLOAD_TEXT_MAX);
                while !v.is_char_boundary(end) {
                    end -= 1;
                }
                quote(&v[..end])
            })
            .collect();
        self.raw(key, format!("[{}]", items.join(",")))
    }

    /// A JSON array of objects.
    pub fn objects(self, key: &str, items: Vec<Payload>) -> Payload {
        let items: Vec<String> = items.into_iter().map(Payload::build).collect();
        self.raw(key, format!("[{}]", items.join(",")))
    }

    pub fn build(self) -> String {
        format!("{{{}}}", self.fields.join(","))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::json::Json;

    fn draft(kind: LedgerKind, payload: &str) -> LedgerDraft {
        LedgerDraft {
            ts: 1_700_000_000_000,
            kind,
            actor: "lead".into(),
            job: Some(JobId(7)),
            session: Some("00000000-0000-4000-8000-000000000001".into()),
            payload_json: payload.into(),
        }
    }

    fn chain(n: usize) -> Vec<LedgerEntry> {
        let mut out: Vec<LedgerEntry> = vec![LedgerEntry::next(
            None,
            LedgerDraft::genesis(1_700_000_000_000, "0.1.0"),
        )];
        for i in 1..n {
            let d = draft(LedgerKind::AgentEvent, &format!(r#"{{"i":{i}}}"#));
            out.push(LedgerEntry::next(out.last(), d));
        }
        out
    }

    #[test]
    fn payload_arrays() {
        let p = Payload::new()
            .strs("gaps", &["a\"b".into(), String::new()])
            .objects("runs", vec![Payload::new().int("exit", 1), Payload::new()])
            .build();
        assert_eq!(p, r#"{"gaps":["a\"b",""],"runs":[{"exit":1},{}]}"#);
        assert!(Json::parse(&p).is_ok());
    }

    #[test]
    fn kinds_round_trip_and_names_are_stable() {
        for k in LedgerKind::ALL {
            assert_eq!(LedgerKind::parse(k.as_str()), Some(k));
        }
        assert_eq!(LedgerKind::parse("nope"), None);
        let names: Vec<&str> = LedgerKind::ALL.iter().map(|k| k.as_str()).collect();
        assert_eq!(
            names,
            [
                "genesis",
                "job_queued",
                "job_started",
                "agent_event",
                "guard_decision",
                "review_verdict",
                "job_finished",
                "acknowledged",
                "label",
                "gate_verdict",
            ]
        );
    }

    #[test]
    fn canonical_bytes_golden() {
        let d = LedgerDraft {
            ts: 2,
            kind: LedgerKind::Label,
            actor: "a".into(),
            job: Some(JobId(12)),
            session: None,
            payload_json: "{}".into(),
        };
        let expected: Vec<u8> = [
            &[0, 0, 0, 0, 0, 0, 0, 1][..],
            &[0, 0, 0, 0, 0, 0, 0, 2],
            &[0, 0, 0, 5],
            b"label",
            &[0, 0, 0, 1],
            b"a",
            &[0, 0, 0, 2],
            b"12",
            &[0, 0, 0, 0],
            &[0, 0, 0, 2],
            b"{}",
        ]
        .concat();
        assert_eq!(canonical_bytes(1, &d), expected);
    }

    #[test]
    fn negative_timestamps_encode_twos_complement() {
        let d = LedgerDraft {
            ts: -1,
            ..draft(LedgerKind::Label, "{}")
        };
        assert_eq!(&canonical_bytes(0, &d)[8..16], &[0xff; 8]);
    }

    /// Frozen vectors, computed independently of this code. If these change,
    /// every stored ledger stops verifying.
    #[test]
    fn golden_hash_vectors() {
        assert_eq!(
            hex(&sha256(b"abc")),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        let genesis = LedgerEntry::next(None, LedgerDraft::genesis(0, "0.1.0"));
        assert_eq!(genesis.seq, 1);
        assert_eq!(genesis.prev_hash, GENESIS_PREV);
        assert_eq!(genesis.draft.payload_json, r#"{"version":"0.1.0"}"#);
        assert_eq!(
            hex(&genesis.hash),
            "680d135e9e3c8d28ed0b7c86d1fdf3e189434a031b046832475b33736cf9b404"
        );
        let second = LedgerEntry::next(
            Some(&genesis),
            draft(
                LedgerKind::AgentEvent,
                r#"{"event":"text","summary":"héllo"}"#,
            ),
        );
        assert_eq!(second.prev_hash, genesis.hash);
        assert_eq!(
            hex(&second.hash),
            "54d3cbb48d0525ba4ebb2904183594d64737b0f9ea4c6bcb7edf43cc11f68159"
        );
        let third = LedgerEntry::next(
            Some(&second),
            LedgerDraft {
                job: None,
                session: None,
                ..draft(LedgerKind::GuardDecision, "{}")
            },
        );
        assert_eq!(
            hex(&third.hash),
            "11cc02c11a674b6f16e2f117525a2d87ba5f0814615d9451d0db5d00b0342ae6"
        );
    }

    #[test]
    fn every_field_changes_the_hash() {
        let base = draft(LedgerKind::AgentEvent, "{}");
        let h = chain_hash(&GENESIS_PREV, 2, &base);
        let variants = [
            LedgerDraft {
                ts: base.ts + 1,
                ..base.clone()
            },
            LedgerDraft {
                kind: LedgerKind::Label,
                ..base.clone()
            },
            LedgerDraft {
                actor: "leaD".into(),
                ..base.clone()
            },
            LedgerDraft {
                job: Some(JobId(8)),
                ..base.clone()
            },
            LedgerDraft {
                job: None,
                ..base.clone()
            },
            LedgerDraft {
                session: None,
                ..base.clone()
            },
            LedgerDraft {
                payload_json: "{ }".into(),
                ..base.clone()
            },
        ];
        for v in variants {
            assert_ne!(chain_hash(&GENESIS_PREV, 2, &v), h, "{v:?}");
        }
        assert_ne!(chain_hash(&GENESIS_PREV, 3, &base), h);
        assert_ne!(chain_hash(&[1; 32], 2, &base), h);
    }

    #[test]
    fn length_prefixes_separate_fields() {
        let a = LedgerDraft {
            actor: "ab".into(),
            job: None,
            session: Some("c".into()),
            ..draft(LedgerKind::Label, "{}")
        };
        let b = LedgerDraft {
            actor: "a".into(),
            job: None,
            session: Some("bc".into()),
            ..a.clone()
        };
        assert_ne!(
            chain_hash(&GENESIS_PREV, 1, &a),
            chain_hash(&GENESIS_PREV, 1, &b)
        );
    }

    #[test]
    fn intact_chain_verifies() {
        assert_eq!(verify(&chain(20)), Ok(()));
        assert_eq!(verify(&[]), Ok(()));
        let mut v = ChainVerifier::default();
        let c = chain(3);
        for e in &c {
            v.push(e).unwrap();
        }
        assert_eq!((v.checked(), v.head()), (3, Some(c[2].hash)));
    }

    #[test]
    fn edited_payload_is_caught_at_its_seq() {
        let mut c = chain(10);
        c[4].draft.payload_json = r#"{"i":999}"#.into();
        assert_eq!(
            verify(&c),
            Err(ChainBreak {
                seq: 5,
                reason: BreakReason::HashMismatch
            })
        );
    }

    #[test]
    fn rehashed_edit_is_caught_at_the_next_entry() {
        let mut c = chain(10);
        c[4].draft.payload_json = r#"{"i":999}"#.into();
        c[4].hash = chain_hash(&c[4].prev_hash, 5, &c[4].draft);
        assert_eq!(
            verify(&c),
            Err(ChainBreak {
                seq: 6,
                reason: BreakReason::PrevMismatch
            })
        );
    }

    #[test]
    fn deleted_entry_is_a_gap() {
        let mut c = chain(10);
        c.remove(3);
        assert_eq!(
            verify(&c),
            Err(ChainBreak {
                seq: 5,
                reason: BreakReason::Gap { expected: 4 }
            })
        );
    }

    #[test]
    fn swapped_contents_are_caught() {
        let mut c = chain(10);
        let (a, b) = (c[3].draft.clone(), c[4].draft.clone());
        c[3].draft = b;
        c[4].draft = a;
        assert_eq!(verify(&c).unwrap_err().seq, 4);
    }

    #[test]
    fn genesis_must_come_first_and_only_once() {
        let mut c = chain(3);
        c.remove(0);
        assert_eq!(verify(&c).unwrap_err().reason, BreakReason::BadGenesis);

        let fake = LedgerEntry::next(None, draft(LedgerKind::Label, "{}"));
        assert_eq!(verify(&[fake]).unwrap_err().reason, BreakReason::BadGenesis);

        let c = chain(2);
        let again = LedgerEntry::next(c.last(), LedgerDraft::genesis(0, "x"));
        let all = [c, vec![again]].concat();
        assert_eq!(
            verify(&all),
            Err(ChainBreak {
                seq: 3,
                reason: BreakReason::BadGenesis
            })
        );

        let mut g = chain(1);
        g[0].prev_hash = [9; 32];
        assert_eq!(verify(&g).unwrap_err().reason, BreakReason::PrevMismatch);
    }

    #[test]
    fn payload_builds_valid_json() {
        let p = Payload::new()
            .str("a", "q\"é")
            .int("b", -3)
            .num("c", 0.25)
            .num("nan", f64::NAN)
            .opt_str("d", None)
            .opt_int("e", Some(4))
            .opt_num("f", None)
            .bool("g", true)
            .json("h", r#"{"x":[1]}"#)
            .build();
        let v = Json::parse(&p).unwrap();
        assert_eq!(v.get("a").and_then(Json::as_str), Some("q\"é"));
        assert_eq!(v.get("c").and_then(Json::as_f64), Some(0.25));
        assert_eq!(v.get("nan"), Some(&Json::Null));
        assert_eq!(v.get("g"), Some(&Json::Bool(true)));
        assert_eq!(Payload::new().build(), "{}");
    }

    #[test]
    fn long_text_is_cut_with_its_digest() {
        let long = "é".repeat(PAYLOAD_TEXT_MAX);
        let p = Payload::new().text("t", &long).build();
        let v = Json::parse(&p).unwrap();
        let kept = v.get("t").and_then(Json::as_str).unwrap();
        assert!(kept.len() <= PAYLOAD_TEXT_MAX && long.starts_with(kept));
        assert_eq!(
            v.get("t_sha256").and_then(Json::as_str),
            Some(hex(&sha256(long.as_bytes())).as_str())
        );
        assert_eq!(
            v.get("t_len").and_then(Json::as_f64),
            Some(long.len() as f64)
        );
        let short = Payload::new().text("t", "hi").build();
        assert_eq!(short, r#"{"t":"hi"}"#);
    }
}
