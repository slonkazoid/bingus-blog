use std::collections::BTreeSet;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::io;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;
use std::time::SystemTime;

use axum::async_trait;
use axum::http::HeaderValue;
use chrono::{DateTime, Utc};
use color_eyre::eyre::{self, Context};
use comrak::plugins::syntect::SyntectAdapter;
use fronma::parser::{parse, ParsedData};
use indexmap::IndexMap;
use serde::Deserialize;
use serde_value::Value;
use tokio::fs;
use tokio::io::AsyncReadExt;
use tracing::{info, instrument, warn};

use crate::config::Config;
use crate::markdown_render::{build_syntect, render};
use crate::systemtime_as_secs::as_secs;

use super::cache::{CacheGuard, CacheValue};
use super::{
    ApplyFilters, Filter, PostError, PostManager, PostMetadata, RenderStats, ReturnedPost,
};

#[derive(Deserialize)]
struct FrontMatter {
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
}

impl FrontMatter {
    pub fn into_full(
        self,
        name: Arc<str>,
        created: Option<SystemTime>,
        modified: Option<SystemTime>,
    ) -> PostMetadata {
        PostMetadata {
            name,
            title: self.title,
            description: self.description,
            author: self.author,
            icon: self.icon,
            icon_alt: self.icon_alt,
            color: self.color,
            created_at: self.created_at.or_else(|| created.map(|t| t.into())),
            modified_at: self.modified_at.or_else(|| modified.map(|t| t.into())),
            tags: self.tags.into_iter().collect(),
        }
    }
}

pub struct MarkdownPosts {
    cache: Option<Arc<CacheGuard>>,
    config: Arc<Config>,
    render_hash: u64,
    syntect: SyntectAdapter,
}

impl MarkdownPosts {
    pub async fn new(
        config: Arc<Config>,
        cache: Option<Arc<CacheGuard>>,
    ) -> eyre::Result<MarkdownPosts> {
        let syntect =
            build_syntect(&config.render).context("failed to create syntax highlighting engine")?;

        let mut hasher = DefaultHasher::new();
        config.render.hash(&mut hasher);
        let render_hash = hasher.finish();

        Ok(Self {
            cache,
            config,
            render_hash,
            syntect,
        })
    }

    async fn parse_and_render(
        &self,
        name: Arc<str>,
        path: impl AsRef<Path>,
    ) -> Result<(PostMetadata, Arc<str>, (Duration, Duration)), PostError> {
        let parsing_start = Instant::now();
        let mut file = match tokio::fs::OpenOptions::new().read(true).open(&path).await {
            Ok(val) => val,
            Err(err) => match err.kind() {
                io::ErrorKind::NotFound => return Err(PostError::NotFound(name)),
                _ => return Err(PostError::IoError(err)),
            },
        };
        let stat = file.metadata().await?;
        let modified = stat.modified()?;
        let created = stat.created().ok();

        let mut content = String::with_capacity(stat.len() as usize);
        file.read_to_string(&mut content).await?;

        let ParsedData { headers, body } = parse::<FrontMatter>(&content)?;
        let metadata = headers.into_full(name.to_owned(), created, Some(modified));
        let parsing = parsing_start.elapsed();

        let before_render = Instant::now();
        let post = render(body, Some(&self.syntect)).into();
        let rendering = before_render.elapsed();

        if let Some(cache) = &self.cache {
            cache
                .insert(
                    name.clone(),
                    metadata.clone(),
                    as_secs(&modified),
                    Arc::clone(&post),
                    self.render_hash,
                )
                .await
                .unwrap_or_else(|err| warn!("failed to insert {:?} into cache", err.0))
        };

        Ok((metadata, post, (parsing, rendering)))
    }
}

