use serde::{Deserialize, Serialize};
use serde_repr::{Deserialize_repr, Serialize_repr};
use serde_tuple::{Deserialize_tuple, Serialize_tuple};

#[derive(Serialize, Deserialize, Debug)]
pub struct HeaderJson {
    pub device_model: String,
    pub sn: String,
    #[serde(alias = "update_at")] // a typo that was fixed?..
    pub updated_at: i64,
    pub version: String,
}

#[derive(Serialize, Deserialize, Debug, Default)]
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

#[derive(Serialize, Deserialize, Debug)]
pub struct User {
    pub platform: String,
    pub uid: u32,
    pub user_name: String,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct UserProfile {
    #[serde(flatten)]
    pub device_info: Option<HeaderJson>,
    pub user: Option<User>,
    pub user_profile: UserProfileInner,
}

#[derive(Serialize_repr, Deserialize_repr, PartialEq, Debug)]
#[repr(u8)]
pub enum WorkoutState {
    NotSyncronized,
    Recording,
    Syncing,
    Synced,
    Broken,
}

#[derive(Serialize_tuple, Deserialize_tuple, Debug)]
pub struct WorkoutsItem {
    pub name: u64,
    pub size: u32,
    pub state: WorkoutState,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct Workouts {
    #[serde(flatten)]
    pub device_info: Option<HeaderJson>,
    pub workouts: Vec<WorkoutsItem>,
}
