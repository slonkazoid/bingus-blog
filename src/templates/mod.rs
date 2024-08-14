pub mod watcher;

use std::{io, path::Path};

use handlebars::{Handlebars, Template};
use include_dir::{include_dir, Dir};
use thiserror::Error;
use tracing::{debug, error, info_span, trace};

const TEMPLATES: Dir<'static> = include_dir!("$CARGO_MANIFEST_DIR/templates");
const PARTIALS: Dir<'static> = include_dir!("$CARGO_MANIFEST_DIR/partials");

#[derive(Error, Debug)]
#[allow(clippy::enum_variant_names)]
pub enum TemplateError {
    #[error(transparent)]
    IoError(#[from] std::io::Error),
    #[error("file doesn't contain valid UTF-8")]
    UTF8Error,
    #[error(transparent)]
    TemplateError(#[from] handlebars::TemplateError),
}

fn is_ext(path: impl AsRef<Path>, ext: &str) -> bool {
    match path.as_ref().extension() {
        Some(path_ext) if path_ext != ext => false,
        None => false,
        _ => true,
    }
}

 fn get_template_name(path: &Path) -> Option<&str> {
    if !is_ext(path, "hbs") {
        return None;
    }

    path.file_stem()?.to_str()
}

fn register_included_file(
    file: &include_dir::File<'_>,
    name: &str,
    registry: &mut Handlebars,
) -> Result<(), TemplateError> {
    let template = compile_included_file(file)?;
    registry.register_template(name, template);
    Ok(())
}

fn register_path(
    path: impl AsRef<std::path::Path>,
    name: &str,
    registry: &mut Handlebars<'_>,
) -> Result<(), TemplateError> {
    let template = compile_path(path)?;
    registry.register_template(name, template);
    Ok(())
}

fn register_partial(
    file: &include_dir::File<'_>,
    name: &str,
    registry: &mut Handlebars,
) -> Result<(), TemplateError> {
    registry.register_partial(name, file.contents_utf8().ok_or(TemplateError::UTF8Error)?)?;
    Ok(())
}

fn compile_included_file(file: &include_dir::File<'_>) -> Result<Template, TemplateError> {
    let contents = file.contents_utf8().ok_or(TemplateError::UTF8Error)?;

    let template = Template::compile(contents)?;
    Ok(template)
}

fn compile_path(path: impl AsRef<std::path::Path>) -> Result<Template, TemplateError> {
    use std::fs::OpenOptions;
    use std::io::Read;

    let mut file = OpenOptions::new().read(true).open(path)?;
    let mut buf = String::new();
    file.read_to_string(&mut buf)?;

    let template = Template::compile(&buf)?;
    Ok(template)
}

 async fn compile_path_async_io(
    path: impl AsRef<std::path::Path>,
) -> Result<Template, TemplateError> {
    use tokio::fs::OpenOptions;
    use tokio::io::AsyncReadExt;

    let mut file = OpenOptions::new().read(true).open(path).await?;
    let mut buf = String::new();
    file.read_to_string(&mut buf).await?;

    let template = Template::compile(&buf)?;
    Ok(template)
}

pub fn new_registry<'a>(custom_templates_path: impl AsRef<Path>) -> io::Result<Handlebars<'a>> {
    let mut reg = Handlebars::new();

    for entry in TEMPLATES.entries() {
        let file = match entry.as_file() {
            Some(file) => file,
            None => continue,
        };

        let span = info_span!("register_included_template", path = ?file.path());
        let _handle = span.enter();

        let name = match get_template_name(file.path()) {
            Some(v) => v,
            None => {
                trace!("skipping file");
                continue;
            }
        };

        match register_included_file(file, name, &mut reg) {
            Ok(()) => debug!("registered template {name:?}"),
            Err(err) => error!("error while registering template: {err}"),
        };
    }

    for entry in PARTIALS.entries() {
        let file = match entry.as_file() {
            Some(file) => file,
            None => continue,
        };

        let span = info_span!("register_partial", path = ?file.path());
        let _handle = span.enter();

        let name = match get_template_name(file.path()) {
            Some(v) => v,
            None => {
                trace!("skipping file");
                continue;
            }
        };

        match register_partial(file, name, &mut reg) {
            Ok(()) => debug!("registered partial {name:?}"),
            Err(err) => error!("error while registering partial: {err}"),
        };
    }

    let read_dir = match std::fs::read_dir(custom_templates_path) {
        Ok(v) => v,
        Err(err) => match err.kind() {
            io::ErrorKind::NotFound => return Ok(reg),
            _ => panic!("{:?}", err),
        },
    };
    for entry in read_dir {
        let entry = entry.unwrap();

        let file_type = entry.file_type()?;
        if !file_type.is_file() {
            continue;
        }

        let path = entry.path();

        let span = info_span!("register_custom_template", ?path);
        let _handle = span.enter();

        let name = match get_template_name(&path) {
            Some(v) => v,
            None => {
                trace!("skipping file");
                continue;
            }
        };

        match register_path(&path, name, &mut reg) {
            Ok(()) => debug!("registered template {name:?}"),
            Err(err) => error!("error while registering template: {err}"),
        };
    }

    Ok(reg)
}
