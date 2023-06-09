mod ctl;
mod uart;

use super::ctl_message::RawControlMessage;
pub use ctl::{CtlBuffer, CTL_BUFFER_SIZE};
use uart::UartChannel;
pub use uart::UartStream;

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use btleplug::api::{Characteristic, Peripheral as _};
use btleplug::platform::Peripheral;
use ctl::CtlChannel;
use futures_util::future::{AbortHandle, Abortable};
use tokio::sync::Mutex;
use tokio_stream::StreamExt;
use tracing::{debug, info, instrument, trace, warn, Level};
use uuid::Uuid;
use crate::transport::ctl_message::ControlMessageType;

const TX_CHARACTERISTIC_UUID: Uuid = Uuid::from_u128(0x6e400002_b5a3_f393_e0a9_e50e24dcca9e);
const RX_CHARACTERISTIC_UUID: Uuid = Uuid::from_u128(0x6e400003_b5a3_f393_e0a9_e50e24dcca9e);
const CTL_CHARACTERISTIC_UUID: Uuid = Uuid::from_u128(0x6e400004_b5a3_f393_e0a9_e50e24dcca9e);

const FIRMWARE_REVISION_CHARACTERISTIC_UUID: Uuid =
    Uuid::from_u128(0x00002a26_0000_1000_8000_00805f9b34fb);
const MANUFACTURER_NAME_CHARACTERISTIC_UUID: Uuid =
    Uuid::from_u128(0x00002a29_0000_1000_8000_00805f9b34fb);
const MODEL_NUMBER_CHARACTERISTIC_UUID: Uuid =
    Uuid::from_u128(0x00002a24_0000_1000_8000_00805f9b34fb);
const HARDWARE_REVISION_CHARACTERISTIC_UUID: Uuid =
    Uuid::from_u128(0x00002a27_0000_1000_8000_00805f9b34fb);
const SERIAL_NUMBER_CHARACTERISTIC_UUID: Uuid =
    Uuid::from_u128(0x00002a25_0000_1000_8000_00805f9b34fb);

const BATTERY_LEVEL_CHARACTERISTIC_UUID: Uuid =
    Uuid::from_u128(0x00002a19_0000_1000_8000_00805f9b34fb);

struct Shared {
    device: Peripheral,
    device_information: DeviceInformation,
    battery_level: Arc<AtomicU32>,
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

#[derive(Debug, Clone)]
pub struct DeviceInformation {
    pub firmware_revision: String,
    pub manufacturer_name: String,
    pub model_number: String,
    pub hardware_revision: String,
    pub serial_number: String,
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

        let mut firmware_revision_characteristic = None;
        let mut manufacturer_name_characteristic = None;
        let mut model_number_characteristic = None;
        let mut hardware_revision_characteristic = None;
        let mut serial_number_characteristic = None;

        let mut battery_level_characteristic = None;

