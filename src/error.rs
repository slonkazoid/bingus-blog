use std::fmt::Display;

use axum::{http::StatusCode, response::IntoResponse};
use thiserror::Error;

// fronma is too lazy to implement std::error::Error for their own types
#[derive(Debug)]
#[repr(transparent)]
pub struct FronmaBalls(fronma::error::Error);

impl std::error::Error for FronmaBalls {}

impl Display for FronmaBalls {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("failed to parse front matter: ")?;
        match &self.0 {
            fronma::error::Error::MissingBeginningLine => f.write_str("missing beginning line"),
            fronma::error::Error::MissingEndingLine => f.write_str("missing ending line"),
            fronma::error::Error::SerdeYaml(_) => {
                unimplemented!("no yaml allowed in this household")
            }
            fronma::error::Error::Toml(toml_error) => write!(f, "{}", toml_error),
        }
    }
}

#[derive(Error, Debug)]
#[allow(clippy::enum_variant_names)]
pub enum PostError {
    #[error(transparent)]
    IoError(#[from] std::io::Error),
    #[error(transparent)]
    AskamaError(#[from] askama::Error),
    #[error(transparent)]
    ParseError(#[from] FronmaBalls),
    #[error("post {0:?} not found")]
    NotFound(String),
}

impl From<fronma::error::Error> for PostError {
    fn from(value: fronma::error::Error) -> Self {
        Self::ParseError(FronmaBalls(value))
    }
}

impl IntoResponse for PostError {
    fn into_response(self) -> axum::response::Response {
        (StatusCode::INTERNAL_SERVER_ERROR, self.to_string()).into_response()
    }
}
