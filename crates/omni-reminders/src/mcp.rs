//! Server Reminders MCP tools.
//!
//! Bounded adapters over [`RemindersService`] under the existing MCP bearer
//! authentication. Metadata (names, schemas, annotations, policy) comes from the
//! golden tool list. Inputs are validated against the golden schema, then decoded the
//! way zod would (integral floats accepted as integers by `typed_tool`, unknown keys
//! stripped from nested `daysOfWeek` items). List creation and deletion are not exposed.

use std::future::Future;

use omni_mcp_kit::{McpTool, ToolContext, ToolError, ToolMetaError, paginate, typed_tool};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Map, Value, json};

use crate::cloudkit::{ReminderCreateFields, ReminderPatch};
use crate::recurrence::{DayOfWeek, Frequency, Opt};
use crate::recurring_completion::RecurringCompletionTarget;
use crate::service::RemindersService;

/// A typed tool over a handler that ignores the call context. `typed_tool`
/// validates against the golden schemas and reads integral floats as integers.
fn tool<I, F, Fut>(name: &str, f: F) -> Result<McpTool, ToolMetaError>
where
    I: DeserializeOwned + Send + 'static,
    F: Fn(I) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Result<Value, ToolError>> + Send + 'static,
{
    typed_tool(name, move |input: I, _cx: ToolContext| f(input))
}

fn execute(error: impl std::fmt::Display) -> ToolError {
    ToolError::execute(error.to_string())
}

fn to_value<T: serde::Serialize>(value: &T) -> Result<Value, ToolError> {
    serde_json::to_value(value).map_err(|e| ToolError::output(e.to_string()))
}

#[derive(Deserialize)]
struct Pagination {
    #[serde(default)]
    cursor: usize,
    #[serde(default = "default_limit")]
    limit: usize,
}

fn default_limit() -> usize {
    25
}

