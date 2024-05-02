#![feature(let_chains)]

mod config;
mod error;
mod filters;
mod hash_arc_store;
mod markdown_render;
mod post;
mod ranged_i128_visitor;
mod systemtime_as_secs;

use std::future::IntoFuture;
use std::io::Read;
use std::net::SocketAddr;
use std::process::exit;
use std::sync::Arc;
use std::time::Duration;

use askama_axum::Template;
use axum::extract::{Path, Query, State};
use axum::http::Request;
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, Router};
use axum::Json;
use color_eyre::eyre::{self, Context};
use serde::Deserialize;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::task::JoinSet;
use tokio::{select, signal};
use tokio_util::sync::CancellationToken;
use tower_http::services::ServeDir;
use tower_http::trace::TraceLayer;
use tracing::level_filters::LevelFilter;
use tracing::{debug, error, info, info_span, warn, Span};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

use crate::config::Config;
use crate::error::{AppResult, PostError};
use crate::post::cache::{Cache, CACHE_VERSION};
use crate::post::{PostManager, PostMetadata, RenderStats};

type ArcState = Arc<AppState>;

#[derive(Clone)]
struct AppState {
    pub config: Config,
    pub posts: PostManager,
}

#[derive(Template)]
#[template(path = "index.html")]
struct IndexTemplate {
    title: String,
    description: String,
    posts: Vec<PostMetadata>,
}

#[derive(Template)]
#[template(path = "view_post.html")]
struct ViewPostTemplate {
    meta: PostMetadata,
    rendered: String,
    rendered_in: RenderStats,
    markdown_access: bool,
}

#[derive(Deserialize)]
struct QueryParams {
    tag: Option<String>,
    #[serde(rename = "n")]
    num_posts: Option<usize>,
}

async fn index(
    State(state): State<ArcState>,
    Query(query): Query<QueryParams>,
) -> AppResult<IndexTemplate> {
    let posts = state
        .posts
        .get_max_n_posts_with_optional_tag_sorted(query.num_posts, query.tag.as_ref())
        .await?;

    Ok(IndexTemplate {
        title: state.config.title.clone(),
        description: state.config.description.clone(),
        posts,
    })
}

async fn all_posts(
    State(state): State<ArcState>,
    Query(query): Query<QueryParams>,
) -> AppResult<Json<Vec<PostMetadata>>> {
    let posts = state
        .posts
        .get_max_n_posts_with_optional_tag_sorted(query.num_posts, query.tag.as_ref())
        .await?;

    Ok(Json(posts))
}

async fn post(State(state): State<ArcState>, Path(name): Path<String>) -> AppResult<Response> {
    if name.ends_with(".md") && state.config.raw_access {
        let mut file = tokio::fs::OpenOptions::new()
            .read(true)
            .open(state.config.dirs.posts.join(&name))
            .await?;

        let mut buf = Vec::new();
        file.read_to_end(&mut buf).await?;

        Ok(([("content-type", "text/plain")], buf).into_response())
    } else {
        let post = state.posts.get_post(&name).await?;
        let page = ViewPostTemplate {
            meta: post.0,
            rendered: post.1,
            rendered_in: post.2,
            markdown_access: state.config.raw_access,
        };

        Ok(page.into_response())
    }
}

