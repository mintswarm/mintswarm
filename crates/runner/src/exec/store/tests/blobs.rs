//! Tests of the blob store: layout, modes, idempotent puts and refusing corrupt blobs on read.

use std::fs::Permissions;
use std::os::unix::fs::PermissionsExt;

use super::super::blobs::blob_path;
use super::super::{BlobId, OutboxMode, StoreError};
use super::{fresh_runner_dir, mode_of, open};

/// The sha256 of `hello`.
const HELLO: &str = "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824";

#[test]
fn a_blob_id_is_64_lowercase_hex_digits() {
    assert_eq!(BlobId::of(b"hello").as_str(), HELLO);
    assert_eq!(HELLO.parse::<BlobId>().unwrap(), BlobId::of(b"hello"));
    for text in [
        "",
        "abc",
        &HELLO.to_uppercase(),
        &format!("{HELLO}0"),
        &HELLO.replace('2', "g"),
    ] {
        let err = text.parse::<BlobId>().unwrap_err();
        assert!(matches!(err, StoreError::BlobId(_)), "{text}: {err:?}");
    }
}

#[test]
fn put_stores_the_blob_at_its_fan_out_path_with_mode_0444() {
    let (_tmp, runner_dir) = fresh_runner_dir();
    let store = open(&runner_dir, OutboxMode::Live).unwrap();
    let id = store.put_blob(b"hello").unwrap();
    assert_eq!(id.as_str(), HELLO);
    let path = runner_dir
        .join("blobs/sha256")
        .join(&HELLO[..2])
        .join(&HELLO[2..]);
    assert_eq!(blob_path(&runner_dir, &id), path);
    assert_eq!(std::fs::read(&path).unwrap(), b"hello");
    assert_eq!(mode_of(&path), 0o444);
    assert_eq!(mode_of(path.parent().unwrap()), 0o700);
    assert_eq!(store.get_blob(&id).unwrap(), b"hello");
    assert_eq!(
        std::fs::read_dir(runner_dir.join("tmp")).unwrap().count(),
        0
    );
}

#[test]
fn put_is_idempotent() {
    let (_tmp, runner_dir) = fresh_runner_dir();
    let store = open(&runner_dir, OutboxMode::Live).unwrap();
    let first = store.put_blob(b"hello").unwrap();
    let second = store.put_blob(b"hello").unwrap();
    assert_eq!(first, second);
    assert_eq!(store.get_blob(&first).unwrap(), b"hello");
    assert_eq!(mode_of(&blob_path(&runner_dir, &first)), 0o444);
}

#[test]
fn get_refuses_a_missing_blob() {
    let (_tmp, runner_dir) = fresh_runner_dir();
    let store = open(&runner_dir, OutboxMode::Live).unwrap();
    let err = store.get_blob(&BlobId::of(b"never stored")).unwrap_err();
    assert!(matches!(err, StoreError::BlobMissing(_)), "{err:?}");
}

#[test]
fn get_refuses_a_blob_rewritten_in_place() {
    let (_tmp, runner_dir) = fresh_runner_dir();
    let store = open(&runner_dir, OutboxMode::Live).unwrap();
    let id = store.put_blob(b"hello").unwrap();
    let path = blob_path(&runner_dir, &id);
    std::fs::set_permissions(&path, Permissions::from_mode(0o644)).unwrap();
    std::fs::write(&path, b"jello").unwrap();
    std::fs::set_permissions(&path, Permissions::from_mode(0o444)).unwrap();
    let err = store.get_blob(&id).unwrap_err();
    assert!(
        matches!(&err, StoreError::BlobCorrupt(bad) if *bad == id),
        "{err:?}"
    );
}

#[test]
fn get_refuses_another_file_moved_to_a_blob_name() {
    let (_tmp, runner_dir) = fresh_runner_dir();
    let store = open(&runner_dir, OutboxMode::Live).unwrap();
    let id = store.put_blob(b"hello").unwrap();
    let other = runner_dir.join("other");
    std::fs::write(&other, b"something else").unwrap();
    std::fs::rename(&other, blob_path(&runner_dir, &id)).unwrap();
    let err = store.get_blob(&id).unwrap_err();
    assert!(matches!(err, StoreError::BlobCorrupt(_)), "{err:?}");
}

#[test]
fn put_repairs_a_corrupt_blob() {
    let (_tmp, runner_dir) = fresh_runner_dir();
    let store = open(&runner_dir, OutboxMode::Live).unwrap();
    let id = store.put_blob(b"hello").unwrap();
    let path = blob_path(&runner_dir, &id);
    std::fs::set_permissions(&path, Permissions::from_mode(0o644)).unwrap();
    std::fs::write(&path, b"jello").unwrap();
    assert_eq!(store.put_blob(b"hello").unwrap(), id);
    assert_eq!(store.get_blob(&id).unwrap(), b"hello");
    assert_eq!(mode_of(&path), 0o444);
}
