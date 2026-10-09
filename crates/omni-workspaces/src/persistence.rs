//! Workspace repository. Every function is
//! one store job; callers wrap failures with the operation name they report.

use std::cmp::Ordering;

use omni_api::workspaces::{
    WorkspaceActionStatus, WorkspaceActionType, WorkspaceArtifactKind, WorkspaceEmailScope,
    WorkspaceMessageRole, WorkspacePapercutCategory, WorkspacePapercutStatus,
    WorkspaceSubjectStatus,
};
use omni_core::ids::uuid_v4;
use omni_core::js::locale_compare;
use omni_store::cbor::Extra;
use omni_store::entity::{EntityOps as _, EntityWrite as _, ModifyOpts, UpsertOpts};
use omni_store::{Store, StoreError};

use crate::entities::{
    ActionRow, ArtifactRevisionRow, EmailScopeRow, MessageRow, NotificationRow, NotificationStatus,
    PapercutRow, SourceRow, SubjectRow,
};

/// Delivery backoff: `min(5 min * 2^min(max(attempts - 1, 0), 6), 6 h)`.
pub fn notification_retry_delay_ms(attempts: i64) -> i64 {
    let exponent = (attempts - 1).clamp(0, 6);
    (5 * 60_000 * (1_i64 << exponent)).min(6 * 60 * 60_000)
}

/// The last attempt's outcome cannot be known; never resend automatically.
pub const UNKNOWN_OUTCOME_ERROR: &str =
    "Provider attempt outcome is unknown; suppressed automatic resend";

/// Latest revision per artifact key, sorted by key (`localeCompare`).
pub fn latest_artifacts(rows: Vec<ArtifactRevisionRow>) -> Vec<ArtifactRevisionRow> {
    let mut latest: Vec<ArtifactRevisionRow> = Vec::new();
    for row in rows {
        match latest
            .iter_mut()
            .find(|r| r.artifact_key == row.artifact_key)
        {
            Some(prior) if row.created_at > prior.created_at => *prior = row,
            Some(_) => {}
            None => latest.push(row),
        }
    }
    latest.sort_by(|a, b| locale_compare(&a.artifact_key, &b.artifact_key));
    latest
}

fn papercut_order(a: &PapercutRow, b: &PapercutRow) -> Ordering {
    let open = |p: &PapercutRow| p.status == WorkspacePapercutStatus::Open;
    open(b)
        .cmp(&open(a))
        .then(b.occurrences.cmp(&a.occurrences))
        .then(b.last_seen_at.cmp(&a.last_seen_at))
}

/// Subject upsert input: timestamps default from the prior row and now.
#[derive(Clone, Debug)]
pub struct SubjectUpsert {
    pub workspace_id: String,
    pub subject_id: String,
    pub title: String,
    pub status: WorkspaceSubjectStatus,
    pub summary: String,
    pub created_at: Option<i64>,
    pub updated_at: Option<i64>,
    pub last_researched_at: Option<i64>,
}

/// New message input.
#[derive(Clone, Debug)]
pub struct NewMessage {
    pub workspace_id: String,
    pub subject_id: Option<String>,
    pub role: WorkspaceMessageRole,
    pub text: String,
    pub run_id: Option<String>,
}

/// New source input; id and time default to a new UUID and now.
#[derive(Clone, Debug)]
pub struct NewSource {
    pub source_id: Option<String>,
    pub workspace_id: String,
    pub subject_id: String,
    pub kind: omni_api::workspaces::WorkspaceSourceKind,
    pub title: String,
    pub url: Option<String>,
    pub excerpt: String,
    pub email_id: Option<String>,
    pub run_id: Option<String>,
}

/// Papercut report input.
#[derive(Clone, Debug)]
pub struct NewPapercut {
    pub workspace_id: String,
    pub subject_id: Option<String>,
    pub run_id: Option<String>,
    pub category: WorkspacePapercutCategory,
    pub title: String,
    pub detail: String,
    pub related_tool: Option<String>,
}

/// New action input (preview seeding).
#[derive(Clone, Debug)]
pub struct NewAction {
    pub workspace_id: String,
    pub subject_id: String,
    pub action_type: WorkspaceActionType,
    pub title: String,
    pub description: String,
    pub payload: String,
    pub run_id: Option<String>,
}

