use btleplug::api::{
    BDAddr, Central, CentralEvent, CharPropFlags, Characteristic, Manager as _, Peripheral as _,
    ScanFilter, WriteType,
};
use btleplug::platform::{Adapter, Manager, Peripheral};
use std::collections::{BTreeMap, BTreeSet};
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use btleplug::api::bleuuid::BleUuid;
use futures_util::future::{join, AbortHandle, Abortable};
use futures_util::FutureExt;
use tokio::sync::mpsc::Receiver;
use tokio::sync::Mutex;
use tokio::{join, select};
use tokio_stream::{Stream, StreamExt};

use tracing::{info, trace, warn};
use uuid::Uuid;

const TX_CHARACTERISTIC_UUID: Uuid = Uuid::from_u128(0x6e400002_b5a3_f393_e0a9_e50e24dcca9e);
const RX_CHARACTERISTIC_UUID: Uuid = Uuid::from_u128(0x6e400003_b5a3_f393_e0a9_e50e24dcca9e);
const CTL_CHARACTERISTIC_UUID: Uuid = Uuid::from_u128(0x6e400004_b5a3_f393_e0a9_e50e24dcca9e);

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
            match event {
                CentralEvent::DeviceDiscovered(id) => {
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
                _ => {}
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

struct CtlChannel {
    shared: Arc<XossDeviceShared>,
    ctl_characteristic: Characteristic,
    ctl_recv: Receiver<Vec<u8>>,
}

struct UartChannel {
    shared: Arc<XossDeviceShared>,
    tx_characteristic: Characteristic,
    rx_characteristic: Characteristic,
    rx_recv: Receiver<Vec<u8>>,
}

struct XossDeviceShared {
    device: Peripheral,
    abort_handle: AbortHandle,
}

struct XossDevice {
    shared: Arc<XossDeviceShared>,
    ctl_channel: Mutex<CtlChannel>,
    uart_channel: Mutex<UartChannel>,
}

impl XossDevice {
    pub async fn new(device: Peripheral) -> Result<Self> {
        info!("Discovering services...");

        device
            .discover_services()
            .await
            .context("Failed to discover services")?;

        let mut tx_characteristic = None;
        let mut rx_characteristic = None;
        let mut ctl_characteristic = None;

        let mut required_characteristics = BTreeMap::from([
            (TX_CHARACTERISTIC_UUID, &mut tx_characteristic),
            (RX_CHARACTERISTIC_UUID, &mut rx_characteristic),
            (CTL_CHARACTERISTIC_UUID, &mut ctl_characteristic),
        ]);

        for characteristic in device.characteristics() {
            trace!(
                "Characteristic {}: {} {:?}",
                characteristic.service_uuid,
                characteristic.uuid,
                characteristic.properties
            );

            if let Some(c) = required_characteristics.get_mut(&characteristic.uuid) {
                **c = Some(characteristic);
            }
        }

        for (uuid, characteristic) in required_characteristics {
            if characteristic.is_none() {
                bail!("Missing characteristic: {}", uuid);
            }
        }

        // pump messages to their respective channels

        let (ctl_send, ctl_recv) = tokio::sync::mpsc::channel(3);
        let (rx_send, rx_recv) = tokio::sync::mpsc::channel(3);

        let mut events = device
            .notifications()
            .await
            .context("Failed to get notifications")?;

        let (abort_handle, registration) = AbortHandle::new_pair();
        tokio::spawn(Abortable::new(
            async move {
                while let Some(notification) = events.next().await {
                    let characteristic = notification.uuid;
                    if characteristic == RX_CHARACTERISTIC_UUID {
                        let data = notification.value;
                        trace!("RX: {}", hex::encode(&data));
                        // this can error out only if the recv side is closed. We have a different way to stop the loop (abort_token), so just ignore the error
                        let _ = rx_send.send(data).await;
                    } else if characteristic == CTL_CHARACTERISTIC_UUID {
                        let data = notification.value;
                        trace!("CTL: {}", hex::encode(&data));
                        // this can error out only if the recv side is closed. We have a different way to stop the loop (abort_token), so just ignore the error
                        let _ = ctl_send.send(data).await;
                    } else {
                        warn!("Unknown notification: {:?}", notification);
                    };
                }

                info!("Notifications stream ended");
            },
            registration,
        ));

        let ctl_characteristic = ctl_characteristic.unwrap();
        let tx_characteristic = tx_characteristic.unwrap();
        let rx_characteristic = rx_characteristic.unwrap();

        // make sure we are subscribed to the characteristics
        device
            .subscribe(&rx_characteristic)
            .await
            .context("Failed to subscribe to the RX characteristic")?;
        device
            .subscribe(&ctl_characteristic)
            .await
            .context("Failed to subscribe to the CTL characteristic")?;

        let shared = Arc::new(XossDeviceShared {
            device,
            abort_handle,
        });

        let result = Self {
            shared: shared.clone(),
            // mutexes are needed to ensure that we receive the correct reply
            // (we don't allow sending a new command until the previous one is replied to)
            ctl_channel: Mutex::new(CtlChannel {
                shared: shared.clone(),
                ctl_characteristic,
                ctl_recv,
            }),
            uart_channel: Mutex::new(UartChannel {
                shared: shared.clone(),
                tx_characteristic,
                rx_characteristic,
                rx_recv,
            }),
        };

        Ok(result)
    }
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

    let mut ctl_guard = device.ctl_channel.lock().await;
    ctl_guard
        .shared
        .device
        .write(
            &ctl_guard.ctl_characteristic,
            &[0xff, 0x00, 0xff],
            WriteType::WithResponse,
        )
        .await
        .context("Failed to write to the CTL characteristic")?;

    let reply = ctl_guard
        .ctl_recv
        .recv()
        .await
        .context("Failed to receive a reply")?;
    println!("Reply: {}", hex::encode(reply));

    Ok(())
}
