//! Authenticated encryption for everything the server hands out.
//!
//! Format: `<key id>.<base64url(nonce || ciphertext)>`. The purpose is bound as
//! associated data, so a value sealed for one use cannot be replayed as
//! another (an authorization code is never an access token). XChaCha20's
//! 192-bit nonce makes random nonces safe at any realistic volume.

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const KEY_BYTES: usize = 32;
const NONCE_BYTES: usize = 24;
const KEY_ID_BYTES: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Purpose {
    Client,
    Consent,
    Flow,
    Install,
    Code,
    Access,
    Refresh,
}

impl Purpose {
    fn label(self) -> &'static str {
        match self {
            Self::Client => "client",
            Self::Consent => "consent",
            Self::Flow => "flow",
            Self::Install => "install",
            Self::Code => "code",
            Self::Access => "access",
            Self::Refresh => "refresh",
        }
    }
}

#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum SealError {
    #[error("malformed sealed value")]
    Malformed,
    #[error("sealed value failed authentication")]
    Invalid,
    #[error("sealed value has expired")]
    Expired,
}

#[derive(Serialize, Deserialize)]
struct Envelope<T> {
    exp: Option<u64>,
    data: T,
}

struct Key {
    id: String,
    cipher: XChaCha20Poly1305,
}

/// Holds key material, hence no `Debug`.
pub struct Sealer {
    /// Newest first: index 0 seals, all of them open, which is what makes key
    /// rotation possible without signing everyone out.
    keys: Vec<Key>,
}

impl Sealer {
    pub fn new(encoded_keys: &[String]) -> Result<Self, String> {
        if encoded_keys.is_empty() {
            return Err("at least one token key is required".into());
        }
        let keys = encoded_keys
            .iter()
            .map(|encoded| {
                let bytes = B64
                    .decode(encoded)
                    .map_err(|_| "token key is not base64url".to_string())?;
                if bytes.len() != KEY_BYTES {
                    return Err(format!("token key must be {KEY_BYTES} bytes"));
                }
                let id = hex_prefix(&Sha256::digest(&bytes));
                let cipher = XChaCha20Poly1305::new_from_slice(&bytes)
                    .map_err(|_| "token key rejected".to_string())?;
                Ok(Key { id, cipher })
            })
            .collect::<Result<_, String>>()?;
        Ok(Self { keys })
    }

    pub fn seal<T: Serialize>(
        &self,
        purpose: Purpose,
        data: &T,
        expires_at: Option<u64>,
    ) -> Result<String, SealError> {
        let key = &self.keys[0];
        let plaintext = serde_json::to_vec(&Envelope {
            exp: expires_at,
            data,
        })
        .map_err(|_| SealError::Malformed)?;
        let mut nonce = [0u8; NONCE_BYTES];
        getrandom::fill(&mut nonce).map_err(|_| SealError::Invalid)?;
        let ciphertext = key
            .cipher
            .encrypt(
                XNonce::from_slice(&nonce),
                Payload {
                    msg: &plaintext,
                    aad: associated_data(purpose, &key.id).as_bytes(),
                },
            )
            .map_err(|_| SealError::Invalid)?;

        let mut raw = nonce.to_vec();
        raw.extend_from_slice(&ciphertext);
        Ok(format!("{}.{}", key.id, B64.encode(raw)))
    }

    pub fn open<T: DeserializeOwned>(
        &self,
        purpose: Purpose,
        sealed: &str,
        now: u64,
    ) -> Result<T, SealError> {
        let (key_id, encoded) = sealed.split_once('.').ok_or(SealError::Malformed)?;
        let key = self
            .keys
            .iter()
            .find(|key| key.id == key_id)
            .ok_or(SealError::Invalid)?;
        let raw = B64.decode(encoded).map_err(|_| SealError::Malformed)?;
        if raw.len() < NONCE_BYTES {
            return Err(SealError::Malformed);
        }
        let (nonce, ciphertext) = raw.split_at(NONCE_BYTES);
        let plaintext = key
            .cipher
            .decrypt(
                XNonce::from_slice(nonce),
                Payload {
                    msg: ciphertext,
                    aad: associated_data(purpose, &key.id).as_bytes(),
                },
            )
            .map_err(|_| SealError::Invalid)?;
        let envelope: Envelope<T> =
            serde_json::from_slice(&plaintext).map_err(|_| SealError::Malformed)?;
        if envelope.exp.is_some_and(|exp| now >= exp) {
            return Err(SealError::Expired);
        }
        Ok(envelope.data)
    }
}

