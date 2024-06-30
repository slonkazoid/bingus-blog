pub mod cache;
pub mod markdown_posts;

use std::time::Duration;

use axum::http::HeaderValue;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::error::PostError;
pub use crate::post::markdown_posts::MarkdownPosts;

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct PostMetadata {
    pub name: String,
    pub title: String,
    pub description: String,
    pub author: String,
    pub icon: Option<String>,
    //pub icon_alt: Option<String>,
    pub color: Option<String>,
    pub created_at: Option<DateTime<Utc>>,
    pub modified_at: Option<DateTime<Utc>>,
    pub tags: Vec<String>,
}

pub enum RenderStats {
    Cached(Duration),
    // format: Total, Parsed in, Rendered in
    ParsedAndRendered(Duration, Duration, Duration),
}

#[allow(clippy::large_enum_variant)] // Raw will be returned very rarely
pub enum ReturnedPost {
    Rendered(PostMetadata, String, RenderStats),
    Raw(Vec<u8>, HeaderValue),
}

pub trait PostManager {
    async fn get_all_post_metadata(
        &self,
        filter: impl Fn(&PostMetadata) -> bool,
    ) -> Result<Vec<PostMetadata>, PostError> {
        self.get_all_posts(|m, _| filter(m))
            .await
            .map(|vec| vec.into_iter().map(|(meta, ..)| meta).collect())
    }

    async fn get_all_posts(
        &self,
        filter: impl Fn(&PostMetadata, &str) -> bool,
    ) -> Result<Vec<(PostMetadata, String, RenderStats)>, PostError>;

    async fn get_max_n_post_metadata_with_optional_tag_sorted(
        &self,
        n: Option<usize>,
        tag: Option<&String>,
    ) -> Result<Vec<PostMetadata>, PostError> {
        let mut posts = self
            .get_all_post_metadata(|metadata| !tag.is_some_and(|tag| !metadata.tags.contains(tag)))
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

    #[allow(unused)]
    async fn get_post_metadata(&self, name: &str) -> Result<PostMetadata, PostError> {
        match self.get_post(name).await? {
            ReturnedPost::Rendered(metadata, ..) => Ok(metadata),
            ReturnedPost::Raw(..) => Err(PostError::NotFound(name.to_string())),
        }
    }

    async fn get_post(&self, name: &str) -> Result<ReturnedPost, PostError>;

    async fn cleanup(&self);
}
