//! CloudKit Reminders records, snapshots and verified writes.
//!
//! Record layout adapted from the MIT-licensed iobroker.icloud implementation at
//! 07a91933e3f05a36d9c8918ece7f3de295aef805. Reads cap at 10,000 records; an
//! incomplete snapshot is an error, never an empty or partial result. Writes check
//! each CloudKit acknowledgment and perform a fresh lookup before confirming.

use std::sync::{Arc, Mutex, PoisonError};

use futures::future::BoxFuture;
use indexmap::IndexMap;
use omni_core::clock::SharedClock;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::cloudkit_extras::RemindersCloudKitExtras;
use crate::codec::{decode_crdt_document, encode_crdt_document};
use crate::recurrence::Opt;
use crate::recurring_completion::{
    RecurringCompletionTarget, VerifiedRecurringCompletion, complete_recurring_occurrence,
};

pub(crate) const MAX_PAGES: usize = 50;
const MAX_LIST_PAGES: usize = 200;
const MAX_QUERY_PAGES: usize = 200;
pub(crate) const MAX_RECORDS: usize = 10_000;
const MAX_FIELD_BYTES: usize = 64 * 1024;

/// A Reminders failure code (`RemindersError.code`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ErrorCode {
    #[serde(rename = "invalid")]
    Invalid,
    #[serde(rename = "protocol")]
    Protocol,
    #[serde(rename = "conflict")]
    Conflict,
    #[serde(rename = "not_found")]
    NotFound,
    #[serde(rename = "unsupported")]
    Unsupported,
    #[serde(rename = "awaiting-device-approval")]
    AwaitingDeviceApproval,
    #[serde(rename = "uncertain")]
    Uncertain,
    #[serde(rename = "transport")]
    Transport,
}

impl ErrorCode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Invalid => "invalid",
            Self::Protocol => "protocol",
            Self::Conflict => "conflict",
            Self::NotFound => "not_found",
            Self::Unsupported => "unsupported",
            Self::AwaitingDeviceApproval => "awaiting-device-approval",
            Self::Uncertain => "uncertain",
            Self::Transport => "transport",
        }
    }
}

/// `RemindersError`: message `Reminders <operation>: <code>`.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("Reminders {operation}: {}", .code.as_str())]
pub struct RemindersError {
    pub operation: String,
    pub code: ErrorCode,
}

pub(crate) fn fail(operation: &str, code: ErrorCode) -> RemindersError {
    RemindersError {
        operation: operation.to_owned(),
        code,
    }
}

pub(crate) fn protocol(operation: &str) -> RemindersError {
    fail(operation, ErrorCode::Protocol)
}

/// A CloudKit field `{type, value}`.
#[derive(Clone, Debug, PartialEq)]
pub struct CkField {
    pub ty: String,
    pub value: Value,
}

/// A decoded CloudKit record (the `RecordSchema` shapes).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CkRecord {
    pub record_name: String,
    pub record_type: Option<String>,
    pub record_change_tag: Option<String>,
    pub fields: Option<IndexMap<String, CkField>>,
    pub created: Option<f64>,
    pub modified: Option<f64>,
    pub deleted: Option<bool>,
    pub server_error_code: Option<String>,
    pub error_code: Option<f64>,
}

pub(crate) type Fields = IndexMap<String, CkField>;

impl CkRecord {
    pub(crate) fn fields(&self) -> &Fields {
        static EMPTY: std::sync::OnceLock<Fields> = std::sync::OnceLock::new();
        self.fields
            .as_ref()
            .unwrap_or_else(|| EMPTY.get_or_init(IndexMap::new))
    }

    pub(crate) fn field(&self, name: &str) -> Option<&CkField> {
        self.fields.as_ref().and_then(|f| f.get(name))
    }

    /// `record.deleted || Deleted(INT64) === 1`.
    pub(crate) fn gone(&self) -> bool {
        self.deleted == Some(true)
            || self
                .field("Deleted")
                .is_some_and(|f| f.ty == "INT64" && f.value.as_f64() == Some(1.0))
    }

    /// `record.serverErrorCode || record.errorCode` (truthy).
    pub(crate) fn has_error(&self) -> bool {
        self.server_error_code
            .as_deref()
            .is_some_and(|c| !c.is_empty())
            || self.error_code.is_some_and(|c| c != 0.0 && !c.is_nan())
    }

    /// The record as CloudKit JSON (round trip for the extras read paths).
    pub(crate) fn to_value(&self) -> Value {
        let mut object = Map::new();
        object.insert("recordName".into(), Value::String(self.record_name.clone()));
        if let Some(t) = &self.record_type {
            object.insert("recordType".into(), Value::String(t.clone()));
        }
        if let Some(t) = &self.record_change_tag {
            object.insert("recordChangeTag".into(), Value::String(t.clone()));
        }
        if let Some(fields) = &self.fields {
            object.insert(
                "fields".into(),
                Value::Object(
                    fields
                        .iter()
                        .map(|(k, f)| (k.clone(), json!({"type": f.ty, "value": f.value})))
                        .collect(),
                ),
            );
        }
        if let Some(d) = self.deleted {
            object.insert("deleted".into(), Value::Bool(d));
        }
        if let Some(c) = &self.server_error_code {
            object.insert("serverErrorCode".into(), Value::String(c.clone()));
        }
        if let Some(c) = self.error_code {
            object.insert("errorCode".into(), json!(c));
        }
        Value::Object(object)
    }
}

/// Absent or a string; anything else fails.
fn opt_string(object: &Map<String, Value>, key: &str) -> Result<Option<String>, ()> {
    match object.get(key) {
        None => Ok(None),
        Some(Value::String(s)) => Ok(Some(s.clone())),
        Some(_) => Err(()),
    }
}

fn opt_number(object: &Map<String, Value>, key: &str) -> Result<Option<f64>, ()> {
    match object.get(key) {
        None => Ok(None),
        Some(Value::Number(n)) => Ok(n.as_f64()),
        Some(_) => Err(()),
    }
}

