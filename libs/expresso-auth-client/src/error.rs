//! Auth client error types.

use thiserror::Error;

pub type Result<T> = std::result::Result<T, AuthError>;

#[derive(Debug, Error)]
pub enum AuthError {
    #[error("config: {0}")]
    Config(String),

    #[error("jwks fetch failed: {0}")]
    JwksFetch(String),

    #[error("jwks has no key matching kid={0:?}")]
    KidNotFound(Option<String>),

    #[error("invalid token: {0}")]
    InvalidToken(String),

    #[error("token expired")]
    Expired,

    #[error("missing required claim: {0}")]
    MissingClaim(&'static str),

    #[error("malformed claim `{0}`: {1}")]
    MalformedClaim(&'static str, String),

    #[error("authorization header missing or malformed")]
    MissingBearer,
}
