use anyhow::{Context, Result};
use btleplug::api::BDAddr;
use directories::ProjectDirs;
use once_cell::sync::Lazy;
use serde::Deserialize;
use serde::{de, Serialize};
use std::io::ErrorKind;
use std::path::PathBuf;

fn deserialize_bdaddr<'de, D>(deserializer: D) -> Result<BDAddr, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use std::str::FromStr;
    let s = String::deserialize(deserializer)?;
    let addr = BDAddr::from_str(&s).map_err(|e| {
        de::Error::custom(format!(
            "Failed to parse BDAddr from string: {:?}: {}",
            s, e
        ))
    })?;

    Ok(addr)
}

fn serialize_bdaddr<S>(addr: &BDAddr, serializer: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    serializer.serialize_str(&addr.to_string())
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct XossDeviceInfo {
    pub name: Option<String>,
    #[serde(
        deserialize_with = "deserialize_bdaddr",
        serialize_with = "serialize_bdaddr"
    )]
    pub address: BDAddr,
}

impl XossDeviceInfo {
    pub fn identify(&self) -> String {
        self.name
            .as_ref()
            .map(|s| s.to_string())
            .unwrap_or_else(|| self.address.to_string())
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq)]
pub struct MgaConfig {
    pub base_url: Option<String>,
    pub period_weeks: Option<u32>,
    pub resolution_days: Option<u32>,
    pub ublox_token: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq)]
pub struct XossUtilConfig {
    pub devices: Vec<XossDeviceInfo>,
    #[serde(default)]
    pub mga: MgaConfig,
}

pub static APP_DIRS: Lazy<ProjectDirs> = Lazy::new(|| {
    ProjectDirs::from("com.dcnick3", "", "f-xoss").expect("Failed to get the project directories")
});

pub fn config_path() -> PathBuf {
    APP_DIRS.config_dir().join("config.toml")
}

pub fn load_config() -> Result<Option<XossUtilConfig>> {
    let config_path = config_path();

    let config = match std::fs::read_to_string(&config_path) {
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(None),
        r => r.map(Some),
    }
    .context(format!("Reading config file {}", config_path.display()))?;

    config
        .map(|config| {
            toml::from_str(&config)
                .with_context(|| format!("Parsing config file {}", config_path.display()))
        })
        .transpose()
}
