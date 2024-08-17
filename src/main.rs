use anyhow::{bail, Result};
use config::{Case, Environment, File};
use log::{debug, info};
use nostr::watch_pubkey_receives;
use nostr_sdk::{Event, PublicKey};
use ntfy::{send_ntfy_messages, NtfyApiClient};
use qrcode::QrCode;
use serde::Deserialize;
use tokio::{
    fs::{create_dir_all, read_to_string, write},
    signal,
};
use tokio_util::task::TaskTracker;
use uuid::Uuid;

use crate::nostr::get_client;

mod nostr;
mod ntfy;

#[tokio::main]
async fn main() -> Result<()> {
    if let Err(e) = dotenvy::dotenv() {
        if !e.not_found() {
            bail!(e)
        }
    }
    env_logger::init();
    info!("Bullhorn process starting up.");

    let cfg = get_config().await?;
    debug!("config: {:?}", cfg);

    let topic = get_subscription_topic().await?;
    let nostr_client = get_client(&cfg.ndb_path).await?;
    let http_client = reqwest::Client::builder().build()?;

    display_subscription_qr(&topic.as_hyphenated().to_string());

    let ntfy_client = NtfyApiClient::new(http_client, topic);

    let (sender, receiver) = tokio::sync::mpsc::channel::<Event>(300);
    let tracker = TaskTracker::new();

    tracker.spawn(watch_pubkey_receives(
        nostr_client.clone(),
        cfg.npub,
        cfg.event_npubs,
        sender,
    ));
    tracker.spawn(send_ntfy_messages(ntfy_client, receiver));
    tracker.close();

    if let Err(err) = signal::ctrl_c().await {
        bail!("Unable to listen for shutdown signal: {}", err)
    }
    info!("Shutdown signal received. Shutting down.");

    nostr_client.shutdown().await?;
    debug!("Nostr client disconnected");
    tracker.wait().await;
    info!("Successfully shut down.");

    Ok(())
}

#[derive(Clone, Debug, Deserialize)]
struct Config {
    ndb_path: String,
    npub: PublicKey,
    event_npubs: Vec<PublicKey>,
}

async fn get_config() -> Result<Config> {
    let data_dir = dirs::data_dir().unwrap().join("bullhorn");
    let db_filepath = data_dir.join("nostr.db").into_os_string();
    let db_filepath = db_filepath.to_str().unwrap();

    let config_dir = dirs::config_dir().unwrap().join("bullhorn");
    let config_file = config_dir.join("config.toml");

    let cfg = config::Config::builder()
        .add_source(
            Environment::default()
                .prefix("bullhorn")
                .prefix_separator("_")
                .convert_case(Case::UpperSnake)
                .separator("__"),
        )
        .add_source(
            File::from(config_file)
                .required(false)
                .format(config::FileFormat::Toml),
        )
        .set_default("ndb_path", db_filepath)?
        .build()?;

    Ok(cfg.try_deserialize()?)
}

async fn get_subscription_topic() -> Result<Uuid> {
    let config_dir = dirs::config_dir().unwrap().join("bullhorn");
    create_dir_all(config_dir.clone()).await?;

    let filepath = config_dir.join("topic");
    if let Ok(contents) = read_to_string(&filepath).await {
        Ok(Uuid::parse_str(&contents)?)
    } else {
        let id = Uuid::new_v4();
        write(filepath, id.as_hyphenated().to_string()).await?;
        Ok(id)
    }
}

fn display_subscription_qr(topic: &str) {
    let code = QrCode::new(topic).unwrap();
    let string = code
        .render::<char>()
        .dark_color('#')
        .quiet_zone(false)
        .module_dimensions(2, 1)
        .build();

    println!("This is your subscription topic. Messages will be sent to this topic in ntfy.");
    println!();
    println!("{}", string);
    println!();
    println!("{}", topic);
    println!();
    println!("Load this into the ntfy app to receive push notifications.");
}
