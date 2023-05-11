//! This module provides high-level device communication functions. They try to be atomic and leave the device in a consistent state.

use crate::transport::{CtlBuffer, XossTransport, CTL_BUFFER_SIZE};
use std::fmt::{Debug, Display};
use std::io::{Cursor, ErrorKind};
use std::time::SystemTime;

use crate::model::{Gear, HeaderJson, Route, Settings, UserProfile, WithHeader, WorkoutsItem};
use crate::transport;
use crate::transport::ctl_message::ControlMessageType;
use anyhow::{Context, Result};
use btleplug::platform::Peripheral;
use chrono::{NaiveDate, NaiveDateTime};
use futures_util::{pin_mut, TryStreamExt};
use serde::{Deserialize, Serialize};
use tokio::io::AsyncReadExt;
use tokio::sync::{Mutex, OnceCell};
use tokio::time::Instant;
use tokio_util::io::StreamReader;
use tracing::{info, instrument, trace, warn, Span};

pub struct XossDevice {
    // TODO: should we allow reconnecting? This might be a good place to do it
    // This would also necessitate BLE disconnect detection
    transport: Mutex<XossTransport>,
    json_header: OnceCell<HeaderJson>,
}

#[derive(Debug, Clone)]
pub struct MemoryCapacity {
    pub free_kb: u32,
    pub total_kb: u32,
}

impl Display for MemoryCapacity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} / {} ({:.02}% used)",
            humansize::format_size(self.free_kb as u64 * 1024, humansize::BINARY),
            humansize::format_size(self.total_kb as u64 * 1024, humansize::BINARY),
            (self.total_kb - self.free_kb) as f32 / self.total_kb as f32 * 100.0
        )
    }
}

#[derive(Debug, Copy, Clone)]
pub enum AssistedGnssState {
    MissingData,
    ValidUntil(NaiveDate),
}

impl Display for AssistedGnssState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AssistedGnssState::MissingData => write!(f, "A-GNSS data missing"),
            AssistedGnssState::ValidUntil(date) => write!(f, "Valid until {}", date),
        }
    }
}

impl XossDevice {
    pub async fn new(peripheral: Peripheral) -> Result<Self> {
        let transport = XossTransport::new(peripheral).await?;

        let mut buffer = [0; CTL_BUFFER_SIZE];
        if transport
            .request_ctl(&mut buffer, ControlMessageType::StatusReturn, &[])
            .await
            .context("Getting transfer status")?
            .message_type
            != ControlMessageType::Idle
        {
            info!("Device has an active transfer, stopping it");
            transport
                .request_ctl(&mut buffer, ControlMessageType::RequestStop, &[])
                .await
                .context("Stopping the transfer")?
                .expect_ok(ControlMessageType::Idle)
                .context("Failed to stop the transfer")?;
        }

        Ok(Self {
            transport: Mutex::new(transport),
            json_header: OnceCell::new(),
        })
    }

    pub async fn disconnect(self) -> Result<()> {
        // TODO: how we handle disconnecting from the device is subject to change
        let transport = self.transport.into_inner();
        transport.disconnect().await
    }

    pub async fn device_info(&self) -> transport::DeviceInformation {
        let transport = self.transport.lock().await;
        transport.device_info().clone()
    }

    pub async fn battery_level(&self) -> u32 {
        let transport = self.transport.lock().await;
        transport.battery_level()
    }

    pub async fn get_memory_capacity(&self) -> Result<MemoryCapacity> {
        let transport = self.transport.lock().await;
        let mut buffer = [0; CTL_BUFFER_SIZE];
        transport
            .request_ctl(&mut buffer, ControlMessageType::RequestCap, &[])
            .await
            .context("Failed to send a control message")?
            .expect_ok(ControlMessageType::ReturnCap)
            .context("Failed to get memory capacity")
            .and_then(|b| {
                std::str::from_utf8(b).context("Failed to parse the capacity string as UTF-8")
            })
            .and_then(|s| {
                let (left, right) = s
                    .split_once('/')
                    .context("Failed to parse the capacity string")?;
                let free_kb = left
                    .parse::<u32>()
                    .context("Failed to parse the free capacity")?;
                let total_kb = right
                    .parse::<u32>()
                    .context("Failed to parse the total capacity")?;
                Ok(MemoryCapacity { free_kb, total_kb })
            })
    }

    /// Delete a file from the device
    ///
    /// Don't try to remove the JSON files, the device will not recreate some of them
    #[allow(unused)]
    pub async fn delete_file(&self, filename: &str) -> Result<()> {
        let transport = self.transport.lock().await;
        let mut buffer = [0; CTL_BUFFER_SIZE];
        transport
            .request_ctl(
                &mut buffer,
                ControlMessageType::RequestDel,
                filename.as_bytes(),
            )
            .await
            .context("Failed to send a control message")?
            .expect_ok(ControlMessageType::DelSuccess)
            .context("Failed to delete the file")
            .map(|b| {
                assert_eq!(b, filename.as_bytes());
            })
    }

