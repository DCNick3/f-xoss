use anyhow::{bail, Context, Result};
use thiserror::Error;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tracing::warn;

#[derive(Error, Debug)]
pub enum Error {
    #[error("Invalid start byte")]
    InvalidStart,
    #[error("Invalid length")]
    InvalidLength,
    #[error("Invalid sequence number")]
    InvalidSeq,
    #[error("Invalid CRC")]
    InvalidCrc,
}

const SOH: u8 = 0x01;
const STX: u8 = 0x02;
const EOT: u8 = 0x04;
const ACK: u8 = 0x06;
const NAK: u8 = 0x15;
const CAN: u8 = 0x18;

pub const MAX_PACKET_SIZE: usize = 1024 + 5;

#[derive(Debug)]
pub struct YModemPacket<'a> {
    seq: u8,
    data: &'a [u8],
}

impl<'a> YModemPacket<'a> {
    #[inline]
    fn data_len(start_byte: u8) -> Result<usize, Error> {
        match start_byte {
            SOH => Ok(128),
            STX => Ok(1024),
            _ => Err(Error::InvalidStart),
        }
    }

    pub fn parse(raw: &'a [u8]) -> Result<Self, Error> {
        if raw.len() < 2 {
            return Err(Error::InvalidLength);
        }

        let data_len = Self::data_len(raw[0])?;

        if raw.len() != data_len + 5 {
            return Err(Error::InvalidLength);
        }

        let seq = raw[1];
        let seq_inv = raw[2];

        if seq != seq_inv ^ 0xff {
            return Err(Error::InvalidSeq);
        }

        let data = &raw[3..raw.len() - 2];

        let crc = (raw[raw.len() - 2] as u16) << 8 | raw[raw.len() - 1] as u16;
        // for some __GODFORSAKEN__ reason Xoss uses CRC-16/ARC instead of CRC-16/XMODEM
        let crc_calc = crc16::State::<crc16::ARC>::calculate(data);

        if crc != crc_calc {
            warn!("Invalid CRC: {:04x} != {:04x}", crc, crc_calc);
            return Err(Error::InvalidCrc);
        }

        Ok(Self { seq, data })
    }

    pub async fn read(
        reader: &mut (impl AsyncRead + Unpin),
        buffer: &'a mut [u8; MAX_PACKET_SIZE],
    ) -> Result<YModemPacket<'a>> {
        reader.read_exact(&mut buffer[..1]).await?;
        let start = buffer[0];
        let data_len = Self::data_len(start)?;

        reader.read_exact(&mut buffer[1..data_len + 5]).await?;

        Self::parse(&buffer[..data_len + 5]).map_err(|e| e.into())
    }
}

#[derive(Debug)]
pub struct YModemHeader {
    pub name: String,
    pub size: usize,
}

impl YModemHeader {
    pub fn parse(packet: &YModemPacket) -> Result<Self> {
        let mut name = String::new();
        let mut size = 0;

        let mut data = packet.data;

        while let Some(s_data) = data.strip_suffix(b"\0") {
            data = s_data;
        }

        data.split(|&v| v == 0 || v == b' ')
            .filter(|s| !s.is_empty())
            .try_for_each(|s| -> anyhow::Result<()> {
                let s = std::str::from_utf8(s).context("Invalid UTF-8")?;

                if name.is_empty() {
                    name = s.to_string();
                } else {
                    size = usize::from_str_radix(s, 10).context("Invalid size")?;
                }

                Ok(())
            })
            .context("Parsing YModem header")?;

        Ok(Self { name, size })
    }
}

pub async fn receive_file(
    io: &mut (impl AsyncRead + AsyncWrite + Unpin),
    out: &mut (impl AsyncWrite + Unpin),
) -> Result<()> {
    io.write_all(b"C").await.context("Sending C")?;

    let mut buffer = [0u8; MAX_PACKET_SIZE];
    let mut seq = 0;

    let header_packet = YModemPacket::read(io, &mut buffer)
        .await
        .context("Reading YModem header")?;
    let header = YModemHeader::parse(&header_packet).context("Parsing YModem header")?;

    if seq != header_packet.seq {
        bail!("Invalid sequence number");
    }
    io.write_all(&[ACK]).await.context("Sending ACK")?;
    io.write_all(b"C").await.context("Sending C")?;

    let mut len_left = header.size;

    while len_left > 0 {
        seq = seq.wrapping_add(1);

        let packet = YModemPacket::read(io, &mut buffer)
            .await
            .context("Reading YModem packet")?;

        if seq != packet.seq {
            bail!("Invalid sequence number");
        }
        io.write_all(&[ACK]).await.context("Sending ACK")?;

        let data_len = std::cmp::min(len_left, packet.data.len());
        let data = &packet.data[..data_len];
        len_left -= data_len;

        out.write_all(data).await.context("Writing YModem packet")?;
    }

    if io.read_u8().await.context("Reading EOT")? != EOT {
        bail!("Invalid EOT");
    }
    io.write_all(&[NAK]).await.context("Sending ACK")?;
    if io.read_u8().await.context("Reading EOT")? != EOT {
        bail!("Invalid EOT");
    }
    io.write_all(&[ACK]).await.context("Sending ACK")?;
    io.flush().await.context("Flushing")?;

    Ok(())
}