/// New artifact revision input (preview seeding).
#[derive(Clone, Debug)]
pub struct NewArtifactRevision {
    pub workspace_id: String,
    pub subject_id: String,
    pub artifact_key: String,
    pub kind: WorkspaceArtifactKind,
    pub content: String,
    pub summary: String,
    pub run_id: Option<String>,
}

/// `[workspaceId, category, relatedTool ?? "", title.trim().toLowerCase()].join(":")`.
pub fn papercut_fingerprint(input: &NewPapercut) -> String {
    [
        input.workspace_id.as_str(),
        input.category.as_str(),
        input.related_tool.as_deref().unwrap_or(""),
        &crate::text::js_trim(&input.title).to_lowercase(),
    ]
    .join(":")
}

/// The workspace repository over the shared store.
#[derive(Clone, Debug)]
pub struct WorkspaceRepo {
    store: Store,
}

impl WorkspaceRepo {
    pub fn new(store: Store) -> Self {
        Self { store }
    }

    pub fn store(&self) -> &Store {
        &self.store
    }

    pub fn now_ms(&self) -> i64 {
        self.store.clock().now_ms()
    }

    /// Subjects of one workspace, most recently updated first.
    pub async fn list_subjects(&self, workspace_id: &str) -> Result<Vec<SubjectRow>, StoreError> {
        let workspace_id = workspace_id.to_owned();
        let mut rows = self
            .store
            .read(move |docs| {
                Ok(docs
                    .get_all::<SubjectRow>()?
                    .into_iter()
                    .filter(|s| s.workspace_id == workspace_id)
                    .collect::<Vec<_>>())
            })
            .await?;
        rows.sort_by_key(|row| std::cmp::Reverse(row.updated_at));
        Ok(rows)
    }

    pub async fn get_subject(
        &self,
        workspace_id: &str,
        subject_id: &str,
    ) -> Result<Option<SubjectRow>, StoreError> {
        let key = (workspace_id.to_owned(), subject_id.to_owned());
        self.store
            .read(move |docs| docs.get::<SubjectRow>(&key))
            .await
    }

    /// Keeps the prior `createdAt` and
    /// `lastResearchedAt` (unless given) and any unknown stored fields.
    pub async fn upsert_subject(&self, input: SubjectUpsert) -> Result<SubjectRow, StoreError> {
        let now = self.now_ms();
        self.store
            .write(move |tx| {
                let key = (input.workspace_id.clone(), input.subject_id.clone());
                let prior = tx.get::<SubjectRow>(&key)?;
                let updated_at = input.updated_at.unwrap_or(now);
                let row = SubjectRow {
                    created_at: prior
                        .as_ref()
                        .map(|p| p.created_at)
                        .or(input.created_at)
                        .unwrap_or(updated_at),
                    last_researched_at: input
                        .last_researched_at
                        .or_else(|| prior.as_ref().and_then(|p| p.last_researched_at)),
                    extra: prior.map(|p| p.extra).unwrap_or_default(),
                    workspace_id: input.workspace_id,
                    subject_id: input.subject_id,
                    title: input.title,
                    status: input.status,
                    summary: input.summary,
                    updated_at,
                };
                tx.upsert(&row, UpsertOpts::default())?;
                Ok(row)
            })
            .await
    }

    pub async fn latest_artifacts(
        &self,
        workspace_id: &str,
        subject_id: &str,
    ) -> Result<Vec<ArtifactRevisionRow>, StoreError> {
        let rows = self.artifact_rows(workspace_id, subject_id, None).await?;
        Ok(latest_artifacts(rows))
    }

    /// Revisions of one subject (optionally one artifact), newest first.
    pub async fn list_artifact_revisions(
        &self,
        workspace_id: &str,
        subject_id: &str,
        artifact_key: Option<&str>,
    ) -> Result<Vec<ArtifactRevisionRow>, StoreError> {
        let mut rows = self
            .artifact_rows(workspace_id, subject_id, artifact_key)
            .await?;
        rows.sort_by_key(|row| std::cmp::Reverse(row.created_at));
        Ok(rows)
    }

