mod config;
mod locate_util;

use std::time::{Duration, SystemTime};

use anyhow::{Context, Result};

use f_xoss::model::{User, UserProfile};
use tracing::{info, info_span};
use tracing_futures::Instrument;
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
                        .with_writer(indicatif_layer.get_stdout_writer()),
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

    let device = locate_util::find_device_from_config(&config)
        .await
        .context("Failed to find the device")?;

    let res = async {
        info!("Device information: {:#?}", device.device_info().await);
        info!("Battery level: {:?}", device.battery_level().await);

        info!("Memory capacity: {}", device.get_memory_capacity().await?);
        info!("A-GPS status: {}", device.get_assisted_gnss_status().await?);

        device
            .set_time(SystemTime::now())
            .await
            .context("Failed to set the time")?;

        let header_json = device.get_device_json_header().await?;
        info!("Device JSON header: {:#?}", header_json);

        let settings = device.read_settings().await?;
        info!("Settings: {:#?}", settings);

        let user_profile = device.read_user_profile().await?;
        info!("User profile: {:#?}", user_profile);

        let user_profile = UserProfile {
            user: Some(User {
                platform: "XOSS".to_string(),
                uid: 42,
                user_name: "ABOBA".to_string(),
            }),
            user_profile: Default::default(),
        };
        device.write_user_profile(&user_profile).await?;

        let gear_profile = device.read_gear_profile().await?;
        info!("Gear profile: {:#?}", gear_profile);

        let workouts = device.read_workouts().await?;
        info!("Workouts: {:#?}", workouts);

        // we can't parse the panels (yet)
        // well, writing a good editor is a lot of effort anyway, so prolly not gonna do it soon
        let panels = device.read_file("panels.json").await?;
        info!("Panels: {:}", String::from_utf8(panels).unwrap());

        let routes = device.read_routes().await?;
        info!("Routes: {:#?}", routes);

        device
            .read_file(
                "20230508021939.fit", // "user_profile.json",
            )
            .await?;

        let offline_gnss_data = std::fs::read(
            // "mgaoffline.ubx",
            "2023-05-08.data",
        )
        .unwrap();
        device
            .write_file("offline.gnss", &offline_gnss_data)
            .await
            .context("Failed to send the offline GNSS data")?;

        Ok::<_, anyhow::Error>(())
    }
    .await;

    tokio::time::sleep(Duration::from_secs(1))
        .instrument(info_span!("final_sleep"))
        .await;

    device
        .disconnect()
        .await
        .context("Failed to disconnect from the device")?;
    res?;

    Ok(())
}
