use thiserror::Error;

#[derive(Debug, Error)]
pub enum ExpressmailParserError {
    #[error("internal error: {0}")]
    Internal(#[from] anyhow::Error),
}
