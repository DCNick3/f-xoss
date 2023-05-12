use serde::{Deserialize, Serialize};
use serde_repr::{Deserialize_repr, Serialize_repr};
use serde_tuple::{Deserialize_tuple, Serialize_tuple};

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct HeaderJson {
    pub device_model: String,
    pub sn: String,
    #[serde(alias = "update_at")] // a typo that was fixed in some fw version?
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
    /// Time zone offset in seconds
    pub time_zone: i32,
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
    /// Finished recording, but was not downloaded from the device
    NotSynchronized = 0,
    Recording = 1,
    Syncing = 2,
    /// Was downloaded from the device
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
    /// Disable backlight after some inactivity
    #[default]
    Auto = 0,
    /// Keep backlight on
    AlwaysOn = 1,
    /// Disable backlight
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
    /// Backlight mode
    pub backlight: Backlight,
    /// Whether to auto-pause the workout when the speed is 0
    pub auto_pause: AutoPause,
    /// This setting is not used by the device, set to 0
    pub overwrite: u8,
    /// Whether to play a tone when device keys are pressed
    pub keytone: bool,
}

#[derive(Serialize, Deserialize, PartialEq, Debug, Clone, Default)]
#[serde(rename_all = "snake_case")]
pub enum GearType {
    #[default]
    Bike,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Gear {
    pub gid: u32,
    /// Weight of the bike, in grams (?)
    pub weight: u32,
    /// Wheel size, in mm
    pub wheel_size: u32,
    /// Whether the bike is active (?)
    pub activated: bool,
    /// Gear profile name
    pub name: String,
    #[serde(rename = "type")]
    pub type_: GearType,
}

#[derive(Serialize, Deserialize, PartialEq, Debug, Clone, Default)]
pub enum SportType {
    #[default]
    Cycling,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Route {
    /// Unique route identifier
    pub rid: u64,
    /// Size of the .ro file in bytes
    pub size: u32,
    pub source: u8, // No idea WTF is this
    /// Route name
    pub name: String,
    #[serde(rename = "type")]
    pub type_: SportType,
    /// The version of the route format, only 2 supported by the device
    #[serde(rename = "verison")] // yes, it's a typo
    pub version: u8,
    /// Route length, in meters
    pub length: u32,
    /// Route total elevation gain, in meters
    pub gain: u32,
}