fn opt_bool(object: &Map<String, Value>, key: &str) -> Result<Option<bool>, ()> {
    match object.get(key) {
        None => Ok(None),
        Some(Value::Bool(b)) => Ok(Some(*b)),
        Some(_) => Err(()),
    }
}

fn opt_timestamp(object: &Map<String, Value>, key: &str) -> Result<Option<f64>, ()> {
    match object.get(key) {
        None => Ok(None),
        Some(Value::Object(inner)) => match inner.get("timestamp") {
            Some(Value::Number(n)) => Ok(n.as_f64()),
            _ => Err(()),
        },
        Some(_) => Err(()),
    }
}

/// Decodes a record strictly.
pub(crate) fn decode_record(value: &Value) -> Option<CkRecord> {
    decode_record_with(value, true)
}

/// Decodes a record for list and recurrence operations: `created`, `modified`
/// and `reason` are not validated
/// (and not retained).
pub(crate) fn decode_lenient_record(value: &Value) -> Option<CkRecord> {
    decode_record_with(value, false)
}

fn decode_record_with(value: &Value, metadata: bool) -> Option<CkRecord> {
    let object = value.as_object()?;
    let record_name = object.get("recordName")?.as_str()?.to_owned();
    let fields = match object.get("fields") {
        None => None,
        Some(Value::Object(raw)) => {
            let mut fields = IndexMap::with_capacity(raw.len());
            for (name, field) in raw {
                let field = field.as_object()?;
                let ty = field.get("type")?.as_str()?.to_owned();
                let value = field.get("value")?.clone();
                fields.insert(name.clone(), CkField { ty, value });
            }
            Some(fields)
        }
        Some(_) => return None,
    };
    let (created, modified) = if metadata {
        opt_string(object, "reason").ok()?;
        (
            opt_timestamp(object, "created").ok()?,
            opt_timestamp(object, "modified").ok()?,
        )
    } else {
        (None, None)
    };
    Some(CkRecord {
        record_name,
        record_type: opt_string(object, "recordType").ok()?,
        record_change_tag: opt_string(object, "recordChangeTag").ok()?,
        fields,
        created,
        modified,
        deleted: opt_bool(object, "deleted").ok()?,
        server_error_code: opt_string(object, "serverErrorCode").ok()?,
        error_code: opt_number(object, "errorCode").ok()?,
    })
}

pub(crate) fn decode_records(value: Option<&Value>) -> Option<Vec<CkRecord>> {
    value?.as_array()?.iter().map(decode_record).collect()
}

pub(crate) fn decode_lenient_records(value: Option<&Value>) -> Option<Vec<CkRecord>> {
    value?
        .as_array()?
        .iter()
        .map(decode_lenient_record)
        .collect()
}

/// A Reminders list.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemindersList {
    pub id: String,
    pub title: String,
    pub color: Option<String>,
    /// Incomplete, nondeleted reminders in this list in the complete snapshot.
    pub count: u64,
    pub record_change_tag: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ListMetadata {
    id: String,
    title: String,
    color: Option<String>,
    record_change_tag: Option<String>,
}

/// One reminder. Dates are absolute UTC epoch milliseconds.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Reminder {
    pub id: String,
    pub list_id: String,
    pub title: String,
    pub description: String,
    pub completed: bool,
    pub completed_date: Option<i64>,
    pub due_date: Option<i64>,
    pub start_date: Option<i64>,
    pub priority: i64,
    pub flagged: bool,
    pub all_day: bool,
    pub deleted: bool,
    pub created_date: Option<i64>,
    pub last_modified_date: Option<i64>,
    pub record_change_tag: String,
    pub recurring: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemindersSnapshot {
    pub lists: Vec<RemindersList>,
    pub reminders: Vec<Reminder>,
}

/// Reminder create input without the record id (the service derives it).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReminderCreateFields {
    pub list_id: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Opt::is_absent")]
    pub due_date: Opt<i64>,
    #[serde(default, skip_serializing_if = "Opt::is_absent")]
    pub start_date: Opt<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub priority: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flagged: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub all_day: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed: Option<bool>,
}

/// Only specified fields change; `null` dates clear them.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReminderPatch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Opt::is_absent")]
    pub due_date: Opt<i64>,
    #[serde(default, skip_serializing_if = "Opt::is_absent")]
    pub start_date: Opt<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub priority: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flagged: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub all_day: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed: Option<bool>,
}

impl ReminderPatch {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    /// Whether `written` carries every patched value.
    fn applied_to(&self, written: &Reminder) -> bool {
        let date = |patch: &Opt<i64>, actual: Option<i64>| match patch {
            Opt::Absent => true,
            Opt::Null => actual.is_none(),
            Opt::Set(v) => actual == Some(*v),
        };
        self.title.as_ref().is_none_or(|t| *t == written.title)
            && self
                .description
                .as_ref()
                .is_none_or(|d| *d == written.description)
            && self.completed.is_none_or(|c| c == written.completed)
            && date(&self.due_date, written.due_date)
            && date(&self.start_date, written.start_date)
            && self.priority.is_none_or(|p| p == written.priority)
            && self.flagged.is_none_or(|f| f == written.flagged)
            && self.all_day.is_none_or(|a| a == written.all_day)
    }
}

/// The CloudKit call seam: `(path, body) -> response JSON`.
pub trait CkPost: Send + Sync {
    fn post(&self, path: &str, body: Value) -> BoxFuture<'static, Result<Value, RemindersError>>;
}

impl<F> CkPost for F
where
    F: Fn(&str, Value) -> BoxFuture<'static, Result<Value, RemindersError>> + Send + Sync,
{
    fn post(&self, path: &str, body: Value) -> BoxFuture<'static, Result<Value, RemindersError>> {
        self(path, body)
    }
}

pub(crate) fn zone() -> Value {
    json!({"zoneName": "Reminders", "zoneType": "REGULAR_CUSTOM_ZONE"})
}

pub(crate) fn int(value: Option<i64>) -> Value {
    json!({"type": "INT64", "value": value})
}

fn stamp(value: Option<i64>) -> Value {
    json!({"type": "TIMESTAMP", "value": value})
}

