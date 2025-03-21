use std::sync::Arc;

use askama::Template;
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Response};
use color_eyre::eyre;
use thiserror::Error;
use tracing::error;

#[derive(Error, Debug)]
#[allow(clippy::enum_variant_names)]
pub enum PostError {
    #[error("io error: {0}")]
    IoError(#[from] std::io::Error),
    #[error("failed to parse post metadata: {0}")]
    ParseError(String),
    #[error("failed to render post: {0}")]
    RenderError(String),
    #[error("post {0:?} not found")]
    NotFound(Arc<str>),
    #[error("unexpected: {0}")]
    Other(#[from] eyre::Error),
}

impl From<fronma::error::Error> for PostError {
    fn from(value: fronma::error::Error) -> Self {
        let binding;
        Self::ParseError(format!(
            "failed to parse front matter: {}",
            match value {
                fronma::error::Error::MissingBeginningLine => "missing beginning line",
                fronma::error::Error::MissingEndingLine => "missing ending line",
                fronma::error::Error::SerdeYaml(yaml_error) => {
                    binding = yaml_error.to_string();
                    &binding
                }
            }
        ))
    }
}

impl From<serde_json::Error> for PostError {
    fn from(value: serde_json::Error) -> Self {
        Self::ParseError(value.to_string())
    }
}

impl IntoResponse for PostError {
    fn into_response(self) -> Response {
        (StatusCode::INTERNAL_SERVER_ERROR, self.to_string()).into_response()
    }
}

pub type AppResult<T> = Result<T, AppError>;

#[derive(Error, Debug)]
pub enum AppError {
    #[error("failed to fetch post: {0}")]
    PostError(#[from] PostError),
    #[error(transparent)]
    HandlebarsError(#[from] handlebars::RenderError),
    #[error("rss is disabled")]
    RssDisabled,
    #[error(transparent)]
    UrlError(#[from] url::ParseError),
}

impl From<std::io::Error> for AppError {
    #[inline(always)]
    fn from(value: std::io::Error) -> Self {
        Self::PostError(PostError::IoError(value))
    }
}

#[derive(Template)]
#[template(path = "error.html")]
struct ErrorTemplate {
    error: String,
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let error = self.to_string();
        error!("error while handling request: {error}");

        let status_code = match &self {
            AppError::PostError(PostError::NotFound(_)) => StatusCode::NOT_FOUND,
            AppError::RssDisabled => StatusCode::FORBIDDEN,
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        };

        match (ErrorTemplate { error }.render()) {
            Ok(rendered) => (status_code, Html(rendered)).into_response(),
            Err(err) => {
                error!("error while rendering error template: {err}");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "error while trying to show error. good job",
                )
                    .into_response()
            }
        }
    }
}
