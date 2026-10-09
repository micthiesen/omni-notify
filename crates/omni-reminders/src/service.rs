//! The serialized account owner (`src/reminders/service.ts`).
//!
//! One lock serializes every Apple and CloudKit exchange for the account. Mutations
//! reserve durably in the encrypted ledger before the external write and confirm
//! after fresh verification; a reserved (uncertain) operation is never replayed,
//! including after restart. Scheduled health checks never sign in, submit codes or
//! request device consent. Loss of access pauses Reminders only.

use std::sync::{Arc, Mutex, PoisonError, Weak};

use futures::future::BoxFuture;
use omni_api::reminders::{
    Diagnostic, DiagnosticCategory, DiagnosticStage, Phase, PublicStatus, Reason,
};
use omni_core::clock::SharedClock;
use serde::Serialize;
use serde_json::{Map, Value, json};
use tokio_util::task::TaskTracker;

use crate::apple::{
    AppleApi, AppleBeginResult, AppleErrorKind, AppleRemindersError, CkPath, SessionStorage,
};
use crate::cloudkit::{
    CkPost, ErrorCode, Reminder, ReminderCreateFields, ReminderPatch, RemindersCloudKitClient,
    RemindersError, RemindersSnapshot,
};
use crate::cloudkit_extras::{ReminderListDetails, ReminderRecurrences};
use crate::config::{RemindersConfiguration, reminders_configured};
use crate::protected_access::ProtectedAccess;
use crate::recurring_completion::RecurringCompletionTarget;
use crate::store::{
    OperationState, RemindersStore, StoredOperation, StoredState, empty_reminders_state,
};

const LOG: &str = "Reminders";
const CHALLENGE_MS: i64 = 10 * 60_000;
const MAX_OPERATIONS: usize = 10_000;

/// A service-level failure code.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ServiceErrorCode {
    Disabled,
    AuthenticationNeeded,
    AwaitingDeviceApproval,
    StaleChallenge,
    RateLimited,
    UncertainWrite,
    IdempotencyConflict,
    Conflict,
    NotFound,
    Synchronizing,
    Storage,
}

impl ServiceErrorCode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Disabled => "disabled",
            Self::AuthenticationNeeded => "authentication-needed",
            Self::AwaitingDeviceApproval => "awaiting-device-approval",
            Self::StaleChallenge => "stale-challenge",
            Self::RateLimited => "rate-limited",
            Self::UncertainWrite => "uncertain-write",
            Self::IdempotencyConflict => "idempotency-conflict",
            Self::Conflict => "conflict",
            Self::NotFound => "not-found",
            Self::Synchronizing => "synchronizing",
            Self::Storage => "storage",
        }
    }
}

/// `RemindersServiceError`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{}", self.message())]
pub struct RemindersServiceError {
    pub code: ServiceErrorCode,
}

impl RemindersServiceError {
    fn message(&self) -> String {
        match self.code {
            ServiceErrorCode::Synchronizing => "Reminders are synchronizing. Retry shortly.".into(),
            code => format!("Reminders: {}", code.as_str()),
        }
    }
}

