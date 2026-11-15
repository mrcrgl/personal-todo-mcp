//! Helpers for the Contents API shared by the file and table adapters.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;

use crate::github::models::ContentRow;

/// The bytes of a contents entry, or `None` when GitHub did not inline them
/// as base64. GitHub wraps the base64 at 60 characters.
pub fn decoded_bytes(row: &ContentRow) -> Option<Vec<u8>> {
    if row.encoding.as_deref() != Some("base64") {
        return None;
    }
    let encoded: String = row
        .content
        .as_deref()
        .unwrap_or_default()
        .chars()
        .filter(|c| !c.is_ascii_whitespace())
        .collect();
    B64.decode(encoded).ok()
}

pub fn encoded(bytes: &[u8]) -> String {
    B64.encode(bytes)
}
