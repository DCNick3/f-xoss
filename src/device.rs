//! This module provides high-level device communication functions. They try to be atomic and leave the device in a consistent state.

use crate::transport::{CtlBuffer, XossTransport};
use std::io::{Cursor, ErrorKind};

use crate::transport;
use crate::transport::ctl_message::{ControlMessageType, RawControlMessage};
use anyhow::{Context, Result};
use btleplug::platform::Peripheral;
use futures_util::{pin_mut, TryStreamExt};
use tokio::io::AsyncReadExt;
use tokio::sync::Mutex;
use tokio::time::Instant;
use tokio_util::io::StreamReader;
use tracing::{info, instrument};

pub struct XossDevice {
    // TODO: should we allow reconnecting? This might be a good place to do it
    // This would also necessitate BLE disconnect detection
    transport: Mutex<XossTransport>,
}

impl XossDevice {
    pub async fn new(peripheral: Peripheral) -> Result<Self> {
        Ok(Self {
            transport: Mutex::new(XossTransport::new(peripheral).await?),
        })
    }

    pub async fn disconnect(self) -> Result<()> {
        // TODO: how we handle disconnecting from the device is subject to change
        let transport = self.transport.into_inner();
        transport.disconnect().await
    }

    #[instrument(skip(self))]
    pub async fn receive_file(&self, filename: &str) -> Result<Vec<u8>> {
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
                RawControlMessage {
                    msg_type: ControlMessageType::RequestReturn,
                    body: filename.as_bytes(),
                },
            )
            .await
            .context("Failed to send a control message")?
            .expect_ok(ControlMessageType::Returning)?;
        assert_eq!(reply, filename.as_bytes());

        let (file_info, out_stream) = transport::ymodem::receive_file(&mut uart_stream).await?;
        let reader =
            StreamReader::new(out_stream.map_err(|e| std::io::Error::new(ErrorKind::Other, e)));
        pin_mut!(reader);

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

    #[instrument(skip(self, content))]
    pub async fn send_file(&self, filename: &str, content: &[u8]) -> Result<()> {
        // we accept the file as a slice, for motivation see the comment in [receive_file]
        let device = self.transport.lock().await;
        let mut uart_stream = device.open_uart_stream().await;

        let start = Instant::now();

        let mut buffer = CtlBuffer::default();
        let reply = device
            .request_ctl(
                &mut buffer,
                RawControlMessage {
                    msg_type: ControlMessageType::RequestSend,
                    body: filename.as_bytes(),
                },
            )
            .await
            .context("Failed to send a control message")?
            .expect_ok(ControlMessageType::Accept)?;
        assert_eq!(reply, filename.as_bytes());

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
}
