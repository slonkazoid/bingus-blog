use std::fmt::Debug;
use std::io::{Read, Write};
use std::num::NonZeroU64;
use std::ops::Deref;
use std::sync::Arc;
use std::time::SystemTime;

use crate::config::CacheConfig;
use crate::post::PostMetadata;
use color_eyre::eyre::{self, Context};
use scc::HashMap;
use serde::{Deserialize, Serialize};
use tokio::io::AsyncReadExt;
use tracing::{debug, info, instrument, trace, Span};

/// do not persist cache if this version number changed
pub const CACHE_VERSION: u16 = 5;

fn now() -> u128 {
    crate::systemtime_as_secs::as_millis(SystemTime::now())
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct CacheValue {
    pub meta: PostMetadata,
    pub body: Arc<str>,
    pub mtime: u64,
    /// when the item was inserted into cache, in milliseconds since epoch
    pub cached_at: u128,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct Cache {
    map: HashMap<CacheKey, CacheValue>,
    version: u16,
    ttl: Option<NonZeroU64>,
}

#[derive(Serialize, Deserialize, Hash, Eq, PartialEq, Clone, Debug)]
#[repr(C)]
pub struct CacheKey {
    pub name: Arc<str>,
    pub extra: u64,
}

impl Cache {
    pub fn new(ttl: Option<NonZeroU64>) -> Self {
        Cache {
            map: Default::default(),
            version: CACHE_VERSION,
            ttl,
        }
    }

    fn up_to_date(&self, cached: &CacheValue, mtime: u64) -> bool {
        mtime <= cached.mtime
            && self
                .ttl
                .is_none_or(|ttl| cached.cached_at + u64::from(ttl) as u128 >= now())
    }

    #[instrument(level = "debug", skip(self), fields(entry_mtime))]
    pub async fn lookup(&self, name: Arc<str>, mtime: u64, extra: u64) -> Option<CacheValue> {
        trace!("looking up in cache");
        match self.map.get_async(&CacheKey { name, extra }).await {
            Some(entry) => {
                let cached = entry.get();
                Span::current().record("entry_mtime", cached.mtime);
                trace!("found in cache");
                if self.up_to_date(cached, mtime) {
                    trace!("entry up-to-date");
                    Some(cached.clone())
                } else {
                    let _ = entry.remove();
                    debug!("removed stale entry");
                    None
                }
            }
            None => None,
        }
    }

    #[instrument(level = "debug", skip(self), fields(entry_mtime))]
    pub async fn lookup_metadata(
        &self,
        name: Arc<str>,
        mtime: u64,
        extra: u64,
    ) -> Option<PostMetadata> {
        trace!("looking up metadata in cache");
        match self.map.get_async(&CacheKey { name, extra }).await {
            Some(entry) => {
                let cached = entry.get();
                Span::current().record("entry_mtime", cached.mtime);
                if self.up_to_date(cached, mtime) {
                    trace!("entry up-to-date");
                    Some(cached.meta.clone())
                } else {
                    let _ = entry.remove();
                    debug!("removed stale entry");
                    None
                }
            }
            None => None,
        }
    }

    #[instrument(level = "debug", skip(self))]
    pub async fn insert(
        &self,
        name: Arc<str>,
        metadata: PostMetadata,
        mtime: u64,
        rendered: Arc<str>,
        extra: u64,
    ) -> Option<CacheValue> {
        trace!("inserting into cache");

        let r = self
            .map
            .upsert_async(
                CacheKey { name, extra },
                CacheValue {
                    meta: metadata,
                    body: rendered,
                    mtime,
                    cached_at: now(),
                },
            )
            .await;

        debug!(
            "{} cache",
            match r {
                Some(_) => "updated in",
                None => "inserted into",
            }
        );

        r
    }

    #[instrument(level = "debug", skip(self))]
    #[allow(unused)]
    pub async fn remove(&self, name: Arc<str>, extra: u64) -> Option<(CacheKey, CacheValue)> {
        trace!("removing from cache");

        let r = self.map.remove_async(&CacheKey { name, extra }).await;

        debug!(
            "item {} cache",
            match r {
                Some(_) => "removed from",
                None => "did not exist in",
            }
        );

        r
    }

    pub async fn retain(&self, predicate: impl Fn(&CacheKey, &CacheValue) -> bool) {
        let old_size = self.map.len();
        let mut i = 0;

        // TODO: multithread
        // not urgent as this is run concurrently anyways
        self.map
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

    #[instrument(level = "debug", skip_all)]
    pub async fn cleanup(&self, predicate: impl Fn(&CacheKey, &CacheValue) -> bool) {
        self.retain(|k, v| {
            self.ttl
                .is_none_or(|ttl| v.cached_at + u64::from(ttl) as u128 >= now())
                && predicate(k, v)
        })
        .await
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    #[inline(always)]
    pub fn version(&self) -> u16 {
        self.version
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
