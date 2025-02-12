#![feature(let_chains, pattern, path_add_extension, if_let_guard)]

mod app;
mod config;
mod de;
mod error;
mod helpers;
mod markdown_render;
mod path;
mod platform;
mod post;
mod serve_dir_included;
mod systemtime_as_secs;
mod templates;

use std::future::IntoFuture;
use std::net::SocketAddr;
use std::process::exit;
use std::sync::Arc;
use std::time::Duration;

use arc_swap::access::Map;
use arc_swap::ArcSwap;
use color_eyre::eyre::{self, Context};
use config::{Config, EngineMode};
use tokio::net::TcpListener;
use tokio::sync::RwLock;
use tokio::task::JoinSet;
use tokio::time::Instant;
use tokio::{select, signal};
use tokio_util::sync::CancellationToken;
use tracing::level_filters::LevelFilter;
use tracing::{debug, error, info, warn};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::{util::SubscriberInitExt, EnvFilter};

use crate::app::AppState;
use crate::post::cache::{load_cache, Cache, CacheGuard, CACHE_VERSION};
use crate::post::{Blag, MarkdownPosts, PostManager};
use crate::templates::new_registry;
use crate::templates::watcher::watch_templates;

#[tokio::main]
async fn main() -> eyre::Result<()> {
    color_eyre::install()?;
    let reg = tracing_subscriber::registry();
    #[cfg(feature = "tokio-console")]
    let reg = reg.with(console_subscriber::spawn());
    #[cfg(not(feature = "tokio-console"))]
    let reg = reg.with(
        EnvFilter::builder()
            .with_default_directive(LevelFilter::INFO.into())
            .from_env_lossy(),
    );
    reg.with(tracing_subscriber::fmt::layer()).init();

    let mut tasks = JoinSet::new();
    let cancellation_token = CancellationToken::new();

    let (config, config_file) = config::load()
        .await
        .context("couldn't load configuration")?;
    let config = Arc::new(config);
    let swapper = Arc::new(ArcSwap::from(config.clone()));
    let config_cache_access: crate::post::cache::ConfigAccess =
        Box::new(arc_swap::access::Map::new(swapper.clone(), |c: &Config| {
            &c.cache
        }));

    info!("loaded config from {config_file:?}");

    let start = Instant::now();
    // NOTE: use tokio::task::spawn_blocking if this ever turns into a concurrent task
    let mut reg =
        new_registry(&config.dirs.templates).context("failed to create handlebars registry")?;
    reg.register_helper("date", Box::new(helpers::date));
    reg.register_helper("duration", Box::new(helpers::duration));
    debug!(duration = ?start.elapsed(), "registered all templates");

    let registry = Arc::new(RwLock::new(reg));

    debug!("setting up watcher");
    let watcher_token = cancellation_token.child_token();
    tasks.spawn(watch_templates(
        config.dirs.templates.clone(),
        watcher_token.clone(),
        registry.clone(),
    ));

    let cache = if config.cache.enable {
        if config.cache.persistence && tokio::fs::try_exists(&config.cache.file).await? {
            info!("loading cache from file");
            let mut cache = load_cache(&config.cache).await.unwrap_or_else(|err| {
                error!("failed to load cache: {}", err);
                info!("using empty cache");
                Cache::new(config.cache.ttl)
            });

            if cache.version() < CACHE_VERSION {
                warn!("cache version changed, clearing cache");
                cache = Cache::new(config.cache.ttl);
            };

            Some(cache)
        } else {
            Some(Cache::new(config.cache.ttl))
        }
    } else {
        None
    }
    .map(|cache| CacheGuard::new(cache, config_cache_access))
    .map(Arc::new);

    let posts: Arc<dyn PostManager + Send + Sync> = match config.engine.mode {
        EngineMode::Markdown => {
            let access = Map::new(swapper.clone(), |c: &Config| &c.engine.markdown);
            Arc::new(MarkdownPosts::new(access, cache.clone()).await?)
        }
        EngineMode::Blag => {
            let access = Map::new(swapper.clone(), |c: &Config| &c.engine.blag);
            Arc::new(Blag::new(access, cache.clone()))
        }
    };

    debug!("setting up config watcher");

    let token = cancellation_token.child_token();

    tasks.spawn(config::watcher(config_file, token, swapper.clone()));

    if config.cache.enable && config.cache.cleanup {
        if let Some(millis) = config.cache.cleanup_interval {
            let posts = Arc::clone(&posts);
            let token = cancellation_token.child_token();
            debug!("setting up cleanup task");
            tasks.spawn(async move {
                let mut interval = tokio::time::interval(Duration::from_millis(millis.into()));
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

    let state = AppState {
        rss: Arc::new(Map::new(swapper.clone(), |c: &Config| &c.rss)),
        style: Arc::new(Map::new(swapper.clone(), |c: &Config| &c.style)),
        posts,
        templates: registry,
    };
    let app = app::new(&config.dirs).with_state(state.clone());

    let socket_addr = SocketAddr::new(config.http.host, config.http.port);
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
