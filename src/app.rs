use std::sync::Arc;
use std::time::Duration;

use askama_axum::Template;
use axum::extract::{Path, Query, State};
use axum::http::header::CONTENT_TYPE;
use axum::http::{header, Request};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::get;
use axum::{Json, Router};
use rss::{Category, ChannelBuilder, ItemBuilder};
use serde::Deserialize;
use tower_http::services::ServeDir;
use tower_http::trace::TraceLayer;
use tracing::{info, info_span, Span};

use crate::config::{Config, DateFormat};
use crate::error::{AppError, AppResult};
use crate::filters;
use crate::post::{MarkdownPosts, PostManager, PostMetadata, RenderStats, ReturnedPost};

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<Config>,
    pub posts: Arc<MarkdownPosts<Arc<Config>>>,
}

#[derive(Template)]
#[template(path = "index.html")]
struct IndexTemplate {
    title: String,
    description: String,
    posts: Vec<PostMetadata>,
    df: DateFormat,
    js: bool,
}

#[derive(Template)]
#[template(path = "post.html")]
struct PostTemplate {
    meta: PostMetadata,
    rendered: String,
    rendered_in: RenderStats,
    markdown_access: bool,
    df: DateFormat,
    js: bool,
}

#[derive(Deserialize)]
struct QueryParams {
    tag: Option<String>,
    #[serde(rename = "n")]
    num_posts: Option<usize>,
}

async fn index(
    State(AppState { config, posts }): State<AppState>,
    Query(query): Query<QueryParams>,
) -> AppResult<IndexTemplate> {
    let posts = posts
        .get_max_n_post_metadata_with_optional_tag_sorted(query.num_posts, query.tag.as_ref())
        .await?;

    Ok(IndexTemplate {
        title: config.title.clone(),
        description: config.description.clone(),
        posts,
        df: config.date_format.clone(),
        js: config.js_enable,
    })
}

async fn all_posts(
    State(AppState { posts, .. }): State<AppState>,
    Query(query): Query<QueryParams>,
) -> AppResult<Json<Vec<PostMetadata>>> {
    let posts = posts
        .get_max_n_post_metadata_with_optional_tag_sorted(query.num_posts, query.tag.as_ref())
        .await?;

    Ok(Json(posts))
}

async fn rss(
    State(AppState { config, posts }): State<AppState>,
    Query(query): Query<QueryParams>,
) -> AppResult<Response> {
    if !config.rss.enable {
        return Err(AppError::RssDisabled);
    }

    let posts = posts
        .get_all_posts(|metadata, _| {
            !query
                .tag
                .as_ref()
                .is_some_and(|tag| !metadata.tags.contains(tag))
        })
        .await?;

    let mut channel = ChannelBuilder::default();
    channel
        .title(&config.title)
        .link(config.rss.link.to_string())
        .description(&config.description);
    //TODO: .language()

    for (metadata, content, _) in posts {
        channel.item(
            ItemBuilder::default()
                .title(metadata.title)
                .description(metadata.description)
                .author(metadata.author)
                .categories(
                    metadata
                        .tags
                        .into_iter()
                        .map(|tag| Category {
                            name: tag,
                            domain: None,
                        })
                        .collect::<Vec<Category>>(),
                )
                .pub_date(metadata.created_at.map(|date| date.to_rfc2822()))
                .content(content)
                .link(
                    config
                        .rss
                        .link
                        .join(&format!("/posts/{}", metadata.name))?
                        .to_string(),
                )
                .build(),
        );
    }

    let body = channel.build().to_string();
    drop(channel);

    Ok(([(header::CONTENT_TYPE, "text/xml")], body).into_response())
}

async fn post(
    State(AppState { config, posts }): State<AppState>,
    Path(name): Path<String>,
) -> AppResult<Response> {
    match posts.get_post(&name).await? {
        ReturnedPost::Rendered(meta, rendered, rendered_in) => {
            let page = PostTemplate {
                meta,
                rendered,
                rendered_in,
                markdown_access: config.markdown_access,
                df: config.date_format.clone(),
                js: config.js_enable,
            };

            Ok(page.into_response())
        }
        ReturnedPost::Raw(body, content_type) => {
            Ok(([(CONTENT_TYPE, content_type)], body).into_response())
        }
    }
}

pub fn new(config: &Config) -> Router<AppState> {
    Router::new()
        .route("/", get(index))
        .route(
            "/post/:name",
            get(
                |Path(name): Path<String>| async move { Redirect::to(&format!("/posts/{}", name)) },
            ),
        )
        .route("/posts/:name", get(post))
        .route("/posts", get(all_posts))
        .route("/feed.xml", get(rss))
        .nest_service(
            "/static",
            ServeDir::new(&config.dirs._static).precompressed_gzip(),
        )
        .nest_service("/media", ServeDir::new(&config.dirs.media))
        .layer(
            TraceLayer::new_for_http()
                .make_span_with(|request: &Request<_>| {
                    info_span!(
                        "request",
                        method = ?request.method(),
                        path = ?request.uri().path(),
                    )
                })
                .on_response(|response: &Response<_>, duration: Duration, span: &Span| {
                    let _ = span.enter();
                    let status = response.status();
                    info!(?status, ?duration, "response");
                }),
        )
}
