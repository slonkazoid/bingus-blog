use std::hash::{DefaultHasher, Hash, Hasher};

use scc::HashMap;
use serde::{Deserialize, Serialize};
use tracing::instrument;

use crate::config::RenderConfig;
use crate::post::PostMetadata;

#[derive(Serialize, Deserialize, Clone)]
pub struct CacheValue {
    pub metadata: PostMetadata,
    pub rendered: String,
    pub mtime: u64,
    config_hash: u64,
}

#[derive(Serialize, Deserialize, Default, Clone)]
pub struct Cache(HashMap<String, CacheValue>);

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

        self.0
            .retain_async(|k, v| {
                if get_mtime(k).is_some_and(|mtime| mtime == v.mtime) {
                    true
                } else {
                    tracing::debug!("removing {k} from cache");
                    i += 1;
                    false
                }
            })
            .await;

        let new_size = self.0.len();
        tracing::debug!("removed {i} entries ({old_size} -> {new_size} entries)");
    }
}