    async fn artifact_rows(
        &self,
        workspace_id: &str,
        subject_id: &str,
        artifact_key: Option<&str>,
    ) -> Result<Vec<ArtifactRevisionRow>, StoreError> {
        let (workspace_id, subject_id) = (workspace_id.to_owned(), subject_id.to_owned());
        let artifact_key = artifact_key.map(str::to_owned);
        self.store
            .read(move |docs| {
                Ok(docs
                    .get_all::<ArtifactRevisionRow>()?
                    .into_iter()
                    .filter(|r| {
                        r.workspace_id == workspace_id
                            && r.subject_id == subject_id
                            && artifact_key.as_ref().is_none_or(|k| &r.artifact_key == k)
                    })
                    .collect())
            })
            .await
    }

    /// `None` when the content is unchanged.
    pub async fn add_artifact_revision(
        &self,
        input: NewArtifactRevision,
    ) -> Result<Option<ArtifactRevisionRow>, StoreError> {
        let now = self.now_ms();
        self.store
            .write(move |tx| {
                let prior = latest_artifacts(
                    tx.get_all::<ArtifactRevisionRow>()?
                        .into_iter()
                        .filter(|r| {
                            r.workspace_id == input.workspace_id && r.subject_id == input.subject_id
                        })
                        .collect(),
                )
                .into_iter()
                .find(|r| r.artifact_key == input.artifact_key);
                if prior.is_some_and(|p| p.content == input.content) {
                    return Ok(None);
                }
                let row = ArtifactRevisionRow {
                    workspace_id: input.workspace_id,
                    subject_id: input.subject_id,
                    artifact_key: input.artifact_key,
                    kind: input.kind,
                    content: input.content,
                    summary: input.summary,
                    revision_id: uuid_v4(),
                    created_at: now,
                    run_id: input.run_id,
                    extra: Extra::new(),
                };
                tx.upsert(&row, UpsertOpts::default())?;
                Ok(Some(row))
            })
            .await
    }

    pub async fn add_message(&self, input: NewMessage) -> Result<MessageRow, StoreError> {
        let row = MessageRow {
            workspace_id: input.workspace_id,
            subject_id: input.subject_id,
            message_id: uuid_v4(),
            role: input.role,
            text: input.text,
            created_at: self.now_ms(),
            run_id: input.run_id,
            extra: Extra::new(),
        };
        let stored = row.clone();
        self.store
            .write(move |tx| tx.upsert(&stored, UpsertOpts::default()))
            .await?;
        Ok(row)
    }

    /// The newest `limit` messages (optionally of one subject), oldest first.
    pub async fn list_messages(
        &self,
        workspace_id: &str,
        subject_id: Option<&str>,
        limit: usize,
    ) -> Result<Vec<MessageRow>, StoreError> {
        let workspace_id = workspace_id.to_owned();
        let subject_id = subject_id.map(str::to_owned);
        let mut rows = self
            .store
            .read(move |docs| {
                Ok(docs
                    .get_all::<MessageRow>()?
                    .into_iter()
                    .filter(|m| {
                        m.workspace_id == workspace_id
                            && subject_id
                                .as_ref()
                                .is_none_or(|s| m.subject_id.as_ref() == Some(s))
                    })
                    .collect::<Vec<_>>())
            })
            .await?;
        rows.sort_by_key(|row| std::cmp::Reverse(row.created_at));
        rows.truncate(limit);
        rows.reverse();
        Ok(rows)
    }

    pub async fn add_source(&self, input: NewSource) -> Result<SourceRow, StoreError> {
        let row = SourceRow {
            workspace_id: input.workspace_id,
            subject_id: input.subject_id,
            source_id: input.source_id.unwrap_or_else(uuid_v4),
            kind: input.kind,
            title: input.title,
            url: input.url,
            excerpt: input.excerpt,
            email_id: input.email_id,
            created_at: self.now_ms(),
            run_id: input.run_id,
            triggered_at: None,
            extra: Extra::new(),
        };
        let stored = row.clone();
        self.store
            .write(move |tx| tx.upsert(&stored, UpsertOpts::default()))
            .await?;
        Ok(row)
    }

    pub async fn get_source(&self, source_id: &str) -> Result<Option<SourceRow>, StoreError> {
        let key = source_id.to_owned();
        self.store
            .read(move |docs| docs.get::<SourceRow>(&key))
            .await
    }

