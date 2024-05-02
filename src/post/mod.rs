pub mod cache;

use std::collections::BTreeSet;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use askama::Template;
use chrono::{DateTime, Utc};
use fronma::parser::{parse, ParsedData};
use serde::{Deserialize, Serialize};
use tokio::fs;
use tokio::io::AsyncReadExt;
use tracing::warn;

use crate::config::RenderConfig;
use crate::markdown_render::render;
use crate::post::cache::Cache;
use crate::systemtime_as_secs::as_secs;
use crate::PostError;

#[derive(Deserialize)]
struct FrontMatter {
    pub title: String,
    pub description: String,
    pub author: String,
    pub icon: Option<String>,
    pub created_at: Option<DateTime<Utc>>,
    pub modified_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub tags: BTreeSet<String>,
}

impl FrontMatter {
    pub fn into_full(
        self,
        name: String,
        created: Option<SystemTime>,
        modified: Option<SystemTime>,
    ) -> PostMetadata {
        PostMetadata {
            name,
            title: self.title,
            description: self.description,
            author: self.author,
            icon: self.icon,
            created_at: self.created_at.or_else(|| created.map(|t| t.into())),
            modified_at: self.modified_at.or_else(|| modified.map(|t| t.into())),
            tags: self.tags.into_iter().collect(),
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct PostMetadata {
    pub name: String,
    pub title: String,
    pub description: String,
    pub author: String,
    pub icon: Option<String>,
    pub created_at: Option<DateTime<Utc>>,
    pub modified_at: Option<DateTime<Utc>>,
    pub tags: Vec<String>,
}

use crate::filters;
#[derive(Template)]
#[template(path = "post.html")]
struct Post<'a> {
    pub meta: &'a PostMetadata,
    pub rendered_markdown: String,
}

#[allow(unused)]
pub enum RenderStats {
    Cached(Duration),
    // format: Total, Parsed in, Rendered in
    ParsedAndRendered(Duration, Duration, Duration),
}

#[derive(Clone)]
pub struct PostManager {
    dir: PathBuf,
    cache: Option<Cache>,
    config: RenderConfig,
}

impl PostManager {
    pub fn new(dir: PathBuf, config: RenderConfig) -> PostManager {
        PostManager {
            dir,
            cache: None,
            config,
        }
    }

    pub fn new_with_cache(dir: PathBuf, config: RenderConfig, cache: Cache) -> PostManager {
        PostManager {
            dir,
            cache: Some(cache),
            config,
        }
    }

    async fn parse_and_render(
        &self,
        name: String,
        path: impl AsRef<Path>,
    ) -> Result<(PostMetadata, String, (Duration, Duration)), PostError> {
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
        let rendered_markdown = render(body, &self.config);
        let post = Post {
            meta: &metadata,
            rendered_markdown,
        }
        .render()?;
        let rendering = before_render.elapsed();

        if let Some(cache) = self.cache.as_ref() {
            cache
                .insert(
                    name.to_string(),
                    metadata.clone(),
                    as_secs(&modified),
                    post.clone(),
                    &self.config,
                )
                .await
                .unwrap_or_else(|err| warn!("failed to insert {:?} into cache", err.0))
        };

        Ok((metadata, post, (parsing, rendering)))
    }

    pub async fn get_all_post_metadata_filtered(
        &self,
        filter: impl Fn(&PostMetadata) -> bool,
    ) -> Result<Vec<PostMetadata>, PostError> {
        let mut posts = Vec::new();

        let mut read_dir = fs::read_dir(&self.dir).await?;
        while let Some(entry) = read_dir.next_entry().await? {
            let path = entry.path();
            let stat = fs::metadata(&path).await?;

            if stat.is_file() && path.extension().is_some_and(|ext| ext == "md") {
                let mtime = as_secs(&stat.modified()?);
                // TODO. this?
                let name = path
                    .clone()
                    .file_stem()
                    .unwrap()
                    .to_string_lossy()
                    .to_string();

                if let Some(cache) = self.cache.as_ref()
                    && let Some(hit) = cache.lookup_metadata(&name, mtime).await
                    && filter(&hit)
                {
                    posts.push(hit);
                } else {
                    match self.parse_and_render(name, path).await {
                        Ok((metadata, ..)) => {
                            if filter(&metadata) {
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

    pub async fn get_all_posts_filtered(
        &self,
        filter: impl Fn(&PostMetadata, &str) -> bool,
    ) -> Result<Vec<(PostMetadata, String, RenderStats)>, PostError> {
        let mut posts = Vec::new();

        let mut read_dir = fs::read_dir(&self.dir).await?;
        while let Some(entry) = read_dir.next_entry().await? {
            let path = entry.path();
            let stat = fs::metadata(&path).await?;

            if stat.is_file() && path.extension().is_some_and(|ext| ext == "md") {
                let name = path
                    .clone()
                    .file_stem()
                    .unwrap()
                    .to_string_lossy()
                    .to_string();

                let post = self.get_post(&name).await?;
                if filter(&post.0, &post.1) {
                    posts.push(post);
                }
            }
        }

        Ok(posts)
    }

    pub async fn get_max_n_post_metadata_with_optional_tag_sorted(
        &self,
        n: Option<usize>,
        tag: Option<&String>,
    ) -> Result<Vec<PostMetadata>, PostError> {
        let mut posts = self
            .get_all_post_metadata_filtered(|metadata| {
                !tag.is_some_and(|tag| !metadata.tags.contains(tag))
            })
            .await?;
        // we still want some semblance of order if created_at is None so sort by mtime as well
        posts.sort_unstable_by_key(|metadata| metadata.modified_at.unwrap_or_default());
        posts.sort_by_key(|metadata| metadata.created_at.unwrap_or_default());
        posts.reverse();
        if let Some(n) = n {
            posts.truncate(n);
        }

        Ok(posts)
    }

    pub async fn get_max_n_posts_with_optional_tag_sorted(
        &self,
        n: Option<usize>,
        tag: Option<&String>,
    ) -> Result<Vec<(PostMetadata, String, RenderStats)>, PostError> {
        let mut posts = self
            .get_all_posts_filtered(|metadata, _| {
                !tag.is_some_and(|tag| !metadata.tags.contains(tag))
            })
            .await?;
        posts.sort_unstable_by_key(|(metadata, ..)| metadata.modified_at.unwrap_or_default());
        posts.sort_by_key(|(metadata, ..)| metadata.created_at.unwrap_or_default());
        posts.reverse();
        if let Some(n) = n {
            posts.truncate(n);
        }

        Ok(posts)
    }

    pub async fn get_post(
        &self,
        name: &str,
    ) -> Result<(PostMetadata, String, RenderStats), PostError> {
        let start = Instant::now();
        let path = self.dir.join(name.to_owned() + ".md");

        let stat = match tokio::fs::metadata(&path).await {
            Ok(value) => value,
            Err(err) => match err.kind() {
                io::ErrorKind::NotFound => {
                    if let Some(cache) = self.cache.as_ref() {
                        cache.remove(name).await;
                    }
                    return Err(PostError::NotFound(name.to_string()));
                }
                _ => return Err(PostError::IoError(err)),
            },
        };
        let mtime = as_secs(&stat.modified()?);

        if let Some(cache) = self.cache.as_ref()
            && let Some(hit) = cache.lookup(name, mtime, &self.config).await
        {
            Ok((
                hit.metadata,
                hit.rendered,
                RenderStats::Cached(start.elapsed()),
            ))
        } else {
            let (metadata, rendered, stats) = self.parse_and_render(name.to_string(), path).await?;
            Ok((
                metadata,
                rendered,
                RenderStats::ParsedAndRendered(start.elapsed(), stats.0, stats.1),
            ))
        }
    }

    pub fn cache(&self) -> Option<&Cache> {
        self.cache.as_ref()
    }

    pub async fn cleanup(&self) {
        if let Some(cache) = self.cache.as_ref() {
            cache
                .cleanup(|name| {
                    std::fs::metadata(self.dir.join(name.to_owned() + ".md"))
                        .ok()
                        .and_then(|metadata| metadata.modified().ok())
                        .map(|mtime| as_secs(&mtime))
                })
                .await
        }
    }
}
