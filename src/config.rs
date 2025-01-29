use std::borrow::Cow;
use std::env;
use std::net::{IpAddr, Ipv6Addr};
use std::num::NonZeroU64;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use arc_swap::ArcSwap;
use color_eyre::eyre::{self, bail, Context};
use const_str::{concat, convert_ascii_case};
use notify_debouncer_full::notify::RecursiveMode;
use notify_debouncer_full::{new_debouncer, DebouncedEvent};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::select;
use tokio_util::sync::CancellationToken;
use tracing::{error, info, instrument, trace};
use url::Url;

use crate::de::*;

#[derive(Serialize, Deserialize, Debug, PartialEq, Eq, Hash)]
#[serde(default)]
pub struct SyntectConfig {
    pub load_defaults: bool,
    pub themes_dir: Option<Box<Path>>,
    pub theme: Option<Box<str>>,
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(default)]
pub struct CacheConfig {
    pub enable: bool,
    #[serde(deserialize_with = "check_millis")]
    pub ttl: Option<NonZeroU64>,
    pub cleanup: bool,
    #[serde(deserialize_with = "check_millis")]
    pub cleanup_interval: Option<NonZeroU64>,
    pub persistence: bool,
    pub file: Box<Path>,
    pub compress: bool,
    #[serde(deserialize_with = "check_zstd_level_bounds")]
    pub compression_level: i32,
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(default)]
pub struct HttpConfig {
    pub host: IpAddr,
    pub port: u16,
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(default)]
pub struct DirsConfig {
    pub media: Box<Path>,
    #[serde(rename = "static")]
    pub static_: Box<Path>,
    pub templates: Box<Path>,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct RssConfig {
    pub enable: bool,
    pub link: Url,
}

#[derive(Serialize, Deserialize, Debug, Default)]
pub enum DateFormat {
    #[default]
    RFC3339,
    #[serde(untagged)]
    Strftime(Box<str>),
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
#[repr(u8)]
pub enum Sort {
    #[default]
    Date,
    Name,
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(default)]
pub struct StyleConfig {
    pub title: Box<str>,
    pub description: Box<str>,
    pub js_enable: bool,
    pub display_dates: DisplayDates,
    pub date_format: DateFormat,
    pub default_sort: Sort,
    pub default_color: Option<Box<str>>,
}

impl Default for StyleConfig {
    fn default() -> Self {
        Self {
            title: "bingus-blog".into(),
            description: "blazingly fast markdown blog software written in rust memory safe".into(),
            js_enable: true,
            display_dates: Default::default(),
            date_format: Default::default(),
            default_sort: Default::default(),
            default_color: Default::default(),
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy)]
#[serde(default)]
pub struct DisplayDates {
    pub creation: bool,
    pub modification: bool,
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Eq, Hash, Default)]
#[serde(default)]
pub struct MarkdownRenderConfig {
    pub syntect: SyntectConfig,
    pub escape: bool,
    #[serde(rename = "unsafe")]
    pub unsafe_: bool,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct MarkdownConfig {
    pub root: Box<Path>,
    pub render: MarkdownRenderConfig,
    pub raw_access: bool,
}

impl Default for MarkdownConfig {
    fn default() -> Self {
        Self {
            root: PathBuf::from("posts").into(),
            render: Default::default(),
            raw_access: true,
        }
    }
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(default)]
pub struct BlagConfig {
    pub root: Box<Path>,
    pub bin: Box<Path>,
    pub raw_access: bool,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, Default)]
#[serde(rename_all = "lowercase")]
pub enum EngineMode {
    #[default]
    Markdown,
    Blag,
}

#[derive(Serialize, Deserialize, Debug, Default)]
#[serde(default, rename_all = "lowercase")]
pub struct Engine {
    pub mode: EngineMode,
    pub markdown: MarkdownConfig,
    pub blag: BlagConfig,
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(default)]
pub struct Config {
    pub engine: Engine,
    pub style: StyleConfig,
    pub rss: RssConfig,
    #[serde(rename = "custom")]
    pub dirs: DirsConfig,
    pub http: HttpConfig,
    pub cache: CacheConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            engine: Default::default(),
            style: Default::default(),
            // i have a love-hate relationship with serde
            // it was engimatic at first, but then i started actually using it
            // writing my own serialize and deserialize implementations.. spending
            // a lot of time in the docs trying to understand each and every option..
            // now with this knowledge i can do stuff like this! (see rss field)
            // and i'm proud to say that it still makes 0 sense.
            rss: RssConfig {
                enable: false,
                link: Url::parse("http://example.com").unwrap(),
            },
            dirs: Default::default(),
            http: Default::default(),
            cache: Default::default(),
        }
    }
}

impl Default for DisplayDates {
    fn default() -> Self {
        Self {
            creation: true,
            modification: true,
        }
    }
}

impl Default for DirsConfig {
    fn default() -> Self {
        Self {
            media: PathBuf::from("media").into_boxed_path(),
            static_: PathBuf::from("static").into_boxed_path(),
            templates: PathBuf::from("templates").into_boxed_path(),
        }
    }
}

