use thiserror::Error;

#[derive(Debug, Error)]
pub enum ExpresscoreError {
    #[error("internal error: {0}")]
    Internal(#[from] anyhow::Error),
}
