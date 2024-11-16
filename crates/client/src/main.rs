mod client;

use client::LightClient;
use common::initialize_logger_and_env;
use eyre::Result;

#[tokio::main]
async fn main() -> Result<()> {
    initialize_logger_and_env()?;

    tracing::info!("Starting Fossil Light Client...");

    let mut client = LightClient::new().await?;
    client.run().await
}