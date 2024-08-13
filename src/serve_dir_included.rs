use std::convert::Infallible;
use std::str::pattern::Pattern;

use axum::extract::Request;
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use include_dir::{Dir, DirEntry};
use tracing::{debug, trace};

fn if_empty<'a>(a: &'a str, b: &'a str) -> &'a str {
    if a.is_empty() {
        b
    } else {
        a
    }
}

fn remove_prefixes(mut src: &str, pat: (impl Pattern + Copy)) -> &str {
    while let Some(removed) = src.strip_prefix(pat) {
        src = removed;
    }
    src
}

fn from_included_file(file: &'static include_dir::File<'static>) -> Response {
    let mime_type = mime_guess::from_path(file.path()).first_or_octet_stream();

    (
        [(
            header::CONTENT_TYPE,
            header::HeaderValue::try_from(mime_type.essence_str()).expect("invalid mime type"),
        )],
        file.contents(),
    )
        .into_response()
}

pub async fn handle(
    req: Request,
    included_dir: &'static Dir<'static>,
) -> Result<Response, Infallible> {
    #[cfg(windows)]
    compile_error!("this is not safe");

    let path = req.uri().path();

    let has_dotdot = path.split('/').any(|seg| seg == "..");
    if has_dotdot {
        return Ok(StatusCode::NOT_FOUND.into_response());
    }

    let relative_path = if_empty(remove_prefixes(path, '/'), ".");

    match included_dir.get_entry(relative_path) {
        Some(DirEntry::Dir(dir)) => {
            trace!("{relative_path:?} is a directory, trying \"index.html\"");
            if let Some(file) = dir.get_file("index.html") {
                debug!("{path:?} (index.html) serving from included dir");
                return Ok(from_included_file(file));
            } else {
                trace!("\"index.html\" not found in {relative_path:?} in included files");
            }
        }
        None if relative_path == "." => {
            trace!("requested root, trying \"index.html\"");
            if let Some(file) = included_dir.get_file("index.html") {
                debug!("{path:?} (index.html) serving from included dir");
                return Ok(from_included_file(file));
            } else {
                trace!("\"index.html\" not found in included files");
            }
        }
        Some(DirEntry::File(file)) => {
            debug!("{path:?} serving from included dir");
            return Ok(from_included_file(file));
        }
        None => trace!("{relative_path:?} not found in included files"),
    };

    Ok(StatusCode::NOT_FOUND.into_response())
}
