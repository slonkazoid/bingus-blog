use std::fs::File;
use std::io::BufReader;
use std::path::PathBuf;

use clap::Parser;
use color_eyre::eyre::{self, Context, Ok, OptionExt};
use syntect::highlighting::{Theme, ThemeSet};
use syntect::html::{css_for_theme_with_class_style, ClassStyle};

#[derive(Parser, Debug)]
#[command(about = "generate CSS from a syntect theme")]
struct Args {
    #[command(subcommand)]
    command: Command,
    #[arg(
        short,
        long,
        help = "prefix for generated classes",
        default_value = "syntect-"
    )]
    prefix: String,
    #[arg(
        long,
        help = "don't add a prefix to generated classes",
        default_value_t = false
    )]
    no_prefix: bool,
}

#[derive(Parser, Debug)]
enum Command {
    #[command(about = "generate CSS from a theme in the default theme set")]
    Default {
        #[arg(help = "name of theme (no .tmTheme)")]
        theme_name: String,
    },
    #[command(about = "generate CSS from a .tmTheme file")]
    File {
        #[arg(help = "path to theme (including .tmTheme)")]
        path: PathBuf,
    },
}

fn main() -> eyre::Result<()> {
    let args = Args::parse();
    color_eyre::install()?;

    let theme = match args.command {
        Command::Default { theme_name } => {
            let ts = ThemeSet::load_defaults();
            ts.themes
                .get(&theme_name)
                .ok_or_eyre(format!("theme {:?} doesn't exist", theme_name))?
                .to_owned()
        }
        Command::File { path } => {
            let mut file = BufReader::new(
                File::open(&path).with_context(|| format!("failed to open {:?}", path))?,
            );
            ThemeSet::load_from_reader(&mut file).with_context(|| "failed to parse theme")?
        }
    };

    let class_style = if args.no_prefix {
        ClassStyle::Spaced
    } else {
        ClassStyle::SpacedPrefixed {
            prefix: args.prefix.leak(),
        }
    };

    let css = css_for_theme_with_class_style(&theme, class_style)
        .with_context(|| "failed to generate css")?;
    println!("{css}");
    Ok(())
}
