//! Re-run / no-op idempotency tests for `month`, `export`, `clean`, and `backup`.
//!
//! `clean` and `backup` are tested through their trait seams (a fake `Trasher` that just
//! removes files, and a fake `RemoteStore` backed by an in-memory map) so the tests are
//! hermetic — they never touch the real OS trash or the network.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use photoflow::b2::{B2Error, RemoteStore};
use photoflow::commands::backup::{self, Candidate};
use photoflow::commands::clean::{self, Trasher};
use photoflow::commands::{export, month};
use photoflow::config::Config;
use tempfile::TempDir;

/// Build a config whose roots point inside `dir` (no `~`, so expansion is a no-op).
fn test_config(dir: &Path) -> Config {
    let mut cfg = Config::defaults();
    cfg.paths.photos_root = dir.join("photos").to_string_lossy().into_owned();
    cfg.paths.exports_root = dir.join("exports").to_string_lossy().into_owned();
    cfg
}

fn touch(path: &Path, contents: &[u8]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, contents).unwrap();
}

#[test]
fn month_is_idempotent_and_returns_same_path() {
    let tmp = TempDir::new().unwrap();
    let cfg = test_config(tmp.path());

    month::run(&cfg, Some("2026-06".to_string())).unwrap();
    let dir = cfg.photos_root().join("2026").join("06-June");
    assert!(dir.is_dir());

    // Second run must not error and the directory must still be there.
    month::run(&cfg, Some("2026-06".to_string())).unwrap();
    assert!(dir.is_dir());
}

#[test]
fn export_is_idempotent_and_returns_same_path() {
    let tmp = TempDir::new().unwrap();
    let cfg = test_config(tmp.path());

    export::run(
        &cfg,
        "instagram".to_string(),
        "Charlie Bean!".to_string(),
        Some("2026-06-15".to_string()),
    )
    .unwrap();
    let dir = cfg
        .exports_root()
        .join("instagram")
        .join("2026-06-15-charlie-bean");
    assert!(dir.is_dir());

    export::run(
        &cfg,
        "instagram".to_string(),
        "Charlie Bean!".to_string(),
        Some("2026-06-15".to_string()),
    )
    .unwrap();
    assert!(dir.is_dir());
}

/// Fake trasher: simulates the OS trash by removing the files, and records what it moved.
struct FakeTrasher {
    moved: RefCell<Vec<PathBuf>>,
}

impl Trasher for FakeTrasher {
    fn trash(&self, paths: &[PathBuf]) -> anyhow::Result<()> {
        for p in paths {
            std::fs::remove_file(p).unwrap();
            self.moved.borrow_mut().push(p.clone());
        }
        Ok(())
    }
}

#[test]
fn clean_trashes_only_discard_files_and_sidecars_and_is_idempotent() {
    let tmp = TempDir::new().unwrap();
    let cfg = test_config(tmp.path());
    let root = cfg.photos_root();

    let jpg = root.join("2026/06-June/DSC_0001.jpg");
    let jpg_sidecar = root.join("2026/06-June/DSC_0001.jpg.xmp");
    let nef = root.join("2026/06-June/DSC_0001.NEF");
    let mov = root.join("2026/06-June/DSC_0002.MOV");
    let heic = root.join("2026/06-June/DSC_0003.HEIC");
    touch(&jpg, b"jpeg");
    touch(&jpg_sidecar, b"sidecar");
    touch(&nef, b"raw-data");
    touch(&mov, b"video");
    touch(&heic, b"heic");

    // Dry run (apply = false) changes nothing.
    let plan = clean::plan_clean(
        &root,
        &cfg.clean.discard_extensions,
        cfg.clean.discard_sidecars,
        &cfg.clean.exclude_dirs,
    );
    assert_eq!(plan.len(), 2, "jpg + its sidecar");
    assert!(jpg.exists() && jpg_sidecar.exists());

    // Apply via the fake trasher.
    let trasher = FakeTrasher {
        moved: RefCell::new(Vec::new()),
    };
    let victims: Vec<PathBuf> = plan.iter().map(|v| v.path.clone()).collect();
    trasher.trash(&victims).unwrap();

    assert!(!jpg.exists(), "jpg trashed");
    assert!(!jpg_sidecar.exists(), "sidecar trashed");
    assert!(nef.exists(), "RAW untouched");
    assert!(mov.exists(), "video untouched");
    assert!(heic.exists(), "HEIC untouched");

    // Second pass is a no-op: nothing left to trash.
    let plan2 = clean::plan_clean(
        &root,
        &cfg.clean.discard_extensions,
        cfg.clean.discard_sidecars,
        &cfg.clean.exclude_dirs,
    );
    assert!(plan2.is_empty());
}

