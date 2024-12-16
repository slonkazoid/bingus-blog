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
    pub title: String,
    pub description: String,
    pub author: String,
    pub icon: Option<String>,
    pub icon_alt: Option<String>,
    pub color: Option<String>,
    pub created_at: Option<DateTime<Utc>>,
    pub modified_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub tags: BTreeSet<String>,
    pub dont_cache: bool,
}

impl BlagMetadata {
    pub fn into_full(self, name: String) -> (PostMetadata, bool) {
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
        )
    }
}

pub struct Blag {
    root: Arc<Path>,
    blag_bin: Arc<Path>,
    cache: Option<Arc<CacheGuard>>,
    _fastblag: bool,
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
        name: &str,
        path: impl AsRef<Path>,
        query_json: String,
    ) -> Result<(PostMetadata, String, (Duration, Duration), bool), PostError> {
        let start = Instant::now();

        debug!(%name, "rendering");

        let mut cmd = tokio::process::Command::new(&*self.blag_bin)
            .arg(path.as_ref())
            .env("BLAG_QUERY", query_json)
            .stdout(Stdio::piped())
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
        let (meta, dont_cache) = blag_meta.into_full(name.to_string());
        let parsed = start.elapsed();

        let rendering = Instant::now();
        buf.clear();
        reader.read_to_string(&mut buf).await?;

        debug!("read output: {} bytes", buf.len());

        let exit_status = cmd.wait().await?;
        debug!("exited: {exit_status}");
        if !exit_status.success() {
            return Err(PostError::RenderError(exit_status.to_string()));
        }

        let rendered = rendering.elapsed();

        Ok((meta, buf, (parsed, rendered), dont_cache))
    }
}

#[async_trait]
impl PostManager for Blag {
    async fn get_all_posts(
        &self,
        filters: &[Filter<'_>],
        query: &IndexMap<String, Value>,
    ) -> Result<Vec<(PostMetadata, String, RenderStats)>, PostError> {
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
                let name = match entry.file_name().into_string() {
                    Ok(v) => v,
                    Err(_) => {
                        continue;
                    }
                };

                if name.ends_with(".sh") {
                    set.push(
                        async move { self.get_post(name.trim_end_matches(".sh"), query).await },
                    );
                }
            }
        }

        while let Some(result) = set.next().await {
            let post = match result {
                Ok(v) => match v {
                    ReturnedPost::Rendered(meta, content, stats) => (meta, content, stats),
                    ReturnedPost::Raw(..) => unreachable!(),
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
        name: &str,
        query: &IndexMap<String, Value>,
    ) -> Result<ReturnedPost, PostError> {
        let start = Instant::now();
        let mut path = self.root.join(name);

        if name.ends_with(".sh") {
            let mut buf = Vec::new();
            let mut file =
                OpenOptions::new()
                    .read(true)
                    .open(&path)
                    .await
                    .map_err(|err| match err.kind() {
                        std::io::ErrorKind::NotFound => PostError::NotFound(name.to_string()),
                        _ => PostError::IoError(err),
                    })?;
            file.read_to_end(&mut buf).await?;

            return Ok(ReturnedPost::Raw(
                buf,
                HeaderValue::from_static("text/x-shellscript"),
            ));
        } else {
            path.add_extension("sh");
        }

        let stat = tokio::fs::metadata(&path)
            .await
            .map_err(|err| match err.kind() {
                std::io::ErrorKind::NotFound => PostError::NotFound(name.to_string()),
                _ => PostError::IoError(err),
            })?;

        if !stat.is_file() {
            return Err(PostError::NotFound(name.to_string()));
        }

        let mtime = as_secs(&stat.modified()?);

        let query_json = serde_json::to_string(&query).expect("this should not fail");
        let mut hasher = DefaultHasher::new();
        query_json.hash(&mut hasher);
        let query_hash = hasher.finish();

        let post = if let Some(cache) = &self.cache {
            if let Some(CacheValue {
                metadata, rendered, ..
            }) = cache.lookup(name, mtime, query_hash).await
            {
                ReturnedPost::Rendered(metadata, rendered, RenderStats::Cached(start.elapsed()))
            } else {
                let (meta, content, (parsed, rendered), dont_cache) =
                    self.render(name, path, query_json).await?;

                if !dont_cache {
                    cache
                        .insert(
                            name.to_string(),
                            meta.clone(),
                            mtime,
                            content.clone(),
                            query_hash,
                        )
                        .await
                        .unwrap_or_else(|err| warn!("failed to insert {:?} into cache", err.0));
                }

                let total = start.elapsed();
                ReturnedPost::Rendered(
                    meta,
                    content,
                    RenderStats::Rendered {
                        total,
                        parsed,
                        rendered,
                    },
                )
            }
        } else {
            let (meta, content, (parsed, rendered), ..) =
                self.render(name, path, query_json).await?;

            let total = start.elapsed();
            ReturnedPost::Rendered(
                meta,
                content,
                RenderStats::Rendered {
                    total,
                    parsed,
                    rendered,
                },
            )
        };

        if let ReturnedPost::Rendered(.., stats) = &post {
            info!("rendered blagpost in {:?}", stats);
        }

        Ok(post)
    }

    async fn as_raw(&self, name: &str) -> Result<Option<String>, PostError> {
        let mut buf = String::with_capacity(name.len() + 3);
        buf += name;
        buf += ".sh";

        Ok(Some(buf))
    }
}
