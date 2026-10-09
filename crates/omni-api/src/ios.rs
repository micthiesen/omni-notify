//! iOS live controls: `GET /api/ios-controls/slots/:slot`,
//! `GET /api/ios-controls/diagnostics`, `PUT /api/ios-controls/registrations`.

use serde::{Deserialize, Serialize};

use crate::common::Ms;

/// Number of control slots the app can register.
pub const IOS_CONTROL_SLOT_COUNT: u8 = 4;

/// `"sandbox" | "production"`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ApnsEnvironment {
    Sandbox,
    Production,
}

/// The state one control slot shows (`LiveControlSlot`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveSlotState {
    pub slot: u8,
    pub is_live: bool,
    pub streamer_id: Option<String>,
    pub display_name: String,
    pub title: Option<String>,
    pub platform: Option<String>,
    pub url: String,
    pub viewer_count: Option<i64>,
    pub started_at: Option<Ms>,
    pub updated_at: Ms,
}

/// `GET /api/ios-controls/diagnostics` (non-secret).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IosDiagnostics {
    pub apns_enabled: bool,
    pub registration_count: u64,
    pub undelivered_count: u64,
    pub last_reconciled_at: Option<Ms>,
}

/// One control in a registration request.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ControlRegistration {
    pub control_id: String,
    pub slot: u8,
    pub push_token: String,
    pub environment: ApnsEnvironment,
}

/// `PUT /api/ios-controls/registrations` body: the device's complete control set.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RegistrationRequest {
    pub device_id: String,
    pub controls: Vec<ControlRegistration>,
}

/// `PUT /api/ios-controls/registrations` success body.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegistrationResponse {
    pub registered: u64,
}

/// Path builders for the iOS routes.
pub mod paths {
    pub const DIAGNOSTICS: &str = "/api/ios-controls/diagnostics";
    pub const REGISTRATIONS: &str = "/api/ios-controls/registrations";

    /// `GET /api/ios-controls/slots/:slot`.
    pub fn slot(slot: u8) -> String {
        format!("/api/ios-controls/slots/{slot}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn empty_slot_serializes_every_nullable_field() {
        let slot = LiveSlotState {
            slot: 4,
            is_live: false,
            streamer_id: None,
            display_name: "Nobody Live".into(),
            title: None,
            platform: None,
            url: "http://omni.boris".into(),
            viewer_count: None,
            started_at: None,
            updated_at: 2,
        };
        assert_eq!(
            serde_json::to_value(slot).unwrap(),
            json!({
                "slot": 4, "isLive": false, "streamerId": null, "displayName": "Nobody Live",
                "title": null, "platform": null, "url": "http://omni.boris",
                "viewerCount": null, "startedAt": null, "updatedAt": 2
            })
        );
    }

    #[test]
    fn registration_request_decodes_the_app_payload() {
        let request: RegistrationRequest = serde_json::from_value(json!({
            "deviceId": "device-12345",
            "controls": [{"controlId": "c", "slot": 1, "pushToken": "ab", "environment": "sandbox"}]
        }))
        .unwrap();
        assert_eq!(request.controls[0].environment, ApnsEnvironment::Sandbox);
    }
}