impl Default for HttpConfig {
    fn default() -> Self {
        Self {
            host: IpAddr::V6(Ipv6Addr::UNSPECIFIED),
            port: 3000,
        }
    }
}

impl Default for SyntectConfig {
    fn default() -> Self {
        Self {
            load_defaults: false,
            themes_dir: Some(PathBuf::from("themes").into_boxed_path()),
            theme: Some("Catppuccin Mocha".into()),
        }
    }
}

impl Default for CacheConfig {
    fn default() -> Self {
        Self {
            enable: true,
            ttl: None,
            cleanup: true,
            cleanup_interval: None,
            persistence: true,
            file: PathBuf::from("cache").into(),
            compress: true,
            compression_level: 3,
        }
    }
}

impl Default for BlagConfig {
    fn default() -> Self {
        Self {
            root: PathBuf::from("posts").into(),
            bin: PathBuf::from("blag").into(),
            raw_access: true,
        }
    }
}

fn config_path() -> Cow<'static, str> {
    env::var(concat!(
        convert_ascii_case!(shouty_snake, env!("CARGO_BIN_NAME")),
        "_CONFIG"
    ))
    .map(Into::into)
    .unwrap_or("config.toml".into())
}

pub async fn load_from(path: (impl AsRef<Path> + std::fmt::Debug)) -> eyre::Result<Config> {
    match tokio::fs::OpenOptions::new().read(true).open(&path).await {
        Ok(mut file) => {
            let mut buf = String::new();
            file.read_to_string(&mut buf)
                .await
                .context("couldn't read configuration file")?;
            toml::from_str(&buf).context("couldn't parse configuration")
        }
        Err(err) => match err.kind() {
            std::io::ErrorKind::NotFound => {
                let config = Config::default();
                info!("configuration file doesn't exist, creating");
                match tokio::fs::OpenOptions::new()
                    .write(true)
                    .create(true)
                    .truncate(true)
                    .open(&path)
                    .await
                {
                    Ok(mut file) => file
                        .write_all(
                            toml::to_string_pretty(&config)
                                .context("couldn't serialize configuration")?
                                .as_bytes(),
                        )
                        .await
                        .unwrap_or_else(|err| error!("couldn't write configuration: {err}")),
                    Err(err) => error!("couldn't open file {path:?} for writing: {err}"),
                }
                Ok(config)
            }
            _ => bail!("couldn't open config file: {err}"),
        },
    }
}

#[instrument]
pub async fn load() -> eyre::Result<(Config, Cow<'static, str>)> {
    let config_file = config_path();
    let config = load_from(&*config_file).await?;
    Ok((config, config_file))
}

async fn process_event(
    event: DebouncedEvent,
    config_file: &Path,
    swapper: &ArcSwap<Config>,
) -> eyre::Result<()> {
    if !event.kind.is_modify() && !event.kind.is_create()
        || !event.paths.iter().any(|p| p == config_file)
    {
        trace!("not interested: {event:?}");
        return Ok(());
    }

    let config = load_from(config_file).await?;
    info!("reloaded config from {config_file:?}");

    swapper.store(Arc::new(config));

    Ok(())
}

#[instrument(skip_all)]
pub async fn watcher(
    config_file: impl AsRef<str>,
    watcher_token: CancellationToken,
    swapper: Arc<ArcSwap<Config>>,
) -> eyre::Result<()> {
    let config_file = tokio::fs::canonicalize(config_file.as_ref())
        .await
        .context("failed to canonicalize path")?;

    let (tx, mut rx) = tokio::sync::mpsc::channel(1);

    let mut debouncer = new_debouncer(Duration::from_millis(100), None, move |events| {
        tx.blocking_send(events)
            .expect("failed to send message over channel")
    })?;

    let dir = config_file
        .as_path()
        .parent()
        .expect("absolute path to have parent");
    debouncer
        .watch(&dir, RecursiveMode::NonRecursive)
        .with_context(|| format!("failed to watch {dir:?}"))?;

    'event_loop: while let Some(ev) = select! {
        _ = watcher_token.cancelled() => {
            info!("2");
            break 'event_loop;
        },
        ev = rx.recv() => ev,
    } {
        let events = match ev {
            Ok(events) => events,
            Err(err) => {
                error!("error getting events: {err:?}");
                continue;
            }
        };

        for event in events {
            if let Err(err) = process_event(event, &config_file, &swapper).await {
                error!("error while processing event: {err}");
            }
        }
    }

    Ok(())
}

fn check_zstd_level_bounds<'de, D>(d: D) -> Result<i32, D::Error>
where
    D: serde::Deserializer<'de>,
{
    d.deserialize_i32(RangedI64Visitor::<1, 22>)
        .map(|x| x as i32)
}

fn check_millis<'de, D>(d: D) -> Result<Option<NonZeroU64>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    d.deserialize_option(MillisVisitor)
}
