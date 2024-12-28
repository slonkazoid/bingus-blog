use std::io::{Read, Write};
use std::ops::Deref;
use std::sync::Arc;

use crate::config::CacheConfig;
use crate::post::PostMetadata;
use color_eyre::eyre::{self, Context};
use scc::HashMap;
use serde::{Deserialize, Serialize};
use tokio::io::AsyncReadExt;
use tracing::{debug, info, instrument};

/// do not persist cache if this version number changed
pub const CACHE_VERSION: u16 = 5;

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct CacheValue {
    pub meta: PostMetadata,
    pub body: Arc<str>,
    pub mtime: u64,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct Cache(HashMap<CacheKey, CacheValue>, u16);

impl Default for Cache {
    fn default() -> Self {
        Self(Default::default(), CACHE_VERSION)
    }
}

#[derive(Serialize, Deserialize, Hash, Eq, PartialEq, Clone, Debug)]
#[repr(C)]
pub struct CacheKey {
    pub name: Arc<str>,
    pub extra: u64,
}

impl Cache {
    pub async fn lookup(&self, name: Arc<str>, mtime: u64, extra: u64) -> Option<CacheValue> {
        match self.0.get_async(&CacheKey { name, extra }).await {
            Some(entry) => {
                let cached = entry.get();
                if mtime <= cached.mtime {
                    Some(cached.clone())
                } else {
                    let _ = entry.remove();
                    None
                }
            }
            None => None,
        }
    }

    pub async fn lookup_metadata(
        &self,
        name: Arc<str>,
        mtime: u64,
        extra: u64,
    ) -> Option<PostMetadata> {
        match self.0.get_async(&CacheKey { name, extra }).await {
            Some(entry) => {
                let cached = entry.get();
                if mtime <= cached.mtime {
                    Some(cached.meta.clone())
                } else {
                    let _ = entry.remove();
                    None
                }
            }
            None => None,
        }
    }

    pub async fn insert(
        &self,
        name: Arc<str>,
        metadata: PostMetadata,
        mtime: u64,
        rendered: Arc<str>,
        extra: u64,
    ) -> Option<CacheValue> {
        self.0
            .upsert_async(
                CacheKey { name, extra },
                CacheValue {
                    meta: metadata,
                    body: rendered,
                    mtime,
                },
            )
            .await
    }

    #[allow(unused)]
    pub async fn remove(&self, name: Arc<str>, extra: u64) -> Option<(CacheKey, CacheValue)> {
        self.0.remove_async(&CacheKey { name, extra }).await
    }

    #[instrument(name = "cleanup", skip_all)]
    pub async fn retain(&self, predicate: impl Fn(&CacheKey, &CacheValue) -> bool) {
        let old_size = self.0.len();
        let mut i = 0;

        // TODO: multithread
        // not urgent as this is run concurrently anyways
        self.0
            .retain_async(|k, v| {
                if predicate(k, v) {
                    true
                } else {
                    debug!("removing {k:?} from cache");
                    i += 1;
                    false
                }
            })
            .await;

        let new_size = self.len();
        debug!("removed {i} entries ({old_size} -> {new_size} entries)");
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    #[inline(always)]
    pub fn version(&self) -> u16 {
        self.1
    }
}

pub struct CacheGuard {
    inner: Cache,
    config: CacheConfig,
}

impl CacheGuard {
    pub fn new(cache: Cache, config: CacheConfig) -> Self {
        Self {
            inner: cache,
            config,
        }
    }

    fn try_drop(&mut self) -> Result<(), eyre::Report> {
        // write cache to file
        let path = &self.config.file;
        let serialized = bitcode::serialize(&self.inner).context("failed to serialize cache")?;
        let mut cache_file = std::fs::File::create(path)
            .with_context(|| format!("failed to open cache at {}", path.display()))?;
        let compression_level = self.config.compression_level;
        if self.config.compress {
            std::io::Write::write_all(
                &mut zstd::stream::write::Encoder::new(cache_file, compression_level)?
                    .auto_finish(),
                &serialized,
            )
        } else {
            cache_file.write_all(&serialized)
        }
        .context("failed to write cache to file")?;
        info!("wrote cache to {}", path.display());
        Ok(())
    }
}

impl Deref for CacheGuard {
    type Target = Cache;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl AsRef<Cache> for CacheGuard {
    fn as_ref(&self) -> &Cache {
        &self.inner
    }
}

impl Drop for CacheGuard {
    fn drop(&mut self) {
        self.try_drop().expect("cache to save successfully")
    }
}

pub(crate) async fn load_cache(config: &CacheConfig) -> Result<Cache, eyre::Report> {
    let path = &config.file;
    let mut cache_file = tokio::fs::File::open(&path)
        .await
        .context("failed to open cache file")?;
    let serialized = if config.compress {
        let cache_file = cache_file.into_std().await;
        tokio::task::spawn_blocking(move || {
            let mut buf = Vec::with_capacity(4096);
            zstd::stream::read::Decoder::new(cache_file)?.read_to_end(&mut buf)?;
            Ok::<_, std::io::Error>(buf)
        })
        .await?
        .context("failed to read cache file")?
    } else {
        let mut buf = Vec::with_capacity(4096);
        cache_file
            .read_to_end(&mut buf)
            .await
            .context("failed to read cache file")?;
        buf
    };

    bitcode::deserialize(serialized.as_slice()).context("failed to parse cache")
}