#[derive(Deserialize)]
struct IdInput {
    id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Target {
    idempotency_key: String,
    id: String,
    change_tag: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ListRename {
    #[serde(flatten)]
    target: Target,
    title: String,
}

/// `recurrenceValue` after zod parsing (nested unknown keys stripped).
#[derive(Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct RuleInput {
    frequency: Frequency,
    interval: i64,
    #[serde(default, skip_serializing_if = "Opt::is_absent")]
    occurrence_count: Opt<i64>,
    #[serde(default, skip_serializing_if = "Opt::is_absent")]
    first_day_of_week: Opt<i64>,
    #[serde(default, skip_serializing_if = "Opt::is_absent")]
    end_date: Opt<i64>,
    #[serde(default, skip_serializing_if = "Opt::is_absent")]
    days_of_week: Opt<Vec<DayOfWeek>>,
    #[serde(default, skip_serializing_if = "Opt::is_absent")]
    days_of_month: Opt<Vec<i64>>,
    #[serde(default, skip_serializing_if = "Opt::is_absent")]
    days_of_year: Opt<Vec<i64>>,
    #[serde(default, skip_serializing_if = "Opt::is_absent")]
    weeks_of_year: Opt<Vec<i64>>,
    #[serde(default, skip_serializing_if = "Opt::is_absent")]
    months_of_year: Opt<Vec<i64>>,
    #[serde(default, skip_serializing_if = "Opt::is_absent")]
    set_positions: Opt<Vec<i64>>,
}

/// `recurrenceValue.partial()`.
#[derive(Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct RulePatch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    frequency: Option<Frequency>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    interval: Option<i64>,
    #[serde(default, skip_serializing_if = "Opt::is_absent")]
    occurrence_count: Opt<i64>,
    #[serde(default, skip_serializing_if = "Opt::is_absent")]
    first_day_of_week: Opt<i64>,
    #[serde(default, skip_serializing_if = "Opt::is_absent")]
    end_date: Opt<i64>,
    #[serde(default, skip_serializing_if = "Opt::is_absent")]
    days_of_week: Opt<Vec<DayOfWeek>>,
    #[serde(default, skip_serializing_if = "Opt::is_absent")]
    days_of_month: Opt<Vec<i64>>,
    #[serde(default, skip_serializing_if = "Opt::is_absent")]
    days_of_year: Opt<Vec<i64>>,
    #[serde(default, skip_serializing_if = "Opt::is_absent")]
    weeks_of_year: Opt<Vec<i64>>,
    #[serde(default, skip_serializing_if = "Opt::is_absent")]
    months_of_year: Opt<Vec<i64>>,
    #[serde(default, skip_serializing_if = "Opt::is_absent")]
    set_positions: Opt<Vec<i64>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateRecurrence {
    #[serde(flatten)]
    target: Target,
    rule: RuleInput,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RuleTarget {
    #[serde(flatten)]
    target: Target,
    rule_id: String,
    rule_change_tag: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct UpdateRecurrence {
    #[serde(flatten)]
    rule: RuleTarget,
    patch: RulePatch,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CompleteRecurring {
    #[serde(flatten)]
    rule: RuleTarget,
    time_zone: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ListReminders {
    #[serde(flatten)]
    page: Pagination,
    list_id: Option<String>,
    query: Option<String>,
    completed: Option<bool>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateReminder {
    idempotency_key: String,
    #[serde(flatten)]
    fields: ReminderCreateFields,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct UpdateReminder {
    #[serde(flatten)]
    target: Target,
    patch: ReminderPatch,
}

fn object(value: Value) -> Map<String, Value> {
    match value {
        Value::Object(map) => map,
        _ => Map::new(),
    }
}

/// Every Reminders tool, in `src/mcp/tools/reminders.ts` order.
pub fn reminders_tools(service: RemindersService) -> Result<Vec<McpTool>, ToolMetaError> {
    let s = service;
    let mut tools = Vec::new();

    let svc = s.clone();
    tools.push(tool("list_reminder_lists", move |input: Pagination| {
        let svc = svc.clone();
        async move {
            let snapshot = svc.snapshot().await.map_err(execute)?;
            to_value(&paginate(snapshot.lists, input.cursor, input.limit))
        }
    })?);

    let svc = s.clone();
    tools.push(tool("get_reminder_list", move |input: IdInput| {
        let svc = svc.clone();
        async move {
            let list = svc.get_list(&input.id).await.map_err(execute)?;
            Ok(json!({"list": to_value(&list)?}))
        }
    })?);

    let svc = s.clone();
    tools.push(tool("update_reminder_list", move |input: ListRename| {
        let svc = svc.clone();
        async move {
            let t = input.target;
            let list = svc
                .update_list(&t.idempotency_key, &t.id, &t.change_tag, &input.title)
                .await
                .map_err(execute)?;
            Ok(json!({"list": list}))
        }
    })?);

    let svc = s.clone();
    tools.push(tool("get_reminder_recurrence", move |input: IdInput| {
        let svc = svc.clone();
        async move { to_value(&svc.get_recurrences(&input.id).await.map_err(execute)?) }
    })?);

    let svc = s.clone();
    tools.push(tool(
        "create_reminder_recurrence",
        move |input: CreateRecurrence| {
            let svc = svc.clone();
            async move {
                let t = input.target;
                let rule = to_value(&input.rule)?;
                svc.create_recurrence(&t.idempotency_key, &t.id, &t.change_tag, rule)
                    .await
                    .map_err(execute)
            }
        },
    )?);

    let svc = s.clone();
    tools.push(tool(
        "update_reminder_recurrence",
        move |input: UpdateRecurrence| {
            let svc = svc.clone();
            async move {
                let patch = object(to_value(&input.patch)?);
                if patch.is_empty() {
                    return Err(ToolError::input("patch: Provide a field to update"));
                }
                let r = input.rule;
                svc.update_recurrence(
                    &r.target.idempotency_key,
                    &r.target.id,
                    &r.target.change_tag,
                    &r.rule_id,
                    &r.rule_change_tag,
                    patch,
                )
                .await
                .map_err(execute)
            }
        },
    )?);

    let svc = s.clone();
    tools.push(tool(
        "remove_reminder_recurrence",
        move |input: RuleTarget| {
            let svc = svc.clone();
            async move {
                let t = input.target;
                svc.remove_recurrence(
                    &t.idempotency_key,
                    &t.id,
                    &t.change_tag,
                    &input.rule_id,
                    &input.rule_change_tag,
                )
                .await
                .map_err(execute)
            }
        },
    )?);

    let svc = s.clone();
    tools.push(tool(
        "complete_recurring_reminder",
        move |input: CompleteRecurring| {
            let svc = svc.clone();
            async move {
                let r = input.rule;
                svc.complete_recurring(
                    &r.target.idempotency_key,
                    RecurringCompletionTarget {
                        id: r.target.id,
                        change_tag: r.target.change_tag,
                        rule_id: r.rule_id,
                        rule_change_tag: r.rule_change_tag,
                        time_zone: input.time_zone,
                    },
                )
                .await
                .map_err(execute)
            }
        },
    )?);

    let svc = s.clone();
    tools.push(tool("list_reminders", move |input: ListReminders| {
        let svc = svc.clone();
        async move {
            let snapshot = svc.snapshot().await.map_err(execute)?;
            let query = input
                .query
                .filter(|q| !q.is_empty())
                .map(|q| q.to_lowercase());
            let list_id = input.list_id.filter(|l| !l.is_empty());
            let items: Vec<_> = snapshot
                .reminders
                .into_iter()
                .filter(|r| {
                    !r.deleted
                        && list_id.as_ref().is_none_or(|l| *l == r.list_id)
                        && input.completed.is_none_or(|c| c == r.completed)
                        && query.as_ref().is_none_or(|q| {
                            format!("{}\n{}", r.title, r.description)
                                .to_lowercase()
                                .contains(q.as_str())
                        })
                })
                .collect();
            to_value(&paginate(items, input.page.cursor, input.page.limit))
        }
    })?);

    let svc = s.clone();
    tools.push(tool("get_reminder", move |input: IdInput| {
        let svc = svc.clone();
        async move {
            let reminder = svc.get(&input.id).await.map_err(execute)?;
            Ok(json!({"reminder": to_value(&reminder)?}))
        }
    })?);

    let svc = s.clone();
    tools.push(tool("create_reminder", move |input: CreateReminder| {
        let svc = svc.clone();
        async move {
            let reminder = svc
                .create(&input.idempotency_key, input.fields)
                .await
                .map_err(execute)?;
            Ok(json!({"reminder": reminder}))
        }
    })?);

    let svc = s.clone();
    tools.push(tool("update_reminder", move |input: UpdateReminder| {
        let svc = svc.clone();
        async move {
            if input.patch.is_empty() {
                return Err(ToolError::input("patch: Provide a field to update"));
            }
            let t = input.target;
            let reminder = svc
                .update(&t.idempotency_key, &t.id, &t.change_tag, input.patch)
                .await
                .map_err(execute)?;
            Ok(json!({"reminder": reminder}))
        }
    })?);

    for (name, completed) in [("complete_reminder", true), ("reopen_reminder", false)] {
        let svc = s.clone();
        tools.push(tool(name, move |input: Target| {
            let svc = svc.clone();
            async move {
                let patch = ReminderPatch {
                    completed: Some(completed),
                    ..ReminderPatch::default()
                };
                let reminder = svc
                    .update(&input.idempotency_key, &input.id, &input.change_tag, patch)
                    .await
                    .map_err(execute)?;
                Ok(json!({"reminder": reminder}))
            }
        })?);
    }

    let svc = s;
    tools.push(tool("delete_reminder", move |input: Target| {
        let svc = svc.clone();
        async move {
            svc.delete(&input.idempotency_key, &input.id, &input.change_tag)
                .await
                .map_err(execute)
        }
    })?);

    Ok(tools)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_like_zod_for_ledger_fingerprints() {
        let input: CreateReminder =
            serde_json::from_value(omni_core::js::normalize_numbers(json!({
                "idempotencyKey": "fixture-key-123456",
                "listId": "List/a",
                "title": "Ä b",
                "dueDate": null,
                "priority": 5.0,
                "allDay": true,
                "description": "x\u{2028}y",
            })))
            .unwrap();
        let mut fields = object(serde_json::to_value(&input.fields).unwrap());
        fields.insert("operation".into(), json!("create"));
        // The same input fingerprinted by the TypeScript service (tests/golden/codec.json).
        assert_eq!(
            crate::service::fingerprint(&Value::Object(fields)),
            "c7b3ba23ce6571ec76e1bb32f661e7a3828d3a4abc1c8cd70c440e0a1ec4e5db"
        );
        let rule: RuleInput = serde_json::from_value(json!({
            "frequency": "monthly",
            "interval": 1,
            "daysOfWeek": [{"dayOfTheWeek": 2, "weekNumber": -1, "stripped": true}],
            "endDate": null,
        }))
        .unwrap();
        assert_eq!(
            serde_json::to_value(&rule).unwrap(),
            json!({
                "frequency": "monthly",
                "interval": 1,
                "endDate": null,
                "daysOfWeek": [{"dayOfTheWeek": 2, "weekNumber": -1}],
            })
        );
    }
}
