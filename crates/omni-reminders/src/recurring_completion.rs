//! One-shot recurring completion with fresh verification.
//!
//! Apple's public Reminders web build 2636Build17 uses a mutating
//! `CompleteRecurringReminder` query instead of a generic Completed update
//! (independently implemented wire request). The query has no change-tag CAS
//! parameter: the caller must reserve durably and check identities and tags first. A
//! response alone is not proof; the request is never replayed after a lost response
//! and no pagination cursor is followed by issuing it again.

use futures::future::BoxFuture;
use jiff::Timestamp;
use jiff::tz::TimeZone;
use omni_core::clock::SharedClock;
use serde::Serialize;
use serde_json::{Value, json};

use crate::cloudkit::{
    CkPost, CkRecord, ErrorCode, Reminder, RemindersCloudKitClient, RemindersError, fail,
};
use crate::cloudkit_extras::ReminderRecurrences;

const OPERATION: &str = "recurring completion query";

/// The exact occurrence to complete (all tags from a fresh read).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecurringCompletionTarget {
    pub id: String,
    pub change_tag: String,
    pub rule_id: String,
    pub rule_change_tag: String,
    pub time_zone: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CompletionState {
    Advanced,
    Ended,
}

/// A receipt whose tags all come from fresh reads.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VerifiedRecurringCompletion {
    pub state: CompletionState,
    pub verified: bool,
    pub reminder_id: String,
    pub reminder_change_tag: String,
    pub completed_reminder_id: String,
    pub completed_reminder_change_tag: String,
    pub rule_id: String,
    pub time_zone: String,
    pub previous_due_date: i64,
    pub next_due_date: Option<i64>,
}

/// The query input (`RecurringCompletionInput`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RecurringCompletionInput {
    pub reminder_id: String,
    pub rule_id: String,
    pub time_zone: String,
    pub owner_record_name: Option<String>,
}

/// Bounded, unverified records for exact follow-up lookups.
#[derive(Clone, Debug, PartialEq)]
pub struct RecurringCompletionResponse {
    pub records: Vec<CkRecord>,
    pub verified: bool,
}

fn failure(code: ErrorCode) -> RemindersError {
    fail(OPERATION, code)
}

fn uncertain() -> RemindersError {
    fail("recurring completion outcome", ErrorCode::Uncertain)
}

fn identity(s: &str) -> bool {
    (1..=256).contains(&omni_core::js::utf16_len(s))
}

/// `new Intl.DateTimeFormat("en", {timeZone})`: IANA names (case-insensitive,
/// including links) and ECMA-402 offset identifiers `±HH`, `±HHMM`, `±HH:MM`.
pub(crate) fn time_zone(name: &str) -> Option<TimeZone> {
    if name.is_empty() || omni_core::js::utf16_len(name) > 128 {
        return None;
    }
    offset_time_zone(name).or_else(|| {
        // jiff resolves `Etc/Unknown` to its sentinel zone; V8 rejects it.
        TimeZone::get(name).ok().filter(|tz| !tz.is_unknown())
    })
}

fn offset_time_zone(name: &str) -> Option<TimeZone> {
    let first = name.chars().next()?;
    let sign = match first {
        '+' => 1,
        // V8 also accepts U+2212 MINUS SIGN.
        '-' | '\u{2212}' => -1,
        _ => return None,
    };
    let rest = &name[first.len_utf8()..];
    if !rest.is_ascii() {
        return None;
    }
    let digits = |s: &str| -> Option<i32> {
        (s.len() == 2 && s.bytes().all(|b| b.is_ascii_digit()))
            .then(|| s.parse().ok())
            .flatten()
    };
    let (hours, minutes) = match rest.len() {
        2 => (digits(rest)?, 0),
        4 => (digits(&rest[..2])?, digits(&rest[2..])?),
        5 if rest.as_bytes()[2] == b':' => (digits(&rest[..2])?, digits(&rest[3..])?),
        _ => return None,
    };
    if hours > 23 || minutes > 59 {
        return None;
    }
    let offset = jiff::tz::Offset::from_seconds(sign * (hours * 3600 + minutes * 60)).ok()?;
    Some(TimeZone::fixed(offset))
}

fn decode_completion_record(value: &Value) -> Option<CkRecord> {
    let record = crate::cloudkit::decode_lenient_record(value)?;
    let tag_ok = record.record_change_tag.as_deref().is_none_or(identity);
    (identity(&record.record_name) && tag_ok).then_some(record)
}