pub(crate) fn str_field(value: &str) -> Value {
    json!({"type": "STRING", "value": value})
}

/// `value(fields, name, type)`: `Null` when absent, protocol error on a type mismatch.
fn typed<'a>(fields: &'a Fields, name: &str, ty: &str) -> Result<&'a Value, RemindersError> {
    match fields.get(name) {
        None => Ok(&Value::Null),
        Some(field) if field.ty != ty => Err(protocol(&format!("decode {name}"))),
        Some(field) => Ok(&field.value),
    }
}

fn string_value(value: &Value, name: &str) -> Result<Option<String>, RemindersError> {
    match value {
        Value::Null => Ok(None),
        Value::String(s) if s.len() <= MAX_FIELD_BYTES * 2 => Ok(Some(s.clone())),
        _ => Err(protocol(&format!("decode {name}"))),
    }
}

fn number_value(fields: &Fields, name: &str, ty: &str) -> Result<Option<i64>, RemindersError> {
    match typed(fields, name, ty)? {
        Value::Null => Ok(None),
        v => crate::json::safe_integer(v)
            .map(Some)
            .ok_or_else(|| protocol(&format!("decode {name}"))),
    }
}

fn flag(fields: &Fields, name: &str) -> Result<bool, RemindersError> {
    match number_value(fields, name, "INT64")? {
        None | Some(0) => Ok(false),
        Some(1) => Ok(true),
        Some(_) => Err(protocol(&format!("decode {name}"))),
    }
}

fn document(fields: &Fields, name: &str) -> Result<String, RemindersError> {
    let field = fields.get(name);
    let raw = match field {
        Some(f) if f.ty == "BYTES" || f.ty == "ENCRYPTED_BYTES" => string_value(&f.value, name)?,
        _ => string_value(typed(fields, name, "STRING")?, name)?,
    };
    let Some(raw) = raw else {
        return Ok(String::new());
    };
    decode_crdt_document(&raw).map_err(|_| {
        if field.is_some_and(|f| f.ty == "ENCRYPTED_BYTES") {
            fail(&format!("decode {name}"), ErrorCode::AwaitingDeviceApproval)
        } else {
            fail(&format!("decode {name}"), ErrorCode::Unsupported)
        }
    })
}

fn reference(fields: &Fields, name: &str) -> Result<String, RemindersError> {
    typed(fields, name, "REFERENCE")?
        .as_object()
        .and_then(|o| o.get("recordName"))
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| protocol(&format!("decode {name}")))
}

fn list_from_record(record: &CkRecord) -> Result<ListMetadata, RemindersError> {
    let fields = record.fields();
    let title = string_value(typed(fields, "Name", "STRING")?, "Name")?
        .filter(|t| !t.is_empty())
        .ok_or_else(|| protocol("decode list"))?;
    Ok(ListMetadata {
        id: record.record_name.clone(),
        title,
        color: string_value(typed(fields, "Color", "STRING")?, "Color")?,
        record_change_tag: record.record_change_tag.clone(),
    })
}

fn is_recurrence_name(name: &str) -> bool {
    let lower = name.to_lowercase();
    lower.contains("recurr") || lower.contains("repeat")
}

fn reminder_from_record(record: &CkRecord) -> Result<Reminder, RemindersError> {
    let fields = record.fields();
    let priority = number_value(fields, "Priority", "INT64")?.unwrap_or(0);
    if !matches!(priority, 0 | 1 | 5 | 9) {
        return Err(protocol("decode priority"));
    }
    let title = document(fields, "TitleDocument")?;
    let Some(tag) = record
        .record_change_tag
        .clone()
        .filter(|_| !title.is_empty())
    else {
        return Err(protocol("decode reminder"));
    };
    let created = record
        .created
        .filter(|c| c.is_finite())
        .map(crate::json::js_to_i64);
    let modified = record
        .modified
        .filter(|c| c.is_finite())
        .map(crate::json::js_to_i64);
    Ok(Reminder {
        id: record.record_name.clone(),
        list_id: reference(fields, "List")?,
        title,
        description: document(fields, "NotesDocument")?,
        completed: flag(fields, "Completed")?,
        completed_date: number_value(fields, "CompletionDate", "TIMESTAMP")?,
        due_date: number_value(fields, "DueDate", "TIMESTAMP")?,
        start_date: number_value(fields, "StartDate", "TIMESTAMP")?,
        priority,
        flagged: flag(fields, "Flagged")?,
        all_day: flag(fields, "AllDay")?,
        deleted: flag(fields, "Deleted")?,
        created_date: number_value(fields, "CreationDate", "TIMESTAMP")?.or(created),
        last_modified_date: number_value(fields, "LastModifiedDate", "TIMESTAMP")?.or(modified),
        record_change_tag: tag,
        recurring: fields.iter().any(|(key, field)| {
            if !is_recurrence_name(key) {
                return false;
            }
            !(key == "RecurrenceRuleIDs"
                && (field.ty == "UNKNOWN_LIST" || field.ty == "STRING_LIST")
                && field.value.as_array().is_some_and(Vec::is_empty))
        }),
    })
}

/// Every record a live recurrence record references (empty for other records).
fn recurrence_references(record: &CkRecord) -> Result<Vec<String>, RemindersError> {
    if record.gone()
        || !record
            .record_type
            .as_deref()
            .is_some_and(is_recurrence_name)
    {
        return Ok(Vec::new());
    }
    let reminder = reference(record.fields(), "Reminder")?;
    if reminder.is_empty() {
        return Err(protocol("recurrence reference"));
    }
    let mut ids = Vec::new();
    for field in record.fields().values() {
        if field.ty != "REFERENCE" {
            continue;
        }
        match field
            .value
            .as_object()
            .and_then(|o| o.get("recordName"))
            .and_then(Value::as_str)
        {
            Some(name) if !name.is_empty() => ids.push(name.to_owned()),
            _ => return Err(protocol("recurrence reference")),
        }
    }
    if ids.is_empty() {
        return Err(protocol("recurrence reference"));
    }
    Ok(ids)
}

