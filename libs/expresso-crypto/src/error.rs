use thiserror::Error;

#[derive(Debug, Error)]
pub enum ExpresscryptoError {
    #[error("internal error: {0}")]
    Internal(#[from] anyhow::Error),
}
