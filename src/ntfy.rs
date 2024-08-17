use std::time::Duration;

use anyhow::{bail, Result};
use humantime::format_duration;
use log::{debug, error, info};
use nostr_sdk::prelude::*;
use reqwest::header::{HeaderName, HeaderValue};
use tokio::select;
use tokio::sync::mpsc::{self, Receiver};
use tokio::time::sleep;

use crate::nostr::get_zap_request_amount;

const API_ENDPOINT: &str = "https://ntfy.sh";

const TITLE: HeaderName = HeaderName::from_static("x-title");
const PRIORITY: HeaderName = HeaderName::from_static("x-priority");
const TAGS: HeaderName = HeaderName::from_static("x-tags");
const CLICK: HeaderName = HeaderName::from_static("x-click");

const DM_TITLE: HeaderValue = HeaderValue::from_static("New DM Received");
const ZAPS_TITLE: HeaderValue = HeaderValue::from_static("Zaps Received");
const COMMENT_TITLE: HeaderValue = HeaderValue::from_static("Comment Received");
const EVENT_TITLE: HeaderValue = HeaderValue::from_static("Event announcement");

#[derive(Debug, Clone)]
pub struct NtfyApiClient {
    api: reqwest::Client,
    endpoint: String,
}

impl NtfyApiClient {
    pub fn new(api: reqwest::Client, topic: impl ToString) -> Self {
        Self {
            api,
            endpoint: format!("{}/{}", API_ENDPOINT, topic.to_string()),
        }
    }

    pub async fn send_dm_notification(&self) -> Result<()> {
        info!("Sending notification about DM");
        self.api
            .post(&self.endpoint)
            .header(TITLE, DM_TITLE)
            .header(PRIORITY, Priority::Default)
            .header(TAGS, "book")
            .body("You've received a new nostr DM.")
            .send()
            .await?;

        Ok(())
    }

    pub async fn send_zap_notification(&self, amount_ms: u32) -> Result<()> {
        let amount = amount_ms / 1_000;
        info!(
            "Sending notification about zaps with amount {} sats",
            amount
        );
        let message = format!("You've received {} sats in zaps on your post!", amount);

        self.api
            .post(&self.endpoint)
            .header(TITLE, ZAPS_TITLE)
            .header(PRIORITY, Priority::Default)
            .header(TAGS, "moneybag")
            .body(message)
            .send()
            .await?;

        Ok(())
    }

    pub async fn send_comment_notification(&self, event_id: EventId) -> Result<()> {
        let event_id = event_id.to_bech32().unwrap();
        info!("Sending notification about comment {}", event_id);
        let message = "You've received a comment on your post!".to_string();
        let uri = format!("nostr:{}", event_id);

        self.api
            .post(&self.endpoint)
            .header(TITLE, COMMENT_TITLE)
            .header(PRIORITY, Priority::Default)
            .header(TAGS, "incoming_envelope")
            .header(CLICK, uri)
            .body(message)
            .send()
            .await?;

        Ok(())
    }

    pub async fn send_event_notification(
        &self,
        event_id: EventId,
        event: &LiveEvent,
    ) -> Result<()> {
        let event_id = event_id.to_bech32().unwrap();
        let title = event.title.clone().unwrap_or(format!("Event {}", event_id));

        let starts_in = event.starts.unwrap_or_default() - Timestamp::now();
        let starts_in = Duration::from_secs(starts_in.as_u64());

        info!("Sending notification about live event {}", event_id);
        let message = format!(r#"{} starts in {}"#, title, format_duration(starts_in));
        let uri = format!("nostr:{}", event_id);

        self.api
            .post(&self.endpoint)
            .header(TITLE, EVENT_TITLE)
            .header(PRIORITY, Priority::Default)
            .header(TAGS, "spiral_calendar")
            .header(CLICK, uri)
            .body(message)
            .send()
            .await?;

        Ok(())
    }
}

pub enum Priority {
    Min = 1,
    Low = 2,
    Default = 3,
    High = 4,
    Max = 5,
}

impl std::fmt::Display for Priority {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Max => write!(f, "max"),
            Self::High => write!(f, "high"),
            Self::Default => write!(f, "default"),
            Self::Low => write!(f, "low"),
            Self::Min => write!(f, "min"),
        }
    }
}

impl TryFrom<Priority> for HeaderValue {
    type Error = http::Error;

    fn try_from(value: Priority) -> std::result::Result<Self, Self::Error> {
        let n = (value as u8).to_string();
        Ok(HeaderValue::from_str(&n)?)
    }
}

