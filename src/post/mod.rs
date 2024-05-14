pub mod cache;
pub mod markdown_posts;

use std::collections::BTreeSet;
use std::time::{Duration, SystemTime};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::error::PostError;
pub use crate::post::markdown_posts::MarkdownPosts;

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

#[allow(unused)]
pub enum RenderStats {
    Cached(Duration),
    // format: Total, Parsed in, Rendered in
    ParsedAndRendered(Duration, Duration, Duration),
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
        self.get_post(name).await.map(|(meta, ..)| meta)
    }

    async fn get_post(&self, name: &str) -> Result<(PostMetadata, String, RenderStats), PostError>;

    async fn cleanup(&self);
}
