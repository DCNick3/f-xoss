use binrw::{BinRead, BinReaderExt, BinResult};
use chrono::NaiveDate;

pub struct MgaData {
    pub data: Vec<u8>,
    pub valid_since: NaiveDate,
    pub valid_until: NaiveDate,
}

#[derive(BinRead)]
#[br(magic = b"\xb5\x62\x13\x20\x4c\x00\x00\x00")]
#[allow(unused)]
struct UbxMgaAno {
    satellite_id: u8,
    gnss_id: u8,
    year: u8,
    month: u8,
    day: u8,
    reserved1: u8,
    data: [u8; 64],
    reserved2: [u8; 4],
    ck_a: u8,
    ck_b: u8,
}

impl UbxMgaAno {
    pub fn date(&self) -> NaiveDate {
        let year = 2000 + self.year as i32;
        let month = self.month as u32;
        let day = self.day as u32;
        NaiveDate::from_ymd_opt(year, month, day).unwrap()
    }
}

pub fn parse_mga_data(data: Vec<u8>) -> BinResult<MgaData> {
    let mut cursor = std::io::Cursor::new(&data);
    let mut items = Vec::new();
    while cursor.position() < cursor.get_ref().len() as u64 {
        let ubx_mga_ano: UbxMgaAno = cursor.read_le()?;
        items.push(ubx_mga_ano);
    }

    let valid_since = items.iter().map(|u| u.date()).min().unwrap();
    let valid_until = items.iter().map(|u| u.date()).max().unwrap();

    Ok(MgaData {
        data,
        valid_since,
        valid_until,
    })
}
