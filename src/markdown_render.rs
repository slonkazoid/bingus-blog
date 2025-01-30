use color_eyre::eyre::{self, Context};
use comrak::adapters::SyntaxHighlighterAdapter;
use comrak::plugins::syntect::{SyntectAdapter, SyntectAdapterBuilder};
use comrak::ComrakOptions;
use comrak::RenderPlugins;
use comrak::{markdown_to_html_with_plugins, Plugins};
use syntect::highlighting::ThemeSet;

use crate::config::MarkdownRenderConfig;

pub fn build_syntect(config: &MarkdownRenderConfig) -> eyre::Result<SyntectAdapter> {
    let mut theme_set = if config.syntect.load_defaults {
        ThemeSet::load_defaults()
    } else {
        ThemeSet::new()
    };
    if let Some(path) = config.syntect.themes_dir.as_ref() {
        theme_set
            .add_from_folder(path)
            .with_context(|| format!("failed to add themes from {path:?}"))?;
    }
    let mut builder = SyntectAdapterBuilder::new().theme_set(theme_set);
    if let Some(theme) = config.syntect.theme.as_ref() {
        builder = builder.theme(theme);
    }
    Ok(builder.build())
}

pub fn render(
    markdown: &str,
    config: &MarkdownRenderConfig,
    syntect: Option<&dyn SyntaxHighlighterAdapter>,
) -> String {
    let mut options = ComrakOptions::default();
    options.extension.table = true;
    options.extension.autolink = true;
    options.extension.tasklist = true;
    options.extension.superscript = true;
    options.extension.strikethrough = true;
    options.extension.multiline_block_quotes = true;
    options.extension.header_ids = Some(String::new());
    options.render.escape = config.escape;
    options.render.unsafe_ = config.unsafe_;

    let render_plugins = RenderPlugins {
        codefence_syntax_highlighter: syntect,
        ..Default::default()
    };

    let plugins = Plugins::builder().render(render_plugins).build();

    markdown_to_html_with_plugins(markdown, &options, &plugins)
}
