use thiserror::Error;

#[derive(Debug, Error)]
pub enum ExpressauthClientError {
    #[error("internal error: {0}")]
    Internal(#[from] anyhow::Error),
}
