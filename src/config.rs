use std::env;
use std::net::{IpAddr, Ipv4Addr};
use std::path::PathBuf;

use color_eyre::eyre::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tracing::{error, info};
use url::Url;

use crate::ranged_i128_visitor::RangedI128Visitor;

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
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(default)]
pub struct CacheConfig {
    pub enable: bool,
    pub cleanup: bool,
    pub cleanup_interval: Option<u64>,
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
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct RssConfig {
    pub enable: bool,
    pub link: Url,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(default)]
pub struct Config {
    pub title: String,
    pub description: String,
    pub raw_access: bool,
    pub num_posts: usize,
    pub rss: RssConfig,
    pub dirs: DirsConfig,
    pub http: HttpConfig,
    pub render: RenderConfig,
    pub cache: CacheConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            title: "bingus-blog".into(),
            description: "blazingly fast markdown blog software written in rust memory safe".into(),
            raw_access: true,
            num_posts: 5,
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
        }
    }
}

impl Default for DirsConfig {
    fn default() -> Self {
        Self {
            posts: "posts".into(),
            media: "media".into(),
        }
    }
}

impl Default for HttpConfig {
    fn default() -> Self {
        Self {
            host: IpAddr::V4(Ipv4Addr::UNSPECIFIED),
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
            cleanup: true,
            cleanup_interval: None,
            persistence: true,
            file: "cache".into(),
            compress: true,
            compression_level: 3,
        }
    }
}

pub async fn load() -> Result<Config> {
    let config_file = env::var(format!("{}_CONFIG", env!("CARGO_BIN_NAME")))
        .unwrap_or(String::from("config.toml"));
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
    d.deserialize_i32(RangedI128Visitor::<1, 22>)
        .map(|x| x as i32)
}
