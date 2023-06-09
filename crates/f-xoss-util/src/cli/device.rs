use anyhow::{anyhow, bail, Context, Result};
use camino::{Utf8Path, Utf8PathBuf};
use chrono::{FixedOffset, Local, TimeZone, Utc};
use indicatif::ProgressStyle;
use prettytable::{row, table};
use std::str::FromStr;
use std::time::SystemTime;
use tracing::{info, instrument};
use tracing_indicatif::span_ext::IndicatifSpanExt;

use super::DeviceCli;
use crate::cli::{DeviceCommand, SyncOptions};
use crate::config::XossUtilConfig;
use f_xoss::device::{MgaState, XossDevice};
use f_xoss::model::{User, UserProfile, UserProfileInner};

#[instrument(skip(device, _options))]
async fn sync_workouts(device: &XossDevice, _options: &SyncOptions) -> Result<()> {
    let local_workouts_dir = crate::config::APP_DIRS.data_dir().join("workouts");
    tokio::fs::create_dir_all(&local_workouts_dir).await?;

    info!("Syncing workouts to {}", local_workouts_dir.display());

    let workouts = device.read_workouts().await?;

    let missing_workouts = workouts
        .iter()
        .filter(|workout| !local_workouts_dir.join(workout.filename()).exists())
        .collect::<Vec<_>>();

    let current_span = tracing::Span::current();
    current_span.pb_set_style(&ProgressStyle::default_bar()
        .template("{span_child_prefix}{spinner:.green} [{bar:40.cyan/blue}] {human_pos}/{human_len} ({eta} @ {per_sec})")
        .unwrap()
        .progress_chars("#>-"));
    current_span.pb_set_length(workouts.len() as u64);

    for workout in missing_workouts {
        let workout_filename = workout.filename();
        let workout_path = local_workouts_dir.join(&workout_filename);

        if workout_path.exists() {
            continue;
        }

        info!(
            "Downloading workout {:?} to {:?}",
            workout.name, workout_path
        );
        let workout_data = device
            .read_file(&workout_filename)
            .await
            .context("Failed to receive workout file")?;
        tokio::fs::write(&workout_path, &workout_data)
            .await
            .context("Failed to write workout file")?;

        current_span.pb_inc(1);
    }

    Ok(())
}

#[instrument(skip(device, config, options))]
async fn sync_mga(
    device: &XossDevice,
    config: Option<&XossUtilConfig>,
    options: &SyncOptions,
) -> Result<()> {
    let Some(config) = config else {
        bail!("Config is required for sync subcommand");
    };

    let mga_state = device
        .get_mga_state()
        .await
        .context("Failed to get MGA status")?;
    let mga_data = crate::mga::get_mga_data(&config.mga, &options.mga_update).await?;

    if match mga_state {
        MgaState::MissingData => true,
        MgaState::ValidUntil(date) => date < mga_data.valid_until,
    } {
        info!("Updating MGA data");
        device
            .write_file("offline.gnss", &mga_data.data)
            .await
            .context("Failed to send the MGA data")?;
    } else {
        info!("MGA data is up to date");
    }

    Ok(())
}

async fn sync(
    device: &XossDevice,
    config: Option<&XossUtilConfig>,
    options: SyncOptions,
) -> Result<()> {
    device
        .set_time(SystemTime::now())
        .await
        .context("Failed to set the time")?;
    info!("Time set");

    let user_profile = device.read_user_profile().await?;

    let time_zone = Local::now().offset().local_minus_utc();

    let user_profile = UserProfile {
        user: Some(user_profile.user.unwrap_or_else(|| User {
            platform: "XOSS".to_string(),
            uid: 42,
            user_name: "ABOBA".to_string(),
        })),
        user_profile: UserProfileInner {
            time_zone,
            ..user_profile.user_profile
        },
    };
    device.write_user_profile(&user_profile).await?;

    sync_workouts(device, &options)
        .await
        .context("Syncing workouts")?;

    sync_mga(device, config, &options)
        .await
        .context("Syncing MGA data")?;

    Ok(())
}

async fn info(device: &XossDevice) -> Result<()> {
    let user_profile = device.read_user_profile().await?;

    let header_json = device.get_device_json_header().await?;
    let updated_at = Utc.timestamp_opt(header_json.updated_at, 0).unwrap();

    let device_info = device.device_info().await;
    let memory_capacity = device.get_memory_capacity().await?;
    let mga_status = device.get_mga_state().await?;

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

async fn pull(
    device: &XossDevice,
    device_filename: &str,
    output_filename: Option<&Utf8Path>,
) -> Result<()> {
    let output_filename = match output_filename {
        Some(output_filename) => output_filename.to_path_buf(),
        None => Utf8PathBuf::from_str(
            Utf8PathBuf::from_str(device_filename)?
                .file_name()
                .ok_or_else(|| {
                    anyhow!(
                        "No output filename provided and could not infer it from device filename"
                    )
                })?,
        )
        .unwrap(),
    };

    let contents = device
        .read_file(device_filename)
        .await
        .with_context(|| format!("Pulling {} from the device", device_filename))?;
    tokio::fs::write(&output_filename, contents)
        .await
        .with_context(|| format!("Writing {} to {}", device_filename, output_filename))?;

    Ok(())
}

async fn push(
    device: &XossDevice,
    input_filename: Utf8PathBuf,
    device_filename: Option<&str>,
) -> Result<()> {
    let Some(device_filename) = device_filename.or(input_filename.file_name()) else {
        bail!("No device filename provided and could not infer it from input filename")
    };

    let contents = tokio::fs::read(&input_filename)
        .await
        .with_context(|| format!("Reading {} from the filesystem", input_filename))?;
    device
        .write_file(device_filename, &contents)
        .await
        .with_context(|| format!("Writing {} to the device", device_filename))?;

    Ok(())
}

async fn delete(device: &XossDevice, device_filename: &str) -> Result<()> {
    device
        .delete_file(device_filename)
        .await
        .with_context(|| format!("Deleting {} from the device", device_filename))?;

    Ok(())
}

impl DeviceCli {
    pub async fn run(self, device: &XossDevice, config: Option<XossUtilConfig>) -> Result<()> {
        match self.subcommand {
            DeviceCommand::Sync(options) => sync(device, config.as_ref(), options).await?,
            DeviceCommand::Info => info(device).await?,
            DeviceCommand::Pull {
                device_filename,
                output_filename,
            } => pull(device, &device_filename, output_filename.as_deref()).await?,
            DeviceCommand::Push {
                input_filename,
                device_filename,
            } => push(device, input_filename, device_filename.as_deref()).await?,
            DeviceCommand::Delete { device_filename } => delete(device, &device_filename).await?,
        }

        Ok(())
    }
}
