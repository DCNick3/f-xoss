mod ctl;
mod uart;

use super::ctl_message::RawControlMessage;
pub use ctl::{CtlBuffer, CTL_BUFFER_SIZE};
use uart::UartChannel;
pub use uart::UartStream;

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use btleplug::api::Peripheral as _;
use btleplug::platform::Peripheral;
use ctl::CtlChannel;
use futures_util::future::{AbortHandle, Abortable};
use tokio::sync::Mutex;
use tokio_stream::StreamExt;
use tracing::{debug, info, instrument, trace, warn, Level};
use uuid::Uuid;

const TX_CHARACTERISTIC_UUID: Uuid = Uuid::from_u128(0x6e400002_b5a3_f393_e0a9_e50e24dcca9e);
const RX_CHARACTERISTIC_UUID: Uuid = Uuid::from_u128(0x6e400003_b5a3_f393_e0a9_e50e24dcca9e);
const CTL_CHARACTERISTIC_UUID: Uuid = Uuid::from_u128(0x6e400004_b5a3_f393_e0a9_e50e24dcca9e);

struct Shared {
    device: Peripheral,
    #[allow(unused)] // yeah lol, it's used to keep the event pump task alive
    abort_handle: AbortHandle,
}

struct Inner {
    ctl_channel: CtlChannel,
    uart_channel: UartChannel,
}

pub struct XossTransport {
    shared: Arc<Shared>,
    inner: Mutex<Inner>,
}

const NORMAL_RESPONSE_TIMEOUT: Duration = Duration::from_secs(1);
const FILE_RESPONSE_TIMEOUT: Duration = Duration::from_secs(10);

impl XossTransport {
    #[instrument(skip(device), fields(id = %device.id()))]
    pub async fn new(device: Peripheral) -> Result<Self> {
        info!("Discovering XOSS services...");

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
            debug!(
                "BLE characteristic {}: {} {:?}",
                characteristic.service_uuid, characteristic.uuid, characteristic.properties
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
                ctl_channel: CtlChannel::new(shared.clone(), ctl_characteristic, ctl_recv),
                uart_channel: UartChannel::new(
                    shared,
                    tx_characteristic,
                    rx_characteristic,
                    rx_recv,
                ),
            }),
        };

        Ok(result)
    }

    #[instrument(skip(self, buffer), ret, level = Level::DEBUG)]
    pub async fn request_ctl<'a>(
        &self,
        buffer: &'a mut CtlBuffer,
        message: RawControlMessage<'_>,
    ) -> Result<RawControlMessage<'a>> {
        let mut inner = self.inner.lock().await;
        inner
            .ctl_channel
            .send_ctl(buffer, message)
            .await
            .context("Sending control message")?;

        inner
            .ctl_channel
            .recv_ctl(buffer, NORMAL_RESPONSE_TIMEOUT)
            .await
            .context("Reading control message")
    }

    #[instrument(skip(self, buffer), ret, level = Level::DEBUG)]
    pub async fn recv_ctl<'a>(&self, buffer: &'a mut CtlBuffer) -> Result<RawControlMessage<'a>> {
        let mut inner = self.inner.lock().await;
        inner
            .ctl_channel
            // This API is used to wait for device to process the file after the file transfer
            // it may take a while, hence the larger timeout
            .recv_ctl(buffer, FILE_RESPONSE_TIMEOUT)
            .await
            .context("Reading (isolated) control message")
    }

    pub async fn open_uart_stream(&self) -> UartStream {
        let inner = self.inner.lock().await;
        inner.uart_channel.open_stream().await
    }

    pub async fn disconnect(self) -> Result<()> {
        self.shared.device.disconnect().await?;

        Ok(())
    }
}