#[tokio::main]
async fn main() -> eyre::Result<()> {
    #[cfg(feature = "tokio-console")]
    console_subscriber::init();
    color_eyre::install()?;
    #[cfg(not(feature = "tokio-console"))]
    tracing_subscriber::registry()
        .with(
            EnvFilter::builder()
                .with_default_directive(LevelFilter::INFO.into())
                .from_env_lossy(),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    let config = config::load()
        .await
        .context("couldn't load configuration")?;

    let socket_addr = SocketAddr::new(config.http.host, config.http.port);

    let mut tasks = JoinSet::new();
    let cancellation_token = CancellationToken::new();

    let posts = if config.cache.enable {
        if config.cache.persistence
            && tokio::fs::try_exists(&config.cache.file)
                .await
                .with_context(|| {
                    format!("failed to check if {} exists", config.cache.file.display())
                })?
        {
            info!("loading cache from file");
            let path = &config.cache.file;
            let load_cache = async {
                let mut cache_file = tokio::fs::File::open(&path)
                    .await
                    .context("failed to open cache file")?;
                let serialized = if config.cache.compress {
                    let cache_file = cache_file.into_std().await;
                    tokio::task::spawn_blocking(move || {
                        let mut buf = Vec::with_capacity(4096);
                        zstd::stream::read::Decoder::new(cache_file)?.read_to_end(&mut buf)?;
                        Ok::<_, std::io::Error>(buf)
                    })
                    .await
                    .context("failed to join blocking thread")?
                    .context("failed to read cache file")?
                } else {
                    let mut buf = Vec::with_capacity(4096);
                    cache_file
                        .read_to_end(&mut buf)
                        .await
                        .context("failed to read cache file")?;
                    buf
                };
                let mut cache: Cache =
                    bitcode::deserialize(serialized.as_slice()).context("failed to parse cache")?;
                if cache.version() < CACHE_VERSION {
                    warn!("cache version changed, clearing cache");
                    cache = Cache::default();
                };

                Ok::<PostManager, color_eyre::Report>(PostManager::new_with_cache(
                    config.dirs.posts.clone(),
                    config.render.clone(),
                    cache,
                ))
            }
            .await;
            match load_cache {
                Ok(posts) => posts,
                Err(err) => {
                    error!("failed to load cache: {}", err);
                    info!("using empty cache");
                    PostManager::new_with_cache(
                        config.dirs.posts.clone(),
                        config.render.clone(),
                        Default::default(),
                    )
                }
            }
        } else {
            PostManager::new_with_cache(
                config.dirs.posts.clone(),
                config.render.clone(),
                Default::default(),
            )
        }
    } else {
        PostManager::new(config.dirs.posts.clone(), config.render.clone())
    };

    let state = Arc::new(AppState { config, posts });

    if state.config.cache.enable && state.config.cache.cleanup {
        if let Some(t) = state.config.cache.cleanup_interval {
            let state = Arc::clone(&state);
            let token = cancellation_token.child_token();
            debug!("setting up cleanup task");
            tasks.spawn(async move {
                let mut interval = tokio::time::interval(Duration::from_millis(t));
                loop {
                    select! {
                        _ = token.cancelled() => break,
                        _ = interval.tick() => {
                            state.posts.cleanup().await
                        }
                    }
                }
            });
        } else {
            state.posts.cleanup().await;
        }
    }

    let app = Router::new()
        .route("/", get(index))
        .route(
            "/post/:name",
            get(
                |Path(name): Path<String>| async move { Redirect::to(&format!("/posts/{}", name)) },
            ),
        )
        .route("/posts/:name", get(post))
        .route("/posts", get(all_posts))
        .nest_service("/static", ServeDir::new("static").precompressed_gzip())
        .nest_service("/media", ServeDir::new("media"))
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
        .with_state(state.clone());

    let listener = TcpListener::bind(socket_addr)
        .await
        .with_context(|| format!("couldn't listen on {}", socket_addr))?;
    let local_addr = listener
        .local_addr()
        .context("couldn't get socket address")?;
    info!("listening on http://{}", local_addr);

    let sigint = signal::ctrl_c();
    #[cfg(unix)]
    let mut sigterm_handler =
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    #[cfg(unix)]
    let sigterm = sigterm_handler.recv();
    #[cfg(not(unix))] // TODO: kill all windows server users
    let sigterm = std::future::pending::<()>();

    let axum_token = cancellation_token.child_token();

    let mut server = axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(async move { axum_token.cancelled().await })
    .into_future();

    tokio::select! {
        result = &mut server => {
            result.context("failed to serve app")?;
        },
        _ = sigint => {
            info!("received SIGINT, exiting gracefully");
        },
        _ = sigterm => {
            info!("received SIGTERM, exiting gracefully");
        }
    };

    let cleanup = async move {
        // stop tasks
        cancellation_token.cancel();
        server.await.context("failed to serve app")?;
        while let Some(task) = tasks.join_next().await {
            task.context("failed to join task")?;
        }

        // write cache to file
        let config = &state.config;
        let posts = &state.posts;
        if config.cache.enable
            && config.cache.persistence
            && let Some(cache) = posts.cache()
        {
            let path = &config.cache.file;
            let serialized = bitcode::serialize(cache).context("failed to serialize cache")?;
            let mut cache_file = tokio::fs::File::create(path)
                .await
                .with_context(|| format!("failed to open cache at {}", path.display()))?;
            let compression_level = config.cache.compression_level;
            if config.cache.compress {
                let cache_file = cache_file.into_std().await;
                tokio::task::spawn_blocking(move || {
                    std::io::Write::write_all(
                        &mut zstd::stream::write::Encoder::new(cache_file, compression_level)?
                            .auto_finish(),
                        &serialized,
                    )
                })
                .await
                .context("failed to join blocking thread")?
            } else {
                cache_file.write_all(&serialized).await
            }
            .context("failed to write cache to file")?;
            info!("wrote cache to {}", path.display());
        }
        Ok::<(), color_eyre::Report>(())
    };

    let sigint = signal::ctrl_c();
    #[cfg(unix)]
    let mut sigterm_handler =
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    #[cfg(unix)]
    let sigterm = sigterm_handler.recv();
    #[cfg(not(unix))]
    let sigterm = std::future::pending::<()>();

    tokio::select! {
        result = cleanup => {
            result.context("cleanup failed, oh well")?;
        },
        _ = sigint => {
            warn!("received second signal, exiting");
            exit(1);
        },
        _ = sigterm => {
            warn!("received second signal, exiting");
            exit(1);
        }
    }

    Ok(())
}
