//! Shared support for the omni-ios-controls tests.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::type_complexity)]
#![allow(dead_code)]

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use futures::future::BoxFuture;
use omni_api::ios::ApnsEnvironment;
use omni_core::clock::SharedClock;
use omni_ios_controls::apns::{ApnsPushResult, ApnsSender, ApnsTransportError};
use omni_ios_controls::persistence::{ControlInput, IosControlRegistration};
use omni_ios_controls::service::IosControlService;
use omni_live::{Roster, Streamer};

/// Throwaway P-256 key pair generated for these tests only.
pub const TEST_PRIVATE_KEY: &str = "-----BEGIN PRIVATE KEY-----
MIGHAgEAMBMGByqGSM49AgEGCCqGSM49AwEHBG0wawIBAQQgkhcCUMKG20YJq7q7
VDFnQusnXDDceipNoMn/vJvDi8GhRANCAASV9GYYZDxVf8qBZ8GIPYVqWvJkCFAT
IljJDRxUj5iASESMSA/bU79Q4mMQtT9/7RuAFb57gsPMsvyDEzmXdPoS
-----END PRIVATE KEY-----
";
pub const TEST_PUBLIC_KEY: &str = "-----BEGIN PUBLIC KEY-----
MFkwEwYHKoZIzj0CAQYIKoZIzj0DAQcDQgAElfRmGGQ8VX/KgWfBiD2FalryZAhQ
EyJYyQ0cVI+YgEhEjEgP21O/UOJjELU/f+0bgBW+e4LDzLL8gxM5l3T6Eg==
-----END PUBLIC KEY-----
";

pub type Reply = Result<ApnsPushResult, ApnsTransportError>;

/// Scripted APNs sender: replies in order, then repeats `default`.
pub struct FakeApns {
    pub script: Mutex<VecDeque<Reply>>,
    pub default: Mutex<Reply>,
    pub calls: Mutex<Vec<IosControlRegistration>>,
    /// When set, every send waits forever after recording the call.
    pub hang: bool,
}

impl FakeApns {
    pub fn new(default: Reply) -> Arc<Self> {
        Arc::new(Self {
            script: Mutex::default(),
            default: Mutex::new(default),
            calls: Mutex::default(),
            hang: false,
        })
    }

    pub fn hanging() -> Arc<Self> {
        Arc::new(Self {
            script: Mutex::default(),
            default: Mutex::new(Ok(ApnsPushResult::Sent)),
            calls: Mutex::default(),
            hang: true,
        })
    }

    pub fn then(self: &Arc<Self>, reply: Reply) -> Arc<Self> {
        self.script.lock().unwrap().push_back(reply);
        self.clone()
    }

    pub fn set_default(&self, reply: Reply) {
        *self.default.lock().unwrap() = reply;
    }

    pub fn calls(&self) -> usize {
        self.calls.lock().unwrap().len()
    }

    pub fn clear(&self) {
        self.calls.lock().unwrap().clear();
    }
}

impl ApnsSender for FakeApns {
    fn send_control_changed<'a>(
        &'a self,
        registration: &'a IosControlRegistration,
    ) -> BoxFuture<'a, Reply> {
        self.calls.lock().unwrap().push(registration.clone());
        let reply = self
            .script
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_else(|| self.default.lock().unwrap().clone());
        let hang = self.hang;
        Box::pin(async move {
            if hang {
                futures::future::pending::<()>().await;
            }
            reply
        })
    }
}

pub fn failed(status: u16, reason: &str) -> Reply {
    Ok(ApnsPushResult::Failed {
        status,
        reason: reason.into(),
    })
}

pub fn control(control_id: &str, slot: u8, token: &str) -> ControlInput {
    ControlInput {
        control_id: control_id.into(),
        slot,
        push_token: token.into(),
        environment: ApnsEnvironment::Sandbox,
    }
}

pub fn alpha() -> Streamer {
    Streamer::new(
        "alpha",
        "Alpha",
        vec![omni_live::PlatformBinding::new(
            omni_live::Platform::Twitch,
            "alpha",
        )],
        omni_api::streamers::StreamerTier::Primary,
    )
}

pub async fn store(epoch_ms: i64) -> (omni_testkit::TestStore, SharedClock) {
    let clock: SharedClock = omni_testkit::test_clock(epoch_ms);
    (omni_testkit::TestStore::new(clock.clone()).await, clock)
}

pub fn service(
    store: &omni_store::Store,
    clock: &SharedClock,
    streamers: Vec<Streamer>,
    apns: Option<Arc<FakeApns>>,
) -> IosControlService {
    IosControlService::new(
        store.clone(),
        Roster::new(streamers),
        "http://omni.boris",
        clock.clone(),
        apns.map(|a| a as Arc<dyn ApnsSender>),
    )
}