/// One mutating request. The owning service must reserve before calling this.
pub async fn request_recurring_completion(
    post: &dyn CkPost,
    input: &RecurringCompletionInput,
) -> Result<RecurringCompletionResponse, RemindersError> {
    if !identity(&input.reminder_id)
        || !identity(&input.rule_id)
        || input
            .owner_record_name
            .as_deref()
            .is_some_and(|o| !identity(o))
        || !input.reminder_id.starts_with("Reminder/")
        || !input.rule_id.starts_with("RecurrenceRule/")
        || time_zone(&input.time_zone).is_none()
    {
        return Err(failure(ErrorCode::Invalid));
    }
    let reference = |name: &str| json!({"type": "REFERENCE", "value": {"recordName": name, "action": "VALIDATE"}});
    let mut zone = json!({"zoneName": "Reminders", "zoneType": "REGULAR_CUSTOM_ZONE"});
    if let (Some(owner), Some(object)) = (&input.owner_record_name, zone.as_object_mut()) {
        object.insert("ownerRecordName".into(), Value::String(owner.clone()));
    }
    let response = post
        .post(
            "/records/query",
            json!({
                "zoneID": zone,
                "query": {
                    "recordType": "CompleteRecurringReminder",
                    "filterBy": [
                        {"comparator": "EQUALS", "fieldName": "Reminder", "fieldValue": reference(&input.reminder_id)},
                        {"comparator": "EQUALS", "fieldName": "RecurrenceRule", "fieldValue": reference(&input.rule_id)},
                        {"comparator": "EQUALS", "fieldName": "TimeZone", "fieldValue": {"type": "STRING", "value": input.time_zone}},
                    ],
                },
            }),
        )
        .await?;
    let object = response
        .as_object()
        .ok_or_else(|| failure(ErrorCode::Protocol))?;
    let raw = object
        .get("records")
        .and_then(Value::as_array)
        .filter(|r| r.len() <= 100)
        .ok_or_else(|| failure(ErrorCode::Protocol))?;
    let records = raw
        .iter()
        .map(decode_completion_record)
        .collect::<Option<Vec<_>>>()
        .ok_or_else(|| failure(ErrorCode::Protocol))?;
    let continuation = match object.get("continuationMarker") {
        None | Some(Value::Null) => false,
        Some(Value::String(s)) => !s.is_empty(),
        Some(_) => return Err(failure(ErrorCode::Protocol)),
    };
    let unique: std::collections::HashSet<&str> =
        records.iter().map(|r| r.record_name.as_str()).collect();
    if continuation
        || records.is_empty()
        || unique.len() != records.len()
        || records.iter().any(CkRecord::has_error)
    {
        return Err(failure(ErrorCode::Protocol));
    }
    // Retain bounded records for exact follow-up lookup. Do not infer a completed
    // series, successful mutation, or a next occurrence from their mere presence.
    Ok(RecurringCompletionResponse {
        records,
        verified: false,
    })
}

/// What completion needs from the CloudKit client (a test seam).
pub trait CompletionDeps: Send + Sync {
    fn post(&self) -> &dyn CkPost;
    fn clock(&self) -> &SharedClock;
    fn get_reminder<'a>(
        &'a self,
        id: &'a str,
    ) -> BoxFuture<'a, Result<Option<Reminder>, RemindersError>>;
    fn get_recurrences<'a>(
        &'a self,
        id: &'a str,
    ) -> BoxFuture<'a, Result<ReminderRecurrences, RemindersError>>;
}

impl CompletionDeps for RemindersCloudKitClient {
    fn post(&self) -> &dyn CkPost {
        self.post_seam()
    }

    fn clock(&self) -> &SharedClock {
        RemindersCloudKitClient::clock(self)
    }

    fn get_reminder<'a>(
        &'a self,
        id: &'a str,
    ) -> BoxFuture<'a, Result<Option<Reminder>, RemindersError>> {
        Box::pin(RemindersCloudKitClient::get_reminder(self, id))
    }

    fn get_recurrences<'a>(
        &'a self,
        id: &'a str,
    ) -> BoxFuture<'a, Result<ReminderRecurrences, RemindersError>> {
        Box::pin(self.extras.get_recurrences(id))
    }
}

fn zoned(ms: i64, tz: &TimeZone) -> Option<jiff::Zoned> {
    Timestamp::from_millisecond(ms)
        .ok()
        .map(|t| t.to_zoned(tz.clone()))
}

/// `HH:mm:ss.SSS` on the wall clock of `tz`.
fn local_clock(ms: i64, tz: &TimeZone) -> Option<(i8, i8, i8, i16)> {
    zoned(ms, tz).map(|z| (z.hour(), z.minute(), z.second(), z.millisecond()))
}

/// The civil date in `tz`.
fn civil_date(ms: i64, tz: &TimeZone) -> Option<jiff::civil::Date> {
    zoned(ms, tz).map(|z| z.date())
}

fn number_is(value: &Value, expected: Option<i64>) -> bool {
    match (value, expected) {
        (Value::Null, None) => true,
        (Value::Number(n), Some(e)) => n.as_f64() == Some(e as f64),
        _ => false,
    }
}

