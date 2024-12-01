use std::hash::{DefaultHasher, Hash, Hasher};
use std::io::Read;

use crate::config::{Config, RenderConfig};
use crate::post::PostMetadata;
use color_eyre::eyre::{self, Context};
use scc::HashMap;
use serde::{Deserialize, Serialize};
use tokio::io::AsyncReadExt;
use tracing::{debug, instrument};

/// do not persist cache if this version number changed
pub const CACHE_VERSION: u16 = 2;

#[derive(Serialize, Deserialize, Clone)]
pub struct CacheValue {
    pub metadata: PostMetadata,
    pub rendered: String,
    pub mtime: u64,
    config_hash: u64,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct Cache(HashMap<String, CacheValue>, u16);

impl Default for Cache {
    fn default() -> Self {
        Self(Default::default(), CACHE_VERSION)
    }
}

impl Cache {
    pub async fn lookup(
        &self,
        name: &str,
        mtime: u64,
        config: &RenderConfig,
    ) -> Option<CacheValue> {
        match self.0.get_async(name).await {
            Some(entry) => {
                let cached = entry.get();
                if mtime <= cached.mtime && {
                    let mut hasher = DefaultHasher::new();
                    config.hash(&mut hasher);
                    hasher.finish()
                } == cached.config_hash
                {
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
        config: &RenderConfig,
    ) -> Result<(), (String, (PostMetadata, String))> {
        let mut hasher = DefaultHasher::new();
        config.hash(&mut hasher);
        let hash = hasher.finish();

        let value = CacheValue {
            metadata,
            rendered,
            mtime,
            config_hash: hash,
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

pub(crate) async fn load_cache(config: &Config) -> Result<Cache, eyre::Report> {
    let path = &config.cache.file;
    let mut cache_file = tokio::fs::File::open(&path)
        .await
        .context("failed to open cache file")?;
    let serialized = if config.cache.compress {
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