fn validate_text(text: &str, operation: &str) -> Result<(), RemindersError> {
    if crate::json::js_blank(text) || text.len() > MAX_FIELD_BYTES {
        return Err(fail(operation, ErrorCode::Invalid));
    }
    Ok(())
}

fn validate_date(date: &Opt<i64>, operation: &str) -> Result<(), RemindersError> {
    if let Opt::Set(d) = date
        && (*d < 0 || *d > 9_007_199_254_740_991)
    {
        return Err(fail(operation, ErrorCode::Invalid));
    }
    Ok(())
}

fn is_all_day_date(date: Option<i64>) -> bool {
    date.is_none_or(|d| d % 86_400_000 == 0)
}

/// Seconds since 2001-01-01 (Apple reference date), as JS computes it.
pub(crate) fn modification_time(now_ms: i64) -> f64 {
    now_ms as f64 / 1000.0 - 978_307_200.0
}

fn token_map(names: &[String], now_ms: i64, replica: &str) -> String {
    let mut map = Map::new();
    for name in names {
        map.insert(
            name.clone(),
            json!({
                "counter": 1,
                "modificationTime": modification_time(now_ms),
                "replicaID": replica,
            }),
        );
    }
    omni_core::js::json_stringify(&json!({"map": map}))
}

pub(crate) fn replica_id() -> String {
    omni_core::ids::uuid_v4().to_uppercase()
}

fn patch_fields(
    patch: &ReminderPatch,
    now_ms: i64,
    replica: &str,
) -> Result<Map<String, Value>, RemindersError> {
    let mut fields = Map::new();
    let mut names: Vec<String> = Vec::new();
    let mut put = |name: &str, entry: Value| {
        fields.insert(name.to_owned(), entry);
        let mut chars = name.chars();
        let lower = chars
            .next()
            .map(|c| c.to_lowercase().chain(chars).collect::<String>())
            .unwrap_or_default();
        names.push(lower);
    };
    let encode =
        |text: &str| encode_crdt_document(text).map_err(|_| fail("update", ErrorCode::Invalid));
    if let Some(title) = &patch.title {
        validate_text(title, "title")?;
        put("TitleDocument", str_field(&encode(title)?));
    }
    if let Some(description) = &patch.description {
        if description.len() > MAX_FIELD_BYTES {
            return Err(fail("notes", ErrorCode::Invalid));
        }
        put("NotesDocument", str_field(&encode(description)?));
    }
    if let Some(completed) = patch.completed {
        put("Completed", int(Some(i64::from(completed))));
        put("CompletionDate", stamp(completed.then_some(now_ms)));
    }
    if !patch.due_date.is_absent() {
        validate_date(&patch.due_date, "due date")?;
        put("DueDate", stamp(patch.due_date.value().copied()));
    }
    if !patch.start_date.is_absent() {
        validate_date(&patch.start_date, "start date")?;
        put("StartDate", stamp(patch.start_date.value().copied()));
    }
    if let Some(priority) = patch.priority {
        if !matches!(priority, 0 | 1 | 5 | 9) {
            return Err(fail("priority", ErrorCode::Invalid));
        }
        put("Priority", int(Some(priority)));
    }
    if let Some(flagged) = patch.flagged {
        put("Flagged", int(Some(i64::from(flagged))));
    }
    if let Some(all_day) = patch.all_day {
        put("AllDay", int(Some(i64::from(all_day))));
    }
    put("LastModifiedDate", stamp(Some(now_ms)));
    if names.len() == 1 {
        return Err(fail("update", ErrorCode::Invalid));
    }
    fields.insert(
        "ResolutionTokenMap".into(),
        str_field(&token_map(&names, now_ms, replica)),
    );
    Ok(fields)
}

/// Decodes `{zones: [...]}`.
struct Zone {
    records: Option<Vec<CkRecord>>,
    sync_token: Option<String>,
    more_coming: bool,
    error: bool,
}

fn decode_changes(raw: &Value) -> Option<Vec<Zone>> {
    let zones = raw.as_object()?.get("zones")?.as_array()?;
    zones
        .iter()
        .map(|zone| {
            let object = zone.as_object()?;
            let records = match object.get("records") {
                None => None,
                some => Some(decode_records(some)?),
            };
            let error = match object.get("error") {
                None => false,
                Some(Value::Object(e)) => {
                    opt_string(e, "serverErrorCode").ok()?;
                    opt_string(e, "reason").ok()?;
                    opt_number(e, "errorCode").ok()?;
                    true
                }
                Some(_) => return None,
            };
            Some(Zone {
                records,
                sync_token: opt_string(object, "syncToken").ok()?,
                more_coming: opt_bool(object, "moreComing").ok()?.unwrap_or(false),
                error,
            })
        })
        .collect()
}

/// `post()`: rejects non-objects and oversized arrays before decoding.
pub(crate) async fn checked_post(
    post: &dyn CkPost,
    path: &str,
    body: Value,
) -> Result<Value, RemindersError> {
    let raw = post.post(path, body).await?;
    let Some(object) = raw.as_object() else {
        return Err(protocol(path));
    };
    let oversized = |key: &str| {
        object
            .get(key)
            .and_then(Value::as_array)
            .is_some_and(|a| a.len() > MAX_RECORDS)
    };
    if oversized("zones") || oversized("records") {
        return Err(protocol(path));
    }
    Ok(raw)
}

/// Shared read budget across one snapshot.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct ReadBudget {
    pub pages: usize,
    pub records: usize,
}

