//! Tracking-number extraction. The prompt
//! separates order numbers from tracking numbers and asks for up to three
//! ranked carrier codes from the live carrier list.

use std::sync::Arc;

use futures::future::BoxFuture;
use omni_ai::{Ai, AiError, CostTag, GenerateRequest, ModelRole};
use omni_config::Config;
use omni_core::js::{json_stringify_pretty2, utf16_len, utf16_slice};
use omni_email::activity::LlmCost;
use omni_email::systemic::{FailureClass, classify_ai_error};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::carriers::candidates::MAX_CARRIER_CANDIDATES;
use crate::carriers::carrier_map::CarrierDirectory;
use crate::error::ParcelError;
use crate::log_file::{LogFile, code_block};

const LOG: &str = "Main:ParcelTracker";
const MAX_BODY_CHARS: usize = 12_000;

/// One extracted delivery (the model's snake_case shape).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ExtractedDelivery {
    /// The package tracking number
    pub tracking_number: String,
    /// Carrier codes from the provided carrier list, ranked most likely first (up to 3)
    #[schemars(length(min = 1))]
    pub carrier_candidates: Vec<String>,
    /// Short title for the package prefixed with a relevant emoji in Title Case (e.g. '👟 Running Shoes', '🔪 Kitchen Knife Set')
    pub description: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DeliveryExtraction {
    pub deliveries: Vec<ExtractedDelivery>,
}

impl DeliveryExtraction {
    /// The contract beyond the JSON shape: every delivery names at least
    /// one carrier candidate.
    pub fn validate(&self) -> Result<(), String> {
        match self
            .deliveries
            .iter()
            .position(|delivery| delivery.carrier_candidates.is_empty())
        {
            Some(index) => Err(format!(
                "deliveries[{index}].carrier_candidates: must contain at least 1 item"
            )),
            None => Ok(()),
        }
    }
}

/// Decodes and validates a model response (`deliveryExtractionSchema.parse`).
pub fn decode_extraction(value: serde_json::Value) -> Result<DeliveryExtraction, String> {
    let decoded: DeliveryExtraction = serde_json::from_value(value).map_err(|e| e.to_string())?;
    decoded.validate()?;
    Ok(decoded)
}

/// What extraction sees of an email.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExtractionEmail {
    pub subject: String,
    pub from: String,
    pub text_body: String,
    pub links: Vec<String>,
}

/// Deliveries plus the extraction call's cost.
#[derive(Clone, Debug, PartialEq)]
pub struct ExtractDeliveriesResult {
    pub deliveries: Vec<ExtractedDelivery>,
    /// `Unpriced` when the extraction model has no price.
    pub cost: LlmCost,
}

/// The extraction step of the pipeline (a test seam).
pub trait DeliveryExtractor: Send + Sync {
    fn extract<'a>(
        &'a self,
        email: &'a ExtractionEmail,
        log_file: Option<&'a LogFile>,
    ) -> BoxFuture<'a, Result<ExtractDeliveriesResult, ParcelError>>;
}

