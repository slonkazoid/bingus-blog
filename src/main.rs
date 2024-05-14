#![feature(let_chains)]

mod app;
mod config;
mod error;
mod filters;
mod hash_arc_store;
mod markdown_render;
mod post;
mod ranged_i128_visitor;
mod systemtime_as_secs;

use std::future::IntoFuture;
use std::net::SocketAddr;
use std::process::exit;
use std::sync::Arc;
use std::time::Duration;

use color_eyre::eyre::{self, Context};
use tokio::net::TcpListener;
use tokio::task::JoinSet;
use tokio::{select, signal};
use tokio_util::sync::CancellationToken;
use tracing::level_filters::LevelFilter;
use tracing::{debug, info, warn};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::{util::SubscriberInitExt, EnvFilter};

use crate::app::AppState;
use crate::post::{MarkdownPosts, PostManager};

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

    let config = Arc::new(
        config::load()
            .await
            .context("couldn't load configuration")?,
    );

    let socket_addr = SocketAddr::new(config.http.host, config.http.port);

    let mut tasks = JoinSet::new();
    let cancellation_token = CancellationToken::new();

    let posts = Arc::new(MarkdownPosts::new(Arc::clone(&config)).await?);
    let state = AppState {
        config: Arc::clone(&config),
        posts: Arc::clone(&posts),
    };

    if config.cache.enable && config.cache.cleanup {
        if let Some(t) = config.cache.cleanup_interval {
            let posts = Arc::clone(&posts);
            let token = cancellation_token.child_token();
            debug!("setting up cleanup task");
            tasks.spawn(async move {
                let mut interval = tokio::time::interval(Duration::from_millis(t));
                loop {
                    select! {
                        _ = token.cancelled() => break,
                        _ = interval.tick() => {
                            posts.cleanup().await
                        }
                    }
                }
            });
        } else {
            posts.cleanup().await;
        }
    }

    let app = app::new(&config).with_state(state.clone());

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

        drop(state);
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
