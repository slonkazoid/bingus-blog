use std::io::{self, Write};
use std::ops::Deref;
use std::path::Path;
use std::time::Duration;
use std::time::Instant;

use color_eyre::eyre::{self, Context};
use fronma::parser::{parse, ParsedData};
use tokio::fs;
use tokio::io::AsyncReadExt;
use tracing::{error, info, warn};

use crate::config::Config;
use crate::markdown_render::render;
use crate::post::cache::{load_cache, Cache, CACHE_VERSION};
use crate::post::{FrontMatter, PostError, PostManager, PostMetadata, RenderStats};
use crate::systemtime_as_secs::as_secs;

pub struct MarkdownPosts<C>
where
    C: Deref<Target = Config>,
{
    cache: Option<Cache>,
    config: C,
}

impl<C> MarkdownPosts<C>
where
    C: Deref<Target = Config>,
{
    pub async fn new(config: C) -> eyre::Result<MarkdownPosts<C>> {
        if config.cache.enable {
            if config.cache.persistence && tokio::fs::try_exists(&config.cache.file).await? {
                info!("loading cache from file");
                let mut cache = load_cache(&config).await.unwrap_or_else(|err| {
                    error!("failed to load cache: {}", err);
                    info!("using empty cache");
                    Default::default()
                });

                if cache.version() < CACHE_VERSION {
                    warn!("cache version changed, clearing cache");
                    cache = Default::default();
                };

                Ok(Self {
                    cache: Some(cache),
                    config,
                })
            } else {
                Ok(Self {
                    cache: Some(Default::default()),
                    config,
                })
            }
        } else {
            Ok(Self {
                cache: None,
                config,
            })
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
        let post = render(body, &self.config.render);
        let rendering = before_render.elapsed();

        if let Some(cache) = self.cache.as_ref() {
            cache
                .insert(
                    name.to_string(),
                    metadata.clone(),
                    as_secs(&modified),
                    post.clone(),
                    &self.config.render,
                )
                .await
                .unwrap_or_else(|err| warn!("failed to insert {:?} into cache", err.0))
        };

        Ok((metadata, post, (parsing, rendering)))
    }

    fn cache(&self) -> Option<&Cache> {
        self.cache.as_ref()
    }

    fn try_drop(&mut self) -> Result<(), eyre::Report> {
        // write cache to file
        let config = &self.config.cache;
        if config.enable
            && config.persistence
            && let Some(cache) = self.cache()
        {
            let path = &config.file;
            let serialized = bitcode::serialize(cache).context("failed to serialize cache")?;
            let mut cache_file = std::fs::File::create(path)
                .with_context(|| format!("failed to open cache at {}", path.display()))?;
            let compression_level = config.compression_level;
            if config.compress {
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
        }
        Ok(())
    }
}

impl<C> Drop for MarkdownPosts<C>
where
    C: Deref<Target = Config>,
{
    fn drop(&mut self) {
        self.try_drop().unwrap()
    }
}

impl<C> PostManager for MarkdownPosts<C>
where
    C: Deref<Target = Config>,
{
    async fn get_all_post_metadata(
        &self,
        filter: impl Fn(&PostMetadata) -> bool,
    ) -> Result<Vec<PostMetadata>, PostError> {
        let mut posts = Vec::new();

        let mut read_dir = fs::read_dir(&self.config.dirs.posts).await?;
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

    async fn get_all_posts(
        &self,
        filter: impl Fn(&PostMetadata, &str) -> bool,
    ) -> Result<Vec<(PostMetadata, String, RenderStats)>, PostError> {
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
                    .to_string();

                let post = self.get_post(&name).await?;
                if filter(&post.0, &post.1) {
                    posts.push(post);
                }
            }
        }

        Ok(posts)
    }

    async fn get_post(&self, name: &str) -> Result<(PostMetadata, String, RenderStats), PostError> {
        let start = Instant::now();
        let path = self.config.dirs.posts.join(name.to_owned() + ".md");

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
            && let Some(hit) = cache.lookup(name, mtime, &self.config.render).await
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

    async fn cleanup(&self) {
        if let Some(cache) = self.cache.as_ref() {
            cache
                .cleanup(|name| {
                    std::fs::metadata(self.config.dirs.posts.join(name.to_owned() + ".md"))
                        .ok()
                        .and_then(|metadata| metadata.modified().ok())
                        .map(|mtime| as_secs(&mtime))
                })
                .await
        }
    }
}
