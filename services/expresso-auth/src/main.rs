//! Keycloak companion + gov.br OIDC adapter

use tracing::info;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .json()
        .init();

    info!(service = "expresso-auth", "starting");

    // TODO: initialize service
    
    Ok(())
}
