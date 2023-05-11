//! This module provides high-level device communication functions. They try to be atomic and leave the device in a consistent state.

use crate::transport::XossTransport;

use anyhow::Result;
use btleplug::platform::Peripheral;
use tokio::sync::Mutex;

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
}
