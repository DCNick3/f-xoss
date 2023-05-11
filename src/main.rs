mod device;
mod model;
mod transport;

use std::pin::Pin;
use std::time::{Duration, SystemTime};

use anyhow::{Context, Result};
use btleplug::api::{BDAddr, Central, CentralEvent, Manager as _, Peripheral as _, ScanFilter};
use btleplug::platform::{Adapter, Manager, Peripheral};
use tokio::select;
use tokio_stream::{Stream, StreamExt};

use crate::device::XossDevice;
use crate::model::{User, UserProfile};
use tracing::{info, info_span, instrument, warn};
use tracing_futures::Instrument;
use tracing_indicatif::IndicatifLayer;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::Layer;

async fn find_adapter(manager: &Manager) -> Result<Adapter> {
    let adapter_list = manager.adapters().await.context("Listing adapters")?;
    let adapter_count = adapter_list.len();

    let result = adapter_list
        .into_iter()
        .next()
        .context("No Bluetooth adapters found")?;

    if adapter_count > 1 {
        let info = result
            .adapter_info()
            .await
            .context("Failed to get adapter info")?;

        warn!(
            "More than one Bluetooth adapter found, using the first one: {}",
            info
        );
    }

    Ok(result)
}

#[instrument(skip(adapter))]
async fn find_device(adapter: &Adapter, mac: BDAddr) -> Result<Option<Peripheral>> {
    let events = adapter.events().await?;

    async fn find_inner(
        adapter: &Adapter,
        mut events: Pin<Box<dyn Stream<Item = CentralEvent> + Send>>,
        mac: BDAddr,
    ) -> Result<Option<Peripheral>> {
        while let Some(event) = events.next().await {
            if let CentralEvent::DeviceDiscovered(id) = event {
                let p = adapter
                    .peripheral(&id)
                    .await
                    .context("Failed to get the discovered peripheral")?;

                let address = p
                    .properties()
                    .await
                    .context("Failed to get peripheral properties")?
                    .context("No peripheral properties")?
                    .address;

                if address == mac {
                    return Ok(Some(p));
                }
            }
        }

        warn!("The event stream ended before the device was found");

        Ok(None)
    }

    info!("Starting scan for {}", mac);
    adapter
        .start_scan(ScanFilter::default())
        .await
        .context("Failed to start scan")?;

    let timeout = tokio::time::sleep(Duration::from_secs(10));
    let find = find_inner(adapter, events, mac);

    let result = select! {
        _ = timeout => {
            warn!("Timeout while waiting for the device to be found");
            Ok(None)
        }
        result = find => result,
    };

    adapter.stop_scan().await.context("Failed to stop scan")?;

    result
}

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

    let manager = Manager::new().await.context("Failed to create a manager")?;
    let adapter = find_adapter(&manager)
        .await
        .context("Failed to find adapter")?;

    let mac = BDAddr::from([0xD9, 0x29, 0xE4, 0x59, 0x55, 0x5C]);
    let device = find_device(&adapter, mac)
        .await
        .context("Failed to find device")?
        .context("Device not found")?;

    info!(
        "Device found: {:?}",
        device.properties().await?.unwrap().local_name
    );

    device
        .connect()
        .instrument(info_span!("connect"))
        .await
        .context("Failed to connect to the device")?;
    info!("Connected to the device");

    let device = XossDevice::new(device)
        .await
        .context("Failed to initialize the device")?;

    let res = async {
        info!("Device information: {:#?}", device.device_info().await);
        info!("Battery level: {:?}", device.battery_level().await);

        info!("Memory capacity: {}", device.get_memory_capacity().await?);
        info!("A-GPS status: {}", device.get_assisted_gnss_status().await?);

        device
            .set_time(SystemTime::now())
            .await
            .context("Failed to set the time")?;

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

        let workouts = device.read_workouts().await?;
        info!("Workouts: {:#?}", workouts);

        device
            .receive_file(
                "20230508021939.fit", // "user_profile.json",
            )
            .await?;

        let offline_gnss_data = std::fs::read(
            // "mgaoffline.ubx",
            "2023-05-08.data",
        )
        .unwrap();
        device
            .send_file("offline.gnss", &offline_gnss_data)
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
