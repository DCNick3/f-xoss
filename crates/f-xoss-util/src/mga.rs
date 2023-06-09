use crate::cli::MgaUpdateOptions;
use crate::config::MgaConfig;
use anyhow::{anyhow, Context, Result};
use f_xoss::mga::{parse_mga_data, MgaData};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use surf::{StatusCode, Url};
use thiserror::Error;
use tracing::{debug, instrument, warn};

fn mga_file_path() -> PathBuf {
    crate::config::APP_DIRS.cache_dir().join("mgaoffline.ubx")
}

#[derive(Serialize, Deserialize, Debug)]
struct ErrorResponse {
    pub message: String,
}

#[derive(Error, Debug)]
enum Error {
    #[error("The u-blox token is invalid")]
    BadToken,
    #[error("Some other error has occurred")]
    Other(#[from] anyhow::Error),
}

fn mga_build_url(config: &MgaConfig) -> Result<Url> {
    let url = config
        .base_url
        .as_deref()
        .unwrap_or("https://offline-live1.services.u-blox.com");
    let mut url = Url::parse(url)?.join("GetOfflineData.ashx").unwrap();

    let period_str = config.period_weeks.unwrap_or(4).to_string();
    let resolution_str = config.resolution_days.unwrap_or(2).to_string();

    let mut query_pairs = Vec::new();
    query_pairs.push((
        "token",
        config
            .ublox_token
            .as_deref()
            .ok_or_else(|| anyhow!("Updating MGA data requires a u-blox AssistNow token"))?,
    ));
    query_pairs.push(("gnss", "gps,glo"));
    query_pairs.push(("format", "mga"));
    query_pairs.push(("period", period_str.as_str()));
    query_pairs.push(("resolution", resolution_str.as_str()));

    // u-blox API uses a non-standard query string format
    let query_string = query_pairs
        .iter()
        .map(|(k, v)| format!("{}={}", k, v))
        .collect::<Vec<_>>()
        .join(";");
    url.set_query(Some(query_string.as_str()));

    let url_str = url.to_string();
    debug!("Constructed MGA URL: {}", url_str);

    Ok(url)
}

#[instrument(skip(config))]
async fn download_mga_data(config: &MgaConfig) -> Result<MgaData, Error> {
    let url = mga_build_url(config)?;

    let mut response = surf::get(url)
        .await
        .map_err(|err| anyhow!(err))
        .context("Failed to download MGA data")?;

    match response.status() {
        StatusCode::Ok => {}
        StatusCode::BadRequest => {
            let error: ErrorResponse = response.body_json().await.map_err(|err| anyhow!(err))?;
            let error = match error.message.as_str() {
                message if message.starts_with("Invalid token: ") => Error::BadToken,
                message => {
                    warn!("Unknown error message from u-blox: {}", message);
                    Error::Other(anyhow!("u-blox API returned Bad Request: {}", message))
                }
            };

            return Err(error);
        }
        _ => return Err(anyhow!("Unexpected response status: {}", response.status()).into()),
    }

    let raw_data = response
        .body_bytes()
        .await
        .map_err(|err| anyhow!(err))
        .context("Failed to read MGA data")?;

    Ok(parse_mga_data(raw_data).context("Parsing downloaded MGA data")?)
}

async fn get_current_mga_data() -> Result<Option<MgaData>> {
    let path = mga_file_path();

    async {
        match tokio::fs::read(&path).await {
            Ok(data) => {
                let data = parse_mga_data(data).context("Parsing cached MGA data")?;
                Ok::<_, anyhow::Error>(Some(data))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }
    .await
    .with_context(|| format!("Reading cached MGA data from {}", path.display()))
}

pub async fn get_mga_data(config: &MgaConfig, options: &MgaUpdateOptions) -> Result<MgaData> {
    let cached_data = get_current_mga_data().await?;
    let today = chrono::Utc::now().date_naive();
    // update if we are > 2 days out of date
    let out_of_date = |data: &MgaData| {
        let duration = today.signed_duration_since(data.valid_since);
        if duration < chrono::Duration::zero() {
            warn!("MGA data is from the future? (or is it timezone troubles?...) (valid since: {}, today: {})", data.valid_since, today);
        }

        duration > chrono::Duration::days(2)
    };

    tokio::fs::create_dir_all(mga_file_path().parent().unwrap()).await?;

    match cached_data {
        Some(data) if options.mga_offline || !out_of_date(&data) && !options.mga_force_update => {
            debug!("Using cached MGA data");
            Ok(data)
        }
        None if options.mga_offline => Err(anyhow!(
            "There is no cached MGA data yet, but mga-offline flag is set"
        )),
        _ => {
            debug!("Downloading new MGA data");
            let data = download_mga_data(config).await?;
            tokio::fs::write(mga_file_path(), &data.data)
                .await
                .context("Writing MGA data to cache")?;
            Ok(data)
        }
    }
}

pub async fn check_ublox_token(token: &str) -> Result<bool> {
    let result = download_mga_data(&MgaConfig {
        ublox_token: Some(token.to_string()),
        ..Default::default()
    })
    .await;

    match result {
        Ok(_) => Ok(true),
        Err(Error::BadToken) => Ok(false),
        Err(e) => Err(e).context("Using token to test-download the data")?,
    }
}
