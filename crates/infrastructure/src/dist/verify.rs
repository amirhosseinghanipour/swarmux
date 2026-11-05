//! Minisign signature check against a public key built into the binary.

use minisign_verify::{PublicKey, Signature};
use swarmux_application::dist::{DistError, DistResult, SignatureVerifier};

/// The release signing key. It is the second line of the `.pub` file that
/// `minisign -G` writes; see RELEASING.md for how it is made and replaced.
const EMBEDDED_KEY: &str = include_str!("minisign.pub");

pub struct MinisignVerifier {
    key: PublicKey,
}

impl MinisignVerifier {
    /// A verifier for the given public key (the text of a `.pub` file, or
    /// just its base64 line).
    pub fn new(key: &str) -> DistResult<Self> {
        let key = key.trim();
        let parsed = if key.lines().count() > 1 {
            PublicKey::decode(key)
        } else {
            PublicKey::from_base64(key)
        };
        parsed.map(|key| Self { key }).map_err(|_| DistError::NoKey)
    }

    /// The verifier for the key built into this binary.
    pub fn embedded() -> DistResult<Self> {
        Self::new(EMBEDDED_KEY)
    }
}

impl SignatureVerifier for MinisignVerifier {
    fn verify(&self, data: &[u8], signature: &str) -> DistResult<()> {
        let sig = Signature::decode(signature)
            .map_err(|e| DistError::BadSignature(format!("unreadable signature ({e})")))?;
        self.key
            .verify(data, &sig, false)
            .map_err(|e| DistError::BadSignature(e.to_string()))
    }
}

/// The verifier for the built-in key, made when first needed so that
/// looking for an update works in a build that has no key yet.
pub struct EmbeddedKey;

impl SignatureVerifier for EmbeddedKey {
    fn verify(&self, data: &[u8], signature: &str) -> DistResult<()> {
        MinisignVerifier::embedded()?.verify(data, signature)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    /// A key pair and the signature of `data` made with it.
    pub(crate) fn sign(data: &[u8]) -> (String, String) {
        let pair = minisign::KeyPair::generate_unencrypted_keypair().unwrap();
        let pk = pair.pk.to_box().unwrap().to_string();
        let sig = minisign::sign(Some(&pair.pk), &pair.sk, Cursor::new(data), None, None)
            .unwrap()
            .to_string();
        (pk, sig)
    }

    #[test]
    fn accepts_the_right_signature_and_rejects_anything_else() {
        let (pk, sig) = sign(b"archive");
        let v = MinisignVerifier::new(&pk).unwrap();
        v.verify(b"archive", &sig).unwrap();
        assert!(matches!(
            v.verify(b"tampered", &sig),
            Err(DistError::BadSignature(_))
        ));
        assert!(v.verify(b"archive", "not a signature").is_err());
        // Signed by another key.
        let (_, other) = sign(b"archive");
        assert!(v.verify(b"archive", &other).is_err());
    }

    #[test]
    fn the_unset_embedded_key_is_reported() {
        assert!(MinisignVerifier::new("untrusted comment: x\nnope").is_err());
    }

    #[test]
    fn the_release_key_is_built_in() {
        assert!(MinisignVerifier::embedded().is_ok());
    }
}
