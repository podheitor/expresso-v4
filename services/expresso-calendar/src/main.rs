//! CalDAV server + meeting management

use tracing::info;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .json()
        .init();

    info!(service = "expresso-calendar", "starting");

    // TODO: initialize service
    
    Ok(())
}