    pub async fn set_time(&self, time: SystemTime) -> Result<()> {
        let unix_time: u32 = time
            .duration_since(SystemTime::UNIX_EPOCH)
            .context("Failed to convert the time to UNIX timestamp")?
            .as_secs()
            .try_into()
            .expect("It's that time of the year again... (the unix timestamp has overflowed unsigned 32-bit integer)");

        let transport = self.transport.lock().await;
        let mut buffer = [0; CTL_BUFFER_SIZE];
        transport
            .request_ctl(
                &mut buffer,
                ControlMessageType::TimeSet,
                unix_time.to_le_bytes().as_ref(),
            )
            .await
            .context("Failed to send a control message")?
            .expect_ok(ControlMessageType::TimeSetRtn)
            .context("Failed to set the time")
            .map(|b| {
                assert_eq!(b, unix_time.to_le_bytes().as_ref());
            })
    }

    pub async fn get_assisted_gnss_status(&self) -> Result<AssistedGnssState> {
        let transport = self.transport.lock().await;
        let mut buffer = [0; CTL_BUFFER_SIZE];
        transport
            .request_ctl(&mut buffer, ControlMessageType::RequestMga, &[])
            .await
            .context("Failed to send a control message")?
            .expect_ok(ControlMessageType::ReturnMga)
            .context("Failed to get the assisted GPS status")
            .map(|b| {
                assert_eq!(b.len(), 6);
                assert_eq!(b[0], 0x01);
                assert_eq!(b[1], 0x00);
                let time = u32::from_le_bytes([b[2], b[3], b[4], b[5]]);
                if time == 0 {
                    AssistedGnssState::MissingData
                } else {
                    // convert unix time to NaiveDate
                    AssistedGnssState::ValidUntil(
                        NaiveDateTime::from_timestamp_opt(time as i64, 0)
                            .unwrap()
                            .date(),
                    )
                }
            })
    }

    #[instrument(skip(self), fields(size))]
    pub async fn read_file(&self, filename: &str) -> Result<Vec<u8>> {
        // even though the underlying implementation of ymodem returns a stream, allowing us to stream the file, we don't do that here
        // it introduces problems with atomicity and will punch us in the face when we try to implement retries
        // the files are small enough that we can just read them into memory
        let transport = self.transport.lock().await;
        let mut uart_stream = transport.open_uart_stream().await;

        let start = Instant::now();

        let mut buffer = CtlBuffer::default();
        let reply = transport
            .request_ctl(
                &mut buffer,
                ControlMessageType::RequestReturn,
                filename.as_bytes(),
            )
            .await
            .context("Failed to send a control message")?
            .expect_ok(ControlMessageType::Returning)?;
        assert_eq!(reply, filename.as_bytes());

        let (file_info, out_stream) = transport::ymodem::receive_file(&mut uart_stream).await?;
        let reader =
            StreamReader::new(out_stream.map_err(|e| std::io::Error::new(ErrorKind::Other, e)));
        pin_mut!(reader);

        Span::current().record("size", file_info.size);

        info!(
            "Downloading {} ({})",
            filename,
            humansize::format_size(file_info.size, humansize::BINARY.decimal_zeroes(2))
        );

        let mut buf = Vec::new();
        reader
            .read_to_end(&mut buf)
            .await
            .context("Failed to read the file")?;
        drop(reader);

        transport
            .recv_ctl(&mut buffer)
            .await
            .context("Receiving the post-download status message")?
            .expect_ok(ControlMessageType::Idle)?;

        let time = start.elapsed();

        let speed = (buf.len() as f64) / (time.as_secs_f64()) / 1024.0;

        info!(
            "Downloaded {} ({}) in {:.2} seconds ({:.2} KiB/s)",
            filename,
            humansize::format_size(buf.len(), humansize::BINARY.decimal_zeroes(2)),
            time.as_secs_f64(),
            speed
        );

        Ok(buf)
    }

    #[instrument(skip(self, content), fields(size = content.len()))]
    pub async fn write_file(&self, filename: &str, content: &[u8]) -> Result<()> {
        // we accept the file as a slice, for motivation see the comment in [receive_file]
        let device = self.transport.lock().await;
        let mut uart_stream = device.open_uart_stream().await;

        let start = Instant::now();

        let mut buffer = CtlBuffer::default();
        let reply = device
            .request_ctl(
                &mut buffer,
                ControlMessageType::RequestSend,
                filename.as_bytes(),
            )
            .await
            .context("Failed to send a control message")?
            .expect_ok(ControlMessageType::Accept)?;
        assert_eq!(reply, filename.as_bytes());

        info!(
            "Uploading {} ({})",
            filename,
            humansize::format_size(content.len(), humansize::BINARY.decimal_zeroes(2))
        );

        transport::ymodem::send_file(&mut uart_stream, filename, &mut Cursor::new(content)).await?;

        let time = start.elapsed();

        let start = Instant::now();

        device
            .recv_ctl(&mut buffer)
            .await
            .context("Receiving the post-download status message")?
            .expect_ok(ControlMessageType::Idle)?;

        let device_proc_time = start.elapsed();

        let speed = (content.len() as f64) / (time.as_secs_f64()) / 1024.0;

        info!(
            "Uploaded {} ({}) in {:.2} seconds ({:.2} KiB/s). Device processed it in {:.2} seconds",
            filename,
            humansize::format_size(content.len(), humansize::BINARY.decimal_zeroes(2)),
            time.as_secs_f64(),
            speed,
            device_proc_time.as_secs_f64()
        );

        Ok(())
    }

