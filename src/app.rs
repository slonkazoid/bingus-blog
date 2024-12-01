use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Path, Query, State};
use axum::http::header::CONTENT_TYPE;
use axum::http::Request;
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::get;
use axum::{Json, Router};
use handlebars::Handlebars;
use include_dir::{include_dir, Dir};
use rss::{Category, ChannelBuilder, ItemBuilder};
use serde::{Deserialize, Serialize};
use serde_json::Map;
use tokio::sync::RwLock;
use tower::service_fn;
use tower_http::services::ServeDir;
use tower_http::trace::TraceLayer;
use tracing::{info, info_span, Span};

use crate::config::{Config, StyleConfig};
use crate::error::{AppError, AppResult};
use crate::post::{MarkdownPosts, PostManager, PostMetadata, RenderStats, ReturnedPost};
use crate::serve_dir_included::handle;

const STATIC: Dir<'static> = include_dir!("$CARGO_MANIFEST_DIR/static");

#[derive(Serialize)]
pub struct BingusInfo {
    pub name: &'static str,
    pub version: &'static str,
    pub repository: &'static str,
}

const BINGUS_INFO: BingusInfo = BingusInfo {
    name: env!("CARGO_PKG_NAME"),
    version: env!("CARGO_PKG_VERSION"),
    repository: env!("CARGO_PKG_REPOSITORY"),
};

#[derive(Clone)]
#[non_exhaustive]
pub struct AppState {
    pub config: Arc<Config>,
    pub posts: Arc<MarkdownPosts<Arc<Config>>>,
    pub reg: Arc<RwLock<Handlebars<'static>>>,
}

#[derive(Serialize)]
struct IndexTemplate<'a> {
    bingus_info: &'a BingusInfo,
    title: &'a str,
    description: &'a str,
    posts: Vec<PostMetadata>,
    rss: bool,
    js: bool,
    tags: Map<String, serde_json::Value>,
    joined_tags: String,
    style: &'a StyleConfig,
}

#[derive(Serialize)]
struct PostTemplate<'a> {
    bingus_info: &'a BingusInfo,
    meta: &'a PostMetadata,
    rendered: String,
    rendered_in: RenderStats,
    markdown_access: bool,
    js: bool,
    color: Option<&'a str>,
    joined_tags: String,
    style: &'a StyleConfig,
}

#[derive(Deserialize)]
struct QueryParams {
    tag: Option<String>,
    #[serde(rename = "n")]
    num_posts: Option<usize>,
}

fn collect_tags(posts: &Vec<PostMetadata>) -> Map<String, serde_json::Value> {
    let mut tags = HashMap::new();

    for post in posts {
        for tag in &post.tags {
            if let Some((existing_tag, count)) = tags.remove_entry(tag) {
                tags.insert(existing_tag, count + 1);
            } else {
                tags.insert(tag.clone(), 1);
            }
        }
    }

    let mut tags: Vec<(String, u64)> = tags.into_iter().collect();

    tags.sort_unstable_by_key(|(v, _)| v.clone());
    tags.sort_by_key(|(_, v)| -(*v as i64));

    let mut map = Map::new();

    for tag in tags.into_iter() {
        map.insert(tag.0, tag.1.into());
    }

    map
}

fn join_tags_for_meta(tags: &Map<String, serde_json::Value>, delim: &str) -> String {
    let mut s = String::new();
    let tags = tags.keys().enumerate();
    let len = tags.len();
    for (i, tag) in tags {
        s += tag;
        if i != len - 1 {
            s += delim;
        }
    }
    s
}

async fn index<'a>(
    State(AppState {
        config, posts, reg, ..
    }): State<AppState>,
    Query(query): Query<QueryParams>,
) -> AppResult<impl IntoResponse> {
    let posts = posts
        .get_max_n_post_metadata_with_optional_tag_sorted(query.num_posts, query.tag.as_ref())
        .await?;

    let tags = collect_tags(&posts);
    let joined_tags = join_tags_for_meta(&tags, ", ");

    let reg = reg.read().await;
    let rendered = reg.render(
        "index",
        &IndexTemplate {
            title: &config.title,
            description: &config.description,
            bingus_info: &BINGUS_INFO,
            posts,
            rss: config.rss.enable,
            js: config.js_enable,
            tags,
            joined_tags,
            style: &config.style,
        },
    );
    drop(reg);
    Ok(Html(rendered?))
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
    State(AppState { config, posts, .. }): State<AppState>,
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

    Ok(([(CONTENT_TYPE, "text/xml")], body).into_response())
}

async fn post(
    State(AppState {
        config, posts, reg, ..
    }): State<AppState>,
    Path(name): Path<String>,
) -> AppResult<impl IntoResponse> {
    match posts.get_post(&name).await? {
        ReturnedPost::Rendered(ref meta, rendered, rendered_in) => {
            let joined_tags = meta.tags.join(", ");

            let reg = reg.read().await;
            let rendered = reg.render(
                "post",
                &PostTemplate {
                    bingus_info: &BINGUS_INFO,
                    meta,
                    rendered,
                    rendered_in,
                    markdown_access: config.markdown_access,
                    js: config.js_enable,
                    color: meta
                        .color
                        .as_deref()
                        .or(config.style.default_color.as_deref()),
                    joined_tags,
                    style: &config.style,
                },
            );
            drop(reg);
            Ok(Html(rendered?).into_response())
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
            ServeDir::new(&config.dirs.custom_static)
                .precompressed_gzip()
                .fallback(service_fn(|req| handle(req, &STATIC))),
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
