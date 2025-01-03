use std::env;
use std::net::{IpAddr, Ipv6Addr};
use std::num::NonZeroU64;
use std::path::PathBuf;

use color_eyre::eyre::{self, bail, Context};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tracing::{error, info, instrument};
use url::Url;

use crate::de::*;

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Hash)]
#[serde(default)]
pub struct SyntectConfig {
    pub load_defaults: bool,
    pub themes_dir: Option<PathBuf>,
    pub theme: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Hash, Default)]
#[serde(default)]
pub struct RenderConfig {
    pub syntect: SyntectConfig,
    pub escape: bool,
    #[serde(rename = "unsafe")]
    pub unsafe_: bool,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(default)]
pub struct CacheConfig {
    pub enable: bool,
    #[serde(deserialize_with = "check_millis")]
    pub ttl: Option<NonZeroU64>,
    pub cleanup: bool,
    #[serde(deserialize_with = "check_millis")]
    pub cleanup_interval: Option<NonZeroU64>,
    pub persistence: bool,
    pub file: PathBuf,
    pub compress: bool,
    #[serde(deserialize_with = "check_zstd_level_bounds")]
    pub compression_level: i32,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(default)]
pub struct HttpConfig {
    pub host: IpAddr,
    pub port: u16,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(default)]
pub struct DirsConfig {
    pub posts: PathBuf,
    pub media: PathBuf,
    pub custom_static: PathBuf,
    pub custom_templates: PathBuf,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct RssConfig {
    pub enable: bool,
    pub link: Url,
}

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub enum DateFormat {
    #[default]
    RFC3339,
    #[serde(untagged)]
    Strftime(String),
}

#[derive(Serialize, Deserialize, Debug, Clone, Default, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
#[repr(u8)]
pub enum Sort {
    #[default]
    Date,
    Name,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(default)]
#[derive(Default)]
pub struct StyleConfig {
    pub display_dates: DisplayDates,
    pub date_format: DateFormat,
    pub default_sort: Sort,
    pub default_color: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(default)]
pub struct DisplayDates {
    pub creation: bool,
    pub modification: bool,
}

#[derive(Serialize, Deserialize, Default, Debug, Clone)]
#[serde(rename_all = "lowercase")]
pub enum Engine {
    #[default]
    Markdown,
    Blag,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(default)]
pub struct BlagConfig {
    pub bin: PathBuf,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(default)]
pub struct Config {
    pub title: String,
    pub description: String,
    pub markdown_access: bool,
    pub js_enable: bool,
    pub engine: Engine,
    pub style: StyleConfig,
    pub rss: RssConfig,
    pub dirs: DirsConfig,
    pub http: HttpConfig,
    pub render: RenderConfig,
    pub cache: CacheConfig,
    pub blag: BlagConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            title: "bingus-blog".into(),
            description: "blazingly fast markdown blog software written in rust memory safe".into(),
            markdown_access: true,
            js_enable: true,
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
            render: Default::default(),
            cache: Default::default(),
            blag: Default::default(),
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
            posts: "posts".into(),
            media: "media".into(),
            custom_static: "static".into(),
            custom_templates: "templates".into(),
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
            themes_dir: Some("themes".into()),
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
            file: "cache".into(),
            compress: true,
            compression_level: 3,
        }
    }
}

impl Default for BlagConfig {
    fn default() -> Self {
        Self { bin: "blag".into() }
    }
}

#[instrument(name = "config")]
pub async fn load() -> eyre::Result<Config> {
    let config_file = env::var(format!(
        "{}_CONFIG",
        env!("CARGO_BIN_NAME").to_uppercase().replace('-', "_")
    ))
    .unwrap_or_else(|_| String::from("config.toml"));
    match tokio::fs::OpenOptions::new()
        .read(true)
        .open(&config_file)
        .await
    {
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
                    .open(&config_file)
                    .await
                {
                    Ok(mut file) => file
                        .write_all(
                            toml::to_string_pretty(&config)
                                .context("couldn't serialize configuration")?
                                .as_bytes(),
                        )
                        .await
                        .unwrap_or_else(|err| error!("couldn't write configuration: {}", err)),
                    Err(err) => {
                        error!("couldn't open file {:?} for writing: {}", &config_file, err)
                    }
                }
                Ok(config)
            }
            _ => bail!("couldn't open config file: {}", err),
        },
    }
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
