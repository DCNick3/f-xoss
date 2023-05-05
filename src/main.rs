mod ctl_message;
mod device;

use btleplug::api::{
    BDAddr, Central, CentralEvent, Manager as _, Peripheral as _, ScanFilter, WriteType,
};
use btleplug::platform::{Adapter, Manager, Peripheral};
use std::pin::Pin;
use std::time::Duration;

use anyhow::{Context, Result};
use tokio::select;
use tokio_stream::{Stream, StreamExt};

use crate::device::XossDevice;
use tracing::{info, warn};

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

struct ScanGuard<'a> {
    adapter: &'a Adapter,
}

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

    let timeout = tokio::time::sleep(Duration::from_secs(4));
    let find = find_inner(adapter, events, mac);

    let result = select! {
        _ = timeout => {
            warn!("Timeout while waiting for the device to be found");
            Ok(None)
        }
        result = find => result,
    };

    adapter.stop_scan().await.context("Failed to stop scan")?;

    Ok(result?)
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    let manager = Manager::new().await.context("Failed to create a manager")?;
    let adapter = find_adapter(&manager)
        .await
        .context("Failed to find adapter")?;

    let mac = BDAddr::from([0xD9, 0x29, 0xE4, 0x59, 0x55, 0x5C]);
    let device = find_device(&adapter, mac)
        .await
        .context("Failed to find device")?
        .context("Device not found")?;

    println!(
        "Device found: {:?}",
        device.properties().await?.unwrap().local_name
    );

    device
        .connect()
        .await
        .context("Failed to connect to the device")?;
    info!("Connected to the device");

    let device = XossDevice::new(device)
        .await
        .context("Failed to initialize the device")?;

    let reply = device
        .send_ctl(vec![0x00, 0x00, 0x00])
        .await
        .context("Failed to send a message")?;
    println!("Reply: {}", hex::encode(reply));

    Ok(())
}
