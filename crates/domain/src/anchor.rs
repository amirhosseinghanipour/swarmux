//! Ledger anchoring: signed checkpoints of the audit ledger's head.
//!
//! A hash chain shows that nothing *inside* it was edited, but anyone who
//! can write the end of the chain can rewrite the whole thing and rechain
//! it. A checkpoint ties the chain to something the writer of the database
//! cannot forge alone: a signature, made with a key kept outside the
//! database, over the chain's length and head hash at a moment in time.
//! Later, the ledger either still has that head at that position, or it has
//! been cut short or rewritten since.
//!
//! This is tamper-evident, not tamper-proof. See the guide for what the key
//! protects against and what it does not.

use crate::Millis;
use crate::json::Json;
use crate::ledger::{Hash, Payload, hex};

/// What a checkpoint's signature covers is this line's bytes.
const MESSAGE_PREFIX: &str = "swarmux-checkpoint/1";

/// A signed statement: "at `at`, the ledger had `seq` entries and the last
/// one hashed to `head`".
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Checkpoint {
    /// The `seq` of the newest entry it covers.
    pub seq: u64,
    pub head: Hash,
    pub at: Millis,
    /// The signer's public key, as text.
    pub key: String,
    /// The signature over [`Checkpoint::message`], as text.
    pub signature: String,
}

impl Checkpoint {
    /// The bytes that are signed.
    pub fn message(seq: u64, head: &Hash, at: Millis) -> Vec<u8> {
        format!("{MESSAGE_PREFIX}\nseq={seq}\nhead={}\nat={at}\n", hex(head)).into_bytes()
    }

    pub fn signed_message(&self) -> Vec<u8> {
        Checkpoint::message(self.seq, &self.head, self.at)
    }

    /// One line of JSON, for exporting.
    pub fn to_json(&self) -> String {
        Payload::new()
            .str("format", MESSAGE_PREFIX)
            .int("seq", i64::try_from(self.seq).unwrap_or(i64::MAX))
            .str("head", &hex(&self.head))
            .int("at", self.at)
            .str("key", &self.key)
            .str("signature", &self.signature)
            .build()
    }

    /// Reads one exported line.
    pub fn from_json(line: &str) -> Result<Checkpoint, String> {
        Checkpoint::from_value(&Json::parse(line)?)
    }

    pub(crate) fn from_value(j: &Json) -> Result<Checkpoint, String> {
        let text = |k: &str| {
            j.get(k)
                .and_then(Json::as_str)
                .map(str::to_string)
                .ok_or_else(|| format!("checkpoint has no {k}"))
        };
        if text("format")? != MESSAGE_PREFIX {
            return Err("not a swarmux checkpoint".into());
        }
        let num = |k: &str| {
            j.get(k)
                .and_then(Json::as_f64)
                .ok_or_else(|| format!("checkpoint has no {k}"))
        };
        Ok(Checkpoint {
            seq: num("seq")? as u64,
            head: parse_hash(&text("head")?)?,
            at: num("at")? as Millis,
            key: text("key")?,
            signature: text("signature")?,
        })
    }
}

/// A hash from its hex text.
pub fn parse_hash(text: &str) -> Result<Hash, String> {
    let text = text.trim();
    if text.len() != 64 || !text.is_ascii() {
        return Err(format!("{text:?} is not a 64 digit hash"));
    }
    let mut out = [0u8; 32];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&text[i * 2..i * 2 + 2], 16)
            .map_err(|_| format!("{text:?} is not a hash"))?;
    }
    Ok(out)
}

/// How a ledger stands against one checkpoint.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Standing {
    /// The entry at the checkpoint's position still has its hash.
    Intact,
    /// The ledger is shorter than when the checkpoint was made.
    Truncated { ledger_head: Option<u64> },
    /// The entry at that position hashes differently now.
    Rewritten,
}

/// Compares a checkpoint with the ledger: its newest `seq` and the hash of
/// the entry at the checkpoint's `seq` (if there is one).
pub fn stand(cp: &Checkpoint, ledger_head: Option<u64>, hash_at: Option<Hash>) -> Standing {
    match (ledger_head, hash_at) {
        (Some(h), Some(hash)) if h >= cp.seq => {
            if hash == cp.head {
                Standing::Intact
            } else {
                Standing::Rewritten
            }
        }
        _ => Standing::Truncated { ledger_head },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cp(seq: u64, head: Hash) -> Checkpoint {
        Checkpoint {
            seq,
            head,
            at: 1_700_000_000_000,
            key: "RWQkey".into(),
            signature: "untrusted comment: x\nsig".into(),
        }
    }

    #[test]
    fn a_checkpoint_survives_export_and_import() {
        let c = cp(12, [7; 32]);
        let line = c.to_json();
        assert!(!line.contains('\n'), "one line: {line}");
        assert_eq!(Checkpoint::from_json(&line).unwrap(), c);
        assert!(Checkpoint::from_json("{\"format\":\"other\"}").is_err());
        assert!(Checkpoint::from_json("nonsense").is_err());
    }

    #[test]
    fn the_signed_message_names_position_head_and_time() {
        let m = String::from_utf8(cp(3, [0xab; 32]).signed_message()).unwrap();
        assert!(
            m.starts_with("swarmux-checkpoint/1\nseq=3\nhead=abab"),
            "{m}"
        );
        assert!(m.ends_with("at=1700000000000\n"));
    }

    #[test]
    fn standing_tells_intact_cut_and_rewritten_apart() {
        let c = cp(10, [1; 32]);
        assert_eq!(stand(&c, Some(12), Some([1; 32])), Standing::Intact);
        assert_eq!(stand(&c, Some(10), Some([1; 32])), Standing::Intact);
        assert_eq!(stand(&c, Some(12), Some([2; 32])), Standing::Rewritten);
        assert_eq!(
            stand(&c, Some(8), None),
            Standing::Truncated {
                ledger_head: Some(8)
            }
        );
        assert_eq!(
            stand(&c, None, None),
            Standing::Truncated { ledger_head: None }
        );
    }

    #[test]
    fn hashes_parse_from_hex_only() {
        assert_eq!(parse_hash(&hex(&[9; 32])).unwrap(), [9; 32]);
        assert!(parse_hash("abc").is_err());
        assert!(parse_hash(&"zz".repeat(32)).is_err());
    }
}
