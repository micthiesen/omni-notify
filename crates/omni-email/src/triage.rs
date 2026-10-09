//! Shared cheap-model relevance triage: one model call
//! per email, shared by the parcel and calendar pipelines. Concurrent callers
//! share the in-flight call; failures are never cached (the keyword fallbacks
//! own the degraded path); the cache holds at most [`MAX_TRIAGE_CACHE_ENTRIES`]
//! settled entries and never evicts an in-flight one.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, Weak};

use futures::FutureExt as _;
use futures::future::{BoxFuture, Shared};
use indexmap::IndexMap;
use omni_ai::{Ai, CostTag, GenerateRequest, ModelRole};
use omni_config::Config;
use omni_core::email::FetchedEmail;
use omni_core::js::utf16_slice;
use omni_store::Store;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::activity::LlmCost;
use crate::feedback::{DIGEST_LIMIT, format_digest};
use crate::sender_rules::RuleTarget;

const LOG: &str = "Main:Email:Triage";

const MAX_BODY_CHARS: usize = 1500;
const MAX_LINKS: usize = 5;
pub const MAX_TRIAGE_CACHE_ENTRIES: usize = 500;

/// Per-pipeline relevance verdict.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TriageVerdict {
    pub parcel: bool,
    pub calendar: bool,
    pub reason: String,
}

/// What triage sees of an email.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TriageEmail {
    pub id: String,
    pub subject: String,
    pub from: String,
    pub text_body: String,
    pub links: Vec<String>,
}

impl From<&FetchedEmail> for TriageEmail {
    fn from(email: &FetchedEmail) -> Self {
        Self {
            id: email.id.clone(),
            subject: email.subject.clone(),
            from: email.from.clone(),
            text_body: email.text_body.clone(),
            links: email.links.clone(),
        }
    }
}

/// A failed classification (`TriageError`). Cloneable so concurrent callers
/// of one in-flight call each receive it.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct TriageError {
    pub email_id: String,
    pub message: String,
}

/// A classification result; `cost` is `None` when the classifier recorded no
/// cost (the triage cost then reads as unpriced).
#[derive(Clone, Debug, PartialEq)]
pub struct Classified {
    pub verdict: TriageVerdict,
    pub cost: Option<LlmCost>,
}

/// The model call behind [`EmailTriage`] (a test seam: `classifyFn`).
pub trait TriageClassifier: Send + Sync {
    fn classify(&self, email: TriageEmail) -> BoxFuture<'static, Result<Classified, TriageError>>;
}

type SharedClassification = Shared<BoxFuture<'static, Result<TriageVerdict, TriageError>>>;

struct Entry {
    call: SharedClassification,
    settled: bool,
}

#[derive(Default)]
struct Cache {
    entries: IndexMap<String, Entry>,
    costs: HashMap<String, LlmCost>,
}

impl Cache {
    fn trim(&mut self) {
        while self.entries.len() > MAX_TRIAGE_CACHE_ENTRIES {
            // Never evict an in-flight entry: that would start a duplicate model call.
            let Some(index) = self.entries.values().position(|e| e.settled) else {
                return;
            };
            if let Some((id, _)) = self.entries.shift_remove_index(index) {
                self.costs.remove(&id);
            }
        }
    }
}

struct Inner {
    classifier: Arc<dyn TriageClassifier>,
    cache: Mutex<Cache>,
}

impl Inner {
    fn lock(&self) -> MutexGuard<'_, Cache> {
        self.cache.lock().unwrap_or_else(|p| p.into_inner())
    }
}

/// The email triage service; cheap to clone.
#[derive(Clone)]
pub struct EmailTriage {
    inner: Arc<Inner>,
}

impl EmailTriage {
    pub fn new(classifier: Arc<dyn TriageClassifier>) -> Self {
        Self {
            inner: Arc::new(Inner {
                classifier,
                cache: Mutex::new(Cache::default()),
            }),
        }
    }

    /// The production triage: the `Triage` model role with feedback digests.
    pub fn with_model(ai: Ai, config: Arc<Config>, store: Store) -> Self {
        Self::new(Arc::new(ModelTriageClassifier { ai, config, store }))
    }

    /// Classifies `email`, sharing an in-flight or settled result for its id.
    pub async fn classify(&self, email: &TriageEmail) -> Result<TriageVerdict, TriageError> {
        let call = self.call_for(email);
        let result = call.await;
        if let Err(error) = &result {
            tracing::warn!(target: LOG, "Triage failed for \"{}\": {}", email.subject, error.message);
        }
        result
    }

    fn call_for(&self, email: &TriageEmail) -> SharedClassification {
        let mut cache = self.inner.lock();
        if let Some(entry) = cache.entries.get(&email.id) {
            return entry.call.clone();
        }
        let weak: Weak<Inner> = Arc::downgrade(&self.inner);
        let id = email.id.clone();
        let classify = self.inner.classifier.classify(email.clone());
        let call = async move {
            let result = classify.await;
            if let Some(inner) = weak.upgrade() {
                let mut cache = inner.lock();
                match &result {
                    Ok(classified) => {
                        if let Some(cost) = classified.cost {
                            cache.costs.insert(id.clone(), cost);
                        }
                        if let Some(entry) = cache.entries.get_mut(&id) {
                            entry.settled = true;
                        }
                        cache.trim();
                    }
                    Err(_) => {
                        cache.entries.shift_remove(&id);
                    }
                }
            }
            result.map(|classified| classified.verdict)
        }
        .boxed()
        .shared();
        cache.entries.insert(
            email.id.clone(),
            Entry {
                call: call.clone(),
                settled: false,
            },
        );
        cache.trim();
        call
    }

