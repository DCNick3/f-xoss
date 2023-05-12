mod cli;
mod config;
mod locate_util;
mod mga;

use anyhow::{Context, Result};
use clap::Parser;

use tracing::info;
use tracing_indicatif::IndicatifLayer;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::Layer;

const DEFAULT_ENV_FILTER: &str = "info";
// const DEFAULT_ENV_FILTER: &str = "debug";

#[tokio::main]
async fn main() -> Result<()> {
    #[cfg(windows)]
    let _enabled = ansi_term::enable_ansi_support();

    let indicatif_layer = IndicatifLayer::new();

    tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(DEFAULT_ENV_FILTER))
        .with_subscriber(
            tracing_subscriber::registry()
                .with(
                    tracing_subscriber::fmt::layer()
                        .with_writer(indicatif_layer.get_stderr_writer()),
                )
                .with(indicatif_layer),
        )
        .init();

    let config = config::load_config().context("Failed to load the config")?;

    match config {
        None => info!(
            "No config file found at {}",
            config::config_path().display()
        ),
        Some(_) => info!(
            "Valid config file found at {}",
            config::config_path().display()
        ),
    }

    let cli = cli::Cli::parse();

    cli.run(config).await?;

    Ok(())
}
