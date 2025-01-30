pub mod blag;
pub mod cache;
pub mod markdown_posts;

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use axum::http::HeaderValue;
use chrono::{DateTime, Utc};
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use serde_value::Value;

use crate::error::PostError;
pub use blag::Blag;
pub use markdown_posts::MarkdownPosts;

// TODO: replace String with Arc<str>
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct PostMetadata {
    pub name: Arc<str>,
    pub title: Arc<str>,
    pub description: Arc<str>,
    pub author: Arc<str>,
    pub icon: Option<Arc<str>>,
    pub icon_alt: Option<Arc<str>>,
    pub color: Option<Arc<str>>,
    pub written_at: Option<DateTime<Utc>>,
    pub modified_at: Option<DateTime<Utc>>,
    pub tags: Vec<Arc<str>>,
}

#[derive(Serialize, Debug, Clone)]
#[allow(unused)]
pub enum RenderStats {
    Cached(Duration),
    Rendered {
        total: Duration,
        parsed: Duration,
        rendered: Duration,
    },
    Fetched(Duration),
    Other {
        verb: Arc<str>,
        time: Duration,
    },
    Unknown,
}

#[allow(clippy::large_enum_variant)] // Raw will be returned very rarely
#[derive(Debug, Clone)]
pub enum ReturnedPost {
    Rendered {
        meta: PostMetadata,
        body: Arc<str>,
        perf: RenderStats,
        raw_name: Option<String>,
    },
    Raw {
        buffer: Vec<u8>,
        content_type: HeaderValue,
    },
}

pub enum Filter<'a> {
    Tags(&'a [&'a str]),
}

impl Filter<'_> {
    pub fn apply(&self, meta: &PostMetadata) -> bool {
        match self {
            Filter::Tags(tags) => tags
                .iter()
                .any(|tag| meta.tags.iter().any(|meta_tag| &**meta_tag == *tag)),
        }
    }
}

pub trait ApplyFilters {
    fn apply_filters(&self, filters: &[Filter<'_>]) -> bool;
}

impl ApplyFilters for PostMetadata {
    fn apply_filters(&self, filters: &[Filter<'_>]) -> bool {
        for filter in filters {
            if !filter.apply(self) {
                return false;
            }
        }
        true
    }
}

#[async_trait]
pub trait PostManager {
    async fn get_all_post_metadata(
        &self,
        filters: &[Filter<'_>],
        query: &IndexMap<String, Value>,
    ) -> Result<Vec<PostMetadata>, PostError> {
        self.get_all_posts(filters, query)
            .await
            .map(|vec| vec.into_iter().map(|(meta, ..)| meta).collect())
    }

    async fn get_all_posts(
        &self,
        filters: &[Filter<'_>],
        query: &IndexMap<String, Value>,
    ) -> Result<Vec<(PostMetadata, Arc<str>, RenderStats)>, PostError>;

    async fn get_max_n_post_metadata_with_optional_tag_sorted(
        &self,
        n: Option<usize>,
        tag: Option<&str>,
        query: &IndexMap<String, Value>,
    ) -> Result<Vec<PostMetadata>, PostError> {
        let filters = tag.and(Some(Filter::Tags(tag.as_slice())));
        let mut posts = self
            .get_all_post_metadata(filters.as_slice(), query)
            .await?;
        // we still want some semblance of order if created_at is None so sort by mtime as well
        posts.sort_unstable_by_key(|metadata| metadata.modified_at.unwrap_or_default());
        posts.sort_by_key(|metadata| metadata.written_at.unwrap_or_default());
        posts.reverse();
        if let Some(n) = n {
            posts.truncate(n);
        }

        Ok(posts)
    }

    #[allow(unused)]
    async fn get_post_metadata(
        &self,
        name: Arc<str>,
        query: &IndexMap<String, Value>,
    ) -> Result<PostMetadata, PostError> {
        match self.get_post(name.clone(), query).await? {
            ReturnedPost::Rendered { meta, .. } => Ok(meta),
            ReturnedPost::Raw { .. } => Err(PostError::NotFound(name)),
        }
    }

    async fn get_post(
        &self,
        name: Arc<str>,
        query: &IndexMap<String, Value>,
    ) -> Result<ReturnedPost, PostError>;

    async fn cleanup(&self) {}
}
