//! Port of `src/podcast-recs/taste.spec.ts`.
//!
//! Dropped case: "aborts the in-flight read when interrupted" — the Rust read
//! is a tokio future, and dropping it is the cancellation; there is no
//! AbortSignal to observe.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use omni_podcasts::taste::{TasteSeedFailure, check_seed, load_taste_seed};

#[tokio::test]
async fn returns_a_typed_failure_when_the_seed_cannot_be_read() {
    let error = load_taste_seed(Some("/definitely/missing/podcast-taste.md"))
        .await
        .unwrap_err();
    assert_eq!(error.reason, TasteSeedFailure::Unreadable);
    assert_eq!(error.path, "/definitely/missing/podcast-taste.md");
}

#[test]
fn rejects_an_empty_seed_as_malformed() {
    let error = check_seed("taste.md", " \n\t ").unwrap_err();
    assert_eq!(error.reason, TasteSeedFailure::Malformed);
    assert!(error.to_string().contains("is empty"));
}

#[tokio::test]
async fn trims_a_readable_seed_and_treats_a_missing_path_as_empty() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("taste.md");
    std::fs::write(&path, "\n  # Profile\n\n").unwrap();
    assert_eq!(load_taste_seed(path.to_str()).await.unwrap(), "# Profile");
    assert_eq!(load_taste_seed(None).await.unwrap(), "");
}
