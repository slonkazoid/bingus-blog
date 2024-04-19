use std::hash::{DefaultHasher, Hash, Hasher};

use scc::HashMap;
use serde::de::Visitor;
use serde::ser::SerializeMap;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::config::RenderConfig;
use crate::post::PostMetadata;

#[derive(Serialize, Deserialize, Clone)]
pub struct CacheValue {
    pub metadata: PostMetadata,
    pub rendered: String,
    pub mtime: u64,
    config_hash: u64,
}

#[derive(Default, Clone)]
pub struct Cache(HashMap<String, CacheValue>);

impl Cache {
    pub fn from_map(cache: HashMap<String, CacheValue>) -> Self {
        Self(cache)
    }

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

    #[inline(always)]
    pub fn into_inner(self) -> HashMap<String, CacheValue> {
        self.0
    }
}

impl Serialize for Cache {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let cache = self.clone().into_inner();
        let mut map = serializer.serialize_map(Some(cache.len()))?;
        let mut entry = cache.first_entry();
        while let Some(occupied) = entry {
            map.serialize_entry(occupied.key(), occupied.get())?;
            entry = occupied.next();
        }
        map.end()
    }
}

impl<'de> Deserialize<'de> for Cache {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct CoolVisitor;
        impl<'de> Visitor<'de> for CoolVisitor {
            type Value = Cache;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                write!(formatter, "expected a map")
            }

            fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
            where
                A: serde::de::MapAccess<'de>,
            {
                let cache = match map.size_hint() {
                    Some(size) => HashMap::with_capacity(size),
                    None => HashMap::new(),
                };

                while let Some((key, value)) = map.next_entry::<String, CacheValue>()? {
                    cache.insert(key, value).ok();
                }

                Ok(Cache::from_map(cache))
            }
        }

        deserializer.deserialize_map(CoolVisitor)
    }
}