/// Pages the compound `reminderList` query for one list.
pub(crate) async fn query_list(
    post: &dyn CkPost,
    list_id: &str,
    budget: &mut ReadBudget,
) -> Result<Vec<CkRecord>, RemindersError> {
    let mut continuation: Option<String> = None;
    let mut cursors: Vec<String> = Vec::new();
    let mut records = Vec::new();
    loop {
        let page = budget.pages;
        budget.pages += 1;
        if page >= MAX_QUERY_PAGES {
            return Err(protocol("list query limit"));
        }
        let mut body = json!({
            "zoneID": zone(),
            "resultsLimit": 50,
            "query": {
                "recordType": "reminderList",
                "filterBy": [
                    {
                        "comparator": "EQUALS",
                        "fieldName": "List",
                        "fieldValue": {
                            "type": "REFERENCE",
                            "value": {"recordName": list_id, "action": "VALIDATE"},
                        },
                    },
                    {"comparator": "EQUALS", "fieldName": "includeCompleted", "fieldValue": int(Some(1))},
                    {"comparator": "EQUALS", "fieldName": "LookupValidatingReference", "fieldValue": int(Some(1))},
                ],
            },
        });
        if let (Some(marker), Some(object)) = (&continuation, body.as_object_mut()) {
            object.insert("continuationMarker".into(), Value::String(marker.clone()));
        }
        let raw = checked_post(post, "/records/query", body).await?;
        let object = raw.as_object().ok_or_else(|| protocol("/records/query"))?;
        let page_records =
            decode_records(object.get("records")).ok_or_else(|| protocol("/records/query"))?;
        let marker = match object.get("continuationMarker") {
            None | Some(Value::Null) => None,
            Some(Value::String(s)) => Some(s.clone()),
            Some(_) => return Err(protocol("/records/query")),
        };
        for record in page_records {
            budget.records += 1;
            if budget.records > MAX_RECORDS || record.has_error() {
                return Err(protocol("list query"));
            }
            records.push(record);
        }
        let Some(marker) = marker.filter(|m| !m.is_empty()) else {
            return Ok(records);
        };
        if cursors.contains(&marker) {
            return Err(protocol("list query cursor"));
        }
        cursors.push(marker.clone());
        continuation = Some(marker);
    }
}

fn counted_lists<'a>(
    lists: impl Iterator<Item = &'a ListMetadata>,
    reminders: &IndexMap<String, Reminder>,
) -> Vec<RemindersList> {
    let mut counts: std::collections::HashMap<&str, u64> = std::collections::HashMap::new();
    for reminder in reminders.values() {
        if !reminder.completed && !reminder.deleted {
            *counts.entry(reminder.list_id.as_str()).or_default() += 1;
        }
    }
    lists
        .map(|list| RemindersList {
            id: list.id.clone(),
            title: list.title.clone(),
            color: list.color.clone(),
            count: counts.get(list.id.as_str()).copied().unwrap_or(0),
            record_change_tag: list.record_change_tag.clone(),
        })
        .collect()
}

fn mark_recurring(
    reminders: IndexMap<String, Reminder>,
    recurring: &std::collections::HashSet<String>,
) -> Vec<Reminder> {
    reminders
        .into_values()
        .map(|mut reminder| {
            if recurring.contains(&reminder.id) {
                reminder.recurring = true;
            }
            reminder
        })
        .collect()
}

#[derive(Clone)]
struct SnapshotIndex {
    lists: IndexMap<String, ListMetadata>,
    records: IndexMap<String, CkRecord>,
    sync_token: String,
}

#[derive(Default)]
struct CacheState {
    list_cache: Option<(IndexMap<String, ListMetadata>, String)>,
    snapshot_index: Option<SnapshotIndex>,
}

/// The CloudKit Reminders client: complete snapshots with an incremental index.
pub struct RemindersCloudKitClient {
    post: Arc<dyn CkPost>,
    clock: SharedClock,
    state: Mutex<CacheState>,
    pub extras: RemindersCloudKitExtras,
}

impl RemindersCloudKitClient {
    pub fn new(post: Arc<dyn CkPost>, clock: SharedClock) -> Self {
        let query_post = post.clone();
        let extras = RemindersCloudKitExtras::new(
            post.clone(),
            clock.clone(),
            Arc::new(move |list_id: &str| {
                let post = query_post.clone();
                let list_id = list_id.to_owned();
                Box::pin(async move {
                    let mut budget = ReadBudget::default();
                    query_list(post.as_ref(), &list_id, &mut budget)
                        .await
                        .map(|records| records.iter().map(CkRecord::to_value).collect())
                })
            }),
        );
        Self {
            post,
            clock,
            state: Mutex::new(CacheState::default()),
            extras,
        }
    }

    fn with_state<R>(&self, f: impl FnOnce(&mut CacheState) -> R) -> R {
        let mut guard = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        f(&mut guard)
    }

    pub fn has_snapshot(&self) -> bool {
        self.with_state(|s| s.snapshot_index.is_some())
    }

    /// Drops both the snapshot index and the list cursor.
    pub fn invalidate_snapshot(&self) {
        self.with_state(|s| {
            s.snapshot_index = None;
            s.list_cache = None;
        });
    }

    /// Completes one recurring occurrence (inside the caller's reservation and lock).
    pub async fn complete_recurring(
        &self,
        target: &RecurringCompletionTarget,
    ) -> Result<VerifiedRecurringCompletion, RemindersError> {
        complete_recurring_occurrence(self, target).await
    }

    pub(crate) fn post_seam(&self) -> &dyn CkPost {
        self.post.as_ref()
    }

    pub(crate) fn clock(&self) -> &SharedClock {
        &self.clock
    }

    /// A complete snapshot: incremental from the index when one exists.
    pub async fn read_snapshot(&self) -> Result<RemindersSnapshot, RemindersError> {
        match self.with_state(|s| s.snapshot_index.clone()) {
            Some(index) => self.refresh_snapshot(index).await,
            None => self.read_full_snapshot().await,
        }
    }

