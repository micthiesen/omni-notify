//! Request timeout behavior, plus a parity check of every role against the committed
//! model-helper table (code-default model, feature, default operation) that the
//! production configuration was built from.
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

/// One `get*Model()` helper of the former TypeScript registry, in declaration order
/// (`fixtures/registry-helpers.json`).
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct Helper {
    env_key: Option<String>,
    default_model: String,
    feature: String,
    operation: Option<String>,
}

fn helpers() -> Vec<Helper> {
    serde_json::from_str(include_str!("fixtures/registry-helpers.json")).unwrap()
}

#[test]
fn roles_match_every_registry_helper() {
    let helpers = helpers();
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
        if let Some(expected) = &helper.operation {
            assert_eq!(operation, expected, "{role:?} operation");
        } else {
            assert_eq!(*role, ModelRole::LivestreamIntelligence);
        }
    }
}