pub async fn send_ntfy_messages(client: NtfyApiClient, mut channel: Receiver<Event>) -> Result<()> {
    info!("Starting notifier loop.");
    let (sender, receiver) = mpsc::channel(100);
    tokio::spawn(aggregate_zaps(
        receiver,
        client.clone(),
        Duration::from_secs(2 * 60),
    ));

    while let Some(event) = channel.recv().await {
        debug!("Received event to notify about: {}", event.as_json());
        match event.kind() {
            Kind::EncryptedDirectMessage => {
                let _ = client.send_dm_notification().await;
            }
            Kind::ZapReceipt => {
                let amount = get_zap_request_amount(&event);
                let _ = sender.send(amount).await;
            }
            Kind::TextNote => {
                let _ = client.send_comment_notification(event.id).await;
            }
            Kind::LiveEvent => {
                tokio::spawn(notify_and_remind_event(client.clone(), event));
            }
            _ => {}
        }
    }

    info!("Notifier task complete");
    Ok(())
}

async fn aggregate_zaps(mut receiver: Receiver<u32>, client: NtfyApiClient, duration: Duration) {
    loop {
        let mut total = 0;
        let Some(amount) = receiver.recv().await else {
            return;
        };
        total += amount;
        debug!(
            "Initial zap received. Aggregating zaps for {}s",
            duration.as_secs()
        );

        loop {
            select! {
                _ = sleep(duration) => break,
                a = receiver.recv() => {
                    match a {
                        Some(v) => total += v,
                        None => return
                    }
                },
            }
        }

        info!(
            "Sending aggregated zap notification for amount {} millisats",
            total
        );
        let _ = client.send_zap_notification(total).await;
    }
}

async fn notify_and_remind_event(client: NtfyApiClient, event: Event) {
    let event_id = event.id();
    let live_event = match tags_to_live_event(event.tags().iter().map(Clone::clone).collect()) {
        Ok(event) => event,
        Err(err) => {
            error!("Unable to create a LiveEvent from the event: {}", err);
            return;
        }
    };

    let _ = client.send_event_notification(event_id, &live_event).await;

    if let Some(starts) = live_event.starts {
        // notify a half hour before the event starts
        let diff = starts - Timestamp::now() - (60 * 30);
        sleep(Duration::from_secs(diff.as_u64())).await;
        let _ = client.send_event_notification(event_id, &live_event).await;
    }
}

fn tags_to_live_event(tags: Vec<Tag>) -> Result<LiveEvent> {
    let id = match tags
        .iter()
        .find(|t| t.kind() == TagKind::SingleLetter(SingleLetterTag::lowercase(Alphabet::D)))
    {
        Some(tag) if tag.content().is_none() => bail!("'d' tag missing content"),
        Some(tag) => tag.content().map(String::from).unwrap(),
        None => bail!("'d' tag missing"),
    };
    let mut live_event = new_live_event(id);

    for tag in tags.into_iter() {
        let Some(tag) = tag.to_standardized() else {
            continue;
        };

        match tag {
            TagStandard::Title(title) => live_event.title = Some(title),
            TagStandard::Summary(summary) => live_event.summary = Some(summary),
            TagStandard::Streaming(url) => live_event.streaming = Some(url),
            TagStandard::LiveEventStatus(status) => live_event.status = Some(status),
            TagStandard::PublicKeyLiveEvent {
                public_key,
                relay_url,
                marker,
                proof,
            } => match marker {
                LiveEventMarker::Host => {
                    live_event.host = Some(LiveEventHost {
                        public_key,
                        relay_url,
                        proof,
                    })
                }
                LiveEventMarker::Speaker => live_event.speakers.push((public_key, relay_url)),
                LiveEventMarker::Participant => {
                    live_event.participants.push((public_key, relay_url))
                }
            },
            TagStandard::Image(image, dim) => live_event.image = Some((image, dim)),
            TagStandard::Hashtag(hashtag) => live_event.hashtags.push(hashtag),
            TagStandard::Recording(url) => live_event.recording = Some(url),
            TagStandard::Starts(starts) => live_event.starts = Some(starts),
            TagStandard::Ends(ends) => live_event.ends = Some(ends),
            TagStandard::CurrentParticipants(n) => live_event.current_participants = Some(n),
            TagStandard::TotalParticipants(n) => live_event.total_participants = Some(n),
            TagStandard::Relays(mut relays) => live_event.relays.append(&mut relays),
            _ => {}
        }
    }

    Ok(live_event)
}

fn new_live_event(id: String) -> LiveEvent {
    LiveEvent {
        id,
        title: None,
        summary: None,
        image: None,
        hashtags: Vec::new(),
        streaming: None,
        recording: None,
        starts: None,
        ends: None,
        status: None,
        current_participants: None,
        total_participants: None,
        relays: Vec::new(),
        host: None,
        speakers: Vec::new(),
        participants: Vec::new(),
    }
}
