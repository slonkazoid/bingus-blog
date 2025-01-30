use axum::extract::rejection::PathRejection;
use axum::extract::{FromRequestParts, Path};
use axum::http::request::Parts;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::de::DeserializeOwned;
use thiserror::Error;

pub struct SafePath<T>(pub T);

impl<S, T> FromRequestParts<S> for SafePath<T>
where
    T: DeserializeOwned,
    T: AsRef<str>,
    T: Send + Sync,
    S: Send + Sync,
{
    type Rejection = SafePathRejection;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let s = Path::<T>::from_request_parts(parts, state).await?.0;

        if s.as_ref().contains("..") || s.as_ref().contains('/') {
            return Err(SafePathRejection::Invalid);
        }

        Ok(SafePath(s))
    }
}

#[derive(Error, Debug)]
pub enum SafePathRejection {
    #[error("path contains invalid characters")]
    Invalid,
    #[error(transparent)]
    PathRejection(#[from] PathRejection),
}

impl IntoResponse for SafePathRejection {
    fn into_response(self) -> Response {
        (StatusCode::BAD_REQUEST, self.to_string()).into_response()
    }
}