    async fn read_full_snapshot(&self) -> Result<RemindersSnapshot, RemindersError> {
        let cached = self.with_state(|s| s.list_cache.clone());
        let mut lists = cached.as_ref().map(|(l, _)| l.clone()).unwrap_or_default();
        let mut token = cached.map(|(_, t)| t);
        let mut latest: Option<String> = None;
        let mut complete = false;
        let mut budget = ReadBudget::default();
        let mut cursors: Vec<String> = token.iter().cloned().collect();
        for _ in 0..MAX_LIST_PAGES {
            let mut zone_request = json!({"zoneID": zone(), "desiredRecordTypes": ["List"]});
            if let (Some(t), Some(o)) = (&token, zone_request.as_object_mut()) {
                o.insert("syncToken".into(), Value::String(t.clone()));
            }
            let raw = checked_post(
                self.post.as_ref(),
                "/changes/zone",
                json!({"zones": [zone_request]}),
            )
            .await?;
            let zones = decode_changes(&raw).ok_or_else(|| protocol("/changes/zone"))?;
            let [zone] = <[Zone; 1]>::try_from(zones).map_err(|_| protocol("snapshot zones"))?;
            if zone.error {
                return Err(protocol("snapshot zone"));
            }
            for record in zone.records.unwrap_or_default() {
                budget.records += 1;
                if budget.records > MAX_RECORDS {
                    return Err(protocol("snapshot limit"));
                }
                if record.has_error() {
                    return Err(protocol("snapshot record"));
                }
                if record.gone() {
                    lists.shift_remove(&record.record_name);
                } else if record.record_type.as_deref() == Some("List") {
                    let list = list_from_record(&record).map_err(|_| protocol("decode list"))?;
                    lists.insert(record.record_name.clone(), list);
                }
            }
            latest = zone.sync_token.clone();
            if !zone.more_coming {
                complete = true;
                break;
            }
            match zone.sync_token {
                Some(t) if !t.is_empty() && !cursors.contains(&t) => {
                    cursors.push(t.clone());
                    token = Some(t);
                }
                _ => return Err(protocol("snapshot cursor")),
            }
        }
        if !complete {
            return Err(protocol("snapshot limit"));
        }
        let latest = latest.filter(|t| !t.is_empty());
        if let Some(t) = &latest {
            let snapshot_lists = lists.clone();
            let t = t.clone();
            self.with_state(|s| s.list_cache = Some((snapshot_lists, t)));
        }
        let mut reminders: IndexMap<String, Reminder> = IndexMap::new();
        let mut recurring = std::collections::HashSet::new();
        let mut indexed: IndexMap<String, CkRecord> = IndexMap::new();
        let list_ids: Vec<String> = lists.keys().cloned().collect();
        for list_id in list_ids {
            let records = query_list(self.post.as_ref(), &list_id, &mut budget).await?;
            for record in records {
                if record.gone() {
                    continue;
                }
                if record.record_type.as_deref() == Some("Reminder") {
                    let reminder = reminder_from_record(&record)?;
                    if reminder.list_id == list_id && !reminder.deleted {
                        reminders.insert(reminder.id.clone(), reminder);
                        indexed.insert(record.record_name.clone(), record);
                    }
                } else {
                    let references = recurrence_references(&record)
                        .map_err(|_| protocol("recurrence reference"))?;
                    let has = !references.is_empty();
                    recurring.extend(references);
                    if has {
                        indexed.insert(record.record_name.clone(), record);
                    }
                }
            }
        }
        if lists.len() + indexed.len() > MAX_RECORDS {
            return Err(protocol("snapshot index limit"));
        }
        let index = latest.map(|sync_token| SnapshotIndex {
            lists: lists.clone(),
            records: indexed,
            sync_token,
        });
        self.with_state(|s| s.snapshot_index = index);
        Ok(RemindersSnapshot {
            lists: counted_lists(lists.values(), &reminders),
            reminders: mark_recurring(reminders, &recurring),
        })
    }

    async fn refresh_snapshot(
        &self,
        index: SnapshotIndex,
    ) -> Result<RemindersSnapshot, RemindersError> {
        let mut lists = index.lists;
        let mut records = index.records;
        let mut token = index.sync_token;
        let mut seen = 0usize;
        let mut cursors = vec![token.clone()];
        for _ in 0..MAX_LIST_PAGES {
            let raw = checked_post(
                self.post.as_ref(),
                "/changes/zone",
                json!({"zones": [{"zoneID": zone(), "syncToken": token}]}),
            )
            .await?;
            let zones = decode_changes(&raw).ok_or_else(|| protocol("/changes/zone"))?;
            let [zone] = <[Zone; 1]>::try_from(zones).map_err(|_| protocol("snapshot refresh"))?;
            if zone.error {
                return Err(protocol("snapshot refresh"));
            }
            let Some(page) = zone.records else {
                return Err(protocol("snapshot refresh"));
            };
            for record in page {
                seen += 1;
                if seen > MAX_RECORDS || record.has_error() {
                    return Err(protocol("snapshot refresh"));
                }
                if record.gone() {
                    lists.shift_remove(&record.record_name);
                    records.shift_remove(&record.record_name);
                } else if record.record_type.as_deref() == Some("List") {
                    let list = list_from_record(&record).map_err(|_| protocol("decode list"))?;
                    lists.insert(record.record_name.clone(), list);
                } else if record.record_type.as_deref() == Some("Reminder")
                    || record
                        .record_type
                        .as_deref()
                        .is_some_and(is_recurrence_name)
                {
                    records.insert(record.record_name.clone(), record);
                } else if record.record_type.as_deref().is_none_or(str::is_empty) {
                    return Err(protocol("snapshot refresh record"));
                }
                if lists.len() + records.len() > MAX_RECORDS {
                    return Err(protocol("snapshot index limit"));
                }
            }
            if !zone.more_coming {
                let Some(sync_token) = zone.sync_token.filter(|t| !t.is_empty()) else {
                    return Err(protocol("snapshot refresh cursor"));
                };
                let mut reminders: IndexMap<String, Reminder> = IndexMap::new();
                let mut recurring = std::collections::HashSet::new();
                let mut orphaned = Vec::new();
                for record in records.values() {
                    if record.record_type.as_deref() == Some("Reminder") {
                        let reminder = reminder_from_record(record)?;
                        if lists.contains_key(&reminder.list_id) {
                            reminders.insert(reminder.id.clone(), reminder);
                        } else {
                            orphaned.push(record.record_name.clone());
                        }
                    } else {
                        recurring.extend(
                            recurrence_references(record)
                                .map_err(|_| protocol("recurrence reference"))?,
                        );
                    }
                }
                for name in orphaned {
                    records.shift_remove(&name);
                }
                let snapshot = RemindersSnapshot {
                    lists: counted_lists(lists.values(), &reminders),
                    reminders: mark_recurring(reminders, &recurring),
                };
                self.with_state(|s| {
                    s.list_cache = Some((lists.clone(), sync_token.clone()));
                    s.snapshot_index = Some(SnapshotIndex {
                        lists,
                        records,
                        sync_token,
                    });
                });
                return Ok(snapshot);
            }
            match zone.sync_token {
                Some(t) if !t.is_empty() && !cursors.contains(&t) => {
                    cursors.push(t.clone());
                    token = t;
                }
                _ => return Err(protocol("snapshot refresh cursor")),
            }
        }
        Err(protocol("snapshot refresh limit"))
    }

