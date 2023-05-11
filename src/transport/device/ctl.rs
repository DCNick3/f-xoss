use crate::transport::ctl_message::RawControlMessage;
use crate::transport::device::Shared;
use anyhow::{bail, Context};
use btleplug::api::{Characteristic, Peripheral, WriteType};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc::Receiver;

pub const CTL_BUFFER_SIZE: usize = 20;
pub type CtlBuffer = [u8; CTL_BUFFER_SIZE];

pub struct CtlChannel {
    shared: Arc<Shared>,
    ctl_characteristic: Characteristic,
    ctl_recv: Receiver<Vec<u8>>,
}

impl CtlChannel {
    pub(super) fn new(
        shared: Arc<Shared>,
        ctl_characteristic: Characteristic,
        ctl_recv: Receiver<Vec<u8>>,
    ) -> Self {
        Self {
            shared,
            ctl_characteristic,
            ctl_recv,
        }
    }

    pub async fn send_ctl(
        &mut self,
        buffer: &mut CtlBuffer,
        message: RawControlMessage<'_>,
    ) -> anyhow::Result<()> {
        // TODO: we may have troubles handling failures after sending but before receiving the reply
        // maybe send the command reset if it happens?

        let message = message
            .write(buffer.as_mut())
            .context("Encoding the message")?;

        self.send_ctl_raw(&message)
            .await
            .context("Sending the message & receiving reply")?;

        Ok(())
    }

    pub async fn recv_ctl<'a>(
        &mut self,
        buffer: &'a mut CtlBuffer,
        timeout: Duration,
    ) -> anyhow::Result<RawControlMessage<'a>> {
        let reply = self.recv_ctl_raw(buffer, timeout).await?;
        let reply = RawControlMessage::read(reply).context("Decoding the control reply")?;
        Ok(reply)
    }

    async fn recv_ctl_raw<'a>(
        &mut self,
        buffer: &'a mut CtlBuffer,
        timeout: Duration,
    ) -> anyhow::Result<&'a [u8]> {
        let recv = self.ctl_recv.recv();
        let timeout = tokio::time::sleep(timeout);

        let recv = tokio::select! {
            msg = recv => msg.context("Failed to receive control reply"),
            _ = timeout => bail!("Timeout waiting for control reply"),
        }?;

        let reply = recv.as_slice();
        buffer[..reply.len()].copy_from_slice(reply);

        Ok(&buffer[..reply.len()])
    }

    async fn send_ctl_raw(&mut self, message: &[u8]) -> anyhow::Result<()> {
        if message.len() > 20 {
            bail!("Control message too long");
        }

        self.shared
            .device
            .write(&self.ctl_characteristic, message, WriteType::WithResponse)
            .await
            .context("Failed to send control message")?;

        Ok(())
    }
}