fn associated_data(purpose: Purpose, key_id: &str) -> String {
    format!("personal-todo-mcp/v1/{}/{key_id}", purpose.label())
}

fn hex_prefix(digest: &[u8]) -> String {
    digest[..KEY_ID_BYTES]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// A random value for browser binding.
pub fn random_nonce() -> Result<String, SealError> {
    let mut bytes = [0u8; KEY_BYTES];
    getrandom::fill(&mut bytes).map_err(|_| SealError::Invalid)?;
    Ok(B64.encode(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(byte: u8) -> String {
        B64.encode([byte; KEY_BYTES])
    }

    fn sealer(bytes: &[u8]) -> Sealer {
        let keys: Vec<String> = bytes.iter().copied().map(key).collect();
        Sealer::new(&keys).unwrap()
    }

    #[test]
    fn round_trips() {
        let s = sealer(&[1]);
        let sealed = s.seal(Purpose::Access, &"hello", Some(100)).unwrap();
        assert_eq!(
            s.open::<String>(Purpose::Access, &sealed, 99).unwrap(),
            "hello"
        );
    }

    #[test]
    fn a_value_is_unreadable_and_unique_per_seal() {
        let s = sealer(&[1]);
        let a = s.seal(Purpose::Access, &"plaintext-marker", None).unwrap();
        let b = s.seal(Purpose::Access, &"plaintext-marker", None).unwrap();
        assert_ne!(a, b);
        assert!(!a.contains("plaintext-marker"));
    }

    #[test]
    fn purpose_is_bound_so_a_code_is_not_an_access_token() {
        let s = sealer(&[1]);
        let sealed = s.seal(Purpose::Code, &"x", None).unwrap();
        assert_eq!(
            s.open::<String>(Purpose::Access, &sealed, 0),
            Err(SealError::Invalid)
        );
    }

    #[test]
    fn expiry_is_enforced_at_the_boundary() {
        let s = sealer(&[1]);
        let sealed = s.seal(Purpose::Code, &"x", Some(100)).unwrap();
        assert_eq!(
            s.open::<String>(Purpose::Code, &sealed, 100),
            Err(SealError::Expired)
        );
    }

    #[test]
    fn tampering_is_detected() {
        let s = sealer(&[1]);
        let sealed = s.seal(Purpose::Access, &"x", None).unwrap();
        let (id, body) = sealed.split_once('.').unwrap();
        let mut raw = B64.decode(body).unwrap();
        let last = raw.len() - 1;
        raw[last] ^= 1;
        let forged = format!("{id}.{}", B64.encode(raw));
        assert_eq!(
            s.open::<String>(Purpose::Access, &forged, 0),
            Err(SealError::Invalid)
        );
    }

    #[test]
    fn another_deployments_key_cannot_open() {
        let sealed = sealer(&[1]).seal(Purpose::Access, &"x", None).unwrap();
        assert_eq!(
            sealer(&[2]).open::<String>(Purpose::Access, &sealed, 0),
            Err(SealError::Invalid)
        );
    }

    #[test]
    fn rotation_keeps_old_values_valid_and_seals_with_the_new_key() {
        let old = sealer(&[1]);
        let sealed_old = old.seal(Purpose::Access, &"x", None).unwrap();

        let rotated = sealer(&[2, 1]);
        assert!(
            rotated
                .open::<String>(Purpose::Access, &sealed_old, 0)
                .is_ok()
        );

        let sealed_new = rotated.seal(Purpose::Access, &"y", None).unwrap();
        assert!(old.open::<String>(Purpose::Access, &sealed_new, 0).is_err());
    }

    #[test]
    fn garbage_is_refused() {
        let s = sealer(&[1]);
        for value in ["", "abc", "abc.def", "...."] {
            assert!(s.open::<String>(Purpose::Access, value, 0).is_err());
        }
    }

    #[test]
    fn keys_must_be_well_formed() {
        assert!(Sealer::new(&[]).is_err());
        assert!(Sealer::new(&["short".to_string()]).is_err());
        assert!(Sealer::new(&["not base64!".to_string()]).is_err());
    }
}