    /// Decodes the newest protected content without scanning the whole history.
    pub async fn verify_read_access(&self) -> Result<(), RemindersError> {
        let mut token: Option<String> = None;
        let mut seen = 0usize;
        let mut cursors: Vec<String> = Vec::new();
        for _ in 0..MAX_PAGES {
            let mut request = json!({
                "zoneID": zone(),
                "reverse": true,
                "desiredRecordTypes": ["List", "Reminder"],
            });
            if let (Some(t), Some(o)) = (&token, request.as_object_mut()) {
                o.insert("syncToken".into(), Value::String(t.clone()));
            }
            let raw = checked_post(
                self.post.as_ref(),
                "/changes/zone",
                json!({"zones": [request]}),
            )
            .await?;
            let zones = decode_changes(&raw).ok_or_else(|| protocol("/changes/zone"))?;
            let [zone] = <[Zone; 1]>::try_from(zones).map_err(|_| protocol("access probe"))?;
            if zone.error {
                return Err(protocol("access probe"));
            }
            let Some(page) = zone.records else {
                return Err(protocol("access probe"));
            };
            let mut decoded_reminder = false;
            for record in page {
                seen += 1;
                if seen > MAX_RECORDS || record.has_error() {
                    return Err(protocol("access probe"));
                }
                if record.gone() {
                    continue;
                }
                match record.record_type.as_deref() {
                    Some("Reminder") => {
                        reminder_from_record(&record)?;
                        decoded_reminder = true;
                    }
                    Some("List") => {
                        list_from_record(&record).map_err(|_| protocol("access probe"))?;
                    }
                    _ => return Err(protocol("access probe")),
                }
            }
            if decoded_reminder || !zone.more_coming {
                return Ok(());
            }
            match zone.sync_token {
                Some(t) if !t.is_empty() && !cursors.contains(&t) => {
                    cursors.push(t.clone());
                    token = Some(t);
                }
                _ => return Err(protocol("access probe cursor")),
            }
        }
        Err(protocol("access probe limit"))
    }

    async fn lookup(&self, id: &str) -> Result<Option<CkRecord>, RemindersError> {
        let raw = checked_post(
            self.post.as_ref(),
            "/records/lookup",
            json!({"records": [{"recordName": id}], "zoneID": zone()}),
        )
        .await?;
        let records =
            decode_records(raw.get("records")).ok_or_else(|| protocol("/records/lookup"))?;
        let [record] = <[CkRecord; 1]>::try_from(records).map_err(|_| protocol("lookup"))?;
        if record.record_name != id {
            return Err(protocol("lookup"));
        }
        if matches!(
            record.server_error_code.as_deref(),
            Some("NOT_FOUND" | "UNKNOWN_ITEM")
        ) {
            return Ok(None);
        }
        if record.has_error() {
            return Err(protocol("lookup"));
        }
        Ok((!record.gone()).then_some(record))
    }