        let mut required_characteristics = BTreeMap::from([
            (TX_CHARACTERISTIC_UUID, &mut tx_characteristic),
            (RX_CHARACTERISTIC_UUID, &mut rx_characteristic),
            (CTL_CHARACTERISTIC_UUID, &mut ctl_characteristic),
            (
                FIRMWARE_REVISION_CHARACTERISTIC_UUID,
                &mut firmware_revision_characteristic,
            ),
            (
                MANUFACTURER_NAME_CHARACTERISTIC_UUID,
                &mut manufacturer_name_characteristic,
            ),
            (
                MODEL_NUMBER_CHARACTERISTIC_UUID,
                &mut model_number_characteristic,
            ),
            (
                HARDWARE_REVISION_CHARACTERISTIC_UUID,
                &mut hardware_revision_characteristic,
            ),
            (
                SERIAL_NUMBER_CHARACTERISTIC_UUID,
                &mut serial_number_characteristic,
            ),
            (
                BATTERY_LEVEL_CHARACTERISTIC_UUID,
                &mut battery_level_characteristic,
            ),
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
        let battery_level = Arc::new(AtomicU32::new(0));
        let battery_level_copy = battery_level.clone();

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
                    } else if characteristic == BATTERY_LEVEL_CHARACTERISTIC_UUID {
                        let data = notification.value;
                        assert_eq!(data.len(), 1);
                        let new_battery_level = data[0] as u32;
                        trace!("Battery level: {}", new_battery_level);
                        battery_level_copy.store(new_battery_level, Ordering::Relaxed);
                    }
                    // for some reason we are getting notifications for these, even though we are not subscribed to them
                    else if matches!(
                        characteristic,
                        FIRMWARE_REVISION_CHARACTERISTIC_UUID
                            | MANUFACTURER_NAME_CHARACTERISTIC_UUID
                            | MODEL_NUMBER_CHARACTERISTIC_UUID
                            | HARDWARE_REVISION_CHARACTERISTIC_UUID
                            | SERIAL_NUMBER_CHARACTERISTIC_UUID
                    ) {
                        debug!(
                            "Ignoring notification for characteristic: {}",
                            characteristic
                        )
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

        let firmware_revision_characteristic = firmware_revision_characteristic.unwrap();
        let manufacturer_name_characteristic = manufacturer_name_characteristic.unwrap();
        let model_number_characteristic = model_number_characteristic.unwrap();
        let hardware_revision_characteristic = hardware_revision_characteristic.unwrap();
        let serial_number_characteristic = serial_number_characteristic.unwrap();

        let battery_level_characteristic = battery_level_characteristic.unwrap();

        // make sure we are subscribed to the characteristics
        device
            .subscribe(&rx_characteristic)
            .await
            .context("Failed to subscribe to the RX characteristic")?;
        device
            .subscribe(&ctl_characteristic)
            .await
            .context("Failed to subscribe to the CTL characteristic")?;
        device
            .subscribe(&battery_level_characteristic)
            .await
            .context("Failed to subscribe to the battery level characteristic")?;

        async fn read_chara_string(
            device: &Peripheral,
            chara: &Characteristic,
            name: &str,
        ) -> Result<String> {
            device
                .read(&chara)
                .await
                .with_context(|| format!("Failed to read {} characteristic", name))
                .and_then(|s| {
                    String::from_utf8(s).with_context(|| format!("{} is not valid UTF-8", name))
                })
        }

        let device_information = DeviceInformation {
            firmware_revision: read_chara_string(
                &device,
                &firmware_revision_characteristic,
                "firmware revision",
            )
            .await?,
            manufacturer_name: read_chara_string(
                &device,
                &manufacturer_name_characteristic,
                "manufacturer name",
            )
            .await?,
            model_number: read_chara_string(&device, &model_number_characteristic, "model number")
                .await?,
            hardware_revision: read_chara_string(
                &device,
                &hardware_revision_characteristic,
                "hardware revision",
            )
            .await?,
            serial_number: read_chara_string(
                &device,
                &serial_number_characteristic,
                "serial number",
            )
            .await?,
        };

        battery_level.store(
            device
                .read(&battery_level_characteristic)
                .await
                .context("Failed to read battery level")?[0] as u32,
            Ordering::Relaxed,
        );

        let shared = Arc::new(Shared {
            device,
            device_information,
            battery_level,
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

    pub fn device_info(&self) -> &DeviceInformation {
        // TODO: maybe make it lazy-retrieve?
        &self.shared.device_information
    }

    pub fn battery_level(&self) -> u32 {
        self.shared.battery_level.load(Ordering::Relaxed)
    }

    #[instrument(skip(self, buffer), ret, level = Level::DEBUG)]
    pub async fn request_ctl<'a>(
        &self,
        buffer: &'a mut CtlBuffer,
        message_type: ControlMessageType,
        body: &[u8],
    ) -> Result<RawControlMessage<'a>> {
        let message = RawControlMessage {
            message_type,
            body,
        }; 
        
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
