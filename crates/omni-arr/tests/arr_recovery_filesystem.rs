//! Download directory inspection through a scripted [`DirLister`]; one case
//! reads a real temporary directory.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::io;
use std::sync::Mutex;

use omni_arr::arr_recovery::filesystem::{DirLister, LocalDirs, verify_download_removed};

struct Scripted {
    reply: Mutex<Option<io::Result<Vec<String>>>>,
    asked: Mutex<Vec<String>>,
}

impl Scripted {
    fn new(reply: io::Result<Vec<String>>) -> Self {
        Self {
            reply: Mutex::new(Some(reply)),
            asked: Mutex::new(Vec::new()),
        }
    }
}

impl DirLister for Scripted {
    async fn list(&self, dir: &str) -> io::Result<Vec<String>> {
        self.asked.lock().unwrap().push(dir.to_owned());
        self.reply
            .lock()
            .unwrap()
            .take()
            .unwrap_or_else(|| Err(io::Error::other("unscripted")))
    }
}

#[tokio::test]
async fn proves_deletion_even_when_its_parent_is_empty() {
    let dirs = Scripted::new(Ok(vec![]));
    assert!(
        verify_download_removed("/tmp/inter/failed-job", &dirs)
            .await
            .unwrap()
    );
    assert_eq!(*dirs.asked.lock().unwrap(), vec!["/tmp/inter".to_owned()]);
}

#[tokio::test]
async fn does_not_accept_an_inaccessible_parent_as_deletion() {
    let dirs = Scripted::new(Err(io::Error::new(
        io::ErrorKind::PermissionDenied,
        "EACCES",
    )));
    assert!(
        verify_download_removed("/tmp/inter/failed-job", &dirs)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn keeps_existing_data_unverified() {
    let dirs = Scripted::new(Ok(vec!["failed-job".into()]));
    assert!(
        !verify_download_removed("/tmp/inter/failed-job", &dirs)
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn rejects_library_and_parent_traversal_paths() {
    for path in [
        "/media/storage/sonarr/Show",
        "/tmp/inter/../../library",
        "/tmp/inter",
    ] {
        let dirs = Scripted::new(Ok(vec![]));
        assert!(
            verify_download_removed(path, &dirs).await.is_err(),
            "{path}"
        );
        assert!(dirs.asked.lock().unwrap().is_empty());
    }
}

#[tokio::test]
async fn local_dirs_lists_a_real_directory() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("present")).unwrap();
    let names = LocalDirs.list(dir.path().to_str().unwrap()).await.unwrap();
    assert_eq!(names, vec!["present".to_owned()]);
    assert!(LocalDirs.list("/nonexistent-wp09-dir").await.is_err());
}