/// Binds a preview record's typed completion and due-date values to a fresh lookup.
fn preview_matches(record: &CkRecord, reminder: &Reminder) -> bool {
    let completed = record.field("Completed");
    let due = record.field("DueDate");
    let completed_ok = completed.is_some_and(|f| {
        f.ty == "INT64" && number_is(&f.value, Some(i64::from(reminder.completed)))
    });
    let due_ok = due.is_some_and(|f| f.ty == "TIMESTAMP" && number_is(&f.value, reminder.due_date));
    if !completed_ok || !due_ok {
        return false;
    }
    // Apple's advanced-root preview omits a null CompletionDate. Completed
    // occurrences must provide their explicit timestamp; missing is not success.
    match record.field("CompletionDate") {
        None => !reminder.completed && reminder.completed_date.is_none(),
        Some(f) => f.ty == "TIMESTAMP" && number_is(&f.value, reminder.completed_date),
    }
}

fn unchanged_content(before: &Reminder, after: &Reminder) -> bool {
    before.title == after.title
        && before.description == after.description
        && before.list_id == after.list_id
        && before.priority == after.priority
        && before.flagged == after.flagged
        && before.all_day == after.all_day
}

/// Call only inside the durable mutation reservation and account serialization.
pub async fn complete_recurring_occurrence(
    deps: &dyn CompletionDeps,
    target: &RecurringCompletionTarget,
) -> Result<VerifiedRecurringCompletion, RemindersError> {
    let Some(before) = deps.get_reminder(&target.id).await? else {
        return Err(fail("recurring completion", ErrorCode::NotFound));
    };
    if before.record_change_tag != target.change_tag {
        return Err(fail("recurring completion", ErrorCode::Conflict));
    }
    let Some(original_due) = before
        .due_date
        .filter(|_| !before.completed && before.recurring && !before.deleted)
    else {
        return Err(fail("recurring completion", ErrorCode::Unsupported));
    };
    let related = deps.get_recurrences(&target.id).await?;
    if related.reminder_id != target.id
        || related.reminder_change_tag != target.change_tag
        || related.rules.len() != 1
        || related.rules[0].id != target.rule_id
        || related.rules[0].record_change_tag.as_deref() != Some(target.rule_change_tag.as_str())
    {
        return Err(fail("recurring completion", ErrorCode::Conflict));
    }
    let original_rule = related.rules[0].clone();
    let Some(rule) = original_rule
        .recurrence
        .rule()
        .filter(|_| original_rule.writable)
        .cloned()
    else {
        return Err(fail("recurring completion", ErrorCode::Unsupported));
    };
    // Validate the timezone before the mutation. There is no protocol CAS; concurrent
    // native-client edits are detected only by the verification below.
    let Some(tz) = time_zone(&target.time_zone) else {
        return Err(failure(ErrorCode::Invalid));
    };
    let started = deps.clock().now_ms();
    verify_outcome(
        deps,
        target,
        &before,
        original_due,
        &original_rule,
        &rule,
        &tz,
        started,
    )
    .await
    .map_err(|_| uncertain())
}

