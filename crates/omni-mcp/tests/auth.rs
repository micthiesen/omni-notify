//! Port of `src/mcp/auth.spec.ts` (all cases kept). The token rules live in
//! `omni-config` (boot validation) and the bearer check in `omni-server-kit`;
//! this spec exercises them as the MCP endpoint relies on them.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::BTreeMap;

use axum::http::HeaderValue;
use omni_config::{Config, is_strong_mcp_token};
use omni_server_kit::bearer_digest_eq;

const TEST_TOKEN: &str = "test-token-0123456789-ABCDEFGHIJKLMNOPQRSTUVWXYZ";

fn valid_bearer(header: Option<&str>) -> bool {
    let value = header.map(|h| HeaderValue::from_str(h).unwrap());
    bearer_digest_eq(value.as_ref(), TEST_TOKEN)
}

fn config(pairs: &[(&str, &str)]) -> Result<Config, String> {
    let mut env: BTreeMap<String, String> = omni_testkit::test_app_env();
    // The token rules start from an environment without either token.
    env.remove("OMNI_MCP_TOKEN");
    env.remove("OMNI_DEVICE_LINK_TOKEN");
    for (key, value) in pairs {
        env.insert((*key).to_owned(), (*value).to_owned());
    }
    Config::from_env(&env).map_err(|e| e.to_string())
}

#[test]
fn accepts_only_the_exact_bearer_token() {
    assert!(valid_bearer(Some(&format!("Bearer {TEST_TOKEN}"))));
    assert!(valid_bearer(Some(&format!("bearer {TEST_TOKEN}"))));
    assert!(!valid_bearer(None));
    assert!(!valid_bearer(Some("Basic abc")));
    assert!(!valid_bearer(Some("Bearer wrong")));
    assert!(!valid_bearer(Some(&format!("Bearer {TEST_TOKEN} extra"))));
}

#[test]
fn requires_a_strong_token_in_production() {
    assert!(config(&[("DOCKERIZED", "true"), ("OMNI_MCP_TOKEN", TEST_TOKEN)]).is_ok());
    assert!(config(&[]).is_ok());
    assert!(
        config(&[("DOCKERIZED", "true")])
            .unwrap_err()
            .contains("OMNI_MCP_TOKEN is required in production")
    );
    assert!(
        config(&[("DOCKERIZED", "true"), ("OMNI_MCP_TOKEN", "short")])
            .unwrap_err()
            .contains("at least 32 characters")
    );
    assert!(!is_strong_mcp_token(&"a".repeat(64)));
    assert!(!is_strong_mcp_token(&format!("{TEST_TOKEN}\n")));
}

#[test]
fn keeps_the_mac_device_token_optional_strong_and_distinct() {
    let device = "device-0123456789-abcdefghijklmnopqrstuvwxyz";
    let with_device = |token: &str| {
        config(&[
            ("OMNI_MCP_TOKEN", TEST_TOKEN),
            ("OMNI_DEVICE_LINK_TOKEN", token),
        ])
    };
    assert!(config(&[("OMNI_MCP_TOKEN", TEST_TOKEN)]).is_ok());
    assert!(with_device("").is_ok());
    assert!(with_device(device).is_ok());
    assert!(
        with_device("short")
            .unwrap_err()
            .contains("at least 32 characters")
    );
    assert!(
        with_device(TEST_TOKEN)
            .unwrap_err()
            .contains("must differ from OMNI_MCP_TOKEN")
    );
}
