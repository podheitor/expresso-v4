use thiserror::Error;

#[derive(Debug, Error)]
pub enum ExpressstorageError {
    #[error("internal error: {0}")]
    Internal(#[from] anyhow::Error),
}
