//! Port of `src/ai/registry.spec.ts`, plus a parity check of every `get*Model()` helper
//! in `src/ai/registry.ts` (code-default model, feature, default operation).
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use futures::future::BoxFuture;
use omni_ai::registry::{ALL_ROLES, LANGUAGE_MODEL_TIMEOUT, role_cost};
use omni_ai::{
    AiError, GenerateRequest, GenerateResponse, LanguageModel, ModelId, ModelRole,
    call_with_retries,
};

struct HangingModel {
    id: ModelId,
    aborted: Arc<AtomicBool>,
}

struct SetOnDrop(Arc<AtomicBool>);

impl Drop for SetOnDrop {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

impl LanguageModel for HangingModel {
    fn id(&self) -> &ModelId {
        &self.id
    }

    fn generate<'a>(
        &'a self,
        _req: &'a GenerateRequest,
    ) -> BoxFuture<'a, Result<GenerateResponse, AiError>> {
        let guard = SetOnDrop(self.aborted.clone());
        Box::pin(async move {
            let _guard = guard;
            futures::future::pending::<Result<GenerateResponse, AiError>>().await
        })
    }
}

#[tokio::test(start_paused = true)]
async fn times_out_and_aborts_a_hanging_model_request() {
    let aborted = Arc::new(AtomicBool::new(false));
    let model = HangingModel {
        id: ModelId::parse("openai:gpt-6-luna").unwrap(),
        aborted: aborted.clone(),
    };
    let request = GenerateRequest::prompt("hello");
    assert_eq!(request.timeout, LANGUAGE_MODEL_TIMEOUT);
    let started = tokio::time::Instant::now();
    let result = call_with_retries(&model, &request).await;
    assert!(matches!(result, Err(AiError::Timeout)));
    assert_eq!(started.elapsed(), LANGUAGE_MODEL_TIMEOUT);
    assert!(
        aborted.load(Ordering::SeqCst),
        "the request future was dropped"
    );
}

struct Helper {
    env_key: Option<String>,
    default_model: String,
    feature: String,
    operation: Option<String>,
}

/// Parses each `resolveModel(configured, "provider:model", "feature", operation)` call.
fn ts_helpers() -> Vec<Helper> {
    let source = include_str!("../../../src/ai/registry.ts");
    let call = regex_lite(
        r#"resolveModel\(\s*(config\.(\w+)|undefined),\s*"([^"]+)",\s*"([^"]+)",\s*("([^"]+)"|operation),?\s*\)"#,
    );
    let mut helpers = Vec::new();
    for chunk in source.split("export function ").skip(1) {
        let Some(caps) = call.captures(chunk) else {
            continue;
        };
        let operation = match caps.get(6) {
            Some(literal) => Some(literal.as_str().to_owned()),
            None => regex_lite(r#"operation = "([^"]+)""#)
                .captures(chunk)
                .map(|c| c[1].to_owned()),
        };
        helpers.push(Helper {
            env_key: caps.get(2).map(|m| m.as_str().to_owned()),
            default_model: caps[3].to_owned(),
            feature: caps[4].to_owned(),
            operation,
        });
    }
    helpers
}

fn regex_lite(pattern: &str) -> regex::Regex {
    regex::Regex::new(pattern).unwrap()
}

#[test]
fn roles_match_every_registry_helper() {
    let helpers = ts_helpers();
    assert_eq!(
        helpers.len(),
        ALL_ROLES.len(),
        "one role per get*Model helper"
    );
    for (role, helper) in ALL_ROLES.iter().zip(&helpers) {
        assert_eq!(
            role.env_key().map(str::to_owned),
            helper.env_key,
            "{role:?} env key"
        );
        assert_eq!(
            role.default_model(),
            helper.default_model,
            "{role:?} default model"
        );
        let (feature, operation) = role_cost(*role);
        assert_eq!(feature, helper.feature, "{role:?} feature");
        if let Some(ts_operation) = &helper.operation {
            assert_eq!(operation, ts_operation, "{role:?} operation");
        } else {
            assert_eq!(*role, ModelRole::LivestreamIntelligence);
        }
    }
}
