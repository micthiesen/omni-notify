//! Port of `src/observer-repair/service.spec.ts` with in-memory dependencies
//! over a file-backed store.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::{Arc, Mutex};

use futures::future::BoxFuture;
use omni_alerts::PushoverError;
use omni_arr::observer::{IssueComment, ObserverIssue};
use omni_arr::observer_repair::agent::{RepairAction, RepairDecision};
use omni_arr::observer_repair::persistence::{
    OBSERVER_REPAIR_LEASE_MS, RepairPhase, acquire_issue, save_issue,
};
use omni_arr::observer_repair::service::{
    PreparedRepair, RepairDependencies, RepairError, issue_revision, run_observer_repair,
};
use omni_core::clock::{SharedClock, TestClock};
use omni_testkit::TestStore;
use serde_json::json;

const NOW: i64 = 1_700_000_000_000;

fn issue(id: i64) -> ObserverIssue {
    serde_json::from_value(json!({
        "id": id,
        "issueType": 1,
        "status": 1,
        "problemSeason": 1,
        "problemEpisode": 1,
        "createdAt": "2026-01-01T00:00:00Z",
        "updatedAt": "2026-01-01T00:00:00Z",
        "media": {
            "id": 10, "tmdbId": 20, "tvdbId": 30, "status": 5, "mediaType": "tv", "title": "Example",
            "externalServiceId": 40, "externalServiceId4k": null, "externalServiceSlug": "example",
            "externalServiceSlug4k": null, "serviceId": 50, "serviceId4k": null, "serviceUrl": "http://arr",
        },
        "createdBy": null,
        "modifiedBy": null,
        "comments": [],
    }))
    .unwrap()
}

fn replace() -> RepairDecision {
    RepairDecision {
        action: RepairAction::Replace,
        season: Some(1),
        episodes: vec![1],
        scope_comment: None,
        reason: "Bad file".into(),
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Counts {
    assessed: usize,
    executed: usize,
    comments: usize,
    resolved: usize,
    sent: usize,
}

struct State {
    status: i64,
    comments: Vec<IssueComment>,
    counts: Counts,
    fail_prepare: bool,
    fail_comment: bool,
    fail_resolve_once: bool,
    comment_during_assessment: Option<String>,
    /// `Some(status)` fails the next send with that Pushover status (`None` = no response).
    send_failure: Option<Option<u16>>,
}

#[derive(Clone)]
struct Deps {
    current: ObserverIssue,
    decision: RepairDecision,
    state: Arc<Mutex<State>>,
}

impl Deps {
    fn new(current: ObserverIssue, decision: RepairDecision) -> Self {
        Self {
            state: Arc::new(Mutex::new(State {
                status: current.status,
                comments: current.comment_list().to_vec(),
                counts: Counts::default(),
                fail_prepare: false,
                fail_comment: false,
                fail_resolve_once: false,
                comment_during_assessment: None,
                send_failure: None,
            })),
            current,
            decision,
        }
    }

    fn with<R>(&self, f: impl FnOnce(&mut State) -> R) -> R {
        f(&mut self.state.lock().unwrap())
    }

    fn counts(&self) -> Counts {
        self.with(|s| s.counts)
    }

    fn add_comment(&self, message: &str) {
        self.with(|s| {
            let id = i64::try_from(s.comments.len()).unwrap() + 1;
            s.comments.push(comment(id, message));
        });
    }

    fn snapshot(&self) -> ObserverIssue {
        self.with(|s| {
            let mut issue = self.current.clone();
            issue.status = s.status;
            issue.comments = Some(s.comments.clone()).into();
            issue
        })
    }
}

fn comment(id: i64, message: &str) -> IssueComment {
    serde_json::from_value(json!({
        "id": id, "message": message, "user": null, "createdAt": null, "updatedAt": null,
    }))
    .unwrap()
}

fn fail(message: &str) -> RepairError {
    RepairError::Other(message.to_owned())
}

impl RepairDependencies for Deps {
    fn list_open(&self) -> BoxFuture<'_, Result<Vec<ObserverIssue>, RepairError>> {
        let open = self.snapshot();
        Box::pin(async move { Ok(if open.status == 1 { vec![open] } else { vec![] }) })
    }

    fn get_issue(&self, _id: i64) -> BoxFuture<'_, Result<ObserverIssue, RepairError>> {
        let issue = self.snapshot();
        Box::pin(async move { Ok(issue) })
    }

    fn assess<'a>(
        &'a self,
        _issue: &'a ObserverIssue,
    ) -> BoxFuture<'a, Result<RepairDecision, RepairError>> {
        if let Some(message) = self.with(|s| s.comment_during_assessment.take()) {
            self.add_comment(&message);
        }
        self.with(|s| s.counts.assessed += 1);
        let decision = self.decision.clone();
        Box::pin(async move { Ok(decision) })
    }

    fn prepare<'a>(
        &'a self,
        _issue: &'a ObserverIssue,
        _decision: &'a RepairDecision,
    ) -> BoxFuture<'a, Result<PreparedRepair<'a>, RepairError>> {
        let failing = self.with(|s| s.fail_prepare);
        Box::pin(async move {
            if failing {
                return Err(fail("arr unavailable"));
            }
            Ok(PreparedRepair {
                summary: "Replacement requested".into(),
                execute: Box::pin(async move {
                    self.with(|s| s.counts.executed += 1);
                    Ok(17)
                }),
            })
        })
    }

    fn comment<'a>(&'a self, _id: i64, message: &'a str) -> BoxFuture<'a, Result<(), RepairError>> {
        let result = self.with(|s| {
            if s.fail_comment {
                return Err(fail("Observer unavailable"));
            }
            s.counts.comments += 1;
            let id = i64::try_from(s.counts.comments).unwrap();
            s.comments.push(comment(id, message));
            Ok(())
        });
        Box::pin(async move { result })
    }

    fn resolve(&self, _id: i64) -> BoxFuture<'_, Result<(), RepairError>> {
        let result = self.with(|s| {
            if s.fail_resolve_once {
                s.fail_resolve_once = false;
                return Err(fail("Observer unavailable"));
            }
            s.counts.resolved += 1;
            s.status = 2;
            Ok(())
        });
        Box::pin(async move { result })
    }

    fn send<'a>(&'a self, _id: i64, _message: &'a str) -> BoxFuture<'a, Result<(), RepairError>> {
        let result = self.with(|s| match s.send_failure.take() {
            Some(status) => Err(RepairError::Pushover(PushoverError {
                status,
                body: "failed".into(),
            })),
            None => {
                s.counts.sent += 1;
                Ok(())
            }
        });
        Box::pin(async move { result })
    }
}

