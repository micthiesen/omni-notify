//! Workspace definitions, plus a verbatim check against the committed
//! definitions JSON.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::HashSet;

use omni_workspaces::definitions::{marketplace_selling, workspace_definitions};

#[test]
fn keeps_workspace_task_and_artifact_identifiers_unique() {
    let definitions = workspace_definitions("0 0 9 * * 0");
    let ids: HashSet<_> = definitions.iter().map(|d| &d.id).collect();
    assert_eq!(ids.len(), definitions.len());
    let tasks: HashSet<_> = definitions.iter().map(|d| &d.task_name).collect();
    assert_eq!(tasks.len(), definitions.len());
    for workspace in &definitions {
        let keys: HashSet<_> = workspace.artifacts.iter().map(|a| &a.key).collect();
        assert_eq!(keys.len(), workspace.artifacts.len());
    }
}

#[test]
fn keeps_marketplace_selling_user_driven_and_listing_complete() {
    let marketplace = marketplace_selling("0 0 9 * * 0");
    assert_eq!(marketplace.scheduled_runs, Some(false));
    let keys: Vec<&str> = marketplace
        .artifacts
        .iter()
        .map(|a| a.key.as_str())
        .collect();
    for key in [
        "item-details",
        "listing-fields",
        "pricing",
        "photos",
        "progress",
    ] {
        assert!(keys.contains(&key));
    }
    assert!(marketplace.instructions.contains("inactivity is normal"));
    assert!(marketplace.instructions.contains("Never publish"));
}

#[test]
fn definitions_match_the_committed_json_verbatim() {
    let expected = omni_testkit::golden("definitions.json");
    let actual = serde_json::to_value(workspace_definitions("0 0 9 * * 0")).unwrap();
    assert_eq!(actual, expected);
}
