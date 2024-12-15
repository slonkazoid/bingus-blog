use std::future::Future;
use std::mem;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::process::{ExitStatus, Stdio};
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
    pub fn new(root: Arc<Path>, blag_bin: Option<Arc<Path>>) -> Blag {
        Self {
            root,
            blag_bin: blag_bin.unwrap_or_else(|| PathBuf::from("blag").into()),
        }
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

        while let Ok(Some(entry)) = files.next_entry().await {
            let file_type = entry.file_type().await?;
            if file_type.is_file() {
                let name = entry.file_name().into_string().unwrap();

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
            .spawn()?;

        let stdout = cmd.stdout.take().unwrap();

        let mut reader = BufReader::new(stdout);
        let mut buf = String::new();
        reader.read_line(&mut buf).await?;

        let mut meta: PostMetadata = serde_json::from_str(&buf)?;
        meta.name = name.to_string();

        enum Return {
            Read(String),
            Exit(ExitStatus),
        }

        let mut futures: FuturesUnordered<
            Pin<Box<dyn Future<Output = Result<Return, std::io::Error>> + Send>>,
        > = FuturesUnordered::new();

        buf.clear();
        let mut fut_buf = mem::take(&mut buf);

        futures.push(Box::pin(async move {
            reader
                .read_to_string(&mut fut_buf)
                .await
                .map(|_| Return::Read(fut_buf))
        }));
        futures.push(Box::pin(async move { cmd.wait().await.map(Return::Exit) }));

        while let Some(res) = futures.next().await {
            match res? {
                Return::Read(fut_buf) => {
                    buf = fut_buf;
                    debug!("read output: {} bytes", buf.len());
                }
                Return::Exit(exit_status) => {
                    debug!("exited: {exit_status}");
                    if !exit_status.success() {
                        return Err(PostError::RenderError(exit_status.to_string()));
                    }
                }
            }
        }

        drop(futures);

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
