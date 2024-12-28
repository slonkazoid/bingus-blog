use color_eyre::eyre::{self, Context};
use comrak::adapters::SyntaxHighlighterAdapter;
use comrak::markdown_to_html_with_plugins;
use comrak::plugins::syntect::{SyntectAdapter, SyntectAdapterBuilder};
use comrak::ComrakOptions;
use comrak::RenderPlugins;
use syntect::highlighting::ThemeSet;

use crate::config::RenderConfig;

pub fn build_syntect(config: &RenderConfig) -> eyre::Result<SyntectAdapter> {
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

pub fn render(markdown: &str, syntect: Option<&dyn SyntaxHighlighterAdapter>) -> String {
    let mut options = ComrakOptions::default();
    options.extension.table = true;
    options.extension.autolink = true;
    options.extension.tasklist = true;
    options.extension.superscript = true;
    options.extension.strikethrough = true;
    options.extension.multiline_block_quotes = true;
    options.extension.header_ids = Some(String::new());

    let mut render_plugins = RenderPlugins::default();
    render_plugins.codefence_syntax_highlighter = syntect;

    let plugins = comrak::Plugins::builder().render(render_plugins).build();

    markdown_to_html_with_plugins(markdown, &options, &plugins)
}
