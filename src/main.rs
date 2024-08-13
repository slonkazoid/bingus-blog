#![feature(let_chains)]

mod app;
mod config;
mod error;
mod hash_arc_store;
mod helpers;
mod markdown_render;
mod platform;
mod post;
mod ranged_i128_visitor;
mod systemtime_as_secs;
mod templates;

use std::future::IntoFuture;
use std::net::SocketAddr;
use std::process::exit;
use std::sync::Arc;
use std::time::Duration;

use color_eyre::eyre::{self, Context};
use tokio::net::TcpListener;
use tokio::sync::RwLock;
use tokio::task::JoinSet;
use tokio::time::Instant;
use tokio::{select, signal};
use tokio_util::sync::CancellationToken;
use tracing::level_filters::LevelFilter;
use tracing::{debug, error, info, info_span, warn, Instrument};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::{util::SubscriberInitExt, EnvFilter};

use crate::app::AppState;
use crate::post::{MarkdownPosts, PostManager};
use crate::templates::new_registry;
use crate::templates::watcher::watch_templates;

#[tokio::main]
async fn main() -> eyre::Result<()> {
    color_eyre::install()?;
    let reg = tracing_subscriber::registry();
    #[cfg(feature = "tokio-console")]
    let reg = reg
        .with(
            EnvFilter::builder()
                .with_default_directive(LevelFilter::TRACE.into())
                .from_env_lossy(),
        )
        .with(console_subscriber::spawn());
    #[cfg(not(feature = "tokio-console"))]
    let reg = reg.with(
        EnvFilter::builder()
            .with_default_directive(LevelFilter::INFO.into())
            .from_env_lossy(),
    );
    reg.with(tracing_subscriber::fmt::layer()).init();

    let config = Arc::new(
        config::load()
            .await
            .context("couldn't load configuration")?,
    );

    let socket_addr = SocketAddr::new(config.http.host, config.http.port);

    let mut tasks = JoinSet::new();
    let cancellation_token = CancellationToken::new();

    let start = Instant::now();
    // NOTE: use tokio::task::spawn_blocking if this ever turns into a concurrent task
    let mut reg =
        new_registry("custom/templates").context("failed to create handlebars registry")?;
    reg.register_helper("date", Box::new(helpers::date));
    reg.register_helper("duration", Box::new(helpers::duration));
    debug!(duration = ?start.elapsed(), "registered all templates");

    let reg = Arc::new(RwLock::new(reg));

    let watcher_token = cancellation_token.child_token();

    let posts = Arc::new(MarkdownPosts::new(Arc::clone(&config)).await?);
    let state = AppState {
        config: Arc::clone(&config),
        posts: Arc::clone(&posts),
        reg: Arc::clone(&reg),
    };

    debug!("setting up watcher");
    tasks.spawn(
        watch_templates("custom/templates", watcher_token.clone(), reg)
            .instrument(info_span!("custom_template_watcher")),
    );

    if config.cache.enable && config.cache.cleanup {
        if let Some(millis) = config.cache.cleanup_interval {
            let posts = Arc::clone(&posts);
            let token = cancellation_token.child_token();
            debug!("setting up cleanup task");
            tasks.spawn(async move {
                let mut interval = tokio::time::interval(Duration::from_millis(millis));
                loop {
                    select! {
                        _ = token.cancelled() => break Ok(()),
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
    let sigterm = platform::sigterm();

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
            let res = task.context("failed to join task")?;
            if let Err(err) = res {
                error!("task failed with error: {err}");
            }
        }

        drop(state);
        Ok::<(), color_eyre::Report>(())
    };

    let sigint = signal::ctrl_c();
    let sigterm = platform::sigterm();

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
