//! The checkpoint signing key.
//!
//! An ed25519 key pair in minisign's format (the crate that already
//! verifies swarmux's own releases). The private key goes to the OS
//! keychain when there is one; otherwise to a file in swarmux's folder,
//! which anything running as the same user can read, and the signer says
//! so. A checkpoint is verified with the public key written in it.

use std::fs;
use std::io::Cursor;
use std::path::PathBuf;
use std::sync::{Mutex, PoisonError};

use swarmux_application::anchor::{CheckpointSigner, KeyProtection};
use swarmux_application::{AppError, AppResult};

/// Keychain service and account of the private key.
const SERVICE: &str = "swarmux";
const ACCOUNT: &str = "audit-signing-key";

/// Where a key pair is kept: line one is the public key, the rest the
/// private key box.
trait Slot: Send + Sync {
    fn read(&self) -> Option<String>;
    fn write(&self, text: &str) -> Result<(), String>;
    fn protection(&self) -> KeyProtection;
}

struct KeychainSlot;

impl KeychainSlot {
    fn entry() -> Result<keyring::Entry, String> {
        keyring::Entry::new(SERVICE, ACCOUNT).map_err(|e| e.to_string())
    }
}

impl Slot for KeychainSlot {
    fn read(&self) -> Option<String> {
        KeychainSlot::entry().ok()?.get_password().ok()
    }

    fn write(&self, text: &str) -> Result<(), String> {
        let entry = KeychainSlot::entry()?;
        entry.set_password(text).map_err(|e| e.to_string())?;
        // Some backends accept a write and keep nothing.
        match entry.get_password() {
            Ok(back) if back == text => Ok(()),
            _ => Err("the keychain did not keep the key".into()),
        }
    }

    fn protection(&self) -> KeyProtection {
        KeyProtection::Keychain
    }
}

struct FileSlot(PathBuf);

impl Slot for FileSlot {
    fn read(&self) -> Option<String> {
        fs::read_to_string(&self.0).ok()
    }

    fn write(&self, text: &str) -> Result<(), String> {
        if let Some(dir) = self.0.parent() {
            fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        fs::write(&self.0, text).map_err(|e| e.to_string())?;
        restrict(&self.0);
        Ok(())
    }

    fn protection(&self) -> KeyProtection {
        KeyProtection::File
    }
}

/// Owner-only permissions where the platform has them.
fn restrict(path: &std::path::Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o600));
    }
    #[cfg(not(unix))]
    let _ = path;
}

struct Keys {
    public: String,
    secret: minisign::SecretKey,
    protection: KeyProtection,
}

/// Signs checkpoints with a key pair that is made on first use.
pub struct MinisignSigner {
    slots: Vec<Box<dyn Slot>>,
    keys: Mutex<Option<std::sync::Arc<Keys>>>,
}

impl MinisignSigner {
    /// The keychain first; `file` when there is no keychain to use.
    pub fn open(file: PathBuf) -> MinisignSigner {
        MinisignSigner {
            slots: vec![Box::new(KeychainSlot), Box::new(FileSlot(file))],
            keys: Mutex::new(None),
        }
    }

    /// Keeps the key in `file` only (for tests and for machines without a
    /// keychain).
    pub fn file_only(file: PathBuf) -> MinisignSigner {
        MinisignSigner {
            slots: vec![Box::new(FileSlot(file))],
            keys: Mutex::new(None),
        }
    }

    fn parse(text: &str, protection: KeyProtection) -> Option<Keys> {
        let (public, secret) = text.split_once('\n')?;
        let secret = minisign::SecretKeyBox::from_string(secret)
            .ok()?
            .into_unencrypted_secret_key()
            .ok()?;
        Some(Keys {
            public: public.trim().to_string(),
            secret,
            protection,
        })
    }

    fn keys(&self) -> AppResult<std::sync::Arc<Keys>> {
        let mut held = self.keys.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(k) = &*held {
            return Ok(k.clone());
        }
        for slot in &self.slots {
            if let Some(k) = slot
                .read()
                .and_then(|t| MinisignSigner::parse(&t, slot.protection()))
            {
                let k = std::sync::Arc::new(k);
                *held = Some(k.clone());
                return Ok(k);
            }
        }
        let pair = minisign::KeyPair::generate_unencrypted_keypair()
            .map_err(|e| AppError::Storage(format!("cannot make a signing key: {e}")))?;
        let secret_box = pair
            .sk
            .to_box(None)
            .map_err(|e| AppError::Storage(format!("cannot write the signing key: {e}")))?
            .to_string();
        let public = pair.pk.to_base64();
        let text = format!("{public}\n{secret_box}");
        let mut last = String::from("no place to keep the key");
        for slot in &self.slots {
            match slot.write(&text) {
                Ok(()) => {
                    let k = std::sync::Arc::new(Keys {
                        public,
                        secret: pair.sk,
                        protection: slot.protection(),
                    });
                    *held = Some(k.clone());
                    return Ok(k);
                }
                Err(e) => last = e,
            }
        }
        Err(AppError::Storage(format!(
            "cannot keep the signing key: {last}"
        )))
    }
}

impl CheckpointSigner for MinisignSigner {
    fn public_key(&self) -> AppResult<String> {
        Ok(self.keys()?.public.clone())
    }

    fn sign(&self, message: &[u8]) -> AppResult<String> {
        let keys = self.keys()?;
        minisign::sign(None, &keys.secret, Cursor::new(message), None, None)
            .map(|s| s.to_string())
            .map_err(|e| AppError::Storage(format!("cannot sign: {e}")))
    }

    fn verify(&self, public_key: &str, message: &[u8], signature: &str) -> bool {
        let Ok(key) = minisign_verify::PublicKey::from_base64(public_key.trim()) else {
            return false;
        };
        let Ok(sig) = minisign_verify::Signature::decode(signature) else {
            return false;
        };
        key.verify(message, &sig, false).is_ok()
    }

    fn protection(&self) -> KeyProtection {
        self.keys().map_or(KeyProtection::File, |k| k.protection)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_key_is_made_once_kept_and_signs_checkpoints_that_verify() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("audit.key");
        let signer = MinisignSigner::file_only(file.clone());
        let key = signer.public_key().unwrap();
        assert!(key.starts_with("RW"), "{key}");
        assert_eq!(signer.protection(), KeyProtection::File);

        let sig = signer.sign(b"seq=3").unwrap();
        assert!(signer.verify(&key, b"seq=3", &sig));
        assert!(!signer.verify(&key, b"seq=4", &sig), "other message");

        // A new signer finds the same key; another key does not verify.
        let again = MinisignSigner::file_only(file);
        assert_eq!(again.public_key().unwrap(), key);
        assert!(again.verify(&key, b"seq=3", &sig));
        let other = MinisignSigner::file_only(dir.path().join("other.key"));
        assert_ne!(other.public_key().unwrap(), key);
        assert!(!other.verify(&other.public_key().unwrap(), b"seq=3", &sig));
        assert!(!signer.verify("garbage", b"seq=3", &sig));
        assert!(!signer.verify(&key, b"seq=3", "garbage"));
    }
}
