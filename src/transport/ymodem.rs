use anyhow::{anyhow, Context, Result};
use async_stream::try_stream;
use bytes::Bytes;
use indicatif::ProgressStyle;
use std::time::Duration;
use thiserror::Error;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::time::timeout;
use tokio_stream::Stream;
use tracing::{debug_span, info_span, warn, Span};
use tracing_futures::Instrument;
use tracing_indicatif::span_ext::IndicatifSpanExt;

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
    pub size: u64,
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
                    size = u64::from_str_radix(s, 10).context("Invalid size")?;
                }

                Ok(())
            })
            .context("Parsing YModem header")?;

        Ok(Self { name, size })
    }
}

pub struct ReceivingFileInfo {
    pub name: String,
    pub size: u64,
}

const UART_TIMEOUT: Duration = Duration::from_secs(1);

pub async fn receive_file(
    io: &mut (impl AsyncRead + AsyncWrite + Unpin),
) -> Result<(ReceivingFileInfo, impl Stream<Item = Result<Bytes>> + '_)> {
    let mut buffer = [0u8; MAX_PACKET_SIZE];
    let mut seq = 0;

    let fut = async {
        io.write_all(b"C").await.context("Sending C")?;

        let header_packet = YModemPacket::read(io, &mut buffer)
            .await
            .context("Reading YModem header")?;
        let header = YModemHeader::parse(&header_packet).context("Parsing YModem header")?;

        if seq != header_packet.seq {
            Err(anyhow!("Invalid sequence number"))?;
        }
        io.write_all(&[ACK]).await.context("Sending ACK")?;
        io.write_all(b"C").await.context("Sending C")?;

        Ok::<_, anyhow::Error>(header)
    };
    let header = timeout(UART_TIMEOUT, fut)
        .await
        .context("Timed out initialing the transfer")??;

    let file_info = ReceivingFileInfo {
        name: header.name,
        size: header.size,
    };

    let mut len_left = header.size;

    Ok((
        file_info,
        try_stream! {
            let cur_span = Span::current();

            cur_span.pb_set_style(&ProgressStyle::default_bar());
            cur_span.pb_set_length(len_left);

            while len_left > 0 {
                seq = seq.wrapping_add(1);

                let fut = async {
                    let packet = YModemPacket::read(io, &mut buffer)
                        .instrument(debug_span!("read_ymodem_packet", seq = seq, len_left = len_left, "Reading YModem packet"))
                        .await
                        .context("Reading YModem packet")?;

                    if seq != packet.seq {
                        Err(anyhow!("Invalid sequence number"))?;
                    }

                    // tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

                    io.write_all(&[ACK]).await.context("Sending ACK")?;

                    let data_len = std::cmp::min(len_left, packet.data.len() as u64) as usize;
                    let data = Bytes::copy_from_slice(&packet.data[..data_len]);
                    cur_span.pb_inc(data_len as u64);
                    len_left -= data_len as u64;

                    Ok::<_, anyhow::Error>(data)
                };
                let data = timeout(UART_TIMEOUT, fut)
                    .await
                    .context("Timed out reading packet")??;

                yield data;
            }

            let fut = async {
                if io.read_u8().await.context("Reading EOT")? != EOT {
                    Err(anyhow!("Invalid EOT"))?;
                }
                io.write_all(&[NAK]).await.context("Sending ACK")?;
                if io.read_u8().await.context("Reading EOT")? != EOT {
                    Err(anyhow!("Invalid EOT"))?;
                }
                io.write_all(&[ACK]).await.context("Sending ACK")?;
                // make sure the last ACK gets written
                io.flush().await.context("Flushing")?;

                Ok::<_, anyhow::Error>(())
            };
            timeout(UART_TIMEOUT, fut)
                .await
                .context("Timed out reading EOT")??;
        }
        .instrument(info_span!("receive_file")),
    ))
}
