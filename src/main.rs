#![feature(let_chains, stmt_expr_attributes, proc_macro_hygiene)]

mod append_path;
mod compress;
mod config;
mod error;
mod filters;
mod hash_arc_store;
mod markdown_render;
mod post;
mod watcher;

use std::future::IntoFuture;
use std::net::SocketAddr;
use std::process::exit;
use std::sync::Arc;
use std::time::Duration;

use askama_axum::Template;
use axum::extract::{MatchedPath, Path, State};
use axum::http::{Request, StatusCode};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, Router};
use axum::Json;
use color_eyre::eyre::{self, Context};
use thiserror::Error;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::signal;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;
use tower_http::services::ServeDir;
use tower_http::trace::TraceLayer;
use tracing::level_filters::LevelFilter;
use tracing::{error, info, info_span, warn, Span};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

use crate::compress::compress_epicly;
use crate::config::Config;
use crate::error::PostError;
use crate::post::{PostManager, PostMetadata, RenderStats};
use crate::watcher::watch;

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
}

type AppResult<T> = Result<T, AppError>;

#[derive(Error, Debug)]
enum AppError {
    #[error("failed to fetch post: {0}")]
    PostError(#[from] PostError),
}

#[derive(Template)]
#[template(path = "error.html")]
struct ErrorTemplate {
    error: String,
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let status_code = match &self {
            AppError::PostError(err) => match err {
                PostError::NotFound(_) => StatusCode::NOT_FOUND,
                _ => StatusCode::INTERNAL_SERVER_ERROR,
            },
            //_ => StatusCode::INTERNAL_SERVER_ERROR,
        };
        (
            status_code,
            ErrorTemplate {
                error: self.to_string(),
            },
        )
            .into_response()
    }
}

async fn index(State(state): State<ArcState>) -> AppResult<IndexTemplate> {
    Ok(IndexTemplate {
        title: state.config.title.clone(),
        description: state.config.description.clone(),
        posts: state.posts.list_posts().await?,
    })
}

async fn post(State(state): State<ArcState>, Path(name): Path<String>) -> AppResult<Response> {
    let post = state.posts.get_post(&name).await?;

    let post = ViewPostTemplate {
        meta: post.0,
        rendered: post.1,
        rendered_in: post.2,
    }
    .into_response();

    Ok(post)
}

async fn all_posts(State(state): State<ArcState>) -> AppResult<Json<Vec<PostMetadata>>> {
    let posts = state.posts.list_posts().await?;
    Ok(Json(posts))
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

    let mut tasks = JoinSet::new();
    let mut cancellation_tokens = Vec::new();

    #[cfg(feature = "precompression")]
    if config.precompression.enable {
        let span = info_span!("compression");
        info!(parent: span.clone(), "compressing static");

        let compressed = tokio::task::spawn_blocking(|| compress_epicly("static"))
            .await
            .unwrap()
            .context("couldn't compress static")?;

        let _handle = span.enter();

        if compressed > 0 {
            info!(compressed_files=%compressed, "compressed {compressed} files");
        }

        if config.precompression.watch {
            info!("starting compressor task");
            let span = span.clone();
            let token = CancellationToken::new();
            let passed_token = token.clone();
            tasks.spawn(async move {
                watch(span, passed_token, Default::default())
                    .await
                    .context("failed to watch static")
                    .unwrap()
            });
            cancellation_tokens.push(token);
        }
    }

    let posts = if let Some(path) = config.cache_file.as_ref()
        && tokio::fs::try_exists(&path)
            .await
            .with_context(|| format!("failed to check if {} exists", path.display()))?
    {
        info!("loading cache from file");
        let load_cache = async {
            let mut cache_file = tokio::fs::File::open(&path)
                .await
                .context("failed to open cache file")?;
            let mut serialized = Vec::with_capacity(4096);
            cache_file
                .read_to_end(&mut serialized)
                .await
                .context("failed to read cache file")?;
            let cache =
                bitcode::deserialize(serialized.as_slice()).context("failed to parse cache")?;
            Ok::<PostManager, color_eyre::Report>(PostManager::new_with_cache(
                config.posts_dir.clone(),
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
                PostManager::new(config.posts_dir.clone(), config.render.clone())
            }
        }
    } else {
        PostManager::new(config.posts_dir.clone(), config.render.clone())
    };

    let state = Arc::new(AppState { config, posts });

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
                    let matched_path = request
                        .extensions()
                        .get::<MatchedPath>()
                        .map(MatchedPath::as_str);

                    info_span!(
                        "request",
                        method = ?request.method(),
                        path = ?request.uri().path(),
                        matched_path,
                    )
                })
                .on_response(|response: &Response<_>, duration: Duration, span: &Span| {
                    let _ = span.enter();
                    let status = response.status();
                    info!(?status, ?duration, "response");
                }),
        )
        .with_state(state.clone());

    let listener = TcpListener::bind((state.config.host, state.config.port))
        .await
        .with_context(|| {
            format!(
                "couldn't listen on {}",
                SocketAddr::new(state.config.host, state.config.port)
            )
        })?;
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

    let axum_token = CancellationToken::new();
    cancellation_tokens.push(axum_token.clone());

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
        for token in cancellation_tokens {
            token.cancel();
        }
        server.await.context("failed to serve app")?;
        while let Some(task) = tasks.join_next().await {
            task.context("failed to join task")?;
        }

        // write cache to file
        let AppState { config, posts } = Arc::<AppState>::try_unwrap(state).unwrap_or_else(|state| {
            warn!("couldn't unwrap Arc over AppState, more than one strong reference exists for Arc. cloning instead");
            AppState::clone(state.as_ref())
        });
        if let Some(path) = config.cache_file.as_ref() {
            let cache = posts.into_cache();
            let mut serialized = bitcode::serialize(&cache).context("failed to serialize cache")?;
            let mut cache_file = tokio::fs::File::create(path)
                .await
                .with_context(|| format!("failed to open cache at {}", path.display()))?;
            cache_file
                .write_all(serialized.as_mut_slice())
                .await
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
