//! PKCE `S256` (RFC 7636).

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

const VERIFIER_LENGTH: std::ops::RangeInclusive<usize> = 43..=128;
const CHALLENGE_LENGTH: usize = 43;

pub fn is_valid_challenge(challenge: &str) -> bool {
    challenge.len() == CHALLENGE_LENGTH && B64.decode(challenge).is_ok()
}

pub fn verify(verifier: &str, challenge: &str) -> bool {
    if !VERIFIER_LENGTH.contains(&verifier.len()) || !verifier.bytes().all(is_unreserved) {
        return false;
    }
    let computed = B64.encode(Sha256::digest(verifier.as_bytes()));
    computed.as_bytes().ct_eq(challenge.as_bytes()).into()
}

fn is_unreserved(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~')
}

#[cfg(test)]
mod tests {
    use super::*;

    // The worked example from RFC 7636, appendix B.
    const VERIFIER: &str = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
    const CHALLENGE: &str = "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM";

    #[test]
    fn accepts_the_rfc_example() {
        assert!(is_valid_challenge(CHALLENGE));
        assert!(verify(VERIFIER, CHALLENGE));
    }

    #[test]
    fn rejects_a_wrong_verifier() {
        assert!(!verify(&"a".repeat(43), CHALLENGE));
    }

    #[test]
    fn rejects_verifiers_outside_the_spec() {
        assert!(!verify("short", CHALLENGE));
        assert!(!verify(&"a".repeat(129), CHALLENGE));
        assert!(!verify(&format!("{}!", "a".repeat(43)), CHALLENGE));
    }

    #[test]
    fn rejects_malformed_challenges() {
        assert!(!is_valid_challenge("plain"));
        assert!(!is_valid_challenge(&"!".repeat(43)));
    }
}