    /// The triage cost for one email: `Unpriced` when no call has completed
    /// for it or its model has no price. Only meaningful once triage admitted
    /// the email (the same cost may also appear on the other pipeline's row).
    pub fn triage_cost_cents(&self, email_id: &str) -> LlmCost {
        self.inner
            .lock()
            .costs
            .get(email_id)
            .copied()
            .filter(|cost| !cost.is_none())
            .unwrap_or(LlmCost::Unpriced)
    }
}

/// The triage prompt given the two feedback digests.
pub fn build_triage_prompt(
    email: &TriageEmail,
    parcel_digest: &str,
    calendar_digest: &str,
) -> String {
    let body = utf16_slice(&email.text_body, 0, MAX_BODY_CHARS);
    let links: Vec<&str> = email
        .links
        .iter()
        .take(MAX_LINKS)
        .map(String::as_str)
        .collect();
    let links_section = if links.is_empty() {
        String::new()
    } else {
        format!("\nLinks:\n{}", links.join("\n"))
    };
    let digests: Vec<(&str, &str)> = [
        ("Parcel pipeline", parcel_digest),
        ("Calendar pipeline", calendar_digest),
    ]
    .into_iter()
    .filter(|(_, digest)| !digest.is_empty())
    .collect();
    let corrections_section = if digests.is_empty() {
        String::new()
    } else {
        format!(
            "\n\n## Recent user corrections — follow these\n{}",
            digests
                .iter()
                .map(|(label, digest)| format!("{label}:\n{digest}"))
                .collect::<Vec<_>>()
                .join("\n")
        )
    };
    format!(
        "Classify this email for two automated pipelines. Answer with a boolean per pipeline and a one-sentence reason covering both.

parcel — true only if the email plausibly carries or references a shipment tracking number for a physical package being delivered to the user (shipping confirmations, carrier updates, \"your package is on the way\"). Order confirmations WITHOUT tracking info, marketing, promotional digests, and order-status-only updates (payment received, order confirmed, awaiting confirmation) are false.

calendar — true only if the email describes a concrete upcoming appointment, booking, event, or service window worth putting on a personal calendar (reservations, flights, medical appointments, building maintenance notices). Newsletters, receipts for completed services, subscription/billing renewals, platform policy notices, and marketing \"deadlines\" are false. Genuine cancellations or reschedules of upcoming events are true.

From: {}
Subject: {}

{body}{links_section}{corrections_section}",
        email.from, email.subject
    )
}

/// Reads the feedback digests and builds the prompt.
pub async fn triage_prompt(
    store: &Store,
    email: &TriageEmail,
) -> Result<String, omni_store::StoreError> {
    let parcel = format_digest(store, RuleTarget::Parcel, DIGEST_LIMIT).await?;
    let calendar = format_digest(store, RuleTarget::Calendar, DIGEST_LIMIT).await?;
    Ok(build_triage_prompt(email, &parcel, &calendar))
}

/// The model-backed classifier (`callModelEffect`).
struct ModelTriageClassifier {
    ai: Ai,
    config: Arc<Config>,
    store: Store,
}

impl TriageClassifier for ModelTriageClassifier {
    fn classify(&self, email: TriageEmail) -> BoxFuture<'static, Result<Classified, TriageError>> {
        let (ai, config, store) = (self.ai.clone(), self.config.clone(), self.store.clone());
        Box::pin(async move {
            let fail = |message: String| TriageError {
                email_id: email.id.clone(),
                message,
            };
            let model_id = config.model(ModelRole::Triage).to_owned();
            let model = ai
                .model_for(&config, ModelRole::Triage)
                .map_err(|e| fail(e.to_string()))?;
            let prompt = triage_prompt(&store, &email)
                .await
                .map_err(|e| fail(e.to_string()))?;
            let (verdict, usage) = ai
                .generate_object::<TriageVerdict>(
                    model.as_ref(),
                    GenerateRequest::prompt(prompt),
                    CostTag::for_role(ModelRole::Triage),
                )
                .await
                .map_err(|e| fail(e.to_string()))?;
            let cost = omni_ai::costs::llm_cost_cents(&model_id, &usage);
            if cost.is_none() {
                tracing::debug!(target: LOG, "No pricing data for triage model \"{model_id}\"");
            }
            tracing::info!(
                target: LOG,
                "Triage ({model_id}) \"{}\": parcel={} calendar={} — {}",
                email.subject,
                verdict.parcel,
                verdict.calendar,
                verdict.reason
            );
            Ok(Classified {
                verdict,
                cost: Some(LlmCost::from_call(cost)),
            })
        })
    }
}
