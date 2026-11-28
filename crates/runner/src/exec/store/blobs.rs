//! The content-addressed blob store: `<runner directory>/blobs/sha256/<2>/<62>`, each file mode 0444.
//!
//! A stage running as the same OS user can reach the runner directory (design §13), so file
//! modes are only a convention: every read re-hashes the content and refuses a blob that no
//! longer matches its id.

use std::fmt;
use std::fs::{File, OpenOptions, Permissions};
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::atomic::{AtomicU64, Ordering};

use sha2::{Digest, Sha256};

use super::{BLOBS_DIR, SHA256_DIR, StoreError, TMP_DIR, ensure_dir, io};

/// The mode of a stored blob.
const BLOB_MODE: u32 = 0o444;

/// The mode of a blob while it is written in `tmp`.
const WRITE_MODE: u32 = 0o600;

/// Tells apart the temporary files of one process.
static TMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// A blob's id: the sha256 of its content, as 64 lowercase hexadecimal digits.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct BlobId(String);

impl BlobId {
    /// The id of `content`.
    pub fn of(content: &[u8]) -> BlobId {
        let digest = Sha256::digest(content);
        let mut hex = String::with_capacity(64);
        for byte in digest.iter() {
            hex.push(char::from(HEX[usize::from(byte >> 4)]));
            hex.push(char::from(HEX[usize::from(byte & 0x0f)]));
        }
        BlobId(hex)
    }

    /// The id as 64 lowercase hexadecimal digits.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// The lowercase hexadecimal digits.
const HEX: &[u8; 16] = b"0123456789abcdef";

impl FromStr for BlobId {
    type Err = StoreError;

    fn from_str(text: &str) -> Result<BlobId, StoreError> {
        if text.len() == 64 && text.bytes().all(|b| HEX.contains(&b)) {
            Ok(BlobId(text.to_owned()))
        } else {
            Err(StoreError::BlobId(text.to_owned()))
        }
    }
}

impl fmt::Display for BlobId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// The path of blob `id` under the runner directory `root`.
pub(super) fn blob_path(root: &Path, id: &BlobId) -> PathBuf {
    let (fan_out, rest) = id.0.split_at(2);
    root.join(BLOBS_DIR)
        .join(SHA256_DIR)
        .join(fan_out)
        .join(rest)
}

/// Store `content` and return its id.
///
/// The content is written to `tmp`, synced, set to mode 0444 and renamed into place, and the
/// fan-out directory and its parent `blobs/sha256` are synced, so that a fan-out directory
/// created here survives a crash too. A blob of that id already stored is verified instead; a
/// corrupt one is replaced, with a warning.
pub(super) fn put(root: &Path, content: &[u8]) -> Result<BlobId, StoreError> {
    let id = BlobId::of(content);
    let path = blob_path(root, &id);
    match get(root, &id) {
        Ok(_) => return Ok(id),
        Err(StoreError::BlobMissing(_)) => {}
        Err(StoreError::BlobCorrupt(_)) => {
            tracing::warn!(blob = %id, "replacing a blob that does not match its id");
        }
        Err(err) => return Err(err),
    }
    let tmp = root.join(TMP_DIR).join(format!(
        "{id}.{}.{}",
        std::process::id(),
        TMP_COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let written = write_tmp(&tmp, content).and_then(|()| {
        let dir = path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| root.to_path_buf());
        ensure_dir(&dir)?;
        std::fs::rename(&tmp, &path).map_err(io(&path, "cannot store the blob"))?;
        sync_dir(&dir)?;
        // Always, not only when `dir` is new here: an earlier put may have created it and
        // failed before syncing the parent.
        match dir.parent() {
            Some(parent) => sync_dir(parent),
            None => Ok(()),
        }
    });
    if written.is_err() {
        // Best effort: `tmp` is cleared when the store is next opened anyway.
        let _ = std::fs::remove_file(&tmp);
    }
    written.map(|()| id)
}

/// Sync the directory `dir`, so that the entries made in it are durable.
fn sync_dir(dir: &Path) -> Result<(), StoreError> {
    File::open(dir)
        .and_then(|dir| dir.sync_all())
        .map_err(io(dir, "cannot sync"))
}

/// Write `content` to the new file `tmp`, sync it and set its mode to 0444.
fn write_tmp(tmp: &Path, content: &[u8]) -> Result<(), StoreError> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(WRITE_MODE)
        .open(tmp)
        .map_err(io(tmp, "cannot create"))?;
    file.write_all(content).map_err(io(tmp, "cannot write"))?;
    file.sync_all().map_err(io(tmp, "cannot sync"))?;
    file.set_permissions(Permissions::from_mode(BLOB_MODE))
        .map_err(io(tmp, "cannot set the mode of"))
}

/// The content of blob `id`, refused when it no longer hashes to `id`.
pub(super) fn get(root: &Path, id: &BlobId) -> Result<Vec<u8>, StoreError> {
    let path = blob_path(root, id);
    let content = match std::fs::read(&path) {
        Ok(content) => content,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return Err(StoreError::BlobMissing(id.clone()));
        }
        Err(err) => return Err(io(&path, "cannot read")(err)),
    };
    if BlobId::of(&content) != *id {
        return Err(StoreError::BlobCorrupt(id.clone()));
    }
    Ok(content)
}
