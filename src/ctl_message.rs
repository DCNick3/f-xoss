use binrw::io::NoSeek;
use binrw::{BinRead, BinResult, BinWrite, Endian};
use std::io::Write;

#[derive(BinRead, BinWrite, Debug)]
#[brw(repr(u8))]
pub enum ControlMessageType {
    DbgCmd = 0x0,
    Idle = 0x4,
    RequestReturn = 0x5,
    Returning = 0x6,
    RequestSend = 0x7,
    Accept = 0x8,
    RequestCap = 0x9,
    ReturnCap = 0xA,
    RequestDel = 0xD,
    DelSuccess = 0xE,
    RequestDetail = 0xF,
    RequestStop = 0x1F,
    ErrVali = 0x11,
    ErrNoFile = 0x12,
    ErrMemory = 0x13,
    ErrStatus = 0x14,
    ErrDecode = 0x15,
    TimeSet = 0x54,
    TimeSetRtn = 0x55,
    RequestMga = 0x77,
    ReturnMga = 0x78,
    StatusAct = 0xAC,
    RequestClr = 0xCC,
    ReturnClr = 0xCD,
    DfuEnter = 0xDF,
    StatusReturn = 0xFF,
}

#[derive(BinRead, BinWrite, Debug)]
#[br(import(len: usize))]
pub struct RawControlMessage {
    pub msg_type: ControlMessageType,
    #[br(count = len - 1)]
    pub body: Vec<u8>,
}

pub fn partial_checksum(buf: &[u8]) -> u8 {
    buf.iter().fold(0, |acc, x| acc ^ x)
}

/// Adds checksum to the data
///
/// The checksum is calculated by XORing all bytes in the data
pub struct CheckSummed<T>(pub T);

struct ChecksumWriter<W> {
    writer: W,
    checksum: u8,
}

impl<W> ChecksumWriter<W> {
    fn new(writer: W) -> Self {
        Self {
            writer,
            checksum: 0,
        }
    }

    fn checksum(&self) -> u8 {
        self.checksum
    }
}

impl<W: Write> Write for ChecksumWriter<W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.checksum ^= partial_checksum(buf);
        Write::write(&mut self.writer, buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.writer.flush()
    }
}

impl<T: BinWrite> BinWrite for CheckSummed<T> {
    type Args<'a> = T::Args<'a>;

    fn write_options<W: Write>(
        &self,
        writer: &mut W,
        endian: Endian,
        options: Self::Args<'_>,
    ) -> BinResult<()> {
        let mut writer = NoSeek::new(ChecksumWriter::new(writer));
        self.0.write_options(&mut writer, endian, options)?;

        let checksum = writer.get_ref().checksum();
        checksum.write_options(&mut writer, endian, ())?;

        Ok(())
    }
}
