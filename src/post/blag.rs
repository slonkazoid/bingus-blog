use std::collections::BTreeSet;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::Path;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use arc_swap::access::Access;
use axum::async_trait;
use axum::http::HeaderValue;
use chrono::{DateTime, Utc};
use futures::stream::FuturesUnordered;
use futures::{FutureExt, StreamExt};
use indexmap::IndexMap;
use serde::Deserialize;
use serde_value::Value;
use tokio::fs::OpenOptions;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
use tokio::time::Instant;
use tracing::{debug, error, info, instrument};

use crate::config::BlagConfig;
use crate::error::PostError;
use crate::post::Filter;
use crate::systemtime_as_secs::as_secs;

use super::cache::{CacheGuard, CacheValue};
use super::{ApplyFilters, PostManager, PostMetadata, RenderStats, ReturnedPost};

#[derive(Deserialize, Debug)]
struct BlagMetadata {
    pub title: Arc<str>,
    pub description: Arc<str>,
    pub author: Arc<str>,
    pub icon: Option<Arc<str>>,
    pub icon_alt: Option<Arc<str>>,
    pub color: Option<Arc<str>>,
    #[serde(alias = "created_at")]
    pub written_at: Option<DateTime<Utc>>,
    pub modified_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub tags: BTreeSet<Arc<str>>,
    pub dont_cache: bool,
    pub raw: Option<Arc<str>>,
}

impl BlagMetadata {
    pub fn into_full(self, name: Arc<str>) -> (PostMetadata, bool, Option<Arc<str>>) {
        (
            PostMetadata {
                name,
                title: self.title,
                description: self.description,
                author: self.author,
                icon: self.icon,
                icon_alt: self.icon_alt,
                color: self.color,
                written_at: self.written_at,
                modified_at: self.modified_at,
                tags: self.tags.into_iter().collect(),
            },
            self.dont_cache,
            self.raw,
        )
    }
}

pub struct Blag<A> {
    config: A,
    cache: Option<Arc<CacheGuard>>,
    _fastblag: bool,
}

enum RenderResult {
    Normal(PostMetadata, String, (Duration, Duration), bool),
    Raw(Vec<u8>, Arc<str>),
}

impl<A> Blag<A>
where
    A: Access<BlagConfig>,
    A: Sync,
    A::Guard: Send,
{
    pub fn new(config: A, cache: Option<Arc<CacheGuard>>) -> Self {
        Self {
            config,
            cache,
            _fastblag: false,
        }
    }

    async fn render(
        &self,
        name: Arc<str>,
        path: impl AsRef<Path>,
        query_json: String,
    ) -> Result<RenderResult, PostError> {
        let start = Instant::now();
        let bin = self.config.load().bin.clone();

        debug!(%name, "rendering");

        let mut cmd = tokio::process::Command::new(&*bin)
            .arg(path.as_ref())
            .env("BLAG_QUERY", query_json)
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .stdin(Stdio::null())
            .spawn()
            .map_err(|err| {
                error!("failed to spawn {bin:?}: {err}");
                err
            })?;

        let stdout = cmd.stdout.take().unwrap();

        let mut reader = BufReader::new(stdout);
        let mut buf = String::new();
        reader.read_line(&mut buf).await?;

        let blag_meta: BlagMetadata = serde_json::from_str(&buf)?;
        debug!("blag meta: {blag_meta:?}");
        let (meta, dont_cache, raw) = blag_meta.into_full(name);
        buf.clear();

        // this is morally reprehensible
        if let Some(raw) = raw {
            let mut buf = buf.into_bytes();
            reader.read_to_end(&mut buf).await?;
            return Ok(RenderResult::Raw(buf, raw));
        }

        let parsed = start.elapsed();
        let rendering = Instant::now();

        reader.read_to_string(&mut buf).await?;

        let status = cmd.wait().await?;
        debug!("exited: {status}");
        if !status.success() {
            return Err(PostError::RenderError(status.to_string()));
        }

        let rendered = rendering.elapsed();

        Ok(RenderResult::Normal(
            meta,
            buf,
            (parsed, rendered),
            dont_cache,
        ))
    }

    fn as_raw(name: &str) -> String {
        let mut buf = String::with_capacity(name.len() + 3);
        buf += name;
        buf += ".sh";

        buf
    }

    fn is_raw(name: &str) -> bool {
        name.ends_with(".sh")
    }
}

