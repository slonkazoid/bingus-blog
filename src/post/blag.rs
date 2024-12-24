use std::collections::BTreeSet;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::Path;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use axum::async_trait;
use axum::http::HeaderValue;
use chrono::{DateTime, Utc};
use futures::stream::FuturesUnordered;
use futures::StreamExt;
use indexmap::IndexMap;
use serde::Deserialize;
use serde_value::Value;
use tokio::fs::OpenOptions;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
use tokio::time::Instant;
use tracing::{debug, error, info, instrument, warn};

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
    pub created_at: Option<DateTime<Utc>>,
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
                created_at: self.created_at,
                modified_at: self.modified_at,
                tags: self.tags.into_iter().collect(),
            },
            self.dont_cache,
            self.raw,
        )
    }
}

pub struct Blag {
    root: Arc<Path>,
    blag_bin: Arc<Path>,
    cache: Option<Arc<CacheGuard>>,
    _fastblag: bool,
}

enum RenderResult {
    Normal(PostMetadata, String, (Duration, Duration), bool),
    Raw(Vec<u8>, Arc<str>),
}

impl Blag {
    pub fn new(root: Arc<Path>, blag_bin: Arc<Path>, cache: Option<Arc<CacheGuard>>) -> Blag {
        Self {
            root,
            blag_bin,
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

        debug!(%name, "rendering");

        let mut cmd = tokio::process::Command::new(&*self.blag_bin)
            .arg(path.as_ref())
            .env("BLAG_QUERY", query_json)
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .stdin(Stdio::null())
            .spawn()
            .map_err(|err| {
                error!("failed to spawn {:?}: {err}", self.blag_bin);
                err
            })?;

        let stdout = cmd.stdout.take().unwrap();

        let mut reader = BufReader::new(stdout);
        let mut buf = String::new();
        reader.read_line(&mut buf).await?;

        let blag_meta: BlagMetadata = serde_json::from_str(&buf)?;
        debug!("blag meta: {blag_meta:?}");
        let (meta, dont_cache, raw) = blag_meta.into_full(name);

        // this is morally reprehensible
        if let Some(raw) = raw {
            let mut buf = buf.into_bytes();
            reader.read_to_end(&mut buf).await?;
            return Ok(RenderResult::Raw(buf, raw));
        }

        let parsed = start.elapsed();

        let rendering = Instant::now();

        buf.clear();
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
}

#[async_trait]
impl PostManager for Blag {
    async fn get_all_posts(
        &self,
        filters: &[Filter<'_>],
        query: &IndexMap<String, Value>,
    ) -> Result<Vec<(PostMetadata, Arc<str>, RenderStats)>, PostError> {
        let mut set = FuturesUnordered::new();
        let mut posts = Vec::new();
        let mut files = tokio::fs::read_dir(&self.root).await?;

        loop {
            let entry = match files.next_entry().await {
                Ok(Some(v)) => v,
                Ok(None) => break,
                Err(err) => {
                    error!("error while getting next entry: {err}");
                    continue;
                }
            };

            let file_type = entry.file_type().await?;
            if file_type.is_file() {
                let mut name = match entry.file_name().into_string() {
                    Ok(v) => v,
                    Err(_) => {
                        continue;
                    }
                };

                if self.is_raw(&name) {
                    name.truncate(name.len() - 3);
                    set.push(self.get_post(name.into(), query));
                }
            }
        }

        while let Some(result) = set.next().await {
            let post = match result {
                Ok(v) => match v {
                    ReturnedPost::Rendered { meta, body, perf } => (meta, body, perf),
                    ReturnedPost::Raw { .. } => unreachable!(),
                },
                Err(err) => {
                    error!("error while rendering blagpost: {err}");
                    continue;
                }
            };

            if post.0.apply_filters(filters) {
                posts.push(post);
            }
        }

        debug!("collected posts");

        Ok(posts)
    }

    #[instrument(level = "info", skip(self))]
    async fn get_post(
        &self,
        name: Arc<str>,
        query: &IndexMap<String, Value>,
    ) -> Result<ReturnedPost, PostError> {
        let start = Instant::now();
        let mut path = self.root.join(&*name);

        if self.is_raw(&name) {
            let mut buffer = Vec::new();
            let mut file =
                OpenOptions::new()
                    .read(true)
                    .open(&path)
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
        } else {
            path.add_extension("sh");
        }

        let stat = tokio::fs::metadata(&path)
            .await
            .map_err(|err| match err.kind() {
                std::io::ErrorKind::NotFound => PostError::NotFound(name.clone()),
                _ => PostError::IoError(err),
            })?;

        if !stat.is_file() {
            return Err(PostError::NotFound(name));
        }

        let mtime = as_secs(&stat.modified()?);

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
                    .await
                    .unwrap_or_else(|err| warn!("failed to insert {:?} into cache", err.0));
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
            }
        };

        if let ReturnedPost::Rendered { perf, .. } = &post {
            info!("rendered blagpost in {:?}", perf);
        }

        Ok(post)
    }

    async fn cleanup(&self) {
        if let Some(cache) = &self.cache {
            cache
                .retain(|key, value| {
                    let mtime = std::fs::metadata(
                        self.root
                            .join(self.as_raw(&key.name).unwrap_or_else(|| unreachable!())),
                    )
                    .ok()
                    .and_then(|metadata| metadata.modified().ok())
                    .map(|mtime| as_secs(&mtime));

                    match mtime {
                        Some(mtime) => mtime <= value.mtime,
                        None => false,
                    }
                })
                .await
        }
    }

    fn is_raw(&self, name: &str) -> bool {
        name.ends_with(".sh")
    }

    fn as_raw(&self, name: &str) -> Option<String> {
        let mut buf = String::with_capacity(name.len() + 3);
        buf += name;
        buf += ".sh";

        Some(buf)
    }
}
