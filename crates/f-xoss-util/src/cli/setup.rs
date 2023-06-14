use crate::{config, mga};
use anyhow::{anyhow, Context, Result};
use btleplug::api::{
    BDAddr, Central, CentralEvent, Peripheral as _, PeripheralProperties, ScanFilter,
};
use btleplug::platform::{Adapter, Peripheral, PeripheralId};
use console::Term;
use dialoguer::theme::ColorfulTheme;
use f_xoss::device::XossDevice;
use itertools::Itertools;
use once_cell::sync::Lazy;
use owo_colors::colored::Color;
use owo_colors::OwoColorize;
use similar::ChangeTag;
use std::fmt::{Display, Formatter};
use std::ops::{Deref, Not};
use std::pin::Pin;
use std::time::Duration;
use tokio::select;
use tokio::sync::Mutex;
use tokio_stream::{Stream, StreamExt};
use tracing::{error, info, info_span, warn, Instrument};

use super::SetupCli;
use crate::config::{MgaConfig, XossDeviceInfo, XossUtilConfig};

static DIALOGUER_THEME: Lazy<ColorfulTheme> = Lazy::new(|| ColorfulTheme::default());

#[derive(Clone, Debug)]
struct ScannerDevice {
    peripheral_id: PeripheralId,
    peripheral: Peripheral,
    address: BDAddr,
    properties: PeripheralProperties,
}

impl ScannerDevice {
    pub fn likely_xoss_device(&self) -> bool {
        self.properties
            .local_name
            .as_ref()
            .map(|v| v.contains("XOSS"))
            .unwrap_or(false)
    }
}

impl Display for ScannerDevice {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        if let Some(name) = &self.properties.local_name {
            write!(f, "{} ({})", name.blue(), self.address.bright_black())
        } else {
            write!(f, "{}", self.address.bright_black())
        }
    }
}
impl PartialEq for ScannerDevice {
    fn eq(&self, other: &Self) -> bool {
        ScannerDevice::partial_cmp(self, other) == Some(std::cmp::Ordering::Equal)
    }
}

impl PartialOrd for ScannerDevice {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        // put the XOSS devices first
        // then the ones with a name
        // then the other ones

        let self_xoss = self.likely_xoss_device();
        let other_xoss = other.likely_xoss_device();

        let self_name = self.properties.local_name.is_some();
        let other_name = other.properties.local_name.is_some();

        // note: order reversed
        Some(
            self_xoss
                .cmp(&other_xoss)
                .reverse()
                .then(self_name.cmp(&other_name).reverse()),
        )
    }
}

impl Eq for ScannerDevice {}
impl Ord for ScannerDevice {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        ScannerDevice::partial_cmp(self, other).unwrap()
    }
}

struct ScannerState {
    devices: Mutex<Vec<ScannerDevice>>,
}

impl ScannerState {
    async fn add_device(&self, device: ScannerDevice) {
        let mut devices = self.devices.lock().await;

        if !devices
            .iter()
            .any(|d| d.peripheral_id == device.peripheral_id)
        {
            devices.push(device);
        }
    }

    async fn select_device(&self, term: &Term) -> Result<Option<ScannerDevice>> {
        let devices = {
            self.devices
                .lock()
                .await
                .iter()
                .cloned()
                .sorted()
                .collect::<Vec<_>>()
        };

        let selected = dialoguer::Select::with_theme(DIALOGUER_THEME.deref())
            .items(&devices)
            .item("[Rescan]")
            .default(0)
            .with_prompt("Select a XOSS device to connect to")
            .interact_on_opt(term)
            .context("Failed to select a device")?;

        Ok(selected.and_then(|index| {
            if index == devices.len() {
                None
            } else {
                Some(devices[index].clone())
            }
        }))
    }

    async fn handle_scan_events(
        &self,
        adapter: &Adapter,
        mut events: Pin<Box<dyn Stream<Item = CentralEvent> + Send>>,
    ) -> Result<()> {
        while let Some(event) = events.next().await {
            if let CentralEvent::DeviceDiscovered(peripheral_id) = event {
                let peripheral = adapter
                    .peripheral(&peripheral_id)
                    .await
                    .context("Failed to get peripheral properties")?;

                let address = peripheral.address();
                let Some(properties) = peripheral.properties().await? else {
                    warn!("Failed to get peripheral properties for {}", address);
                    continue;
                };

                let device = ScannerDevice {
                    peripheral_id,
                    peripheral,
                    address,
                    properties,
                };

                self.add_device(device).await;
            }
        }

        Ok(())
    }
}

