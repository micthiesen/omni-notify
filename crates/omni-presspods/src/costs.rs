//! Per-episode cost accounting.
//!
//! The counter is shared by concurrent retriever ratings, so it sits behind a
//! mutex; the totals are persisted on the episode as [`Costs`]. TTS usage is
//! also recorded as a durable cost event (LLM events are recorded by
//! `omni_ai::Ai` itself).

use std::sync::{Arc, Mutex};

use omni_ai::Usage;
use omni_ai::costs::{
    CostRecorder, NewCostEvent, TTS_CHARACTER_CENTS, bare_model_id, current_cost_feature,
    llm_cost_cents,
};
use omni_api::costs::{CostCategory, CostPriceStatus, CostUsage};

use crate::model::Costs;

const LOG: &str = "PressPods.CostCounter";

/// `CompletionUsage`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CompletionUsage {
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
}

impl From<Usage> for CompletionUsage {
    fn from(usage: Usage) -> Self {
        Self {
            prompt_tokens: usage.input_tokens,
            completion_tokens: usage.output_tokens,
        }
    }
}

/// `CostCounter`; cheap to clone (shared state).
#[derive(Clone, Default)]
pub struct CostCounter {
    inner: Arc<Mutex<Costs>>,
}

#[allow(clippy::cast_precision_loss)]
fn as_f64(n: u64) -> f64 {
    n as f64
}

impl CostCounter {
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Costs> {
        self.inner.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// A snapshot of the totals.
    pub fn costs(&self) -> Costs {
        self.lock().clone()
    }

    /// Adds an LLM call; an unpriced model counts as $0.
    pub fn record_llm_usage(&self, model: &str, function: &str, usage: CompletionUsage) {
        let bare = bare_model_id(model).to_owned();
        let input = Usage {
            input_tokens: usage.prompt_tokens,
            ..Usage::default()
        };
        let output = Usage {
            output_tokens: usage.completion_tokens,
            ..Usage::default()
        };
        let input_cents = llm_cost_cents(model, &input);
        if input_cents.is_none() {
            tracing::debug!(target: LOG, "No pricing for model {bare}; counting as $0");
        }
        let input_cents = input_cents.unwrap_or(0.0);
        let output_cents = llm_cost_cents(model, &output).unwrap_or(0.0);
        let mut costs = self.lock();
        costs.llm_cents += input_cents + output_cents;
        *costs
            .detail_cents
            .entry(format!("{bare}-{function}-input"))
            .or_insert(0.0) += input_cents;
        *costs
            .detail_cents
            .entry(format!("{bare}-{function}-output"))
            .or_insert(0.0) += output_cents;
        let tokens = costs
            .detail_tokens
            .entry(format!("{bare}-{function}"))
            .or_default();
        tokens.input += as_f64(usage.prompt_tokens);
        tokens.output += as_f64(usage.completion_tokens);
    }

    /// Adds a TTS call for `text` and records a durable `tts` cost event.
    pub async fn record_tts_usage(
        &self,
        recorder: &CostRecorder,
        model: &str,
        function: &str,
        text: &str,
    ) {
        let chars = as_f64(omni_core::js::utf16_len(text) as u64);
        let price = TTS_CHARACTER_CENTS
            .iter()
            .find(|(id, _)| *id == model)
            .map(|(_, cents)| *cents);
        let cents = price.unwrap_or(0.0) * chars;
        {
            let mut costs = self.lock();
            costs.tts_cents += cents;
            *costs
                .detail_cents
                .entry(format!("{model}-{function}"))
                .or_insert(0.0) += cents;
            *costs
                .detail_chars
                .entry(format!("{model}-{function}"))
                .or_insert(0.0) += chars;
        }
        recorder
            .record(NewCostEvent {
                category: CostCategory::Tts,
                feature: current_cost_feature("press-pods").to_owned(),
                operation: function.to_owned(),
                service: if model == "eleven_v3" {
                    "elevenlabs".to_owned()
                } else {
                    "self-hosted".to_owned()
                },
                model: Some(model.to_owned()),
                cost_cents: price.map(|_| cents),
                price_status: match price {
                    None => CostPriceStatus::Unknown,
                    Some(p) => {
                        if p == 0.0 {
                            CostPriceStatus::Free
                        } else {
                            CostPriceStatus::Estimated
                        }
                    }
                },
                usage: CostUsage {
                    characters: Some(chars),
                    requests: Some(1.0),
                    ..CostUsage::default()
                },
                event_id: None,
                incurred_at: None,
                run_id: None,
            })
            .await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accumulates_llm_details_per_model_and_function() {
        let counter = CostCounter::new();
        let usage = CompletionUsage {
            prompt_tokens: 1000,
            completion_tokens: 100,
        };
        counter.record_llm_usage("openai:unpriced-model", "meta", usage);
        counter.record_llm_usage("openai:unpriced-model", "meta", usage);
        let costs = counter.costs();
        assert_eq!(costs.llm_cents, 0.0);
        let keys: Vec<&String> = costs.detail_cents.keys().collect();
        assert_eq!(
            keys,
            ["unpriced-model-meta-input", "unpriced-model-meta-output"]
        );
        let tokens = &costs.detail_tokens["unpriced-model-meta"];
        assert_eq!((tokens.input, tokens.output), (2000.0, 200.0));
    }
}
