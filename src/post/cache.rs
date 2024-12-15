use std::io::{Read, Write};
use std::ops::Deref;

use crate::config::CacheConfig;
use crate::post::PostMetadata;
use color_eyre::eyre::{self, Context};
use scc::HashMap;
use serde::{Deserialize, Serialize};
use tokio::io::AsyncReadExt;
use tracing::{debug, info, instrument};

/// do not persist cache if this version number changed
pub const CACHE_VERSION: u16 = 2;

#[derive(Serialize, Deserialize, Clone)]
pub struct CacheValue {
    pub metadata: PostMetadata,
    pub rendered: String,
    pub mtime: u64,
    extra: u64,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct FileCache(HashMap<String, CacheValue>, u16);

impl Default for FileCache {
    fn default() -> Self {
        Self(Default::default(), CACHE_VERSION)
    }
}

impl FileCache {
    pub async fn lookup(&self, name: &str, mtime: u64, extra: u64) -> Option<CacheValue> {
        match self.0.get_async(name).await {
            Some(entry) => {
                let cached = entry.get();
                if extra == cached.extra && mtime <= cached.mtime {
                    Some(cached.clone())
                } else {
                    let _ = entry.remove();
                    None
                }
            }
            None => None,
        }
    }

    pub async fn lookup_metadata(&self, name: &str, mtime: u64) -> Option<PostMetadata> {
        match self.0.get_async(name).await {
            Some(entry) => {
                let cached = entry.get();
                if mtime <= cached.mtime {
                    Some(cached.metadata.clone())
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
        name: String,
        metadata: PostMetadata,
        mtime: u64,
        rendered: String,
        extra: u64,
    ) -> Result<(), (String, (PostMetadata, String))> {
        let value = CacheValue {
            metadata,
            rendered,
            mtime,
            extra,
        };

        if self
            .0
            .update_async(&name, |_, _| value.clone())
            .await
            .is_none()
        {
            self.0
                .insert_async(name, value)
                .await
                .map_err(|x| (x.0, (x.1.metadata, x.1.rendered)))
        } else {
            Ok(())
        }
    }

    pub async fn remove(&self, name: &str) -> Option<(String, CacheValue)> {
        self.0.remove_async(name).await
    }

    #[instrument(name = "cleanup", skip_all)]
    pub async fn cleanup(&self, get_mtime: impl Fn(&str) -> Option<u64>) {
        let old_size = self.0.len();
        let mut i = 0;

        // TODO: multithread
        self.0
            .retain_async(|k, v| {
                if get_mtime(k).is_some_and(|mtime| mtime == v.mtime) {
                    true
                } else {
                    debug!("removing {k} from cache");
                    i += 1;
                    false
                }
            })
            .await;

        let new_size = self.0.len();
        debug!("removed {i} entries ({old_size} -> {new_size} entries)");
    }

    #[inline(always)]
    pub fn version(&self) -> u16 {
        self.1
    }
}

pub struct CacheGuard {
    inner: FileCache,
    config: CacheConfig,
}

impl CacheGuard {
    pub fn new(cache: FileCache, config: CacheConfig) -> Self {
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
    type Target = FileCache;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl AsRef<FileCache> for CacheGuard {
    fn as_ref(&self) -> &FileCache {
        &self.inner
    }
}

impl Drop for CacheGuard {
    fn drop(&mut self) {
        self.try_drop().expect("cache to save successfully")
    }
}

pub(crate) async fn load_cache(config: &CacheConfig) -> Result<FileCache, eyre::Report> {
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