async fn find_device() -> Result<XossDeviceInfo> {
    let manager = btleplug::platform::Manager::new()
        .await
        .context("Failed to create a manager")?;
    let adapter = crate::locate_util::find_adapter(&manager).await?;

    let events = adapter
        .events()
        .await
        .context("Failed to get adapter events stream")?;

    adapter
        .start_scan(ScanFilter::default())
        .await
        .context("Starting scan")?;

    let scanner = ScannerState {
        devices: Mutex::new(Vec::new()),
    };

    let term = Term::stdout();

    let cli = async {
        loop {
            tokio::time::sleep(Duration::from_secs(5))
                .instrument(info_span!("Scanning for bluetooth devices"))
                .await;

            let Some(device) = scanner
                .select_device(&term)
                .await
                .context("Selecting device")?
                else { continue; };

            info!("Connecting to {}...", device);

            let connect_result = async {
                device
                    .peripheral
                    .connect()
                    .await
                    .context("Connecting to device...")?;

                XossDevice::new(device.peripheral.clone())
                    .await
                    .context("Failed to connect to XOSS device")
            }
            .await;

            let xoss_device = match connect_result {
                Ok(d) => d,
                Err(e) => {
                    error!("Failed to connect to XOSS device:\n {:?}", e);
                    continue;
                }
            };

            break Ok::<_, anyhow::Error>((xoss_device, device));
        }
    };

    let events_handler = scanner.handle_scan_events(&adapter, events);

    let result = select! {
        res = cli => res,
        res = events_handler => {
            match res {
                Ok(()) => Err(anyhow!("Scan events handler exited")),
                Err(e) => Err(e),
            }
        }
    };

    adapter.stop_scan().await.context("Stopping scan")?;

    let (xoss_device, device): (XossDevice, ScannerDevice) = result?;

    info!("Device info: {:#?}", xoss_device.device_info().await);

    Ok(XossDeviceInfo {
        name: device.properties.local_name.clone(),
        peripheral_id: device.peripheral_id,
    })
}

async fn get_ublox_token() -> Result<Option<String>> {
    println!("Updating the satellite data requires an u-blox AssistNow token.\n You can get one for free from https://www.u-blox.com/en/assistnow-service-evaluation-token-request-form\n Alternatively, you can skip this setup step if you don't want to update the satellite data. You can re-run setup to configure it later.");

    loop {
        let token = dialoguer::Input::<String>::with_theme(DIALOGUER_THEME.deref())
            .with_prompt("Enter your u-blox token")
            .allow_empty(true)
            .interact_text()
            .map(|s: String| s.is_empty().not().then_some(s))
            .context("Failed to get u-blox token")?;

        let Some(token) = token else {
            return Ok(None);
        };

        let token_valid = mga::check_ublox_token(&token)
            .await
            .context("Failed to check u-blox token")?;

        if token_valid {
            info!("The u-blox token is valid!");
            return Ok(Some(token));
        } else {
            println!("The u-blox server does not accept the token you entered. Please try again.");
        }
    }
}

async fn save_config(config: &XossUtilConfig) -> Result<()> {
    let config_path = config::config_path();

    info!("Saving the config to {}", config_path.display());
    std::fs::create_dir_all(config_path.parent().unwrap())
        .context("Creating the config directory")?;
    std::fs::write(
        &config_path,
        toml::to_string_pretty(config).context("Serializing the config file")?,
    )
    .context("Writing the config file")?;

    Ok(())
}

async fn save_config_with_confirmation(config: &XossUtilConfig) -> Result<()> {
    // find the diff of the config & the current config, and show it to the user

    let config_path = config::config_path();

    // note: this never fails because the function is only called when a config file already exists
    let old_config = std::fs::read_to_string(&config_path).context("Reading old config file")?;
    let new_config = toml::to_string_pretty(config).context("Serializing the new config file")?;

    println!(
        "The following changes will be made to the config file at {}:",
        config_path.display()
    );

    let diff = similar::TextDiff::from_lines(&old_config, &new_config);

    for change in diff.iter_all_changes() {
        let (tag, color) = match change.tag() {
            ChangeTag::Delete => ("-", Color::Red),
            ChangeTag::Insert => ("+", Color::Green),
            ChangeTag::Equal => (" ", Color::White),
        };

        print!("{} {}", tag.color(color), change.color(color));
    }

    println!();

    let confirm = dialoguer::Confirm::with_theme(DIALOGUER_THEME.deref())
        .with_prompt("Do you want to save the config?")
        .default(true)
        .interact()
        .context("Failed to get user confirmation")?;

    if confirm {
        save_config(config).await?;
        Ok(())
    } else {
        Err(anyhow!("User cancelled the config save"))
    }
}

impl SetupCli {
    pub async fn run(self, config: Option<XossUtilConfig>) -> Result<()> {
        let mut devices = config.as_ref().map_or_else(Vec::new, |v| v.devices.clone());
        let mut new_config = config.clone().unwrap_or_default();

        if devices.is_empty() {
            info!("No devices configured, scanning for devices...");
            let device = find_device().await?;
            devices.push(device);
            new_config = XossUtilConfig {
                devices: devices.clone(),
                ..new_config
            };
            // save the config file, but only if it doesn't exist
            // for the final save we'll ask the user
            if config.is_none() {
                save_config(&new_config).await?;
            }
        } else {
            info!("Found device in config, skipping scan");
        }

        let ublox_token = config.as_ref().and_then(|v| v.mga.ublox_token.clone());
        if ublox_token.is_none() {
            info!("No ublox token configured, asking for it...");
            if let Some(ublox_token) = get_ublox_token().await? {
                new_config = XossUtilConfig {
                    mga: MgaConfig {
                        ublox_token: Some(ublox_token),
                        ..new_config.mga
                    },
                    ..new_config
                };

                if config.is_none() {
                    save_config(&new_config).await?;
                }
            } else {
                info!("No ublox token provided, not saving it");
            }
        } else {
            info!("Found ublox token in config, skipping prompt");
        }

        if config.as_ref().map_or(true, |config| config != &new_config) {
            // changes!
            if config.is_none() {
                // no confirmation
                save_config(&new_config).await?;
            } else {
                // confirmation
                save_config_with_confirmation(&new_config).await?;
            }
        } else {
            info!("No changes to the config, no need to save it");
        }

        Ok(())
    }
}