#[async_trait]
impl<A> PostManager for Blag<A>
where
    A: Access<BlagConfig>,
    A: Sync,
    A::Guard: Send,
{
    async fn get_all_posts(
        &self,
        filters: &[Filter<'_>],
        query: &IndexMap<String, Value>,
    ) -> Result<Vec<(PostMetadata, Arc<str>, RenderStats)>, PostError> {
        let root = &self.config.load().root;

        let mut set = FuturesUnordered::new();
        let mut posts = Vec::new();
        let mut files = tokio::fs::read_dir(&root).await?;

        loop {
            let entry = match files.next_entry().await {
                Ok(Some(v)) => v,
                Ok(None) => break,
                Err(err) => {
                    error!("error while getting next entry: {err}");
                    continue;
                }
            };

            let stat = tokio::fs::metadata(entry.path()).await?;

            if stat.is_file() {
                let mut name = match entry.file_name().into_string() {
                    Ok(v) => v,
                    Err(_) => {
                        continue;
                    }
                };

                if Self::is_raw(&name) {
                    name.truncate(name.len() - 3);
                    let name = name.into();
                    set.push(self.get_post(Arc::clone(&name), query).map(|v| (name, v)));
                }
            }
        }

        while let Some((name, result)) = set.next().await {
            let post = match result {
                Ok(v) => v,
                Err(err) => {
                    error!("error while rendering blagpost {name:?}: {err}");
                    continue;
                }
            };

            if let ReturnedPost::Rendered {
                meta, body, perf, ..
            } = post
                && meta.apply_filters(filters)
            {
                posts.push((meta, body, perf));
            }
        }

        debug!("collected posts");

        Ok(posts)
    }

    #[instrument(skip(self))]
    async fn get_post(
        &self,
        name: Arc<str>,
        query: &IndexMap<String, Value>,
    ) -> Result<ReturnedPost, PostError> {
        let start = Instant::now();
        let BlagConfig {
            ref root,
            ref raw_access,
            ..
        } = &*self.config.load();

        if Self::is_raw(&name) {
            let mut buffer = Vec::new();
            let mut file = OpenOptions::new()
                .read(true)
                .open(root.join(&*name))
                .await
                .map_err(|err| match err.kind() {
                    std::io::ErrorKind::NotFound => PostError::NotFound(name),
                    _ => PostError::IoError(err),
                })?;
            file.read_to_end(&mut buffer).await?;

            return Ok(ReturnedPost::Raw {
                buffer,
                content_type: HeaderValue::from_static("text/x-shellscript"),
            });
        }

        let raw_name = Self::as_raw(&name);
        let path = root.join(&raw_name);
        let raw_name = raw_access.then_some(raw_name);

        let stat = tokio::fs::metadata(&path)
            .await
            .map_err(|err| match err.kind() {
                std::io::ErrorKind::NotFound => PostError::NotFound(name.clone()),
                _ => PostError::IoError(err),
            })?;

        if !stat.is_file() {
            return Err(PostError::NotFound(name));
        }

        let mtime = as_secs(stat.modified()?);

        let query_json = serde_json::to_string(&query).expect("this should not fail");
        let mut hasher = DefaultHasher::new();
        query_json.hash(&mut hasher);
        let query_hash = hasher.finish();

        let post = if let Some(cache) = &self.cache
            && let Some(CacheValue { meta, body, .. }) =
                cache.lookup(name.clone(), mtime, query_hash).await
        {
            ReturnedPost::Rendered {
                meta,
                body,
                perf: RenderStats::Cached(start.elapsed()),
                raw_name,
            }
        } else {
            let (meta, content, (parsed, rendered), dont_cache) =
                match self.render(name.clone(), path, query_json).await? {
                    RenderResult::Normal(x, y, z, w) => (x, y, z, w),
                    RenderResult::Raw(buffer, content_type) => {
                        return Ok(ReturnedPost::Raw {
                            buffer,
                            content_type: HeaderValue::from_str(&content_type)
                                .map_err(Into::into)
                                .map_err(PostError::Other)?,
                        });
                    }
                };
            let body = content.into();

            if !dont_cache && let Some(cache) = &self.cache {
                cache
                    .insert(name, meta.clone(), mtime, Arc::clone(&body), query_hash)
                    .await;
            }

            let total = start.elapsed();
            ReturnedPost::Rendered {
                meta,
                body,
                perf: RenderStats::Rendered {
                    total,
                    parsed,
                    rendered,
                },
                raw_name,
            }
        };

        if let ReturnedPost::Rendered { perf, .. } = &post {
            info!("rendered blagpost in {:?}", perf);
        }

        Ok(post)
    }

    async fn cleanup(&self) {
        if let Some(cache) = &self.cache {
            let root = &self.config.load().root;
            cache
                .cleanup(|key, value| {
                    let mtime = std::fs::metadata(root.join(Self::as_raw(&key.name)))
                        .ok()
                        .and_then(|metadata| metadata.modified().ok())
                        .map(as_secs);

                    match mtime {
                        Some(mtime) => mtime <= value.mtime,
                        None => false,
                    }
                })
                .await
        }
    }
}