/// Any failure a service call can surface.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ServiceError {
    #[error(transparent)]
    Service(#[from] RemindersServiceError),
    #[error(transparent)]
    Reminders(#[from] RemindersError),
    #[error(transparent)]
    Apple(#[from] AppleRemindersError),
}

impl ServiceError {
    /// The `code` a caller matches on (service, Reminders or Apple kind).
    pub fn code(&self) -> &'static str {
        match self {
            Self::Service(e) => e.code.as_str(),
            Self::Reminders(e) => e.code.as_str(),
            Self::Apple(_) => "apple",
        }
    }
}

fn service(code: ServiceErrorCode) -> ServiceError {
    ServiceError::Service(RemindersServiceError { code })
}

/// `JSON.stringify(canonical(value))` with keys sorted by `localeCompare`.
fn canonical(value: &Value) -> Value {
    match value {
        Value::Array(items) => Value::Array(items.iter().map(canonical).collect()),
        Value::Object(object) => {
            let mut keys: Vec<&String> = object.keys().collect();
            keys.sort_by(|a, b| omni_core::js::locale_compare(a, b));
            Value::Object(
                keys.into_iter()
                    .map(|k| (k.clone(), canonical(&object[k])))
                    .collect(),
            )
        }
        other => other.clone(),
    }
}

/// The ledger fingerprint (sha256 hex) of any JSON value.
pub fn fingerprint(value: &Value) -> String {
    omni_core::digest::sha256_hex(omni_core::js::json_stringify(&canonical(value)))
}

fn derived_id(prefix: &str, key: &str) -> String {
    let digest = fingerprint(&Value::String(key.to_owned()));
    format!("{prefix}/{}", digest[..32].to_uppercase())
}

fn valid_key(key: &str) -> bool {
    (16..=128).contains(&key.len())
        && key
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// `appleDiagnostic(error)`.
pub fn apple_diagnostic(error: &AppleRemindersError) -> Diagnostic {
    let stage = match error.operation.as_str() {
        "SRP init" => DiagnosticStage::SignInInit,
        "SRP proof" => DiagnosticStage::SignInProof,
        "SRP complete" => DiagnosticStage::SignInComplete,
        "account login" | "validate session" => DiagnosticStage::AccountSession,
        "MFA options" => DiagnosticStage::SecondFactorOptions,
        "MFA push" => DiagnosticStage::DeviceNotification,
        "MFA verify" => DiagnosticStage::CodeVerification,
        "SMS request" | "trust browser" => DiagnosticStage::SecondFactor,
        "load session" | "save session" => DiagnosticStage::PrivateStorage,
        "PCS state" | "PCS consent" | "PCS cookies" => DiagnosticStage::ProtectedDataAccess,
        _ => DiagnosticStage::AppleRequest,
    };
    let http_status = error.status.filter(|s| (100..=599).contains(s));
    let category = if stage == DiagnosticStage::PrivateStorage {
        DiagnosticCategory::Storage
    } else if http_status.is_some() {
        DiagnosticCategory::AppleResponse
    } else {
        match error.kind {
            AppleErrorKind::UnsupportedProtocol => DiagnosticCategory::Protocol,
            AppleErrorKind::TransientOutage => DiagnosticCategory::Transport,
            _ => DiagnosticCategory::Authentication,
        }
    };
    Diagnostic {
        stage,
        category,
        http_status,
    }
}

fn phase_of(kind: AppleErrorKind) -> Phase {
    match kind {
        AppleErrorKind::AuthenticationNeeded => Phase::AuthenticationNeeded,
        AppleErrorKind::TransientOutage => Phase::TransientOutage,
        AppleErrorKind::RateLimited => Phase::RateLimited,
        AppleErrorKind::UnsupportedProtocol => Phase::UnsupportedProtocol,
        AppleErrorKind::AwaitingDeviceApproval => Phase::AwaitingDeviceApproval,
        AppleErrorKind::TermsRequired => Phase::TermsRequired,
    }
}

/// Delivers the one "needs attention" notification (errors are swallowed).
pub type Notifier = Arc<dyn Fn() -> BoxFuture<'static, Result<(), ()>> + Send + Sync>;
/// Logs a bounded diagnostic.
pub type FailureLogger = Arc<dyn Fn(&Diagnostic) + Send + Sync>;
/// Runs work in the application's lifetime, never a request's.
pub type Background = Arc<dyn Fn(BoxFuture<'static, ()>) + Send + Sync>;

/// Builds the Apple client over the service's session storage.
pub type AppleFactory = Box<dyn FnOnce(Arc<dyn SessionStorage>) -> Arc<dyn AppleApi> + Send>;

/// Collaborators. `store`/`apple` default to the encrypted file store and the real
/// Apple client when a [`RemindersService::from_parts`] caller supplies them.
pub struct ServiceDeps {
    pub store: Arc<dyn RemindersStore>,
    /// Built with the service's session storage (the encrypted state).
    pub apple: AppleFactory,
    pub notify: Notifier,
    pub log_failure: Option<FailureLogger>,
    pub background: Option<Background>,
    /// Mutations and saves run to completion on this tracker when present.
    pub tracker: Option<TaskTracker>,
    pub clock: SharedClock,
}

/// The decrypted state, loaded once and saved in order.
struct PrivateState {
    store: Arc<dyn RemindersStore>,
    cell: Mutex<(StoredState, bool)>,
    write_lock: tokio::sync::Mutex<()>,
    tracker: Option<TaskTracker>,
}

impl PrivateState {
    fn get<R>(&self, f: impl FnOnce(&StoredState) -> R) -> R {
        let guard = self.cell.lock().unwrap_or_else(PoisonError::into_inner);
        f(&guard.0)
    }

    fn update(&self, f: impl FnOnce(&mut StoredState)) {
        let mut guard = self.cell.lock().unwrap_or_else(PoisonError::into_inner);
        f(&mut guard.0);
    }

    async fn load(&self) -> Result<(), ServiceError> {
        if self.cell.lock().unwrap_or_else(PoisonError::into_inner).1 {
            return Ok(());
        }
        let state = self
            .store
            .read()
            .await
            .map_err(|_| service(ServiceErrorCode::Storage))?;
        let mut guard = self.cell.lock().unwrap_or_else(PoisonError::into_inner);
        if !guard.1 {
            *guard = (state, true);
        }
        Ok(())
    }

    /// Writes the latest state; uninterruptible and ordered.
    async fn save(self: &Arc<Self>) -> Result<(), ServiceError> {
        let this = self.clone();
        let write = async move {
            let _ordered = this.write_lock.lock().await;
            let snapshot = this.get(Clone::clone);
            this.store.write(snapshot).await
        };
        let result = match &self.tracker {
            Some(tracker) => omni_core::spawn::must_complete(tracker, write).await,
            None => write.await,
        };
        result.map_err(|_| service(ServiceErrorCode::Storage))
    }
}

struct PrivateSession(Arc<PrivateState>);

impl SessionStorage for PrivateSession {
    fn load(&self) -> BoxFuture<'_, Result<Value, ()>> {
        Box::pin(async move {
            self.0.load().await.map_err(|_| ())?;
            Ok(self.0.get(|s| s.session.clone()))
        })
    }

    fn save(&self, session: Value) -> BoxFuture<'_, Result<(), ()>> {
        Box::pin(async move {
            self.0.update(|s| s.session = session);
            self.0.save().await.map_err(|_| ())
        })
    }
}

#[derive(Clone, Debug)]
struct Challenge {
    id: String,
    expires: i64,
    attempts: u32,
}

struct Mutable {
    current: PublicStatus,
    challenge: Option<Challenge>,
    next_auth_at: i64,
    indexing: bool,
}

struct Enabled {
    private: Arc<PrivateState>,
    apple: Arc<dyn AppleApi>,
    cloud: Arc<RemindersCloudKitClient>,
}

struct Inner {
    enabled: Option<Enabled>,
    lock: tokio::sync::Mutex<()>,
    state: Mutex<Mutable>,
    notify: Notifier,
    log_failure: Option<FailureLogger>,
    background: Option<Background>,
    tracker: Option<TaskTracker>,
    clock: SharedClock,
}

/// A failure fed to `recordFailure`.
enum Recorded<'a> {
    Apple(&'a AppleRemindersError),
    Reminders(&'a RemindersError),
    Service(&'a RemindersServiceError),
}

impl<'a> From<&'a ServiceError> for Recorded<'a> {
    fn from(error: &'a ServiceError) -> Self {
        match error {
            ServiceError::Apple(e) => Self::Apple(e),
            ServiceError::Reminders(e) => Self::Reminders(e),
            ServiceError::Service(e) => Self::Service(e),
        }
    }
}

/// Routes CloudKit calls through the Apple client and records Apple failures.
struct ServicePost {
    apple: Arc<dyn AppleApi>,
    inner: Weak<Inner>,
}

impl CkPost for ServicePost {
    fn post(&self, path: &str, body: Value) -> BoxFuture<'static, Result<Value, RemindersError>> {
        let apple = self.apple.clone();
        let inner = self.inner.clone();
        let path = CkPath::parse(path);
        Box::pin(async move {
            let transport = || RemindersError {
                operation: "request".into(),
                code: ErrorCode::Transport,
            };
            let Some(path) = path else {
                return Err(transport());
            };
            match apple.ck_post(path, body).await {
                Ok(value) => Ok(value),
                Err(error) => {
                    if let Some(inner) = inner.upgrade() {
                        // Every Apple failure surfaces as a transport error here.
                        let _recorded = RemindersService(inner)
                            .record_failure(Recorded::Apple(&error))
                            .await;
                    }
                    Err(transport())
                }
            }
        })
    }
}

/// The Reminders account owner; cheap to clone.
#[derive(Clone)]
pub struct RemindersService(Arc<Inner>);

impl RemindersService {
    /// Builds the service; incomplete configuration yields a disabled service that
    /// never touches storage or the network.
    pub fn new(config: &RemindersConfiguration, deps: ServiceDeps) -> Self {
        let enabled = reminders_configured(config);
        let current = if enabled {
            PublicStatus::new(true, Phase::AuthenticationNeeded).with_reason(Reason::Credentials)
        } else {
            PublicStatus::new(false, Phase::Disabled).with_reason(Reason::Configuration)
        };
        let ServiceDeps {
            store,
            apple,
            notify,
            log_failure,
            background,
            tracker,
            clock,
        } = deps;
        let inner = Arc::new_cyclic(|weak: &Weak<Inner>| {
            let enabled = enabled.then(|| {
                let private = Arc::new(PrivateState {
                    store,
                    cell: Mutex::new((empty_reminders_state(), false)),
                    write_lock: tokio::sync::Mutex::new(()),
                    tracker: tracker.clone(),
                });
                let apple = apple(Arc::new(PrivateSession(private.clone())));
                let cloud = Arc::new(RemindersCloudKitClient::new(
                    Arc::new(ServicePost {
                        apple: apple.clone(),
                        inner: weak.clone(),
                    }),
                    clock.clone(),
                ));
                Enabled {
                    private,
                    apple,
                    cloud,
                }
            });
            Inner {
                enabled,
                lock: tokio::sync::Mutex::new(()),
                state: Mutex::new(Mutable {
                    current,
                    challenge: None,
                    next_auth_at: 0,
                    indexing: false,
                }),
                notify,
                log_failure,
                background,
                tracker,
                clock,
            }
        });
        Self(inner)
    }

    fn state<R>(&self, f: impl FnOnce(&mut Mutable) -> R) -> R {
        let mut guard = self.0.state.lock().unwrap_or_else(PoisonError::into_inner);
        f(&mut guard)
    }

    fn phase(&self) -> Phase {
        self.state(|s| s.current.phase)
    }

    fn set_current(&self, status: PublicStatus) {
        self.state(|s| s.current = status);
    }

    fn enabled(&self) -> Result<&Enabled, ServiceError> {
        self.0
            .enabled
            .as_ref()
            .ok_or_else(|| service(ServiceErrorCode::Disabled))
    }

    pub fn is_enabled(&self) -> bool {
        self.0.enabled.is_some()
    }

    async fn load(&self) -> Result<&Enabled, ServiceError> {
        let enabled = self.enabled()?;
        enabled.private.load().await?;
        Ok(enabled)
    }

    async fn save(&self) -> Result<(), ServiceError> {
        self.enabled()?.private.save().await
    }

    fn log(&self, diagnostic: &Diagnostic) {
        if let Some(log) = &self.0.log_failure {
            log(diagnostic);
        }
    }

    /// Reserves the notification durably before delivery; uncertain delivery is not
    /// repeated. A failed reservation surfaces as a storage error (as in TS).
    async fn notify_once(&self) -> Result<(), ServiceError> {
        let enabled = self.enabled()?;
        if enabled.private.get(|s| s.notified) {
            return Ok(());
        }
        enabled.private.update(|s| s.notified = true);
        self.save().await?;
        let _ignored = (self.0.notify)().await;
        Ok(())
    }

    /// `recordFailure`: updates the public status. Like the TS `tapError`, a failure
    /// to reserve the notification replaces the original error.
    async fn record_failure(&self, error: Recorded<'_>) -> Result<(), ServiceError> {
        match error {
            Recorded::Apple(error) => {
                let diagnostic = apple_diagnostic(error);
                self.set_current(PublicStatus {
                    diagnostic: Some(diagnostic),
                    ..PublicStatus::new(true, phase_of(error.kind))
                });
                self.log(&diagnostic);
                if matches!(
                    error.kind,
                    AppleErrorKind::AuthenticationNeeded
                        | AppleErrorKind::AwaitingDeviceApproval
                        | AppleErrorKind::TermsRequired
                ) {
                    self.notify_once().await?;
                }
            }
            Recorded::Reminders(error) if error.code == ErrorCode::AwaitingDeviceApproval => {
                self.state(|s| {
                    s.challenge = None;
                    s.current = PublicStatus::new(true, Phase::AwaitingDeviceApproval)
                        .with_reason(Reason::Pcs);
                });
                self.notify_once().await?;
            }
            Recorded::Reminders(error) if error.code == ErrorCode::Protocol => {
                let diagnostic = Diagnostic {
                    stage: DiagnosticStage::AppleRequest,
                    category: DiagnosticCategory::Protocol,
                    http_status: None,
                };
                self.set_current(PublicStatus {
                    diagnostic: Some(diagnostic),
                    ..PublicStatus::new(true, Phase::UnsupportedProtocol)
                        .with_reason(Reason::Protocol)
                });
                self.log(&diagnostic);
                self.notify_once().await?;
            }
            Recorded::Service(error) if error.code == ServiceErrorCode::Storage => {
                let diagnostic = Diagnostic {
                    stage: DiagnosticStage::PrivateStorage,
                    category: DiagnosticCategory::Storage,
                    http_status: None,
                };
                self.set_current(PublicStatus {
                    diagnostic: Some(diagnostic),
                    ..PublicStatus::new(self.is_enabled(), Phase::TransientOutage)
                });
                self.log(&diagnostic);
            }
            Recorded::Service(error) if error.code == ServiceErrorCode::RateLimited => {
                self.state(|s| s.current.phase = Phase::RateLimited);
            }
            _ => {}
        }
        Ok(())
    }

    async fn tap<T>(&self, result: Result<T, ServiceError>) -> Result<T, ServiceError> {
        if let Err(error) = &result {
            self.record_failure(error.into()).await?;
        }
        result
    }

    /// The public status, including an active challenge handle.
    pub fn status(&self) -> PublicStatus {
        let now = self.0.clock.now_ms();
        self.state(|s| {
            let mut status = s.current.clone();
            if let Some(challenge) = s.challenge.as_ref().filter(|c| c.expires > now) {
                status.challenge_id = Some(challenge.id.clone());
                status.challenge_expires_at = Some(challenge.expires);
            }
            status
        })
    }

    async fn authenticated(&self) -> Result<PublicStatus, ServiceError> {
        let enabled = self.enabled()?;
        if self.phase() == Phase::UnsupportedProtocol {
            enabled.cloud.invalidate_snapshot();
        }
        // Decode protected content before claiming access.
        enabled.cloud.verify_read_access().await?;
        self.state(|s| {
            s.challenge = None;
            s.current = PublicStatus::new(true, Phase::Authenticated);
        });
        enabled.private.update(|s| s.notified = false);
        self.save().await?;
        self.start_indexing();
        Ok(self.status())
    }

    async fn check_access(&self, explicit: bool) -> Result<PublicStatus, ServiceError> {
        let enabled = self.load().await?;
        if !enabled.apple.verify().await? {
            self.set_current(
                PublicStatus::new(true, Phase::AuthenticationNeeded)
                    .with_reason(Reason::SessionExpired),
            );
            self.notify_once().await?;
            return Ok(self.status());
        }
        if explicit && enabled.apple.request_pcs_access().await? == ProtectedAccess::ConsentRequired
        {
            self.set_current(
                PublicStatus::new(true, Phase::AwaitingDeviceApproval).with_reason(Reason::Pcs),
            );
            self.notify_once().await?;
            return Ok(self.status());
        }
        self.authenticated().await
    }

    /// Explicit access check (the page's "Check access"). Never fails.
    pub async fn verify_access(&self) -> PublicStatus {
        if self.state(|s| s.indexing) && self.phase() == Phase::Authenticated {
            return self.status();
        }
        let _guard = self.0.lock.lock().await;
        let result = self.check_access(true).await;
        let _ = self.tap(result).await;
        self.status()
    }

    /// Scheduled validation: never signs in, submits codes, or requests device consent.
    pub async fn health_check(&self) {
        if !self.is_enabled() {
            return;
        }
        let _guard = self.0.lock.lock().await;
        let now = self.0.clock.now_ms();
        let skip = self.state(|s| {
            if s.challenge.as_ref().is_some_and(|c| c.expires <= now) {
                s.challenge = None;
            }
            s.challenge.is_some()
                || matches!(
                    s.current.phase,
                    Phase::AwaitingDeviceApproval | Phase::TermsRequired
                )
        });
        if skip {
            return;
        }
        let result = self.check_access(false).await;
        let _ = self.tap(result).await;
    }

    /// Starts an explicit sign-in (reuses an active challenge; 10-minute cooldown).
    pub async fn start_authentication(&self) -> Result<PublicStatus, ServiceError> {
        let _guard = self.0.lock.lock().await;
        let result = self.start_authentication_locked().await;
        self.tap(result).await
    }

    async fn start_authentication_locked(&self) -> Result<PublicStatus, ServiceError> {
        let enabled = self.load().await?;
        let now = self.0.clock.now_ms();
        let active = self.state(|s| s.challenge.as_ref().is_some_and(|c| now < c.expires));
        if active || self.phase() == Phase::Authenticated {
            return Ok(self.status());
        }
        if now < self.state(|s| s.next_auth_at) {
            return Err(service(ServiceErrorCode::RateLimited));
        }
        self.state(|s| {
            s.next_auth_at = now + CHALLENGE_MS;
            s.challenge = None;
        });
        if enabled.apple.begin().await? == AppleBeginResult::Ready {
            return self.check_access(true).await;
        }
        self.state(|s| {
            s.challenge = Some(Challenge {
                id: omni_core::ids::uuid_v4(),
                expires: now + CHALLENGE_MS,
                attempts: 0,
            });
            s.current =
                PublicStatus::new(true, Phase::AuthenticationNeeded).with_reason(Reason::Mfa);
        });
        self.notify_once().await?;
        Ok(self.status())
    }

    /// Submits the six-digit code for the active challenge (at most five attempts).
    pub async fn submit_code(
        &self,
        challenge_id: &str,
        code: &str,
    ) -> Result<PublicStatus, ServiceError> {
        let _guard = self.0.lock.lock().await;
        let result = self.submit_code_locked(challenge_id, code).await;
        self.tap(result).await
    }

    async fn submit_code_locked(
        &self,
        challenge_id: &str,
        code: &str,
    ) -> Result<PublicStatus, ServiceError> {
        let enabled = self.load().await?;
        let now = self.0.clock.now_ms();
        let six_digits = code.len() == 6 && code.bytes().all(|b| b.is_ascii_digit());
        let attempt = self.state(|s| match s.challenge.as_mut() {
            Some(c) if c.id == challenge_id && c.expires > now && six_digits => {
                if c.attempts >= 5 {
                    Err(service(ServiceErrorCode::RateLimited))
                } else {
                    c.attempts += 1;
                    Ok(())
                }
            }
            _ => Err(service(ServiceErrorCode::StaleChallenge)),
        });
        attempt?;
        enabled.apple.submit_2fa(code).await?;
        // A verified code is consumed even if CloudKit is unavailable.
        self.state(|s| s.challenge = None);
        match self.check_access(true).await {
            Err(ServiceError::Reminders(error))
                if error.code == ErrorCode::AwaitingDeviceApproval =>
            {
                self.record_failure(Recorded::Reminders(&error)).await?;
                Ok(self.status())
            }
            other => other,
        }
    }

    async fn ready(&self) -> Result<&Enabled, ServiceError> {
        let enabled = self.load().await?;
        match self.phase() {
            Phase::AwaitingDeviceApproval => Err(service(ServiceErrorCode::AwaitingDeviceApproval)),
            Phase::Authenticated => Ok(enabled),
            _ => Err(service(ServiceErrorCode::AuthenticationNeeded)),
        }
    }

    /// Starts the initial complete snapshot in the application's lifetime.
    fn start_indexing(&self) {
        let Some(background) = self.0.background.clone() else {
            return;
        };
        let Ok(enabled) = self.enabled() else {
            return;
        };
        let start = self.state(|s| {
            if s.indexing || enabled.cloud.has_snapshot() {
                false
            } else {
                s.indexing = true;
                true
            }
        });
        if !start {
            return;
        }
        let this = self.clone();
        background(Box::pin(async move {
            let result = async {
                let _guard = this.0.lock.lock().await;
                let enabled = this.ready().await?;
                enabled.cloud.read_snapshot().await?;
                if enabled.cloud.has_snapshot() {
                    Ok(())
                } else {
                    Err(ServiceError::Reminders(crate::cloudkit::protocol(
                        "snapshot cursor",
                    )))
                }
            }
            .await;
            let _ = this.tap(result).await;
            this.state(|s| s.indexing = false);
        }));
    }

    fn synchronizing(&self) -> bool {
        self.phase() == Phase::Authenticated
            && self.0.background.is_some()
            && (self.state(|s| s.indexing) || self.enabled().is_ok_and(|e| !e.cloud.has_snapshot()))
    }

    /// Runs `work` under the account lock unless the initial snapshot is loading.
    async fn with_data_lock<T, F>(&self, work: impl FnOnce(Self) -> F) -> Result<T, ServiceError>
    where
        F: std::future::Future<Output = Result<T, ServiceError>>,
    {
        if self.synchronizing() {
            self.start_indexing();
            return Err(service(ServiceErrorCode::Synchronizing));
        }
        let _guard = self.0.lock.lock().await;
        if self.synchronizing() {
            self.start_indexing();
            return Err(service(ServiceErrorCode::Synchronizing));
        }
        work(self.clone()).await
    }

    /// The complete snapshot (incremental after the first).
    pub async fn snapshot(&self) -> Result<RemindersSnapshot, ServiceError> {
        self.with_data_lock(|this| async move {
            let result = async {
                let enabled = this.ready().await?;
                Ok(enabled.cloud.read_snapshot().await?)
            }
            .await;
            this.tap(result).await
        })
        .await
    }

    /// One reminder by exact id.
    pub async fn get(&self, id: &str) -> Result<Option<Reminder>, ServiceError> {
        self.with_data_lock(|this| async move {
            let result = async {
                let enabled = this.ready().await?;
                Ok(enabled.cloud.get_reminder(id).await?)
            }
            .await;
            this.tap(result).await
        })
        .await
    }

    pub async fn get_list(&self, id: &str) -> Result<Option<ReminderListDetails>, ServiceError> {
        self.with_data_lock(|this| async move {
            let enabled = this.ready().await?;
            Ok(enabled.cloud.extras.get_list(id).await?)
        })
        .await
    }

    pub async fn get_recurrences(&self, id: &str) -> Result<ReminderRecurrences, ServiceError> {
        self.with_data_lock(|this| async move {
            let enabled = this.ready().await?;
            Ok(enabled.cloud.extras.get_recurrences(id).await?)
        })
        .await
    }

    /// Reserve, run once, confirm. A reserved entry means the outcome is unknown and
    /// the same key returns `uncertain-write` forever; a confirmed entry returns its
    /// original result. The sequence runs to completion even if the caller is dropped.
    async fn mutation<F>(
        &self,
        key: &str,
        input: Value,
        record_id: String,
        run: F,
    ) -> Result<Value, ServiceError>
    where
        F: FnOnce(Arc<RemindersCloudKitClient>) -> BoxFuture<'static, Result<Value, ServiceError>>
            + Send
            + 'static,
    {
        let this = self.clone();
        let key = key.to_owned();
        let work = async move {
            this.with_data_lock(|this| async move {
                let enabled = this.ready().await?;
                if !valid_key(&key) {
                    return Err(service(ServiceErrorCode::IdempotencyConflict));
                }
                let key_hash = fingerprint(&Value::String(key));
                let digest = fingerprint(&input);
                let existing = enabled
                    .private
                    .get(|s| s.operations.get(&key_hash).cloned());
                if let Some(existing) = existing {
                    if existing.fingerprint != digest {
                        return Err(service(ServiceErrorCode::IdempotencyConflict));
                    }
                    return match existing.state {
                        OperationState::Reserved => Err(service(ServiceErrorCode::UncertainWrite)),
                        OperationState::Confirmed => Ok(existing.result.unwrap_or(Value::Null)),
                    };
                }
                if enabled.private.get(|s| s.operations.len()) >= MAX_OPERATIONS {
                    return Err(service(ServiceErrorCode::Storage));
                }
                enabled.private.update(|s| {
                    s.operations.insert(
                        key_hash.clone(),
                        StoredOperation {
                            fingerprint: digest.clone(),
                            record_id: record_id.clone(),
                            state: OperationState::Reserved,
                            result: None,
                        },
                    );
                });
                this.save().await?;
                tracing::info!(target: LOG, record = %record_id, "Reserved Reminders mutation");
                let result = run(enabled.cloud.clone()).await?;
                enabled.private.update(|s| {
                    s.operations.insert(
                        key_hash,
                        StoredOperation {
                            fingerprint: digest,
                            record_id,
                            state: OperationState::Confirmed,
                            result: Some(result.clone()),
                        },
                    );
                });
                this.save().await?;
                Ok(result)
            })
            .await
        };
        match &self.0.tracker {
            Some(tracker) => omni_core::spawn::must_complete(tracker, work).await,
            None => work.await,
        }
    }

    pub async fn create(
        &self,
        key: &str,
        input: ReminderCreateFields,
    ) -> Result<Value, ServiceError> {
        let id = derived_id("Reminder", key);
        let mut fingerprinted = Map::new();
        fingerprinted.insert("operation".into(), json!("create"));
        if let Value::Object(fields) = serde_json::to_value(&input).unwrap_or(Value::Null) {
            fingerprinted.extend(fields);
        }
        let record = id.clone();
        self.mutation(key, Value::Object(fingerprinted), id, move |cloud| {
            Box::pin(async move {
                let reminder = cloud.create_reminder(&record, &input).await?;
                Ok(to_value(&reminder))
            })
        })
        .await
    }

    pub async fn update(
        &self,
        key: &str,
        id: &str,
        change_tag: &str,
        patch: ReminderPatch,
    ) -> Result<Value, ServiceError> {
        let input = json!({
            "operation": "update",
            "id": id,
            "changeTag": change_tag,
            "patch": serde_json::to_value(&patch).unwrap_or(Value::Null),
        });
        let (rid, tag) = (id.to_owned(), change_tag.to_owned());
        self.mutation(key, input, id.to_owned(), move |cloud| {
            Box::pin(async move {
                let Some(current) = cloud.get_reminder(&rid).await? else {
                    return Err(service(ServiceErrorCode::NotFound));
                };
                if current.record_change_tag != tag {
                    return Err(service(ServiceErrorCode::Conflict));
                }
                Ok(to_value(&cloud.update_reminder(&current, &patch).await?))
            })
        })
        .await
    }

    pub async fn delete(
        &self,
        key: &str,
        id: &str,
        change_tag: &str,
    ) -> Result<Value, ServiceError> {
        let input = json!({"operation": "delete", "id": id, "changeTag": change_tag});
        let (rid, tag) = (id.to_owned(), change_tag.to_owned());
        self.mutation(key, input, id.to_owned(), move |cloud| {
            Box::pin(async move {
                let Some(current) = cloud.get_reminder(&rid).await? else {
                    return Err(service(ServiceErrorCode::NotFound));
                };
                if current.record_change_tag != tag {
                    return Err(service(ServiceErrorCode::Conflict));
                }
                cloud.delete_reminder(&current).await?;
                Ok(json!({"id": rid, "deleted": true, "verified": true}))
            })
        })
        .await
    }

    pub async fn update_list(
        &self,
        key: &str,
        id: &str,
        change_tag: &str,
        title: &str,
    ) -> Result<Value, ServiceError> {
        let input =
            json!({"operation": "update-list", "id": id, "changeTag": change_tag, "title": title});
        let (rid, tag, title) = (id.to_owned(), change_tag.to_owned(), title.to_owned());
        self.mutation(key, input, id.to_owned(), move |cloud| {
            Box::pin(async move {
                Ok(to_value(
                    &cloud.extras.update_list(&rid, &tag, &title).await?,
                ))
            })
        })
        .await
    }

    pub async fn create_recurrence(
        &self,
        key: &str,
        id: &str,
        change_tag: &str,
        rule: Value,
    ) -> Result<Value, ServiceError> {
        let rule_id = derived_id("RecurrenceRule", key);
        let input = json!({"operation": "create-recurrence", "id": id, "changeTag": change_tag, "rule": rule});
        let (rid, tag, rule_record) = (id.to_owned(), change_tag.to_owned(), rule_id.clone());
        self.mutation(key, input, rule_id, move |cloud| {
            Box::pin(async move {
                Ok(to_value(
                    &cloud
                        .extras
                        .create_recurrence(&rid, &tag, &rule_record, &rule)
                        .await?,
                ))
            })
        })
        .await
    }

    pub async fn update_recurrence(
        &self,
        key: &str,
        id: &str,
        change_tag: &str,
        rule_id: &str,
        rule_change_tag: &str,
        patch: Map<String, Value>,
    ) -> Result<Value, ServiceError> {
        let input = json!({
            "operation": "update-recurrence",
            "id": id,
            "changeTag": change_tag,
            "ruleId": rule_id,
            "ruleChangeTag": rule_change_tag,
            "patch": patch,
        });
        let args = (
            id.to_owned(),
            change_tag.to_owned(),
            rule_id.to_owned(),
            rule_change_tag.to_owned(),
        );
        self.mutation(key, input, rule_id.to_owned(), move |cloud| {
            Box::pin(async move {
                let (id, tag, rule, rule_tag) = args;
                Ok(to_value(
                    &cloud
                        .extras
                        .update_recurrence(&id, &tag, &rule, &rule_tag, &patch)
                        .await?,
                ))
            })
        })
        .await
    }

    pub async fn remove_recurrence(
        &self,
        key: &str,
        id: &str,
        change_tag: &str,
        rule_id: &str,
        rule_change_tag: &str,
    ) -> Result<Value, ServiceError> {
        let input = json!({
            "operation": "remove-recurrence",
            "id": id,
            "changeTag": change_tag,
            "ruleId": rule_id,
            "ruleChangeTag": rule_change_tag,
        });
        let args = (
            id.to_owned(),
            change_tag.to_owned(),
            rule_id.to_owned(),
            rule_change_tag.to_owned(),
        );
        self.mutation(key, input, rule_id.to_owned(), move |cloud| {
            Box::pin(async move {
                let (id, tag, rule, rule_tag) = args;
                Ok(to_value(
                    &cloud
                        .extras
                        .remove_recurrence(&id, &tag, &rule, &rule_tag)
                        .await?,
                ))
            })
        })
        .await
    }

    pub async fn complete_recurring(
        &self,
        key: &str,
        target: RecurringCompletionTarget,
    ) -> Result<Value, ServiceError> {
        let mut input = Map::new();
        input.insert("operation".into(), json!("complete-recurring"));
        if let Value::Object(fields) = serde_json::to_value(&target).unwrap_or(Value::Null) {
            input.extend(fields);
        }
        let id = target.id.clone();
        self.mutation(key, Value::Object(input), id, move |cloud| {
            Box::pin(async move { Ok(to_value(&cloud.complete_recurring(&target).await?)) })
        })
        .await
    }

    /// The private ledger (tests and diagnostics).
    pub fn stored_operations(&self) -> Vec<StoredOperation> {
        self.0
            .enabled
            .as_ref()
            .map(|e| e.private.get(|s| s.operations.values().cloned().collect()))
            .unwrap_or_default()
    }
}

fn to_value<T: Serialize>(value: &T) -> Value {
    serde_json::to_value(value).unwrap_or(Value::Null)
}