    /// Marks email sources triggered once their workspace run completed.
    pub async fn mark_sources_triggered(&self, source_ids: Vec<String>) -> Result<(), StoreError> {
        let triggered_at = self.now_ms();
        self.store
            .write(move |tx| {
                for source_id in &source_ids {
                    tx.update::<SourceRow>(
                        source_id,
                        |mut row| {
                            row.triggered_at = Some(triggered_at);
                            row
                        },
                        ModifyOpts::default(),
                    )?;
                }
                Ok(())
            })
            .await
    }

    /// The newest `limit` sources of one subject, newest first.
    pub async fn list_sources(
        &self,
        workspace_id: &str,
        subject_id: &str,
        limit: usize,
    ) -> Result<Vec<SourceRow>, StoreError> {
        let (workspace_id, subject_id) = (workspace_id.to_owned(), subject_id.to_owned());
        let mut rows = self
            .store
            .read(move |docs| {
                Ok(docs
                    .get_all::<SourceRow>()?
                    .into_iter()
                    .filter(|s| s.workspace_id == workspace_id && s.subject_id == subject_id)
                    .collect::<Vec<_>>())
            })
            .await?;
        rows.sort_by_key(|row| std::cmp::Reverse(row.created_at));
        rows.truncate(limit);
        Ok(rows)
    }

    /// Reuses an identical pending proposal.
    pub async fn add_action(&self, input: NewAction) -> Result<(ActionRow, bool), StoreError> {
        let now = self.now_ms();
        self.store
            .write(move |tx| {
                if let Some(duplicate) = tx.get_all::<ActionRow>()?.into_iter().find(|row| {
                    row.workspace_id == input.workspace_id
                        && row.subject_id == input.subject_id
                        && row.action_type == input.action_type
                        && row.status == WorkspaceActionStatus::Pending
                        && row.payload == input.payload
                }) {
                    return Ok((duplicate, false));
                }
                let row = ActionRow {
                    workspace_id: input.workspace_id,
                    subject_id: input.subject_id,
                    action_id: uuid_v4(),
                    action_type: input.action_type,
                    status: WorkspaceActionStatus::Pending,
                    title: input.title,
                    description: input.description,
                    payload: input.payload,
                    created_at: now,
                    run_id: input.run_id,
                    result: None,
                    resolved_at: None,
                    extra: Extra::new(),
                };
                tx.upsert(&row, UpsertOpts::default())?;
                Ok((row, true))
            })
            .await
    }

    /// Actions of one workspace (optionally one subject), newest first.
    pub async fn list_actions(
        &self,
        workspace_id: &str,
        subject_id: Option<&str>,
    ) -> Result<Vec<ActionRow>, StoreError> {
        let workspace_id = workspace_id.to_owned();
        let subject_id = subject_id.map(str::to_owned);
        let mut rows = self
            .store
            .read(move |docs| {
                Ok(docs
                    .get_all::<ActionRow>()?
                    .into_iter()
                    .filter(|a| {
                        a.workspace_id == workspace_id
                            && subject_id.as_ref().is_none_or(|s| &a.subject_id == s)
                    })
                    .collect::<Vec<_>>())
            })
            .await?;
        rows.sort_by_key(|row| std::cmp::Reverse(row.created_at));
        Ok(rows)
    }

    pub async fn get_action(&self, action_id: &str) -> Result<Option<ActionRow>, StoreError> {
        let key = action_id.to_owned();
        self.store
            .read(move |docs| docs.get::<ActionRow>(&key))
            .await
    }

    /// `None` when the action is gone.
    pub async fn set_action_result(
        &self,
        action_id: &str,
        status: WorkspaceActionStatus,
        result: &str,
    ) -> Result<Option<ActionRow>, StoreError> {
        let (key, result) = (action_id.to_owned(), result.to_owned());
        let now = self.now_ms();
        self.store
            .write(move |tx| {
                tx.update::<ActionRow>(
                    &key,
                    |mut row| {
                        row.status = status;
                        row.result = Some(result);
                        row.resolved_at = Some(now);
                        row
                    },
                    ModifyOpts::default(),
                )
            })
            .await
    }