/// The extraction prompt.
pub fn extraction_prompt(email: &ExtractionEmail, carrier_codes: &str) -> String {
    let body = utf16_slice(&email.text_body, 0, MAX_BODY_CHARS);
    let links_section = if email.links.is_empty() {
        String::new()
    } else {
        format!(
            "\n\nURLs from the email (tracking numbers sometimes appear only inside these):\n{}",
            email.links.join("\n")
        )
    };
    format!(
        "Extract package tracking numbers from this email. If no tracking numbers are found, return an empty deliveries array.

Rules for what counts as a tracking number:
- A number labeled \"Order #\", \"order number\", or appearing in a subject like \"Order Shipped #123456\" or \"Order Confirmed #123456\" is NEVER a tracking number. Order numbers identify the merchant order, not the shipment.
- If the body names a carrier but says tracking information will be available later (e.g. \"the shipping provider needs 24-48 hours\"), there is no tracking number yet — return an empty deliveries array.

For carrier_candidates, list up to {MAX_CARRIER_CANDIDATES} plausible carrier codes ranked most likely first, using the short code (left of the colon) from this list — not the display name. If no carrier in this list could plausibly match, omit that tracking number entirely.
{carrier_codes}

Carrier guidance:
- The recipient is in Canada. When a carrier brand has entries for multiple countries, rank the Canadian entry first for domestic shipments (e.g. \"dicom\" for GLS Canada ahead of \"gls\" for GLS Europe) and include the other regional variants only as lower-ranked candidates.
- Dragonfly: Always use carrier code \"intelc\" (Dragonfly is Intelcom's brand).
- Tracking numbers starting with \"JY\" (e.g. JY25CA10A002279541): Use carrier code \"uniuni\". These are UniUni last-mile deliveries, often from AliExpress shipments. Prefer \"uniuni\" over any AliExpress carrier.

From: {}
Subject: {}

{body}{links_section}",
        email.from, email.subject
    )
}

/// The model-backed extractor (`extractDeliveriesEffect`).
pub struct ModelExtractor {
    ai: Ai,
    config: Arc<Config>,
    carriers: Arc<CarrierDirectory>,
}

impl ModelExtractor {
    pub fn new(ai: Ai, config: Arc<Config>, carriers: Arc<CarrierDirectory>) -> Self {
        Self {
            ai,
            config,
            carriers,
        }
    }

    async fn run(
        &self,
        email: &ExtractionEmail,
        log_file: Option<&LogFile>,
    ) -> Result<ExtractDeliveriesResult, ParcelError> {
        let model_id = self.config.model(ModelRole::Extraction).to_owned();
        let carrier_codes = self.carriers.prompt_codes().await;
        let prompt = extraction_prompt(email, &carrier_codes);
        match log_file {
            Some(file) => {
                file.section(
                    &format!("Extraction Prompt ({model_id})"),
                    &code_block(&prompt, None),
                )
                .await;
                tracing::info!(
                    target: LOG,
                    "Extraction prompt ({model_id}) [{} chars]",
                    utf16_len(&prompt)
                );
            }
            // The prompt carries the email body: DEBUG keeps it in the captured
            // activity log without printing bodies at INFO.
            None => tracing::debug!(target: LOG, "Extraction prompt ({model_id}):\n{prompt}"),
        }

        let model = self
            .ai
            .model_for(&self.config, ModelRole::Extraction)
            .map_err(|error| match classify_ai_error(&error) {
                FailureClass::Systemic(signature) => ParcelError::SystemicExtraction {
                    message: error.to_string(),
                    signature,
                },
                FailureClass::Content | FailureClass::Transient => transient(error),
            })?;
        let (decoded, usage) = self
            .ai
            .generate_object::<DeliveryExtraction>(
                model.as_ref(),
                GenerateRequest::prompt(prompt),
                CostTag::for_role(ModelRole::Extraction),
            )
            .await
            .map_err(|error| match classify_ai_error(&error) {
                FailureClass::Systemic(signature) => ParcelError::SystemicExtraction {
                    message: match error {
                        AiError::Schema(message) => message,
                        other => other.to_string(),
                    },
                    signature,
                },
                // The output did not match the schema: replaying will not help.
                FailureClass::Content | FailureClass::Transient => match error {
                    AiError::Schema(message) => ParcelError::Extraction {
                        message,
                        transient: false,
                    },
                    other => transient(other),
                },
            })?;
        decoded
            .validate()
            .map_err(|message| ParcelError::Extraction {
                message,
                transient: false,
            })?;

        let response = serde_json::to_value(&decoded)
            .map(|value| json_stringify_pretty2(&value))
            .unwrap_or_default();
        match log_file {
            Some(file) => {
                file.section("Extraction Response", &code_block(&response, Some("json")))
                    .await;
                tracing::info!(target: LOG, "{}", code_block(&response, Some("json")));
            }
            None => tracing::info!(target: LOG, "Extraction response: {response}"),
        }
        tracing::info!(
            target: LOG,
            "Token usage: {} prompt, {} completion",
            usage.input_tokens,
            usage.output_tokens
        );
        let cost = omni_ai::costs::llm_cost_cents(&model_id, &usage);
        if cost.is_none() {
            tracing::debug!(target: LOG, "No pricing data for extraction model \"{model_id}\"");
        }
        Ok(ExtractDeliveriesResult {
            deliveries: decoded
                .deliveries
                .into_iter()
                .map(|mut delivery| {
                    delivery.carrier_candidates.truncate(MAX_CARRIER_CANDIDATES);
                    delivery
                })
                .collect(),
            cost: LlmCost::from_call(cost),
        })
    }
}

fn transient(error: AiError) -> ParcelError {
    ParcelError::Extraction {
        message: error.to_string(),
        transient: true,
    }
}

impl DeliveryExtractor for ModelExtractor {
    fn extract<'a>(
        &'a self,
        email: &'a ExtractionEmail,
        log_file: Option<&'a LogFile>,
    ) -> BoxFuture<'a, Result<ExtractDeliveriesResult, ParcelError>> {
        Box::pin(self.run(email, log_file))
    }
}
