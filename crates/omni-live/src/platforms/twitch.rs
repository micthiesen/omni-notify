//! Twitch live status over the public GQL endpoint.
//!
//! The username is escaped as a JSON string literal before interpolation into
//! the GraphQL document (the TS port interpolated it raw, a known defect).

use omni_http::public::PublicHttpClient;
use serde::Deserialize;

use super::{PLATFORM_GQL_MAX_BYTES, fetch_gql};
use crate::platform::{FetchedLive, FetchedStatus, js_count};

const TWITCH_GQL_URL: &str = "https://gql.twitch.tv/gql";
/// Public client ID used by Twitch's web player; no authentication needed.
const TWITCH_CLIENT_ID: &str = "kimne78kx3ncx6brgo4mv6wki5h1ko";

#[derive(Debug, Deserialize)]
pub struct TwitchGqlResponse {
    pub data: TwitchData,
}

#[derive(Debug, Deserialize)]
pub struct TwitchData {
    pub user: Option<TwitchUser>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TwitchUser {
    pub stream: Option<TwitchStream>,
    pub broadcast_settings: TwitchBroadcastSettings,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TwitchStream {
    pub title: String,
    pub viewers_count: f64,
    pub game: Option<TwitchGame>,
}

#[derive(Debug, Deserialize)]
pub struct TwitchGame {
    pub name: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TwitchBroadcastSettings {
    pub live_up_notification: Option<String>,
}

/// The GQL document for one login, with the login escaped as a JSON string.
pub fn twitch_query(username: &str) -> String {
    let login = serde_json::Value::String(username.to_owned()).to_string();
    format!(
        "query{{user(login:{login}){{stream{{title viewersCount game{{name}}}}broadcastSettings{{liveUpNotification}}}}}}"
    )
}

/// Pure: a decoded response as a status.
pub fn extract_twitch_status(response: &TwitchGqlResponse) -> FetchedStatus {
    let Some(user) = &response.data.user else {
        return FetchedStatus::Offline;
    };
    let Some(stream) = &user.stream else {
        return FetchedStatus::Offline;
    };
    // Prefer the custom go-live notification text over the stream title.
    let title = user
        .broadcast_settings
        .live_up_notification
        .clone()
        .filter(|text| !text.is_empty())
        .unwrap_or_else(|| stream.title.clone());
    FetchedStatus::Live(FetchedLive {
        title,
        viewer_count: js_count(stream.viewers_count),
        category: stream.game.as_ref().map(|g| g.name.clone()),
        started_at: None,
    })
}

#[derive(Clone)]
pub struct TwitchClient {
    http: PublicHttpClient,
}

impl TwitchClient {
    pub fn new(http: PublicHttpClient) -> Self {
        Self { http }
    }

    pub async fn fetch_live_status(&self, username: &str) -> FetchedStatus {
        match fetch_gql::<TwitchGqlResponse>(
            &self.http,
            TWITCH_GQL_URL,
            TWITCH_CLIENT_ID,
            &twitch_query(username),
            PLATFORM_GQL_MAX_BYTES,
        )
        .await
        {
            Ok(response) => extract_twitch_status(&response),
            Err(error) => FetchedStatus::unknown(error.message),
        }
    }
}