#[test]
fn clean_never_descends_into_excluded_export_dirs() {
    let tmp = TempDir::new().unwrap();
    let cfg = test_config(tmp.path());
    let root = cfg.photos_root();

    // A throwaway JPEG in the originals area...
    let original_jpg = root.join("2026/06-June/DSC_0001.jpg");
    // ...and finished export JPEGs nested under photos_root (the real-world layout).
    let export_jpg = root.join("Exports/Mexico-2026/mexico-exports-2026-1.jpg");
    let export_jpg2 = root.join("exports/lowercase-dir/pic.JPG");
    touch(&original_jpg, b"throwaway");
    touch(&export_jpg, b"finished");
    touch(&export_jpg2, b"finished");

    let victims = clean::plan_clean(
        &root,
        &cfg.clean.discard_extensions,
        cfg.clean.discard_sidecars,
        &cfg.clean.exclude_dirs,
    );

    let paths: Vec<_> = victims.iter().map(|v| v.path.clone()).collect();
    assert_eq!(
        paths,
        vec![original_jpg],
        "only the originals-area JPEG is a victim"
    );
    assert!(
        export_jpg.exists() && export_jpg2.exists(),
        "exports untouched"
    );
}

/// Fake remote store: an in-memory `key -> size` map. `put` records the uploaded size, so
/// a second `plan` against the same tree sees everything as already-present.
#[derive(Default)]
struct FakeStore {
    objects: RefCell<HashMap<String, u64>>,
}

impl RemoteStore for FakeStore {
    async fn head(&self, key: &str) -> Result<Option<u64>, B2Error> {
        Ok(self.objects.borrow().get(key).copied())
    }

    async fn put(&self, key: &str, path: &Path) -> Result<(), B2Error> {
        let size = std::fs::metadata(path)?.len();
        self.objects.borrow_mut().insert(key.to_string(), size);
        Ok(())
    }
}

#[tokio::test(flavor = "current_thread")]
async fn backup_uploads_once_then_skips() {
    let tmp = TempDir::new().unwrap();
    let cfg = test_config(tmp.path());
    let root = cfg.photos_root();

    touch(&root.join("2026/06-June/DSC_0001.NEF"), b"raw-data");
    touch(&root.join("2026/06-June/DSC_0001.NEF.xmp"), b"edits");
    touch(&root.join("2026/06-June/clip.MOV"), b"video-bytes");
    // Not a backup extension -> excluded.
    touch(&root.join("2026/06-June/DSC_0001.jpg"), b"jpeg");
    // Hidden/system file -> excluded.
    touch(&root.join("2026/06-June/.DS_Store"), b"junk");

    let candidates = backup::gather_candidates(&cfg).unwrap();
    assert_eq!(candidates.len(), 3, "nef + xmp + mov");
    // Keys are forward-slash, relative to photos_root.
    assert!(candidates
        .iter()
        .any(|c| c.key == "2026/06-June/DSC_0001.NEF"));

    let store = FakeStore::default();

    // First pass: everything is missing -> all upload.
    let plan1 = backup::plan(&store, &candidates, None).await.unwrap();
    assert_eq!(plan1.to_upload.len(), 3);
    assert_eq!(plan1.skipped, 0);
    let progress = backup::Progress::silent();
    backup::upload(
        &store,
        &plan1.to_upload,
        &progress,
        4,
        backup::RetryPolicy::none(),
    )
    .await
    .unwrap();
    assert_eq!(progress.files_done(), 3);

    // Second pass: all present with matching sizes -> zero uploads.
    let plan2 = backup::plan(&store, &candidates, None).await.unwrap();
    assert!(
        plan2.to_upload.is_empty(),
        "idempotent re-run uploads nothing"
    );
    assert_eq!(plan2.skipped, 3);

    // Size mismatch forces a re-upload of just that one file.
    let mismatched = candidates[0].key.clone();
    store
        .objects
        .borrow_mut()
        .insert(mismatched.clone(), 999_999);
    let plan3 = backup::plan(&store, &candidates, None).await.unwrap();
    assert_eq!(plan3.to_upload.len(), 1);
    assert_eq!(plan3.to_upload[0].key, mismatched);
}