    async fn has_recurrence_reference(
        &self,
        id: &str,
        list_id: &str,
    ) -> Result<bool, RemindersError> {
        if self.has_snapshot() {
            let snapshot = self.read_snapshot().await?;
            return snapshot
                .reminders
                .iter()
                .find(|r| r.id == id)
                .map(|r| r.recurring)
                .ok_or_else(|| protocol("recurrence snapshot"));
        }
        let mut budget = ReadBudget::default();
        let records = query_list(self.post.as_ref(), list_id, &mut budget).await?;
        for record in &records {
            let references =
                recurrence_references(record).map_err(|_| protocol("recurrence reference"))?;
            if references.iter().any(|r| r == id) {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// One reminder by exact id, with recurrence protection resolved.
    pub async fn get_reminder(&self, id: &str) -> Result<Option<Reminder>, RemindersError> {
        let Some(record) = self.lookup(id).await? else {
            return Ok(None);
        };
        if record.record_type.as_deref() != Some("Reminder") {
            return Err(protocol("get"));
        }
        let reminder = reminder_from_record(&record)?;
        if reminder.deleted {
            return Ok(None);
        }
        if reminder.recurring {
            return Ok(Some(reminder));
        }
        let recurring = self.has_recurrence_reference(id, &reminder.list_id).await?;
        Ok(Some(Reminder {
            recurring,
            ..reminder
        }))
    }

    async fn modify(
        &self,
        operation_type: &str,
        record: Value,
        id: &str,
    ) -> Result<String, RemindersError> {
        let raw = checked_post(
            self.post.as_ref(),
            "/records/modify",
            json!({
                "operations": [{"operationType": operation_type, "record": record}],
                "zoneID": zone(),
                "atomic": true,
            }),
        )
        .await?;
        let records =
            decode_records(raw.get("records")).ok_or_else(|| protocol("/records/modify"))?;
        if let [only] = records.as_slice()
            && only.record_name == id
            && matches!(
                only.server_error_code.as_deref(),
                Some("CONFLICT" | "SERVER_RECORD_CHANGED")
            )
        {
            return Err(fail("modify", ErrorCode::Conflict));
        }
        match records.as_slice() {
            [only]
                if only.record_name == id
                    && !only.has_error()
                    && only
                        .record_change_tag
                        .as_deref()
                        .is_some_and(|t| !t.is_empty()) =>
            {
                Ok(only.record_change_tag.clone().unwrap_or_default())
            }
            _ => Err(protocol("modify")),
        }
    }

    /// Creates a reminder with a caller-chosen stable id, or returns the identical prior one.
    pub async fn create_reminder(
        &self,
        id: &str,
        input: &ReminderCreateFields,
    ) -> Result<Reminder, RemindersError> {
        let valid_id = id.strip_prefix("Reminder/").is_some_and(|rest| {
            (8..=128).contains(&rest.len())
                && rest.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
        });
        if !valid_id || input.list_id.is_empty() || omni_core::js::utf16_len(&input.list_id) > 256 {
            return Err(fail("create", ErrorCode::Invalid));
        }
        if input.all_day == Some(true) && !is_all_day_date(input.due_date.value().copied()) {
            return Err(fail("all-day due date", ErrorCode::Invalid));
        }
        let prior = self.get_reminder(id).await?;
        let description = input.description.clone().unwrap_or_default();
        let matches = |reminder: &Reminder| {
            reminder.list_id == input.list_id
                && reminder.title == input.title
                && reminder.description == description
                && reminder.completed == input.completed.unwrap_or(false)
                && reminder.due_date == input.due_date.value().copied()
                && reminder.start_date == input.start_date.value().copied()
                && reminder.priority == input.priority.unwrap_or(0)
                && reminder.flagged == input.flagged.unwrap_or(false)
                && reminder.all_day == input.all_day.unwrap_or(false)
        };
        if let Some(prior) = prior {
            return if matches(&prior) {
                Ok(prior)
            } else {
                Err(fail("create", ErrorCode::Conflict))
            };
        }
        let now = self.clock.now_ms();
        let patch = ReminderPatch {
            title: Some(input.title.clone()),
            description: Some(description.clone()),
            completed: Some(input.completed.unwrap_or(false)),
            due_date: input.due_date.clone(),
            start_date: input.start_date.clone(),
            priority: Some(input.priority.unwrap_or(0)),
            flagged: Some(input.flagged.unwrap_or(false)),
            all_day: Some(input.all_day.unwrap_or(false)),
        };
        let mut fields = patch_fields(&patch, now, &replica_id())?;
        fields.insert("CreationDate".into(), stamp(Some(now)));
        fields.insert("Deleted".into(), int(Some(0)));
        fields.insert("Imported".into(), int(Some(0)));
        fields.insert(
            "List".into(),
            json!({"type": "REFERENCE", "value": {"recordName": input.list_id, "action": "VALIDATE"}}),
        );
        let tag = self
            .modify(
                "create",
                json!({
                    "recordName": id,
                    "recordType": "Reminder",
                    "fields": fields,
                    "createShortGUID": true,
                }),
                id,
            )
            .await?;
        match self.get_reminder(id).await? {
            Some(written) if written.record_change_tag == tag && matches(&written) => Ok(written),
            _ => Err(protocol("create verification")),
        }
    }

    /// Patches a non-recurring reminder at its exact change tag and verifies the read-back.
    pub async fn update_reminder(
        &self,
        current: &Reminder,
        patch: &ReminderPatch,
    ) -> Result<Reminder, RemindersError> {
        if current.recurring {
            return Err(fail("update recurring", ErrorCode::Unsupported));
        }
        if current.record_change_tag.is_empty() {
            return Err(fail("update", ErrorCode::Invalid));
        }
        let Some(latest) = self.get_reminder(&current.id).await? else {
            return Err(fail("update", ErrorCode::NotFound));
        };
        if latest.recurring {
            return Err(fail("update recurring", ErrorCode::Unsupported));
        }
        if latest.record_change_tag != current.record_change_tag {
            return Err(fail("update", ErrorCode::Conflict));
        }
        if (!patch.due_date.is_absent() || patch.all_day.is_some())
            && patch.all_day.unwrap_or(latest.all_day)
            && !is_all_day_date(match &patch.due_date {
                Opt::Absent => latest.due_date,
                other => other.value().copied(),
            })
        {
            return Err(fail("all-day due date", ErrorCode::Invalid));
        }
        let fields = patch_fields(patch, self.clock.now_ms(), &replica_id())?;
        let tag = self
            .modify(
                "update",
                json!({
                    "recordName": current.id,
                    "recordType": "Reminder",
                    "recordChangeTag": current.record_change_tag,
                    "fields": fields,
                }),
                &current.id,
            )
            .await?;
        match self.get_reminder(&current.id).await? {
            Some(written) if written.record_change_tag == tag && patch.applied_to(&written) => {
                Ok(written)
            }
            _ => Err(protocol("update verification")),
        }
    }

    pub async fn set_completed(
        &self,
        current: &Reminder,
        completed: bool,
    ) -> Result<Reminder, RemindersError> {
        self.update_reminder(
            current,
            &ReminderPatch {
                completed: Some(completed),
                ..ReminderPatch::default()
            },
        )
        .await
    }

    /// Soft-deletes a non-recurring reminder via Apple's `Deleted` field and verifies it.
    pub async fn delete_reminder(&self, current: &Reminder) -> Result<(), RemindersError> {
        if current.recurring {
            return Err(fail("delete recurring", ErrorCode::Unsupported));
        }
        if current.record_change_tag.is_empty() {
            return Err(fail("delete", ErrorCode::Invalid));
        }
        let Some(latest) = self.get_reminder(&current.id).await? else {
            return Err(fail("delete", ErrorCode::NotFound));
        };
        if latest.recurring {
            return Err(fail("delete recurring", ErrorCode::Unsupported));
        }
        if latest.record_change_tag != current.record_change_tag {
            return Err(fail("delete", ErrorCode::Conflict));
        }
        let now = self.clock.now_ms();
        self.modify(
            "update",
            json!({
                "recordName": current.id,
                "recordType": "Reminder",
                "recordChangeTag": current.record_change_tag,
                "fields": {
                    "Deleted": int(Some(1)),
                    "LastModifiedDate": stamp(Some(now)),
                    "ResolutionTokenMap": str_field(&token_map(
                        &["deleted".into(), "lastModifiedDate".into()],
                        now,
                        &replica_id(),
                    )),
                },
            }),
            &current.id,
        )
        .await?;
        if self.get_reminder(&current.id).await?.is_some() {
            return Err(protocol("delete verification"));
        }
        Ok(())
    }
}
