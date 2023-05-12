use anyhow::{Context, Result};
use chrono::{FixedOffset, TimeZone, Utc};
use prettytable::{row, table};
use std::time::SystemTime;
use tracing::info;

use super::DeviceCli;
use crate::cli::{DeviceCommand, SyncOptions};
use crate::config::XossUtilConfig;
use f_xoss::device::XossDevice;
use f_xoss::model::{User, UserProfile};

async fn sync(device: &XossDevice, options: SyncOptions) -> Result<()> {
    device
        .set_time(SystemTime::now())
        .await
        .context("Failed to set the time")?;

    let header_json = device.get_device_json_header().await?;
    info!("Device JSON header: {:#?}", header_json);

    let settings = device.read_settings().await?;
    info!("Settings: {:#?}", settings);

    let user_profile = device.read_user_profile().await?;
    info!("User profile: {:#?}", user_profile);

    let user_profile = UserProfile {
        user: Some(User {
            platform: "XOSS".to_string(),
            uid: 42,
            user_name: "ABOBA".to_string(),
        }),
        user_profile: Default::default(),
    };
    device.write_user_profile(&user_profile).await?;

    let gear_profile = device.read_gear_profile().await?;
    info!("Gear profile: {:#?}", gear_profile);

    let workouts = device.read_workouts().await?;
    info!("Workouts: {:#?}", workouts);

    // we can't parse the panels (yet)
    // well, writing a good editor is a lot of effort anyway, so prolly not gonna do it soon
    let panels = device.read_file("panels.json").await?;
    info!("Panels: {:}", String::from_utf8(panels).unwrap());

    let routes = device.read_routes().await?;
    info!("Routes: {:#?}", routes);

    device
        .read_file(
            "20230508021939.fit", // "user_profile.json",
        )
        .await?;

    let offline_gnss_data = std::fs::read(
        // "mgaoffline.ubx",
        "2023-05-08.data",
    )
    .unwrap();
    device
        .write_file("offline.gnss", &offline_gnss_data)
        .await
        .context("Failed to send the offline GNSS data")?;

    Ok(())
}

async fn info(device: &XossDevice) -> Result<()> {
    let user_profile = device.read_user_profile().await?;

    let header_json = device.get_device_json_header().await?;
    let updated_at = Utc.timestamp_opt(header_json.updated_at, 0).unwrap();

    let device_info = device.device_info().await;
    let memory_capacity = device.get_memory_capacity().await?;
    let mga_status = device.get_mga_status().await?;

    let mut table = prettytable::Table::new();
    table.set_format(*prettytable::format::consts::FORMAT_CLEAN);
    table.add_row(row!["Firmware Revision:", device_info.firmware_revision]);
    table.add_row(row!["Manufacturer Name:", device_info.manufacturer_name]);
    table.add_row(row!["Model Number:", device_info.model_number]);
    table.add_row(row!["Hardware Revision:", device_info.hardware_revision]);
    table.add_row(row!["Serial Number:", device_info.serial_number]);
    table.add_row(row!["Protocol Version:", header_json.version]);
    table.add_row(row!["", ""]);

    let mut user_profile_table = match user_profile.user {
        None => {
            table!(["(No user profile)"])
        }
        Some(u) => {
            table!(
                ["User Name:", u.user_name],
                ["User ID:", u.uid],
                ["Platform:", u.platform]
            )
        }
    };
    user_profile_table.set_format(*prettytable::format::consts::FORMAT_CLEAN);

    table.add_row(row!["User Name:", user_profile_table]);
    table.add_row(row![
        "Time Zone:",
        FixedOffset::east_opt(user_profile.user_profile.time_zone).unwrap()
    ]);
    table.add_row(row!["", ""]);
    table.add_row(row![
        "Battery Level:",
        format!("{}%", device.battery_level().await)
    ]);
    table.add_row(row!["Last Updated At:", updated_at]);
    table.add_row(row!["Memory Capacity:", memory_capacity]);
    table.add_row(row!["A-GPS Status:", mga_status]);

    info!("Device info:\n{}", table);

    Ok(())
}

impl DeviceCli {
    pub async fn run(self, device: &XossDevice, config: Option<XossUtilConfig>) -> Result<()> {
        match self.subcommand {
            DeviceCommand::Sync(options) => sync(device, options).await?,
            DeviceCommand::Info => info(device).await?,
            DeviceCommand::Pull { .. } => todo!(),
            DeviceCommand::Push { .. } => todo!(),
            DeviceCommand::Delete { .. } => todo!(),
        }

        Ok(())
    }
}
