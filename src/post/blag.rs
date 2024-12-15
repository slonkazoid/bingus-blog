use std::path::Path;
use std::process::Stdio;
use std::sync::Arc;

use axum::async_trait;
use axum::http::HeaderValue;
use futures::stream::FuturesUnordered;
use futures::StreamExt;
use tokio::fs::OpenOptions;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
use tokio::time::Instant;
use tracing::{debug, error};

use crate::error::PostError;
use crate::post::Filter;

use super::{ApplyFilters, PostManager, PostMetadata, RenderStats, ReturnedPost};

pub struct Blag {
    root: Arc<Path>,
    blag_bin: Arc<Path>,
}

impl Blag {
    pub fn new(root: Arc<Path>, blag_bin: Arc<Path>) -> Blag {
        Self { root, blag_bin }
    }
}

#[async_trait]
impl PostManager for Blag {
    async fn get_all_posts(
        &self,
        filters: &[Filter<'_>],
    ) -> Result<Vec<(PostMetadata, String, RenderStats)>, PostError> {
        let mut set = FuturesUnordered::new();
        let mut meow = Vec::new();
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
                    set.push(async move { self.get_post(name.trim_end_matches(".sh")).await });
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
                meow.push(post);
            }
        }

        debug!("collected posts");

        Ok(meow)
    }

    async fn get_post(&self, name: &str) -> Result<ReturnedPost, PostError> {
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

        let start = Instant::now();
        let stat = tokio::fs::metadata(&path)
            .await
            .map_err(|err| match err.kind() {
                std::io::ErrorKind::NotFound => PostError::NotFound(name.to_string()),
                _ => PostError::IoError(err),
            })?;

        if !stat.is_file() {
            return Err(PostError::NotFound(name.to_string()));
        }

        let mut cmd = tokio::process::Command::new(&*self.blag_bin)
            .arg(path)
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

        let mut meta: PostMetadata = serde_json::from_str(&buf)?;
        meta.name = name.to_string();
        buf.clear();
        reader.read_to_string(&mut buf).await?;

        debug!("read output: {} bytes", buf.len());

        let exit_status = cmd.wait().await?;
        debug!("exited: {exit_status}");
        if !exit_status.success() {
            return Err(PostError::RenderError(exit_status.to_string()));
        }

        let elapsed = start.elapsed();

        Ok(ReturnedPost::Rendered(
            meta,
            buf,
            RenderStats::ParsedAndRendered(elapsed, elapsed, elapsed),
        ))
    }

    async fn get_raw(&self, name: &str) -> Result<Option<String>, PostError> {
        let mut buf = String::with_capacity(name.len() + 3);
        buf += name;
        buf += ".sh";

        Ok(Some(buf))
    }
}
