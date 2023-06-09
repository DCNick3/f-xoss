use std::pin::Pin;
use std::time::Duration;

use crate::config::XossUtilConfig;
use anyhow::{anyhow, bail, Context, Result};
use btleplug::api::{BDAddr, Central, CentralEvent, Manager as _, Peripheral as _, ScanFilter};
use btleplug::platform::{Adapter, Manager, Peripheral};
use f_xoss::device::XossDevice;
use tokio::select;
use tokio_stream::{Stream, StreamExt};
use tracing::{info, info_span, instrument, warn};
use tracing_futures::Instrument;

pub async fn find_adapter(manager: &Manager) -> Result<Adapter> {
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
async fn find_ble_peripheral(adapter: &Adapter, ble_addr: BDAddr) -> Result<Option<Peripheral>> {
    let events = adapter.events().await?;

    async fn find_inner(
        adapter: &Adapter,
        mut events: Pin<Box<dyn Stream<Item = CentralEvent> + Send>>,
        ble_addr: BDAddr,
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

                if address == ble_addr {
                    return Ok(Some(p));
                }
            }
        }

        warn!("The event stream ended before the device was found");

        Ok(None)
    }

    info!("Starting scan for {}", ble_addr);
    adapter
        .start_scan(ScanFilter::default())
        .await
        .context("Failed to start scan")?;

    let timeout = tokio::time::sleep(Duration::from_secs(10));
    let find = find_inner(adapter, events, ble_addr);

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

pub async fn find_device_from_config(config: &Option<XossUtilConfig>) -> Result<XossDevice> {
    // TODO: accept cli options allowing to specify the device from cli
    let Some(config) = config.as_ref() else {
        bail!("Cannot connect to device without a config")
    };

    let [device_info] = config.devices.as_slice() else {
        bail!("Only exactly one device in config is supported for now")
    };

    info!("Will try to connect to {}", device_info.identify());

    let ble_addr = device_info.address;

    let manager = Manager::new().await.context("Failed to create a manager")?;
    let adapter = find_adapter(&manager) // TODO: allow specifying adapter in config/cli
        .await
        .context("Failed to find adapter")?;

    const MAX_RECONNECTION_ATTEMPTS: usize = 3;
    for attempt in 0..=MAX_RECONNECTION_ATTEMPTS {
        let attempt_result = async {
            let peripheral = find_ble_peripheral(&adapter, ble_addr)
                .await
                .context("Failed to find device")?
                .ok_or_else(|| anyhow!("Device not found"))?;

            peripheral
                .connect()
                .instrument(info_span!("ble_connect"))
                .await
                .context("Failed to connect to device")?;

            XossDevice::new(peripheral)
                .await
                .context("Failed to initialize connection to a XOSS device")
        }
        .instrument(info_span!("connect_attempt", attempt = attempt + 1))
        .await;

        match attempt_result {
            Ok(device) => {
                info!("Connected to {}", device_info.identify());
                return Ok(device);
            }
            Err(e) => {
                if attempt == MAX_RECONNECTION_ATTEMPTS {
                    break;
                }
                warn!("Failed to connect to {}: {}", device_info.identify(), e);
                info!(
                    "Will retry in 5 seconds (attempt {}/{})",
                    attempt + 1,
                    MAX_RECONNECTION_ATTEMPTS
                );
                tokio::time::sleep(Duration::from_secs(5)).await;
            }
        }
    }

    bail!("Failed to connect to {}", device_info.identify())
}
