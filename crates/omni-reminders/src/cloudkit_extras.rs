//! List rename and recurrence-rule operations.
//!
//! All writes use change-tag CAS, atomic multi-record modify, exact acknowledgment of
//! every expected record, and a fresh read before success. One rule per reminder;
//! multiple, unlinked or unknown rule relationships stay read-only.

use std::collections::HashMap;
use std::sync::Arc;

use futures::future::BoxFuture;
use omni_core::clock::SharedClock;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::cloudkit::{
    CkPost, CkRecord, ErrorCode, Fields, RemindersError, fail, int, modification_time, protocol,
    replica_id, str_field, zone,
};
use crate::recurrence::{
    RecurrenceDecoded, RecurrenceEncoded, RecurrenceUnsupported, UnsupportedReason,
    decode_recurrence_values, encode_recurrence_values, is_selector_wire,
};

/// Reads every record of one list's compound query (raw JSON, decoded here).
pub type ListQuery =
    Arc<dyn Fn(&str) -> BoxFuture<'static, Result<Vec<Value>, RemindersError>> + Send + Sync>;

/// One recurrence rule as the tools expose it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReminderRecurrence {
    pub id: String,
    pub reminder_id: String,
    pub record_change_tag: Option<String>,
    pub writable: bool,
    pub recurrence: RecurrenceDecoded,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReminderRecurrences {
    pub reminder_id: String,
    pub reminder_change_tag: String,
    pub rules: Vec<ReminderRecurrence>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReminderListDetails {
    pub id: String,
    pub title: String,
    pub color: Option<String>,
    pub record_change_tag: Option<String>,
}

const ADMIN_FIELDS: [&str; 4] = ["Reminder", "Deleted", "Imported", "ResolutionTokenMap"];
const EXTRAS_SCALARS: [&str; 4] = [
    "Frequency",
    "Interval",
    "OccurrenceCount",
    "FirstDayOfTheWeek",
];

fn recurrence_like(name: &str) -> bool {
    let lower = name.to_lowercase();
    lower.contains("recurr") || lower.contains("repeat")
}

/// `refName(field)`: the referenced record name of a `REFERENCE` field.
fn ref_name(record: &CkRecord, name: &str) -> Option<String> {
    let field = record.field(name)?;
    if field.ty != "REFERENCE" {
        return None;
    }
    field
        .value
        .as_object()?
        .get("recordName")?
        .as_str()
        .map(str::to_owned)
}

fn reference(record_name: &str) -> Value {
    json!({"type": "REFERENCE", "value": {"recordName": record_name, "action": "VALIDATE"}})
}

/// Merges new counters into the record's `ResolutionTokenMap`, keeping other entries.
fn token_map(record: &CkRecord, names: &[&str], now_ms: i64) -> Result<String, RemindersError> {
    let bad = || protocol("resolution tokens");
    let mut current: Map<String, Value> = Map::new();
    current.insert("map".into(), Value::Object(Map::new()));
    if let Some(field) = record.field("ResolutionTokenMap") {
        let text = match (&field.ty[..], &field.value) {
            ("STRING", Value::String(s)) if omni_core::js::utf16_len(s) <= 65_536 => s,
            _ => return Err(bad()),
        };
        let parsed: Value = serde_json::from_str(text).map_err(|_| bad())?;
        current = parsed.as_object().cloned().ok_or_else(bad)?;
    }
    let mut next = current
        .get("map")
        .and_then(Value::as_object)
        .cloned()
        .ok_or_else(bad)?;
    for name in names {
        let counter = match next.get(*name) {
            None => 0.0,
            Some(prior) => prior
                .as_object()
                .and_then(|o| o.get("counter"))
                .and_then(Value::as_f64)
                .ok_or_else(bad)?,
        };
        if counter.fract() != 0.0 || !(0.0..crate::json::MAX_SAFE_INTEGER).contains(&counter) {
            return Err(bad());
        }
        next.insert(
            (*name).to_owned(),
            json!({
                "counter": crate::json::js_to_i64(counter) + 1,
                "modificationTime": modification_time(now_ms),
                "replicaID": replica_id(),
            }),
        );
    }
    current.insert("map".into(), Value::Object(next));
    Ok(omni_core::js::json_stringify(&Value::Object(current)))
}

/// The reminder's `RecurrenceRuleIDs` (raw ids without the record-type prefix).
fn linked_ids(record: &CkRecord) -> Result<Vec<String>, RemindersError> {
    let unsupported = || fail("recurrence IDs", ErrorCode::Unsupported);
    let Some(field) = record.field("RecurrenceRuleIDs") else {
        return Ok(Vec::new());
    };
    if field.ty != "STRING_LIST" && field.ty != "UNKNOWN_LIST" {
        return Err(unsupported());
    }
    let ids: Vec<String> = field
        .value
        .as_array()
        .ok_or_else(unsupported)?
        .iter()
        .map(|v| v.as_str().map(str::to_owned))
        .collect::<Option<_>>()
        .ok_or_else(unsupported)?;
    if ids.len() > 100 || (field.ty == "UNKNOWN_LIST" && !ids.is_empty()) {
        return Err(unsupported());
    }
    let valid = |id: &String| {
        (8..=128).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
    };
    let unique: std::collections::HashSet<&String> = ids.iter().collect();
    if !ids.iter().all(valid) || unique.len() != ids.len() {
        return Err(unsupported());
    }
    Ok(ids)
}

fn rule_details(
    record: &CkRecord,
    reminder_id: &str,
) -> Result<ReminderRecurrence, RemindersError> {
    if record.record_type.as_deref() != Some("RecurrenceRule")
        || ref_name(record, "Reminder").as_deref() != Some(reminder_id)
    {
        return Err(protocol("recurrence owner"));
    }
    let mut values = Map::new();
    let mut valid_types = true;
    for (name, field) in record.fields() {
        if ADMIN_FIELDS.contains(&name.as_str()) {
            continue;
        }
        values.insert(name.clone(), field.value.clone());
        if EXTRAS_SCALARS.contains(&name.as_str()) && field.ty != "INT64" {
            valid_types = false;
        }
        if is_selector_wire(name) && field.ty != "STRING" && field.ty != "BYTES" {
            valid_types = false;
        }
        if name == "EndDate" && !matches!(field.ty.as_str(), "TIMESTAMP" | "INT64" | "DOUBLE") {
            valid_types = false;
        }
    }
    let recurrence = if valid_types {
        decode_recurrence_values(&Value::Object(values))
    } else {
        RecurrenceDecoded::Unsupported(RecurrenceUnsupported {
            supported: false,
            reason: UnsupportedReason::InvalidFields,
            fields: Vec::new(),
        })
    };
    Ok(ReminderRecurrence {
        id: record.record_name.clone(),
        reminder_id: reminder_id.to_owned(),
        record_change_tag: record.record_change_tag.clone(),
        writable: record
            .record_change_tag
            .as_deref()
            .is_some_and(|t| !t.is_empty())
            && recurrence.is_supported(),
        recurrence,
    })
}

/// Write fields for a rule: scalar types are `INT64`; other fields keep the exact
/// existing field's type, or omit the optional CloudKit type as Apple's client does.
fn recurrence_fields(
    input: &Value,
    existing: Option<&Fields>,
) -> Result<Map<String, Value>, RemindersError> {
    let values = match encode_recurrence_values(input) {
        RecurrenceEncoded::Supported(values) => values,
        RecurrenceEncoded::Unsupported(_) => {
            return Err(fail("recurrence fields", ErrorCode::Unsupported));
        }
    };
    let mut fields = Map::new();
    for (name, value) in values {
        let entry = if EXTRAS_SCALARS.contains(&name.as_str()) {
            json!({"type": "INT64", "value": value})
        } else if let Some(prior) = existing.and_then(|f| f.get(&name)) {
            json!({"type": prior.ty, "value": value})
        } else {
            json!({"value": value})
        };
        fields.insert(name, entry);
    }
    Ok(fields)
}

fn has_other_recurrence_keys(record: &CkRecord) -> bool {
    record
        .fields()
        .keys()
        .any(|key| recurrence_like(key) && key != "RecurrenceRuleIDs")
}

fn raw_rule_id(rule_id: &str) -> &str {
    rule_id.strip_prefix("RecurrenceRule/").unwrap_or(rule_id)
}

struct Context {
    reminder: CkRecord,
    ids: Vec<String>,
    records: Vec<CkRecord>,
}

/// Extra bounded operations; every write uses CAS and exact acknowledgment.
pub struct RemindersCloudKitExtras {
    post: Arc<dyn CkPost>,
    clock: SharedClock,
    query_list: ListQuery,
}

impl RemindersCloudKitExtras {
    pub fn new(post: Arc<dyn CkPost>, clock: SharedClock, query_list: ListQuery) -> Self {
        Self {
            post,
            clock,
            query_list,
        }
    }

    async fn request(&self, path: &str, body: Value) -> Result<Vec<CkRecord>, RemindersError> {
        let response = self.post.post(path, body).await?;
        crate::cloudkit::decode_lenient_records(response.as_object().and_then(|o| o.get("records")))
            .ok_or_else(|| protocol("records"))
    }

    async fn lookup(&self, id: &str) -> Result<Option<CkRecord>, RemindersError> {
        let records = self
            .request(
                "/records/lookup",
                json!({"zoneID": zone(), "records": [{"recordName": id}]}),
            )
            .await?;
        let [record] = <[CkRecord; 1]>::try_from(records).map_err(|_| protocol("lookup"))?;
        if record.record_name != id {
            return Err(protocol("lookup"));
        }
        if matches!(
            record.server_error_code.as_deref(),
            Some("NOT_FOUND" | "UNKNOWN_ITEM")
        ) || record.gone()
        {
            return Ok(None);
        }
        if record.has_error() {
            return Err(protocol("lookup"));
        }
        Ok(Some(record))
    }

    async fn require(
        &self,
        id: &str,
        ty: &str,
        tag: Option<&str>,
    ) -> Result<CkRecord, RemindersError> {
        let Some(record) = self.lookup(id).await? else {
            return Err(fail("record", ErrorCode::NotFound));
        };
        if record.record_type.as_deref() != Some(ty)
            || record
                .record_change_tag
                .as_deref()
                .is_none_or(str::is_empty)
        {
            return Err(protocol("record"));
        }
        if let Some(tag) = tag
            && record.record_change_tag.as_deref() != Some(tag)
        {
            return Err(fail("record", ErrorCode::Conflict));
        }
        Ok(record)
    }

    async fn modify(
        &self,
        operations: Vec<Value>,
    ) -> Result<HashMap<String, String>, RemindersError> {
        let expected: std::collections::HashSet<String> = operations
            .iter()
            .filter_map(|op| {
                op.get("record")?
                    .get("recordName")?
                    .as_str()
                    .map(str::to_owned)
            })
            .collect();
        let records = self
            .request(
                "/records/modify",
                json!({"zoneID": zone(), "atomic": true, "operations": operations}),
            )
            .await?;
        if records.len() != expected.len() {
            return Err(protocol("modify acknowledgments"));
        }
        let mut tags = HashMap::new();
        for record in records {
            if !expected.contains(&record.record_name) || tags.contains_key(&record.record_name) {
                return Err(protocol("modify acknowledgments"));
            }
            if matches!(
                record.server_error_code.as_deref(),
                Some("CONFLICT" | "SERVER_RECORD_CHANGED")
            ) {
                return Err(fail("modify", ErrorCode::Conflict));
            }
            let Some(tag) = record
                .record_change_tag
                .clone()
                .filter(|t| !t.is_empty() && !record.has_error())
            else {
                return Err(protocol("modify acknowledgments"));
            };
            tags.insert(record.record_name, tag);
        }
        Ok(tags)
    }

    /// One exact list's name, color and change tag.
    pub async fn get_list(&self, id: &str) -> Result<Option<ReminderListDetails>, RemindersError> {
        let Some(record) = self.lookup(id).await? else {
            return Ok(None);
        };
        if record.record_type.as_deref() != Some("List") {
            return Err(protocol("list"));
        }
        let title = match record.field("Name") {
            Some(f) if f.ty == "STRING" => match &f.value {
                Value::String(s) if omni_core::js::utf16_len(s) <= 4096 => s.clone(),
                _ => return Err(protocol("list name")),
            },
            _ => return Err(protocol("list name")),
        };
        let color = match record.field("Color") {
            None => None,
            Some(f) if f.ty == "STRING" => match &f.value {
                Value::Null => None,
                Value::String(s) => Some(s.clone()),
                _ => return Err(protocol("list color")),
            },
            Some(_) => return Err(protocol("list color")),
        };
        Ok(Some(ReminderListDetails {
            id: id.to_owned(),
            title,
            color,
            record_change_tag: record.record_change_tag.clone(),
        }))
    }

    /// Renames one list (Name and its merged resolution token only).
    pub async fn update_list(
        &self,
        id: &str,
        change_tag: &str,
        title: &str,
    ) -> Result<ReminderListDetails, RemindersError> {
        if crate::json::js_blank(title) || omni_core::js::utf16_len(title) > 4096 {
            return Err(fail("list title", ErrorCode::Invalid));
        }
        let current = self.require(id, "List", Some(change_tag)).await?;
        let now = self.clock.now_ms();
        let tokens =
            token_map(&current, &["name"], now).map_err(|_| protocol("resolution tokens"))?;
        let tags = self
            .modify(vec![json!({
                "operationType": "update",
                "record": {
                    "recordName": id,
                    "recordType": "List",
                    "recordChangeTag": change_tag,
                    "fields": {"Name": str_field(title), "ResolutionTokenMap": str_field(&tokens)},
                },
            })])
            .await?;
        match self.get_list(id).await? {
            Some(result)
                if result.title == title && result.record_change_tag.as_ref() == tags.get(id) =>
            {
                Ok(result)
            }
            _ => Err(protocol("list verification")),
        }
    }

    async fn context(
        &self,
        reminder_id: &str,
        change_tag: Option<&str>,
    ) -> Result<Context, RemindersError> {
        let reminder = self.require(reminder_id, "Reminder", change_tag).await?;
        let Some(list_id) = ref_name(&reminder, "List").filter(|l| !l.is_empty()) else {
            return Err(protocol("reminder list"));
        };
        let ids =
            linked_ids(&reminder).map_err(|_| fail("recurrence IDs", ErrorCode::Unsupported))?;
        let query = (self.query_list)(&list_id).await?;
        let records = query
            .iter()
            .map(crate::cloudkit::decode_lenient_record)
            .collect::<Option<Vec<_>>>()
            .ok_or_else(|| protocol("recurrence query"))?;
        let mut related: indexmap::IndexMap<String, CkRecord> = indexmap::IndexMap::new();
        for record in records {
            if record.has_error() {
                return Err(protocol("recurrence query"));
            }
            let recurrence =
                !record.gone() && record.record_type.as_deref().is_some_and(recurrence_like);
            if !recurrence {
                continue;
            }
            match ref_name(&record, "Reminder") {
                None => return Err(protocol("recurrence owner")),
                Some(owner) if owner.is_empty() => return Err(protocol("recurrence owner")),
                Some(owner) if owner == reminder_id => {
                    related.insert(record.record_name.clone(), record);
                }
                Some(_) => {}
            }
        }
        for id in &ids {
            let name = format!("RecurrenceRule/{id}");
            let record = self.require(&name, "RecurrenceRule", None).await?;
            if ref_name(&record, "Reminder").as_deref() != Some(reminder_id) {
                return Err(protocol("recurrence owner"));
            }
            related.insert(name, record);
        }
        if related.len() > 100 {
            return Err(protocol("recurrence limit"));
        }
        Ok(Context {
            reminder,
            ids,
            records: related.into_values().collect(),
        })
    }

    /// Exact rule ids, change tags, decoded details and writability.
    pub async fn get_recurrences(
        &self,
        reminder_id: &str,
    ) -> Result<ReminderRecurrences, RemindersError> {
        let context = self.context(reminder_id, None).await?;
        let mut rules = Vec::with_capacity(context.records.len());
        for record in &context.records {
            if record.record_type.as_deref() == Some("RecurrenceRule") {
                rules.push(
                    rule_details(record, reminder_id)
                        .map_err(|_| protocol("recurrence details"))?,
                );
            } else {
                rules.push(ReminderRecurrence {
                    id: record.record_name.clone(),
                    reminder_id: reminder_id.to_owned(),
                    record_change_tag: record.record_change_tag.clone(),
                    writable: false,
                    recurrence: RecurrenceDecoded::Unsupported(RecurrenceUnsupported {
                        supported: false,
                        reason: UnsupportedReason::UnknownFields,
                        fields: vec!["recordType".into()],
                    }),
                });
            }
        }
        let linked = context.ids.len() == 1
            && context.records.len() == 1
            && context.records[0].record_name == format!("RecurrenceRule/{}", context.ids[0])
            && !has_other_recurrence_keys(&context.reminder);
        Ok(ReminderRecurrences {
            reminder_id: reminder_id.to_owned(),
            reminder_change_tag: context
                .reminder
                .record_change_tag
                .clone()
                .unwrap_or_default(),
            rules: rules
                .into_iter()
                .map(|rule| ReminderRecurrence {
                    writable: rule.writable && linked,
                    ..rule
                })
                .collect(),
        })
    }

    fn link_fields(
        &self,
        reminder: &CkRecord,
        ids: &[String],
        now_ms: i64,
    ) -> Result<Value, RemindersError> {
        let tokens = token_map(reminder, &["recurrenceRuleIDs", "lastModifiedDate"], now_ms)
            .map_err(|_| protocol("recurrence link"))?;
        Ok(json!({
            "RecurrenceRuleIDs": {"type": "STRING_LIST", "value": ids},
            "LastModifiedDate": {"type": "TIMESTAMP", "value": now_ms},
            "ResolutionTokenMap": str_field(&tokens),
        }))
    }

    /// Atomically attaches one validated rule to a reminder without recurrence.
    pub async fn create_recurrence(
        &self,
        reminder_id: &str,
        reminder_tag: &str,
        rule_id: &str,
        input: &Value,
    ) -> Result<ReminderRecurrences, RemindersError> {
        let valid_id = rule_id.strip_prefix("RecurrenceRule/").is_some_and(|raw| {
            (8..=128).contains(&raw.len())
                && raw.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
        });
        if !valid_id {
            return Err(fail("rule ID", ErrorCode::Invalid));
        }
        let fields = recurrence_fields(input, None)?;
        let context = self.context(reminder_id, Some(reminder_tag)).await?;
        // Multiple/unknown recurrence relationships require their own proven semantics.
        if !context.ids.is_empty()
            || !context.records.is_empty()
            || has_other_recurrence_keys(&context.reminder)
        {
            return Err(fail("existing recurrence", ErrorCode::Unsupported));
        }
        if self.lookup(rule_id).await?.is_some() {
            return Err(fail("rule ID", ErrorCode::Conflict));
        }
        let links = self.link_fields(
            &context.reminder,
            &[raw_rule_id(rule_id).to_owned()],
            self.clock.now_ms(),
        )?;
        let mut rule_fields = fields.clone();
        rule_fields.insert("Reminder".into(), reference(reminder_id));
        rule_fields.insert("Deleted".into(), int(Some(0)));
        rule_fields.insert("Imported".into(), int(Some(0)));
        let tags = self
            .modify(vec![
                json!({
                    "operationType": "update",
                    "record": {
                        "recordName": reminder_id,
                        "recordType": "Reminder",
                        "recordChangeTag": reminder_tag,
                        "fields": links,
                    },
                }),
                json!({
                    "operationType": "create",
                    "record": {
                        "recordName": rule_id,
                        "recordType": "RecurrenceRule",
                        "parent": {"recordName": reminder_id},
                        "fields": rule_fields,
                    },
                }),
            ])
            .await?;
        self.verify_recurrence(reminder_id, rule_id, &tags, &fields, false)
            .await
    }

    async fn verify_recurrence(
        &self,
        reminder_id: &str,
        rule_id: &str,
        tags: &HashMap<String, String>,
        fields: &Map<String, Value>,
        removed: bool,
    ) -> Result<ReminderRecurrences, RemindersError> {
        let failed = || protocol("recurrence verification");
        let reminder = self
            .require(
                reminder_id,
                "Reminder",
                tags.get(reminder_id).map(String::as_str),
            )
            .await?;
        let ids = linked_ids(&reminder).map_err(|_| failed())?;
        if ids.iter().any(|id| id == raw_rule_id(rule_id)) == removed {
            return Err(failed());
        }
        let rule = self.lookup(rule_id).await?;
        if removed {
            if rule.is_some() {
                return Err(protocol("recurrence deletion verification"));
            }
        } else {
            let Some(rule) = rule else {
                return Err(failed());
            };
            let field_mismatch = fields.iter().any(|(name, expected)| {
                let actual = rule.field(name);
                let expected_value = expected.get("value").unwrap_or(&Value::Null);
                let value_differs = if expected_value.is_null() {
                    actual.is_some_and(|f| !f.value.is_null())
                } else {
                    actual.is_none_or(|f| {
                        omni_core::js::json_stringify(&f.value)
                            != omni_core::js::json_stringify(expected_value)
                    })
                };
                let type_differs = match (expected.get("type").and_then(Value::as_str), actual) {
                    (Some(ty), Some(f)) => f.ty != ty,
                    _ => false,
                };
                value_differs || type_differs
            });
            if rule.record_change_tag.as_ref() != tags.get(rule_id)
                || ref_name(&rule, "Reminder").as_deref() != Some(reminder_id)
                || field_mismatch
            {
                return Err(failed());
            }
        }
        let result = self.get_recurrences(reminder_id).await?;
        let ok = Some(&result.reminder_change_tag) == tags.get(reminder_id)
            && if removed {
                result.rules.is_empty()
            } else {
                result.rules.len() == 1
                    && result.rules[0].id == rule_id
                    && result.rules[0].record_change_tag.as_ref() == tags.get(rule_id)
            };
        if !ok {
            return Err(failed());
        }
        Ok(result)
    }

    /// Updates specified fields of the single supported rule (both tags required).
    pub async fn update_recurrence(
        &self,
        reminder_id: &str,
        reminder_tag: &str,
        rule_id: &str,
        rule_tag: &str,
        patch: &Map<String, Value>,
    ) -> Result<ReminderRecurrences, RemindersError> {
        let context = self.context(reminder_id, Some(reminder_tag)).await?;
        let current = self
            .require(rule_id, "RecurrenceRule", Some(rule_tag))
            .await?;
        let details =
            rule_details(&current, reminder_id).map_err(|_| protocol("recurrence owner"))?;
        if has_other_recurrence_keys(&context.reminder) {
            return Err(fail("recurrence parent", ErrorCode::Unsupported));
        }
        let Some(rule) = details.recurrence.rule().filter(|_| details.writable) else {
            return Err(fail("recurrence update", ErrorCode::Unsupported));
        };
        if context.records.len() != 1
            || context.records[0].record_name != rule_id
            || !context.ids.iter().any(|id| id == raw_rule_id(rule_id))
        {
            return Err(fail("recurrence update", ErrorCode::Unsupported));
        }
        let mut merged = rule.to_value().as_object().cloned().unwrap_or_default();
        for (key, value) in patch {
            merged.insert(key.clone(), value.clone());
        }
        let fields = recurrence_fields(&Value::Object(merged), current.fields.as_ref())?;
        if patch.is_empty() {
            return Err(fail("recurrence update", ErrorCode::Invalid));
        }
        let links = self.link_fields(&context.reminder, &context.ids, self.clock.now_ms())?;
        let tags = self
            .modify(vec![
                json!({
                    "operationType": "update",
                    "record": {
                        "recordName": reminder_id,
                        "recordType": "Reminder",
                        "recordChangeTag": reminder_tag,
                        "fields": links,
                    },
                }),
                json!({
                    "operationType": "update",
                    "record": {
                        "recordName": rule_id,
                        "recordType": "RecurrenceRule",
                        "recordChangeTag": rule_tag,
                        "fields": fields,
                    },
                }),
            ])
            .await?;
        self.verify_recurrence(reminder_id, rule_id, &tags, &fields, false)
            .await
    }

    /// Unlinks and soft-deletes the single supported rule, preserving the reminder.
    pub async fn remove_recurrence(
        &self,
        reminder_id: &str,
        reminder_tag: &str,
        rule_id: &str,
        rule_tag: &str,
    ) -> Result<ReminderRecurrences, RemindersError> {
        let context = self.context(reminder_id, Some(reminder_tag)).await?;
        let current = self
            .require(rule_id, "RecurrenceRule", Some(rule_tag))
            .await?;
        let details =
            rule_details(&current, reminder_id).map_err(|_| protocol("recurrence owner"))?;
        if has_other_recurrence_keys(&context.reminder) {
            return Err(fail("recurrence parent", ErrorCode::Unsupported));
        }
        let raw = raw_rule_id(rule_id);
        if !details.writable
            || context.records.len() != 1
            || context.records[0].record_name != rule_id
            || !context.ids.iter().any(|id| id == raw)
        {
            return Err(fail("recurrence removal", ErrorCode::Unsupported));
        }
        let remaining: Vec<String> = context
            .ids
            .iter()
            .filter(|id| *id != raw)
            .cloned()
            .collect();
        let links = self.link_fields(&context.reminder, &remaining, self.clock.now_ms())?;
        let tags = self
            .modify(vec![
                json!({
                    "operationType": "update",
                    "record": {
                        "recordName": reminder_id,
                        "recordType": "Reminder",
                        "recordChangeTag": reminder_tag,
                        "fields": links,
                    },
                }),
                json!({
                    "operationType": "update",
                    "record": {
                        "recordName": rule_id,
                        "recordType": "RecurrenceRule",
                        "recordChangeTag": rule_tag,
                        "fields": {"Deleted": int(Some(1)), "Reminder": reference(reminder_id)},
                    },
                }),
            ])
            .await?;
        self.verify_recurrence(reminder_id, rule_id, &tags, &Map::new(), true)
            .await
    }
}
