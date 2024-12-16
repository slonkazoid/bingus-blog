pub mod blag;
pub mod cache;
pub mod markdown_posts;

use std::{collections::HashMap, time::Duration};

use axum::{async_trait, http::HeaderValue};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_value::Value;

use crate::error::PostError;
pub use blag::Blag;
pub use markdown_posts::MarkdownPosts;

// TODO: replace String with Arc<str>
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct PostMetadata {
    pub name: String,
    pub title: String,
    pub description: String,
    pub author: String,
    pub icon: Option<String>,
    pub icon_alt: Option<String>,
    pub color: Option<String>,
    pub created_at: Option<DateTime<Utc>>,
    pub modified_at: Option<DateTime<Utc>>,
    pub tags: Vec<String>,
}

#[derive(Serialize, Debug)]
#[allow(unused)]
pub enum RenderStats {
    Cached(Duration),
    Rendered {
        total: Duration,
        parsed: Duration,
        rendered: Duration,
    },
    Fetched(Duration),
    Unknown,
}

#[allow(clippy::large_enum_variant)] // Raw will be returned very rarely
pub enum ReturnedPost {
    Rendered(PostMetadata, String, RenderStats),
    Raw(Vec<u8>, HeaderValue),
}

pub enum Filter<'a> {
    Tags(&'a [&'a str]),
}

impl Filter<'_> {
    pub fn apply(&self, meta: &PostMetadata) -> bool {
        match self {
            Filter::Tags(tags) => tags
                .iter()
                .any(|tag| meta.tags.iter().any(|meta_tag| meta_tag == tag)),
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
        query: &HashMap<String, Value>,
    ) -> Result<Vec<PostMetadata>, PostError> {
        self.get_all_posts(filters, query)
            .await
            .map(|vec| vec.into_iter().map(|(meta, ..)| meta).collect())
    }

    async fn get_all_posts(
        &self,
        filters: &[Filter<'_>],
        query: &HashMap<String, Value>,
    ) -> Result<Vec<(PostMetadata, String, RenderStats)>, PostError>;

    async fn get_max_n_post_metadata_with_optional_tag_sorted(
        &self,
        n: Option<usize>,
        tag: Option<&str>,
        query: &HashMap<String, Value>,
    ) -> Result<Vec<PostMetadata>, PostError> {
        let filters = tag.and(Some(Filter::Tags(tag.as_slice())));
        let mut posts = self
            .get_all_post_metadata(filters.as_slice(), query)
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
    async fn get_post_metadata(
        &self,
        name: &str,
        query: &HashMap<String, Value>,
    ) -> Result<PostMetadata, PostError> {
        match self.get_post(name, query).await? {
            ReturnedPost::Rendered(metadata, ..) => Ok(metadata),
            ReturnedPost::Raw(..) => Err(PostError::NotFound(name.to_string())),
        }
    }

    async fn get_post(
        &self,
        name: &str,
        query: &HashMap<String, Value>,
    ) -> Result<ReturnedPost, PostError>;

    async fn cleanup(&self) {}

    #[allow(unused)]
    async fn as_raw(&self, name: &str) -> Result<Option<String>, PostError> {
        Ok(None)
    }
}