struct Harness {
    store: TestStore,
    clock: Arc<TestClock>,
    shared: SharedClock,
}

impl Harness {
    async fn new() -> Self {
        let clock = TestClock::new(NOW);
        let shared: SharedClock = clock.clone();
        Self {
            store: TestStore::new(shared.clone()).await,
            clock,
            shared,
        }
    }

    async fn run(&self, deps: &Deps) -> Result<String, RepairError> {
        run_observer_repair(deps, &self.store.store, &self.shared).await
    }
}

#[tokio::test]
async fn repairs_comments_resolves_and_notifies_once() {
    let h = Harness::new().await;
    let deps = Deps::new(issue(1), replace());
    assert!(h.run(&deps).await.unwrap().contains("repaired"));
    assert_eq!(
        deps.counts(),
        Counts {
            assessed: 1,
            executed: 1,
            comments: 1,
            resolved: 1,
            sent: 1
        }
    );
    assert_eq!(h.run(&deps).await.unwrap(), "No unhandled Observer issues");
    assert_eq!(deps.counts().sent, 1);
}

#[tokio::test]
async fn cannot_handle_leaves_the_issue_open_and_notifies_once() {
    let h = Harness::new().await;
    let deps = Deps::new(
        issue(2),
        RepairDecision {
            action: RepairAction::CannotHandle,
            season: None,
            episodes: vec![],
            scope_comment: None,
            reason: "Unsupported player".into(),
        },
    );
    h.run(&deps).await.unwrap();
    assert_eq!(deps.snapshot().status, 1);
    assert_eq!(
        deps.counts(),
        Counts {
            assessed: 1,
            executed: 0,
            comments: 0,
            resolved: 0,
            sent: 1
        }
    );
    h.run(&deps).await.unwrap();
    assert_eq!(deps.counts().sent, 1);
}