    pub async fn upsert_email_scope(
        &self,
        workspace_id: &str,
        subject_id: &str,
        scope: WorkspaceEmailScope,
    ) -> Result<EmailScopeRow, StoreError> {
        let row = EmailScopeRow {
            workspace_id: workspace_id.to_owned(),
            subject_id: subject_id.to_owned(),
            senders: scope.senders,
            domains: scope.domains,
            subject_keywords: scope.subject_keywords,
            body_keywords: scope.body_keywords,
            updated_at: self.now_ms(),
            extra: Extra::new(),
        };
        let stored = row.clone();
        self.store
            .write(move |tx| tx.upsert(&stored, UpsertOpts::default()))
            .await?;
        Ok(row)
    }

    pub async fn get_email_scope(
        &self,
        workspace_id: &str,
        subject_id: &str,
    ) -> Result<Option<EmailScopeRow>, StoreError> {
        let key = (workspace_id.to_owned(), subject_id.to_owned());
        self.store
            .read(move |docs| docs.get::<EmailScopeRow>(&key))
            .await
    }

    pub async fn list_email_scopes(
        &self,
        workspace_id: &str,
    ) -> Result<Vec<EmailScopeRow>, StoreError> {
        let workspace_id = workspace_id.to_owned();
        Ok(self
            .list_all_email_scopes()
            .await?
            .into_iter()
            .filter(|s| s.workspace_id == workspace_id)
            .collect())
    }

    pub async fn list_all_email_scopes(&self) -> Result<Vec<EmailScopeRow>, StoreError> {
        self.store
            .read(|docs| docs.get_all::<EmailScopeRow>())
            .await
    }

    /// Records a papercut, folding repeats of an open one into its occurrence count.
    pub async fn report_papercut(&self, input: NewPapercut) -> Result<PapercutRow, StoreError> {
        let now = self.now_ms();
        self.store
            .write(move |tx| {
                let fingerprint = papercut_fingerprint(&input);
                let prior = tx.get_all::<PapercutRow>()?.into_iter().find(|row| {
                    row.fingerprint == fingerprint && row.status == WorkspacePapercutStatus::Open
                });
                if let Some(prior) = prior {
                    let updated = tx.update::<PapercutRow>(
                        &prior.papercut_id,
                        |mut row| {
                            row.detail = input.detail;
                            row.run_id = input.run_id;
                            row.subject_id = input.subject_id;
                            row.occurrences = prior.occurrences + 1;
                            row.last_seen_at = now;
                            row
                        },
                        ModifyOpts::default(),
                    )?;
                    return updated.ok_or_else(|| StoreError::CorruptRow {
                        pk: prior.papercut_id,
                        reason: "papercut disappeared inside its transaction".to_owned(),
                    });
                }
                let row = PapercutRow {
                    workspace_id: input.workspace_id,
                    subject_id: input.subject_id,
                    run_id: input.run_id,
                    category: input.category,
                    title: input.title,
                    detail: input.detail,
                    related_tool: input.related_tool,
                    papercut_id: uuid_v4(),
                    fingerprint,
                    occurrences: 1,
                    first_seen_at: now,
                    last_seen_at: now,
                    status: WorkspacePapercutStatus::Open,
                    resolution: None,
                    extra: Extra::new(),
                };
                tx.upsert(&row, UpsertOpts::default())?;
                Ok(row)
            })
            .await
    }

    /// Open first, then by occurrences and recency.
    pub async fn list_papercuts(
        &self,
        workspace_id: Option<&str>,
        status: Option<WorkspacePapercutStatus>,
    ) -> Result<Vec<PapercutRow>, StoreError> {
        let workspace_id = workspace_id.map(str::to_owned);
        let mut rows = self
            .store
            .read(move |docs| {
                Ok(docs
                    .get_all::<PapercutRow>()?
                    .into_iter()
                    .filter(|p| {
                        workspace_id.as_ref().is_none_or(|w| &p.workspace_id == w)
                            && status.is_none_or(|s| p.status == s)
                    })
                    .collect::<Vec<_>>())
            })
            .await?;
        rows.sort_by(papercut_order);
        Ok(rows)
    }

