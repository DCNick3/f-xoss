use std::collections::BTreeMap;
use std::sync::Arc;

use btleplug::api::{Central, Characteristic, Manager as _, Peripheral as _, WriteType};
use btleplug::platform::Peripheral;

use anyhow::{bail, Context, Result};
use futures_util::future::{AbortHandle, Abortable};
use tokio::sync::mpsc::Receiver;
use tokio::sync::Mutex;
use tokio_stream::StreamExt;

use tracing::{info, trace, warn};
use uuid::Uuid;

const TX_CHARACTERISTIC_UUID: Uuid = Uuid::from_u128(0x6e400002_b5a3_f393_e0a9_e50e24dcca9e);
const RX_CHARACTERISTIC_UUID: Uuid = Uuid::from_u128(0x6e400003_b5a3_f393_e0a9_e50e24dcca9e);
const CTL_CHARACTERISTIC_UUID: Uuid = Uuid::from_u128(0x6e400004_b5a3_f393_e0a9_e50e24dcca9e);

struct CtlChannel {
    shared: Arc<Shared>,
    ctl_characteristic: Characteristic,
    ctl_recv: Receiver<Vec<u8>>,
}

struct UartChannel {
    shared: Arc<Shared>,
    tx_characteristic: Characteristic,
    rx_characteristic: Characteristic,
    rx_recv: Receiver<Vec<u8>>,
}

struct Shared {
    device: Peripheral,
    #[allow(unused)] // yeah lol, it's used to keep the event pump task alive
    abort_handle: AbortHandle,
}

struct Inner {
    ctl_channel: CtlChannel,
    uart_channel: UartChannel,
}

pub struct XossDevice {
    shared: Arc<Shared>,
    inner: Mutex<Inner>,
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

        let shared = Arc::new(Shared {
            device,
            abort_handle,
        });

        let result = Self {
            shared: shared.clone(),
            // mutex is needed to ensure that we receive the correct reply
            // (we don't allow sending a new command until the previous one is replied to)
            inner: Mutex::new(Inner {
                ctl_channel: CtlChannel {
                    shared: shared.clone(),
                    ctl_characteristic,
                    ctl_recv,
                },
                uart_channel: UartChannel {
                    shared: shared.clone(),
                    tx_characteristic,
                    rx_characteristic,
                    rx_recv,
                },
            }),
        };

        Ok(result)
    }

    pub async fn send_ctl(&self, message: Vec<u8>) -> Result<Vec<u8>> {
        let mut inner = self.inner.lock().await;
        inner.ctl_channel.send_ctl(message).await
    }
}

impl CtlChannel {
    pub async fn send_ctl(&mut self, message: Vec<u8>) -> Result<Vec<u8>> {
        // TODO: we may have troubles handling failures after sending but before receiving the reply
        // maybe send the command reset if it happens?

        self.shared
            .device
            .write(&self.ctl_characteristic, &message, WriteType::WithResponse)
            .await
            .context("Failed to send CTL message")?;
        self.ctl_recv
            .recv()
            .await
            .context("Failed to receive CTL reply")
    }
}