#[test]
fn needs_upload_logic() {
    assert!(backup::needs_upload(None, 10), "missing -> upload");
    assert!(backup::needs_upload(Some(9), 10), "size mismatch -> upload");
    assert!(
        !backup::needs_upload(Some(10), 10),
        "present + same size -> skip"
    );
}

#[test]
fn backup_skip_extensions_excludes_those_files() {
    let tmp = TempDir::new().unwrap();
    let mut cfg = test_config(tmp.path());
    // Don't want videos uploaded.
    cfg.backup.skip_extensions = vec!["mov".to_string(), "mp4".to_string()];
    let root = cfg.photos_root();

    touch(&root.join("2026/06-June/DSC_0001.NEF"), b"raw");
    touch(&root.join("2026/06-June/clip.MOV"), b"video"); // skip wins (case-insensitive)
    touch(&root.join("2026/06-June/reel.mp4"), b"video");

    let candidates = backup::gather_candidates(&cfg).unwrap();
    let keys: Vec<&str> = candidates.iter().map(|c| c.key.as_str()).collect();
    assert_eq!(keys, vec!["2026/06-June/DSC_0001.NEF"], "videos skipped");

    // The helper is the single source of truth for the decision.
    assert!(backup::is_eligible(
        Path::new("a.NEF"),
        &cfg.backup.backup_extensions,
        &cfg.backup.skip_extensions
    ));
    assert!(!backup::is_eligible(
        Path::new("a.mov"),
        &cfg.backup.backup_extensions,
        &cfg.backup.skip_extensions
    ));
}

/// Store that fails `put` a fixed number of times per key before succeeding, to prove the
/// upload retries transient errors instead of aborting.
#[derive(Default)]
struct FlakyStore {
    remaining_failures: RefCell<usize>,
    puts_attempted: RefCell<usize>,
}

impl RemoteStore for FlakyStore {
    async fn head(&self, _key: &str) -> Result<Option<u64>, B2Error> {
        Ok(None)
    }

    async fn put(&self, _key: &str, _path: &Path) -> Result<(), B2Error> {
        *self.puts_attempted.borrow_mut() += 1;
        let mut left = self.remaining_failures.borrow_mut();
        if *left > 0 {
            *left -= 1;
            // A remote-style error is transient and should be retried.
            return Err(B2Error::Build("simulated transient failure".to_string()));
        }
        Ok(())
    }
}

#[tokio::test(flavor = "current_thread")]
async fn upload_retries_transient_errors() {
    let items = vec![Candidate {
        key: "k".into(),
        path: PathBuf::from("/does/not/matter"),
        size: 1,
    }];
    let store = FlakyStore {
        remaining_failures: RefCell::new(2),
        ..Default::default()
    };
    let progress = backup::Progress::silent();
    let policy = backup::RetryPolicy {
        attempts: 3,
        base: std::time::Duration::from_millis(10),
        max: std::time::Duration::from_millis(50),
    };

    // `Build` is treated as transient by the retry logic, so 2 failures + 1 success = ok.
    backup::upload(&store, &items, &progress, 1, policy)
        .await
        .unwrap();

    assert_eq!(*store.puts_attempted.borrow(), 3, "2 retries then success");
    assert_eq!(progress.files_done(), 1);
}

/// A `Candidate` is constructible from outside (used implicitly above via gather), but
/// assert key construction directly too.
#[test]
fn object_key_uses_forward_slashes_and_prefix() {
    let root = Path::new("/photos");
    let file = Path::new("/photos/2026/06-June/DSC.NEF");
    assert_eq!(
        backup::object_key(root, file, "").unwrap(),
        "2026/06-June/DSC.NEF"
    );
    assert_eq!(
        backup::object_key(root, file, "originals").unwrap(),
        "originals/2026/06-June/DSC.NEF"
    );
    // A bare Candidate value is part of the public API.
    let _ = Candidate {
        key: "k".into(),
        path: PathBuf::from("/x"),
        size: 1,
    };
}
