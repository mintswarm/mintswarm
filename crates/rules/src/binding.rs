//! sha256 bindings: a file named by its path and held to the digest of its bytes.

use mintswarm_domain::records::Bound;
use sha2::{Digest, Sha256};

/// The sha256 of `bytes` in lowercase hex, as Python's `hashlib.sha256(bytes).hexdigest()` gives.
pub fn digest(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let hash = Sha256::digest(bytes);
    let mut out = String::with_capacity(hash.len() * 2);
    for byte in hash.iter() {
        out.push(char::from(HEX[usize::from(byte >> 4)]));
        out.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    out
}

/// The binding of the file at `path` to the digest of its `bytes`.
pub fn bind(path: impl Into<String>, bytes: &[u8]) -> Bound {
    Bound {
        path: path.into(),
        sha256: digest(bytes),
    }
}

/// Whether `bytes` still have the digest `bound` holds.
pub fn verify(bound: &Bound, bytes: &[u8]) -> bool {
    bound.sha256 == digest(bytes)
}
