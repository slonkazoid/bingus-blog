use std::sync::{Arc, OnceLock, RwLock};

use comrak::markdown_to_html_with_plugins;
use comrak::plugins::syntect::{SyntectAdapter, SyntectAdapterBuilder};
use comrak::ComrakOptions;
use comrak::RenderPlugins;
use syntect::highlighting::ThemeSet;

use crate::config::RenderConfig;
use crate::hash_arc_store::HashArcStore;

fn syntect_adapter(config: &RenderConfig) -> Arc<SyntectAdapter> {
    static STATE: OnceLock<RwLock<HashArcStore<SyntectAdapter, RenderConfig>>> = OnceLock::new();
    let lock = STATE.get_or_init(|| RwLock::new(HashArcStore::new()));
    let mut guard = lock.write().unwrap();
    guard.get_or_init(config, build_syntect)
}

fn build_syntect(config: &RenderConfig) -> Arc<SyntectAdapter> {
    let mut theme_set = if config.syntect.load_defaults {
        ThemeSet::load_defaults()
    } else {
        ThemeSet::new()
    };
    if let Some(path) = config.syntect.themes_dir.as_ref() {
        theme_set.add_from_folder(path).unwrap();
    }
    let mut builder = SyntectAdapterBuilder::new().theme_set(theme_set);
    if let Some(theme) = config.syntect.theme.as_ref() {
        builder = builder.theme(theme);
    }
    Arc::new(builder.build())
}

pub fn render(markdown: &str, config: &RenderConfig) -> String {
    let mut options = ComrakOptions::default();
    options.extension.table = true;
    options.extension.autolink = true;
    options.extension.tasklist = true;
    options.extension.superscript = true;
    options.extension.strikethrough = true;
    options.extension.multiline_block_quotes = true;
    options.extension.header_ids = Some(String::new());

    let mut render_plugins = RenderPlugins::default();
    let syntect = syntect_adapter(config);
    render_plugins.codefence_syntax_highlighter = Some(syntect.as_ref());

    let plugins = comrak::PluginsBuilder::default()
        .render(render_plugins)
        .build()
        .unwrap();

    markdown_to_html_with_plugins(markdown, &options, &plugins)
}
