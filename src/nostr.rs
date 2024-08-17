use std::collections::HashSet;
use std::sync::RwLock;
use std::time::Duration;

use anyhow::Result;
use log::{debug, error, info, trace, warn};
use nostr_sdk::prelude::*;
use tokio::sync::{broadcast::error::RecvError, mpsc::Sender};

const RELAYS: [&str; 9] = [
    "wss://relay.damus.io",
    "wss://nostr.plebchain.org/",
    "wss://bitcoiner.social/",
    "wss://relay.snort.social",
    "wss://relayable.org",
    "wss://nos.lol",
    "wss://nostr.mom",
    "wss://e.nos.lol",
    "wss://nostr.bitcoiner.social",
];

pub async fn get_client(ndb_path: &str) -> Result<Client> {
    debug!("Getting nostr client");
    let db = NdbDatabase::open(ndb_path)?;
    let client = Client::builder().database(db).build();
    // add reader relays
    for relay in RELAYS {
        client
            .add_relay_with_opts(relay, RelayOptions::default().write(false))
            .await?;
    }

    client.connect().await;
    debug!("Nostr client connected to relays");

    Ok(client)
}

fn pubkey_receives_filter(pubkey: PublicKey, event_npubs: Vec<PublicKey>) -> Vec<Filter> {
    vec![
        // DMs and zaps to our events
        Filter::new()
            .kinds([Kind::EncryptedDirectMessage, Kind::ZapReceipt])
            .pubkey(pubkey)
            .since(Timestamp::now()),
        // Events we wrote. This is used when validating responses to ensure
        // it is a direct response to our notes
        Filter::new()
            .kind(Kind::TextNote)
            .author(pubkey)
            .since(Timestamp::now() - Duration::from_secs(60 * 60 * 24 * 2)),
        // Events we are tagged in. This will be paired down to just responses
        // directly to notes authored by us
        Filter::new()
            .kind(Kind::TextNote)
            .pubkey(pubkey)
            .since(Timestamp::now()),
        // Live events from npubs we care about
        Filter::new()
            .kind(Kind::LiveEvent)
            .pubkeys(event_npubs)
            .since(Timestamp::now() - Duration::from_secs(60 * 60 * 24)),
    ]
}

pub async fn watch_pubkey_receives(
    client: Client,
    pubkey: PublicKey,
    event_npubs: Vec<PublicKey>,
    channel: Sender<Event>,
) -> Result<()> {
    let mut notifications = client.notifications();
    let db = client.database();

    let filters = pubkey_receives_filter(pubkey, event_npubs.clone());
    client.subscribe(filters, None).await?;

    let events_seen = RwLock::new(HashSet::new());

    info!("Starting pubkey monitor task.");
    loop {
        let (event, relay_url) = match notifications.recv().await {
            Ok(RelayPoolNotification::Event {
                event, relay_url, ..
            }) => (event, relay_url),
            Err(RecvError::Lagged(n)) => {
                warn!("Nostr server notifications lagged behind. Skipped {}", n);
                continue;
            }
            Err(RecvError::Closed) => {
                error!("Nostr notifications channel closed suddenly. Exiting pubkey monitor loop.");
                break;
            }
            Ok(RelayPoolNotification::Shutdown) => break,
            _ => continue,
        };

        trace!(
            "Received event from relay {}: {:?}",
            relay_url,
            event.as_json()
        );

        let incoming_id = event.id;
        match event.kind() {
            Kind::EncryptedDirectMessage | Kind::ZapReceipt => {
                if let Err(err) = channel.send(*event).await {
                    error!(
                        "Unable to send valid event {} on sender channel: {}",
                        incoming_id, err
                    );
                }
            }
            Kind::TextNote => {
                let event = event.clone();
                // We are assuming the first (and probably only) event in the tags
                // is the event being responded to.
                let Some(id) = event.event_ids().next() else {
                    trace!("No event ids found in event {}. Skipping.", event.id);
                    continue;
                };

                // Get the event out of the local ndb database. If it's not there, then we haven't seen it
                // yet. So we are just going to ignore it.
                let Ok(stored_event) = db.event_by_id(*id).await else {
                    trace!("Event {} in comment {} not found. Skipping.", id, event.id);
                    continue;
                };

                if stored_event.author() != pubkey {
                    continue;
                }

                // We wrote the initial note. So the incoming event is a comment
                // on our note. So we will notify.
                if let Err(err) = channel.send(*event).await {
                    error!(
                        "Unable to send valid event {} on sender channel: {}",
                        incoming_id, err
                    );
                }
            }
            Kind::LiveEvent => {
                let event_id = event.id();

                if events_seen.read().unwrap().contains(&event_id) {
                    continue;
                }

                events_seen.write().unwrap().insert(event_id);
                if let Err(err) = channel.send(*event).await {
                    error!(
                        "Unable to send valid event {} on sender channel: {}",
                        incoming_id, err
                    );
                }
            }
            _ => {}
        }
    }

    info!("Pubkey monitor task closed.");
    Ok(())
}

fn get_zap_request(event: &Event) -> Option<Event> {
    let Some(tag) = event
        .tags()
        .iter()
        .find(|t| t.kind() == TagKind::Description)
    else {
        debug!("no description tag found in event {}", event.id());
        return None;
    };

    let Ok(event) = Event::from_json(tag.content().unwrap()) else {
        debug!("description tag is not a valid event");
        return None;
    };
    if let Err(e) = event.verify() {
        debug!("invalid zap request event: {:?}", e);
        return None;
    }

    Some(event)
}

pub fn get_zap_request_amount(event: &Event) -> u32 {
    let Some(event) = get_zap_request(event) else {
        return 0;
    };

    let Some(tag) = event.tags().iter().find(|t| t.kind() == TagKind::Amount) else {
        debug!("no amount tag found in event {}", event.id());
        return 0;
    };

    tag.content()
        .unwrap_or_default()
        .parse()
        .unwrap_or_default()
}