#[async_trait]
impl PostManager for MarkdownPosts {
    async fn get_all_posts(
        &self,
        filters: &[Filter<'_>],
        query: &IndexMap<String, Value>,
    ) -> Result<Vec<(PostMetadata, Arc<str>, RenderStats)>, PostError> {
        let mut posts = Vec::new();

        let mut read_dir = fs::read_dir(&self.config.dirs.posts).await?;
        while let Some(entry) = read_dir.next_entry().await? {
            let path = entry.path();
            let stat = fs::metadata(&path).await?;

            if stat.is_file() && path.extension().is_some_and(|ext| ext == "md") {
                let name = path
                    .clone()
                    .file_stem()
                    .unwrap()
                    .to_string_lossy()
                    .to_string()
                    .into();

                let post = self.get_post(Arc::clone(&name), query).await?;
                if let ReturnedPost::Rendered { meta, body, perf } = post
                    && meta.apply_filters(filters)
                {
                    posts.push((meta, body, perf));
                }
            }
        }

        Ok(posts)
    }

    async fn get_all_post_metadata(
        &self,
        filters: &[Filter<'_>],
        _query: &IndexMap<String, Value>,
    ) -> Result<Vec<PostMetadata>, PostError> {
        let mut posts = Vec::new();

        let mut read_dir = fs::read_dir(&self.config.dirs.posts).await?;
        while let Some(entry) = read_dir.next_entry().await? {
            let path = entry.path();
            let stat = fs::metadata(&path).await?;

            if stat.is_file() && path.extension().is_some_and(|ext| ext == "md") {
                let mtime = as_secs(&stat.modified()?);
                let name: Arc<str> =
                    String::from(path.file_stem().unwrap().to_string_lossy()).into();

                if let Some(cache) = &self.cache
                    && let Some(hit) = cache.lookup_metadata(&name, mtime).await
                    && hit.apply_filters(filters)
                {
                    posts.push(hit);
                } else {
                    match self.parse_and_render(name, path).await {
                        Ok((metadata, ..)) => {
                            if metadata.apply_filters(filters) {
                                posts.push(metadata);
                            }
                        }
                        Err(err) => match err {
                            PostError::IoError(ref io_err)
                                if matches!(io_err.kind(), io::ErrorKind::NotFound) =>
                            {
                                warn!("TOCTOU: {}", err)
                            }
                            _ => return Err(err),
                        },
                    }
                }
            }
        }

        Ok(posts)
    }

    #[instrument(level = "info", skip(self))]
    async fn get_post(
        &self,
        name: Arc<str>,
        _query: &IndexMap<String, Value>,
    ) -> Result<ReturnedPost, PostError> {
        let post = if self.config.markdown_access && self.is_raw(&name) {
            let path = self.config.dirs.posts.join(&*name);

            let mut file = match tokio::fs::OpenOptions::new().read(true).open(&path).await {
                Ok(value) => value,
                Err(err) => match err.kind() {
                    io::ErrorKind::NotFound => {
                        if let Some(cache) = &self.cache {
                            cache.remove(&name).await;
                        }
                        return Err(PostError::NotFound(name));
                    }
                    _ => return Err(PostError::IoError(err)),
                },
            };

            let mut buffer = Vec::with_capacity(4096);

            file.read_to_end(&mut buffer).await?;

            ReturnedPost::Raw {
                buffer,
                content_type: HeaderValue::from_static("text/plain"),
            }
        } else {
            let start = Instant::now();
            let path = self
                .config
                .dirs
                .posts
                .join(self.as_raw(&name).unwrap_or_else(|| unreachable!()));

            let stat = match tokio::fs::metadata(&path).await {
                Ok(value) => value,
                Err(err) => match err.kind() {
                    io::ErrorKind::NotFound => {
                        if let Some(cache) = &self.cache {
                            cache.remove(&name).await;
                        }
                        return Err(PostError::NotFound(name));
                    }
                    _ => return Err(PostError::IoError(err)),
                },
            };
            let mtime = as_secs(&stat.modified()?);

            if let Some(cache) = &self.cache
                && let Some(CacheValue { meta, body, .. }) =
                    cache.lookup(&name, mtime, self.render_hash).await
            {
                ReturnedPost::Rendered {
                    meta,
                    body,
                    perf: RenderStats::Cached(start.elapsed()),
                }
            } else {
                let (meta, body, stats) = self.parse_and_render(name, path).await?;
                ReturnedPost::Rendered {
                    meta,
                    body,
                    perf: RenderStats::Rendered {
                        total: start.elapsed(),
                        parsed: stats.0,
                        rendered: stats.1,
                    },
                }
            }
        };

        if let ReturnedPost::Rendered { perf, .. } = &post {
            info!("rendered post in {:?}", perf);
        }

        Ok(post)
    }

    async fn cleanup(&self) {
        if let Some(cache) = &self.cache {
            cache
                .cleanup(|name| {
                    std::fs::metadata(
                        self.config
                            .dirs
                            .posts
                            .join(self.as_raw(name).unwrap_or_else(|| unreachable!())),
                    )
                    .ok()
                    .and_then(|metadata| metadata.modified().ok())
                    .map(|mtime| as_secs(&mtime))
                })
                .await
        }
    }

    fn is_raw(&self, name: &str) -> bool {
        name.ends_with(".md")
    }

    fn as_raw(&self, name: &str) -> Option<String> {
        let mut buf = String::with_capacity(name.len() + 3);
        buf += name;
        buf += ".md";

        Some(buf)
    }
}