#[allow(clippy::too_many_arguments)]
async fn verify_outcome(
    deps: &dyn CompletionDeps,
    target: &RecurringCompletionTarget,
    before: &Reminder,
    original_due: i64,
    original_rule: &crate::cloudkit_extras::ReminderRecurrence,
    rule: &crate::recurrence::RecurrenceRule,
    tz: &TimeZone,
    started: i64,
) -> Result<VerifiedRecurringCompletion, RemindersError> {
    let response = request_recurring_completion(
        deps.post(),
        &RecurringCompletionInput {
            reminder_id: target.id.clone(),
            rule_id: target.rule_id.clone(),
            time_zone: target.time_zone.clone(),
            owner_record_name: None,
        },
    )
    .await?;
    let returned: Vec<&CkRecord> = response
        .records
        .iter()
        .filter(|r| r.record_type.as_deref() == Some("Reminder"))
        .collect();
    let root = returned
        .iter()
        .find(|r| r.record_name == target.id)
        .copied();
    let clone = returned
        .iter()
        .find(|r| r.record_name != target.id)
        .copied();
    let in_window = |completed: Option<i64>, finished: i64| {
        completed.is_some_and(|c| c >= started - 120_000 && c <= finished + 120_000)
    };
    // Query records are previews: live responses inherited the old root tag on both
    // root and clone. Bind their exact IDs and typed completion/date values to fresh
    // lookups, never their non-authoritative recordChangeTag fields.
    if returned.len() == 1
        && let Some(root) = root.filter(|r| r.deleted != Some(true))
    {
        let current = deps.get_reminder(&target.id).await?;
        let finished = deps.clock().now_ms();
        // Proven finite-series shape: the same root completes without a clone. Limit
        // inference to simple rules with at most one occurrence per civil date whose
        // inclusive end boundary lies on this occurrence's civil date.
        let simple = rule.frequency.is_calendar() && !rule.has_selectors();
        let Some(end) = rule.end_date.value().copied() else {
            return Err(uncertain());
        };
        let Some(current) = current else {
            return Err(uncertain());
        };
        let same_day =
            civil_date(end, tz).is_some() && civil_date(end, tz) == civil_date(original_due, tz);
        if !simple
            || end < original_due
            || !same_day
            || current.id != target.id
            || current.deleted
            || !current.completed
            || !current.recurring
            || current.due_date != Some(original_due)
            || current.start_date != before.start_date
            || current.record_change_tag == target.change_tag
            || !preview_matches(root, &current)
            || !unchanged_content(before, &current)
            || !in_window(current.completed_date, finished)
        {
            return Err(uncertain());
        }
        let after = deps.get_recurrences(&target.id).await?;
        if after.reminder_change_tag != current.record_change_tag
            || after.rules.len() != 1
            || after.rules[0].id != original_rule.id
            || after.rules[0].record_change_tag != original_rule.record_change_tag
            || !after.rules[0].writable
            || after.rules[0].recurrence != original_rule.recurrence
        {
            return Err(uncertain());
        }
        return Ok(VerifiedRecurringCompletion {
            state: CompletionState::Ended,
            verified: true,
            reminder_id: current.id.clone(),
            reminder_change_tag: current.record_change_tag.clone(),
            completed_reminder_id: current.id.clone(),
            completed_reminder_change_tag: current.record_change_tag,
            rule_id: target.rule_id.clone(),
            time_zone: target.time_zone.clone(),
            previous_due_date: original_due,
            next_due_date: None,
        });
    }
    let (Some(root), Some(clone)) = (root, clone) else {
        return Err(uncertain());
    };
    if returned.len() != 2 || root.deleted == Some(true) || clone.deleted == Some(true) {
        return Err(uncertain());
    }
    let current = deps.get_reminder(&target.id).await?;
    let completed = deps.get_reminder(&clone.record_name).await?;
    let finished = deps.clock().now_ms();
    let (Some(current), Some(completed)) = (current, completed) else {
        return Err(uncertain());
    };
    let calendar = rule.frequency.is_calendar();
    let Some(next_due) = current.due_date else {
        return Err(uncertain());
    };
    let clock_preserved = before.all_day
        || !calendar
        || (local_clock(next_due, tz).is_some()
            && local_clock(next_due, tz) == local_clock(original_due, tz));
    let start_shift_ok = match before.start_date {
        None => current.start_date.is_none(),
        Some(start) => current.start_date == Some(start + next_due - original_due),
    };
    if current.id != target.id
        || completed.id != clone.record_name
        || current.record_change_tag == target.change_tag
        || !preview_matches(root, &current)
        || !preview_matches(clone, &completed)
        || current.deleted
        || completed.deleted
        || current.completed
        || !current.recurring
        || next_due <= original_due
        || !clock_preserved
        || !completed.completed
        || completed.recurring
        || completed.due_date != Some(original_due)
        || completed.start_date != before.start_date
        || !in_window(completed.completed_date, finished)
        || !unchanged_content(before, &current)
        || !unchanged_content(before, &completed)
        || !start_shift_ok
    {
        return Err(uncertain());
    }
    let after = deps.get_recurrences(&target.id).await?;
    if after.reminder_change_tag != current.record_change_tag
        || after.rules.len() != 1
        || after.rules[0].id != original_rule.id
        || after.rules[0].record_change_tag != original_rule.record_change_tag
        || !after.rules[0].writable
        || !after.rules[0].recurrence.is_supported()
        || after.rules[0].recurrence != original_rule.recurrence
    {
        return Err(uncertain());
    }
    let stable = deps.get_reminder(&completed.id).await?;
    let stable_ok = stable.as_ref().is_some_and(|s| {
        s.record_change_tag == completed.record_change_tag
            && unchanged_content(&completed, s)
            && !s.deleted
            && !s.recurring
            && s.completed
            && s.due_date == completed.due_date
            && s.start_date == completed.start_date
            && s.completed_date == completed.completed_date
    });
    if !stable_ok {
        return Err(uncertain());
    }
    Ok(VerifiedRecurringCompletion {
        state: CompletionState::Advanced,
        verified: true,
        reminder_id: current.id,
        reminder_change_tag: current.record_change_tag,
        completed_reminder_id: completed.id,
        completed_reminder_change_tag: completed.record_change_tag,
        rule_id: target.rule_id.clone(),
        time_zone: target.time_zone.clone(),
        previous_due_date: original_due,
        next_due_date: Some(next_due),
    })
}
