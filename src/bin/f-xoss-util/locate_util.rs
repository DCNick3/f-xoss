use std::pin::Pin;
use std::time::Duration;

use anyhow::{Context, Result};
use btleplug::api::{BDAddr, Central, CentralEvent, Manager as _, Peripheral as _, ScanFilter};
use btleplug::platform::{Adapter, Manager, Peripheral};
use tokio::select;
use tokio_stream::{Stream, StreamExt};
use tracing::{info, instrument, warn};

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
pub async fn find_ble_device(adapter: &Adapter, mac: BDAddr) -> Result<Option<Peripheral>> {
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
