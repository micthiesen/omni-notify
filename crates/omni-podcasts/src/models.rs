//! Structured-output calls (AI SDK `generateText` + `Output.object`) with the
//! zod refinements re-checked after decoding: an out-of-range value fails the
//! call like a schema mismatch did in TS.

use std::sync::Arc;

use omni_ai::{Ai, AiError, CostTag, GenerateRequest, ModelRole, Usage};
use omni_config::Config;
use serde::de::DeserializeOwned;

/// Range and length checks the JSON schema cannot express strictly.
pub trait Refine {
    fn refine(&self) -> Result<(), String>;
}

pub fn in_range(value: f64, min: f64, max: f64, field: &str) -> Result<(), String> {
    if (min..=max).contains(&value) {
        Ok(())
    } else {
        Err(format!("{field} {value} outside {min}..={max}"))
    }
}

/// The model registry for one subsystem.
#[derive(Clone)]
pub struct Models {
    pub ai: Ai,
    pub config: Arc<Config>,
}

/// A decoded object plus the usage and model id for logging.
pub struct Generated<T> {
    pub output: T,
    pub usage: Usage,
}

impl Models {
    /// `provider:model` configured for `role`.
    pub fn model_id(&self, role: ModelRole) -> String {
        self.config.model(role).to_owned()
    }

    pub async fn object<T>(
        &self,
        role: ModelRole,
        operation: &'static str,
        prompt: String,
    ) -> Result<Generated<T>, AiError>
    where
        T: DeserializeOwned + schemars::JsonSchema + Refine,
    {
        let model = self.ai.model_for(&self.config, role)?;
        let (output, usage): (T, Usage) = self
            .ai
            .generate_object(
                model.as_ref(),
                GenerateRequest::prompt(prompt),
                CostTag::with_operation(role, operation),
            )
            .await?;
        output
            .refine()
            .map_err(|e| AiError::Schema(format!("No object generated: {e}")))?;
        Ok(Generated { output, usage })
    }
}
