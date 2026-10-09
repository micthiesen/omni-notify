//! Port of `src/reminders/store.spec.ts` (encrypted Reminders store).
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::type_complexity)]

use std::os::unix::fs::PermissionsExt as _;

use omni_reminders::store::{
    FileRemindersStore, RemindersStore as _, StoredState, empty_reminders_state,
};
use serde_json::json;

const KEY: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const OTHER_KEY: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const ACCOUNT: &str = "test@example.com";

fn mode(path: &std::path::Path) -> u32 {
    std::fs::metadata(path).unwrap().permissions().mode() & 0o777
}

fn only_file(dir: &std::path::Path) -> std::path::PathBuf {
    let entries: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    assert_eq!(entries.len(), 1, "{entries:?}");
    entries[0].clone()
}

#[tokio::test]
async fn round_trips_private_state_with_owner_only_file_permissions() {
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join("private");
    let store = FileRemindersStore::new(&dir, KEY, ACCOUNT);
    let state = StoredState {
        session: json!({"sessionToken": "private-token", "cookies": "private-cookie"}),
        notified: true,
        ..empty_reminders_state()
    };
    store.write(state.clone()).await.unwrap();
    assert_eq!(store.read().await.unwrap(), state);
    let file = only_file(&dir);
    let bytes = std::fs::read(&file).unwrap();
    let text = String::from_utf8_lossy(&bytes);
    assert!(!text.contains("private-token"));
    assert!(!text.contains("private-cookie"));
    assert_eq!(mode(&dir), 0o700);
    assert_eq!(mode(&file), 0o600);
}

#[tokio::test]
async fn rejects_the_wrong_key_and_corrupted_ciphertext() {
    let root = tempfile::tempdir().unwrap();
    let store = FileRemindersStore::new(root.path(), KEY, ACCOUNT);
    store.write(empty_reminders_state()).await.unwrap();
    assert!(
        FileRemindersStore::new(root.path(), OTHER_KEY, ACCOUNT)
            .read()
            .await
            .is_err()
    );
    let file = only_file(root.path());
    let mut bytes = std::fs::read(&file).unwrap();
    let last = bytes.len() - 1;
    bytes[last] ^= 1;
    std::fs::write(&file, bytes).unwrap();
    assert!(store.read().await.is_err());
}

#[tokio::test]
async fn rejects_readable_by_others_ciphertext() {
    let root = tempfile::tempdir().unwrap();
    let store = FileRemindersStore::new(root.path(), KEY, ACCOUNT);
    store.write(empty_reminders_state()).await.unwrap();
    let file = only_file(root.path());
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert!(store.read().await.is_err());
}

#[tokio::test]
async fn refuses_a_symlinked_private_directory() {
    let root = tempfile::tempdir().unwrap();
    let target = root.path().join("target");
    let link = root.path().join("link");
    std::os::unix::fs::symlink(&target, &link).unwrap();
    let store = FileRemindersStore::new(&link, KEY, ACCOUNT);
    assert!(store.read().await.is_err());
}

#[tokio::test]
async fn refuses_a_symlinked_state_file_and_an_invalid_key() {
    let root = tempfile::tempdir().unwrap();
    let store = FileRemindersStore::new(root.path(), KEY, ACCOUNT);
    store.write(empty_reminders_state()).await.unwrap();
    let file = only_file(root.path());
    let moved = root.path().join("elsewhere");
    std::fs::rename(&file, &moved).unwrap();
    std::os::unix::fs::symlink(&moved, &file).unwrap();
    assert!(store.read().await.is_err());
    let invalid = FileRemindersStore::new(root.path(), "not-hex", ACCOUNT);
    assert!(invalid.read().await.is_err());
    assert!(invalid.write(empty_reminders_state()).await.is_err());
}

#[tokio::test]
async fn reads_a_missing_file_as_empty_state() {
    let root = tempfile::tempdir().unwrap();
    let store = FileRemindersStore::new(root.path().join("new"), KEY, ACCOUNT);
    assert_eq!(store.read().await.unwrap(), empty_reminders_state());
}

/// Rust-only: a ledger entry whose confirmed result is `null` (valid under the TS
/// `Schema.optional(Schema.Unknown)`) keeps it through decode and rewrite, and an
/// absent result stays absent.
#[test]
fn preserves_null_and_absent_ledger_results() {
    let document = json!({
        "version": 1,
        "session": null,
        "notified": false,
        "operations": {
            "a": {"fingerprint": "f", "recordId": "Reminder/A", "state": "confirmed", "result": null},
            "b": {"fingerprint": "g", "recordId": "Reminder/B", "state": "reserved"},
        },
    });
    let state: StoredState = serde_json::from_value(document.clone()).unwrap();
    assert_eq!(serde_json::to_value(&state).unwrap(), document);
}
