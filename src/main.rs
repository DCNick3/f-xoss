mod transport;

use std::io::ErrorKind;
use std::pin::Pin;
use std::time::Duration;

use anyhow::{Context, Result};
use btleplug::api::{BDAddr, Central, CentralEvent, Manager as _, Peripheral as _, ScanFilter};
use btleplug::platform::{Adapter, Manager, Peripheral};
use futures_util::{pin_mut, TryStreamExt};
use tokio::io::AsyncReadExt;
use tokio::select;
use tokio::time::Instant;
use tokio_stream::{Stream, StreamExt};
use tokio_util::io::StreamReader;

use crate::transport::ctl_message::{ControlMessageType, RawControlMessage};
use crate::transport::device::XossTransport;
use tracing::{info, info_span, instrument, warn, Span};
use tracing_futures::Instrument;
use tracing_indicatif::span_ext::IndicatifSpanExt;
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

#[instrument(skip(device))]
async fn receive_file(device: &XossTransport, filename: &str) -> Result<()> {
    Span::current().pb_set_message(&format!("Downloading {} from the device", filename));

    let mut uart_stream = device.open_uart_stream().await;

    let reply = device
        .send_ctl(RawControlMessage {
            msg_type: ControlMessageType::RequestReturn,
            // body: (*b"workouts.json").into(),
            body: filename.into(),
        })
        .await
        .context("Failed to send a control message")?;
    assert_eq!(reply.msg_type, ControlMessageType::Returning);

    let (file_info, out_stream) = transport::ymodem::receive_file(&mut uart_stream).await?;
    let reader =
        StreamReader::new(out_stream.map_err(|e| std::io::Error::new(ErrorKind::Other, e)));
    pin_mut!(reader);

    info!(
        "Downloading {} ({})",
        filename,
        humansize::format_size(file_info.size, humansize::BINARY.decimal_zeroes(2))
    );

    let start = Instant::now();

    let mut buf = Vec::new();
    reader
        .read_to_end(&mut buf)
        .await
        .context("Failed to read the file")?;
    drop(reader);

    let time = start.elapsed();

    let speed = (buf.len() as f64) / (time.as_secs_f64()) / 1024.0;

    info!(
        "Downloaded {} ({}) in {:.2} seconds ({:.2} KiB/s)",
        filename,
        humansize::format_size(buf.len(), humansize::BINARY.decimal_zeroes(2)),
        time.as_secs_f64(),
        speed
    );

    // println!("File received: {}", hex::encode(&buf));
    // println!("Speed: {:.2} KiB/s", speed);
    // println!("File received: {}", String::from_utf8(buf).unwrap());

    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    #[cfg(windows)]
    let _enabled = ansi_term::enable_ansi_support();

    let indicatif_layer = IndicatifLayer::new();

    tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("debug"))
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

    let device = XossTransport::new(device)
        .await
        .context("Failed to initialize the device")?;

    let res = receive_file(&device, "20230508021939.fit").await;

    tokio::time::sleep(Duration::from_secs(10))
        .instrument(info_span!("final_sleep"))
        .await;

    device
        .disconnect()
        .await
        .context("Failed to disconnect from the device")?;
    res?;

    Ok(())
}