    pub async fn get_device_json_header(&self) -> Result<HeaderJson> {
        Ok(match self.json_header.get() {
            Some(h) => h.clone(),
            None => {
                self.read_user_profile().await?;
                self.json_header
                    .get()
                    .expect("We have read the user profile but it did not initialize the header??")
                    .clone()
            }
        })
    }

    #[instrument(skip(self))]
    pub async fn read_json_file<T: for<'de> Deserialize<'de>>(&self, filename: &str) -> Result<T> {
        {
            let data = self.read_file(filename).await?;
            let data =
                std::str::from_utf8(&data).context("Failed to parse a json file as UTF-8")?;

            trace!("Retrieved {}: {}", filename, data);

            let WithHeader { header, data } =
                serde_json::from_str(data).context("Failed to parse the json file")?;

            if header.version != "2.0.0" {
                warn!(
                    "The json file {} has an unknown version {}",
                    filename, header.version
                )
            }

            self.json_header.get_or_init(|| async move { header }).await;

            Ok::<_, anyhow::Error>(data)
        }
        .with_context(|| format!("Failed to read {}", filename))
    }

    #[instrument(skip(self, data))]
    pub async fn write_json_file<T: Serialize>(&self, filename: &str, data: &T) -> Result<()> {
        let header_json = self.get_device_json_header().await?;

        let data = WithHeader {
            // we should provide the header, as the device doesn't always re-generate it
            // and it may confuse other software trying to read the JSON files
            header: header_json,
            data,
        };

        let data = serde_json::to_string(&data).context("Failed to serialize the json file")?;

        trace!("Writing {}: {}", filename, data);

        self.write_file(filename, data.as_bytes()).await?;

        Ok(())
    }

    pub async fn read_user_profile(&self) -> Result<UserProfile> {
        self.read_json_file("user_profile.json")
            .await
            .context("Failed to read user profile")
    }

    pub async fn write_user_profile(&self, profile: &UserProfile) -> Result<()> {
        self.write_json_file("user_profile.json", profile)
            .await
            .context("Failed to write user profile")
    }

    pub async fn read_workouts(&self) -> Result<Vec<WorkoutsItem>> {
        #[derive(Deserialize)]
        struct WorkoutsWrap {
            pub workouts: Vec<WorkoutsItem>,
        }

        self.read_json_file("workouts.json")
            .await
            .context("Failed to read workouts")
            .map(|w: WorkoutsWrap| w.workouts)
    }

    pub async fn read_settings(&self) -> Result<Settings> {
        #[derive(Deserialize)]
        struct SettingsWrap {
            pub settings: Settings,
        }

        self.read_json_file("settings.json")
            .await
            .context("Failed to read settings")
            .map(|s: SettingsWrap| s.settings)
    }

    pub async fn write_settings(&self, settings: &Settings) -> Result<()> {
        #[derive(Serialize)]
        struct SettingsWrap<'a> {
            pub settings: &'a Settings,
        }

        self.write_json_file("settings.json", &SettingsWrap { settings })
            .await
            .context("Failed to write settings")
    }

    pub async fn read_gear_profile(&self) -> Result<Vec<Gear>> {
        #[derive(Deserialize)]
        struct GearProfileWrap {
            pub gears: Vec<Gear>,
        }

        self.read_json_file("gear_profile.json")
            .await
            .context("Failed to read gear profile")
            .map(|g: GearProfileWrap| g.gears)
    }

    pub async fn write_gear_profile(&self, gears: &[Gear]) -> Result<()> {
        #[derive(Serialize)]
        struct GearProfileWrap<'a> {
            pub gears: &'a [Gear],
        }

        self.write_json_file("gear_profile.json", &GearProfileWrap { gears })
            .await
            .context("Failed to write gear profile")
    }

    pub async fn read_routes(&self) -> Result<Vec<Route>> {
        #[derive(Deserialize)]
        struct RoutesWrap {
            pub routes: Vec<Route>,
        }

        self.read_json_file("routebooks.json")
            .await
            .context("Failed to read routes")
            .map(|r: RoutesWrap| r.routes)
    }
}
