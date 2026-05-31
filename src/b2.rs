//! The remote object store used by `backup` and `status --remote`.
//!
//! [`RemoteStore`] is trait-abstracted so command logic can be tested against a fake
//! ("already present, same size") implementation without touching the network. The real
//! implementation, [`B2Store`], talks to Backblaze B2 via its S3-compatible endpoint
//! using the `object_store` crate (which uses rustls, so no native crypto build is needed).

use std::path::Path;
use std::sync::Arc;

use object_store::aws::AmazonS3Builder;
use object_store::buffered::BufWriter;
use object_store::path::Path as ObjectPath;
use object_store::{ObjectStore, ObjectStoreExt};
use thiserror::Error;
use tokio::io::AsyncWriteExt;

use crate::config::{BackupConfig, Credentials};

#[derive(Debug, Error)]
pub enum B2Error {
    #[error("object store error: {0}")]
    Store(#[from] object_store::Error),
    #[error("local I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("failed to build S3 client: {0}")]
    Build(String),
}

/// A minimal, idempotency-friendly object store: check existence + size, and upload.
///
/// Implementors must never delete anything — `backup` is append-only on the remote side.
#[allow(async_fn_in_trait)] // intentionally not `dyn`-used; callers are generic over `S`.
pub trait RemoteStore {
    /// Return `Some(size_in_bytes)` if `key` exists remotely, or `None` if it does not.
    async fn head(&self, key: &str) -> Result<Option<u64>, B2Error>;

    /// Upload the file at `path` to `key`, creating or overwriting the object.
    async fn put(&self, key: &str, path: &Path) -> Result<(), B2Error>;
}

/// Backblaze B2 store over the S3-compatible API.
pub struct B2Store {
    store: Arc<dyn ObjectStore>,
}

impl B2Store {
    /// Build a client for `cfg`'s endpoint/region/bucket using `credentials`.
    ///
    /// Uses path-style addressing (`endpoint/bucket/key`), which B2's regional S3 endpoint
    /// supports — that is `object_store`'s default when an explicit endpoint is set.
    pub fn new(cfg: &BackupConfig, credentials: &Credentials) -> Result<Self, B2Error> {
        let s3 = AmazonS3Builder::new()
            .with_endpoint(&cfg.endpoint)
            .with_region(&cfg.region)
            .with_bucket_name(&cfg.bucket)
            .with_access_key_id(&credentials.key_id)
            .with_secret_access_key(&credentials.app_key)
            .build()
            .map_err(|e| B2Error::Build(e.to_string()))?;
        Ok(Self {
            store: Arc::new(s3),
        })
    }
}

impl RemoteStore for B2Store {
    async fn head(&self, key: &str) -> Result<Option<u64>, B2Error> {
        let location = ObjectPath::from(key);
        match self.store.head(&location).await {
            Ok(meta) => Ok(Some(meta.size)),
            Err(object_store::Error::NotFound { .. }) => Ok(None),
            Err(other) => Err(B2Error::Store(other)),
        }
    }

    async fn put(&self, key: &str, path: &Path) -> Result<(), B2Error> {
        let location = ObjectPath::from(key);
        // `BufWriter` automatically switches to a multipart upload for large files
        // (videos), so we get resumable-on-retry semantics with a single code path.
        let mut writer = BufWriter::new(self.store.clone(), location);
        let mut file = tokio::fs::File::open(path).await?;
        tokio::io::copy(&mut file, &mut writer).await?;
        writer.shutdown().await?;
        Ok(())
    }
}