    pub async fn resolve_papercut(
        &self,
        papercut_id: &str,
        status: WorkspacePapercutStatus,
        resolution: &str,
    ) -> Result<Option<PapercutRow>, StoreError> {
        let (key, resolution) = (papercut_id.to_owned(), resolution.to_owned());
        self.store
            .write(move |tx| {
                tx.update::<PapercutRow>(
                    &key,
                    |mut row| {
                        row.status = status;
                        row.resolution = Some(resolution);
                        row
                    },
                    ModifyOpts::default(),
                )
            })
            .await
    }

    /// Outbox rows to process now: every `sending` row (to acknowledge) and
    /// due `pending` rows, oldest due first, at most `limit`.
    pub async fn list_due_notifications(
        &self,
        now: i64,
        limit: usize,
    ) -> Result<Vec<NotificationRow>, StoreError> {
        let mut rows = self
            .store
            .read(move |docs| {
                Ok(docs
                    .get_all::<NotificationRow>()?
                    .into_iter()
                    .filter(|n| {
                        n.status == NotificationStatus::Sending
                            || (n.status == NotificationStatus::Pending && n.next_attempt_at <= now)
                    })
                    .collect::<Vec<_>>())
            })
            .await?;
        rows.sort_by_key(|row| row.next_attempt_at);
        rows.truncate(limit);
        Ok(rows)
    }

    async fn modify_notification(
        &self,
        notification_id: &str,
        f: impl FnOnce(NotificationRow) -> NotificationRow + Send + 'static,
    ) -> Result<(), StoreError> {
        let key = notification_id.to_owned();
        self.store
            .write(move |tx| {
                tx.update::<NotificationRow>(&key, f, ModifyOpts::default())
                    .map(drop)
            })
            .await
    }

    pub async fn mark_notification_sent(&self, notification_id: &str) -> Result<(), StoreError> {
        let now = self.now_ms();
        self.modify_notification(notification_id, move |mut row| {
            row.status = NotificationStatus::Sent;
            row.sent_at = Some(now);
            row.last_error = None;
            row
        })
        .await
    }

    /// Reserves one at-most-once provider attempt before leaving SQLite.
    pub async fn mark_notification_sending(
        &self,
        notification_id: &str,
        attempts: i64,
    ) -> Result<(), StoreError> {
        self.modify_notification(notification_id, move |mut row| {
            row.status = NotificationStatus::Sending;
            row.attempts = attempts;
            row.last_error = None;
            row
        })
        .await
    }

    pub async fn mark_notification_unknown(&self, notification_id: &str) -> Result<(), StoreError> {
        self.modify_notification(notification_id, |mut row| {
            row.status = NotificationStatus::Unknown;
            row.last_error = Some(UNKNOWN_OUTCOME_ERROR.to_owned());
            row
        })
        .await
    }

    pub async fn mark_notification_failed(
        &self,
        notification_id: &str,
        attempts: i64,
        error: String,
    ) -> Result<(), StoreError> {
        let next = self.now_ms() + notification_retry_delay_ms(attempts);
        self.modify_notification(notification_id, move |mut row| {
            row.status = NotificationStatus::Pending;
            row.attempts = attempts;
            row.last_error = Some(error);
            row.next_attempt_at = next;
            row
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retry_delay_doubles_up_to_six_doublings() {
        assert_eq!(notification_retry_delay_ms(0), 5 * 60_000);
        assert_eq!(notification_retry_delay_ms(1), 5 * 60_000);
        assert_eq!(notification_retry_delay_ms(2), 10 * 60_000);
        assert_eq!(notification_retry_delay_ms(4), 40 * 60_000);
        // The exponent caps at 6 (320 min), below the 6 h ceiling.
        assert_eq!(notification_retry_delay_ms(7), 320 * 60_000);
        assert_eq!(notification_retry_delay_ms(50), 320 * 60_000);
    }

    #[test]
    fn fingerprints_normalize_titles() {
        let input = NewPapercut {
            workspace_id: "purchase-research".to_owned(),
            subject_id: None,
            run_id: None,
            category: WorkspacePapercutCategory::MissingCapability,
            title: "  Price History API ".to_owned(),
            detail: String::new(),
            related_tool: None,
        };
        assert_eq!(
            papercut_fingerprint(&input),
            "purchase-research:missing-capability::price history api"
        );
    }
}
