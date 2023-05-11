use anyhow::{bail, Context, Result};
use num_enum::TryFromPrimitive;

#[derive(TryFromPrimitive, Debug, PartialEq, Eq, Clone, Copy)]
#[repr(u8)]
pub enum ControlMessageType {
    /// Returns a device identifier (8 hex bytes
    DbgCmd = 0x0,
    /// Interrupts the current file transfer when sent to the device, or indicates that the device is idle when sent to the host
    Idle = 0x4,

    /// Request a file from the device (starting a file transfer)
    ///
    /// The file itself is sent using YMODEM protocol outside of the control channel.
    RequestReturn = 0x5,
    /// Successful response to [ControlMessageType::RequestReturn]
    Returning = 0x6,

    /// Send a file to the device (starting a file transfer)
    ///
    /// The file itself is sent using YMODEM protocol outside of the control channel.
    RequestSend = 0x7,
    /// Successful response to [ControlMessageType::RequestSend]
    Accept = 0x8,

    /// Get free space on the device
    RequestCap = 0x9,
    /// Successful response to [ControlMessageType::RequestCap]
    ReturnCap = 0xA,

    /// Delete a file
    RequestDel = 0xD,
    /// Successful response to [ControlMessageType::RequestDel]
    DelSuccess = 0xE,

    /// Always returns [ControlMessageType::ErrVali]
    RequestDetail = 0xF,
    /// Request to stop the current file transfer
    RequestStop = 0x1F,

    ErrVali = 0x11,
    ErrNoFile = 0x12,
    ErrMemory = 0x13,
    ErrStatus = 0x14,
    ErrDecode = 0x15,

    /// Set time
    TimeSet = 0x54,
    /// Successful response to [ControlMessageType::TimeSet]
    TimeSetRtn = 0x55,

    RequestMga = 0x77,
    ReturnMga = 0x78,

    StatusAct = 0xAC,

    /// Perform factory reset
    RequestClr = 0xCC,
    /// Successful response to [ControlMessageType::RequestClr]
    ReturnClr = 0xCD,

    /// Reboot device to DFU mode
    DfuEnter = 0xDF,

    /// Get transfer status
    StatusReturn = 0xFF,
}

#[derive(Debug)]
pub struct RawControlMessage<'a> {
    pub msg_type: ControlMessageType,
    pub body: &'a [u8],
}

fn calc_checksum(buf: &[u8]) -> u8 {
    buf.iter().fold(0, |acc, x| acc ^ x)
}

impl<'a> RawControlMessage<'a> {
    pub fn read(buf: &'a [u8]) -> Result<Self> {
        let len = buf.len();

        let msg_type = buf[0];
        let data = &buf[1..len - 1];
        let checksum = buf[len - 1];

        let msg_type = ControlMessageType::try_from_primitive(msg_type)
            .with_context(|| format!("Unknown message type: {}", msg_type))?;

        let expected_checksum = calc_checksum(&buf[..len - 1]);
        if checksum != expected_checksum {
            bail!(
                "Invalid checksum: expected {:02X}, got {:02X}",
                expected_checksum,
                checksum
            );
        }

        Ok(Self {
            msg_type,
            body: data,
        })
    }

    pub fn write<'b>(&self, buf: &'b mut [u8]) -> Result<&'b [u8]> {
        let len = self.body.len();
        assert!(
            len + 2 <= buf.len(),
            "Message too long ({} > {})",
            len + 2,
            buf.len()
        );

        buf[0] = self.msg_type as u8;
        buf[1..len + 1].copy_from_slice(self.body);
        buf[len + 1] = calc_checksum(&buf[..len + 1]);

        Ok(&buf[..len + 2])
    }
}

#[derive(Debug)]
pub enum ControlError {
    Validation,
    NoFile,
    NoMemory,
    InvalidStatus,
    DecodeFailed,
}
