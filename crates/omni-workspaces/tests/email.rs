//! Workspace email scope matching.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use common::email;
use omni_store::cbor::Extra;
use omni_workspaces::email::matches_workspace_email;
use omni_workspaces::entities::EmailScopeRow;

fn mail() -> omni_core::email::FetchedEmail {
    email(
        "mail-1",
        "Your Framework Laptop order shipped",
        "Framework <orders@frame.work>",
        "Track order ABC123 and review your invoice.",
    )
}

fn scope(f: impl FnOnce(&mut EmailScopeRow)) -> EmailScopeRow {
    let mut scope = EmailScopeRow {
        workspace_id: "purchase-research".to_owned(),
        subject_id: "laptop".to_owned(),
        senders: vec![],
        domains: vec![],
        subject_keywords: vec![],
        body_keywords: vec![],
        updated_at: 0,
        extra: Extra::new(),
    };
    f(&mut scope);
    scope
}

fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(|v| (*v).to_owned()).collect()
}

#[test]
fn matches_exact_normalized_sender_addresses() {
    assert!(matches_workspace_email(
        &mail(),
        &scope(|s| s.senders = strings(&["orders@frame.work"]))
    ));
    assert!(!matches_workspace_email(
        &mail(),
        &scope(|s| s.senders = strings(&["other@frame.work"]))
    ));
}

#[test]
fn matches_domains_and_strips_an_optional_leading_at() {
    assert!(matches_workspace_email(
        &mail(),
        &scope(|s| s.domains = strings(&["@frame.work"]))
    ));
    assert!(!matches_workspace_email(
        &mail(),
        &scope(|s| s.domains = strings(&["work.example"]))
    ));
}

#[test]
fn matches_subject_and_body_keywords_case_insensitively() {
    assert!(matches_workspace_email(
        &mail(),
        &scope(|s| s.subject_keywords = strings(&["LAPTOP ORDER"]))
    ));
    assert!(matches_workspace_email(
        &mail(),
        &scope(|s| s.body_keywords = strings(&["abc123"]))
    ));
}

#[test]
fn does_not_ingest_an_email_when_the_approved_scope_has_no_match() {
    assert!(!matches_workspace_email(
        &mail(),
        &scope(|s| s.subject_keywords = strings(&["camera"]))
    ));
}
