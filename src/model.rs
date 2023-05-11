use serde::{Deserialize, Serialize};
use serde_repr::{Deserialize_repr, Serialize_repr};
use serde_tuple::{Deserialize_tuple, Serialize_tuple};

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct HeaderJson {
    pub device_model: String,
    pub sn: String,
    #[serde(alias = "update_at")] // a typo that was fixed?..
    pub updated_at: i64,
    pub version: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct WithHeader<T> {
    #[serde(flatten)]
    pub header: HeaderJson,
    #[serde(flatten)]
    pub data: T,
}

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct UserProfileInner {
    #[serde(rename = "ALAHR")]
    pub alahr: i64,
    #[serde(rename = "ALASPEED")]
    pub alaspeed: i64,
    #[serde(rename = "FTP")]
    pub ftp: i64,
    #[serde(rename = "LTHR")]
    pub lthr: i64,
    #[serde(rename = "MAXHR")]
    pub maxhr: i64,
    pub birthday: i64,
    pub gender: i64,
    pub height: i64,
    pub time_zone: i64,
    pub weight: i64,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct User {
    pub platform: String,
    pub uid: u32,
    pub user_name: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct UserProfile {
    pub user: Option<User>,
    pub user_profile: UserProfileInner,
}

#[derive(Serialize_repr, Deserialize_repr, PartialEq, Debug, Clone, Copy)]
#[repr(u8)]
pub enum WorkoutState {
    NotSynchronized = 0,
    Recording = 1,
    Syncing = 2,
    Synced = 3,
    Broken = 4,
}

#[derive(Serialize_tuple, Deserialize_tuple, Debug, Clone)]
pub struct WorkoutsItem {
    pub name: u64,
    pub size: u32,
    pub state: WorkoutState,
}

#[derive(Serialize, Deserialize, PartialEq, Debug, Clone, Default)]
pub enum Language {
    #[serde(rename = "en")]
    #[default]
    English,
    #[serde(rename = "zh-cn")]
    Chinese,
}

#[derive(Serialize_repr, Deserialize_repr, PartialEq, Debug, Clone, Default)]
#[repr(u8)]
pub enum DistanceUnit {
    #[default]
    Metric = 0,
    Imperial = 1,
}

#[derive(Serialize_repr, Deserialize_repr, PartialEq, Debug, Clone, Default)]
#[repr(u8)]
pub enum TemperatureUnit {
    #[default]
    Celsius = 0,
    Fahrenheit = 1,
}

#[derive(Serialize_repr, Deserialize_repr, PartialEq, Debug, Clone, Default)]
#[repr(u8)]
pub enum Backlight {
    #[default]
    Auto = 0,
    AlwaysOn = 1,
    Off = 2,
}

#[derive(Serialize_repr, Deserialize_repr, PartialEq, Debug, Clone, Default)]
#[repr(u8)]
pub enum AutoPause {
    #[default]
    On = 0,
    Off = 1,
}

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct Settings {
    #[serde(rename = "language_i18n")]
    pub language: Language,
    pub unit: DistanceUnit,
    pub temperature_unit: TemperatureUnit,
    /// This setting is not used by the device, set to 0
    pub time_formatter: u8,
    pub backlight: Backlight,
    pub auto_pause: AutoPause,
    /// This setting is not used by the device, set to 0
    pub overwrite: u8,
    pub keytone: bool,
}