#[tokio::test]
async fn does_not_resolve_after_repair_failure_and_does_not_repeat_it() {
    let h = Harness::new().await;
    let deps = Deps::new(issue(3), replace());
    deps.with(|s| s.fail_prepare = true);
    h.run(&deps).await.unwrap();
    let counts = deps.counts();
    assert_eq!((counts.executed, counts.resolved, counts.sent), (0, 0, 1));
    h.run(&deps).await.unwrap();
    assert_eq!(deps.counts().sent, 1);
}

#[tokio::test]
async fn resumes_an_interrupted_execution_as_unhandled_without_assessing_or_repairing() {
    let h = Harness::new().await;
    let current = issue(4);
    let owner = "crashed-worker";
    let seed_now = NOW - OBSERVER_REPAIR_LEASE_MS - 1;
    h.clock.set(seed_now);
    let mut reserved = acquire_issue(
        &h.store.store,
        current.id,
        &issue_revision(&current),
        owner,
        seed_now,
    )
    .await
    .unwrap()
    .unwrap();
    reserved.phase = RepairPhase::Executing;
    save_issue(&h.store.store, &reserved, owner, seed_now + 1)
        .await
        .unwrap();
    let deps = Deps::new(current, replace());
    h.clock.set(NOW);

    h.run(&deps).await.unwrap();

    let counts = deps.counts();
    assert_eq!(
        (
            counts.assessed,
            counts.executed,
            counts.resolved,
            counts.sent
        ),
        (0, 0, 0, 1)
    );
}

#[tokio::test]
async fn retries_comment_completion_without_repeating_repair() {
    let h = Harness::new().await;
    let deps = Deps::new(issue(5), replace());
    deps.with(|s| s.fail_comment = true);
    assert!(h.run(&deps).await.is_err());
    let counts = deps.counts();
    assert_eq!((counts.executed, counts.resolved), (1, 0));
    deps.with(|s| s.fail_comment = false);
    assert!(h.run(&deps).await.is_ok());
    assert_eq!(deps.counts().executed, 1);
}

#[tokio::test]
async fn blocks_repair_when_the_issue_changes_during_assessment() {
    let h = Harness::new().await;
    let deps = Deps::new(issue(6), replace());
    deps.with(|s| s.comment_during_assessment = Some("Actually, the scope changed".into()));
    h.run(&deps).await.unwrap();
    let counts = deps.counts();
    assert_eq!((counts.executed, counts.resolved, counts.sent), (0, 0, 1));
}

#[tokio::test]
async fn reassesses_a_completed_issue_when_it_is_reopened_with_a_human_comment() {
    let h = Harness::new().await;
    let deps = Deps::new(issue(7), replace());
    h.run(&deps).await.unwrap();
    deps.with(|s| s.status = 1);
    deps.add_comment("Please retry this report");
    h.run(&deps).await.unwrap();
    let counts = deps.counts();
    assert_eq!((counts.assessed, counts.executed, counts.sent), (2, 2, 2));
}

#[tokio::test]
async fn does_not_repeat_repair_when_a_changed_comment_interrupts_completion() {
    let h = Harness::new().await;
    let deps = Deps::new(issue(8), replace());
    deps.with(|s| s.fail_resolve_once = true);
    assert!(h.run(&deps).await.is_err());
    deps.add_comment("A new detail from the user");
    h.run(&deps).await.unwrap();
    let counts = deps.counts();
    assert_eq!(
        (counts.assessed, counts.executed, counts.resolved),
        (1, 1, 0)
    );
}

#[tokio::test]
async fn retries_a_confirmed_pushover_rejection_without_repeating_the_repair() {
    let h = Harness::new().await;
    let deps = Deps::new(issue(9), replace());
    deps.with(|s| s.send_failure = Some(Some(429)));
    assert!(h.run(&deps).await.is_err());
    assert_eq!(deps.counts().sent, 0);
    h.run(&deps).await.unwrap();
    let counts = deps.counts();
    assert_eq!((counts.executed, counts.comments, counts.sent), (1, 1, 1));
}

#[tokio::test]
async fn holds_an_ambiguous_pushover_delivery_instead_of_sending_twice() {
    let h = Harness::new().await;
    let deps = Deps::new(issue(10), replace());
    deps.with(|s| s.send_failure = Some(None));
    assert!(h.run(&deps).await.is_err());
    let error = h.run(&deps).await.unwrap_err();
    assert!(
        error.to_string().contains("complete Observer issues 10"),
        "{error}"
    );
    let counts = deps.counts();
    assert_eq!((counts.executed, counts.sent), (1, 0));
}
