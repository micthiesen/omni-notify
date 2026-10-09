//! The calendar tools' contracts: metadata, policy and the schema types
//! their input and output schemas derive from (`omni_mcp_kit::schema`). These
//! types describe the wire format only; the handlers decode and encode with
//! their own types. After changing anything here run `cargo xtask mcp-golden`
//! and review the snapshot diff.

use omni_mcp_kit::schema::{Lit, Literal, nullable};
use omni_mcp_kit::{Annotations, ExecutorPolicy, Policy, ToolDef, ToolDefinition, ToolInfo};
use schemars::JsonSchema;

pub static CALENDAR_EVENTS_LIST: ToolDef<CalendarEventsListInput, CalendarEventsListOutput> =
    ToolDef::new(ToolInfo {
        name: "calendar_events_list",
        title: "List Tracked Calendar Events",
        description: "List Omni's locally tracked CalDAV events with bounded filtering and pagination. This does not enumerate unrelated events directly from the remote calendar.",
        annotations: Annotations {
            read_only_hint: true,
            destructive_hint: false,
            idempotent_hint: true,
            open_world_hint: false,
        },
        policy: Policy {
            side_effects: &[],
            cost: "None",
            recommended: ExecutorPolicy::Allow,
        },
    });

pub static CALENDAR_EVENT_GET: ToolDef<CalendarEventGetInput, CalendarEventGetOutput> =
    ToolDef::new(ToolInfo {
        name: "calendar_event_get",
        title: "Get Tracked Calendar Event",
        description: "Get one Omni-tracked calendar event by eventHash, including its stable CalDAV UID and local status.",
        annotations: Annotations {
            read_only_hint: true,
            destructive_hint: false,
            idempotent_hint: true,
            open_world_hint: false,
        },
        policy: Policy {
            side_effects: &[],
            cost: "None",
            recommended: ExecutorPolicy::Allow,
        },
    });

pub static CALENDAR_STATUS: ToolDef<CalendarStatusInput, CalendarStatusOutput> = ToolDef::new(
    ToolInfo {
        name: "calendar_status",
        title: "Inspect Tracked Calendar Status",
        description: "Report the configured CalDAV provider and local tracked-event counts without contacting the provider or revealing calendar URLs or credentials.",
        annotations: Annotations {
            read_only_hint: true,
            destructive_hint: false,
            idempotent_hint: true,
            open_world_hint: false,
        },
        policy: Policy {
            side_effects: &[],
            cost: "None",
            recommended: ExecutorPolicy::Allow,
        },
    },
);

pub static CALENDAR_EVENT_PREVIEW: ToolDef<CalendarEventPreviewInput, CalendarEventPreviewOutput> =
    ToolDef::new(ToolInfo {
        name: "calendar_event_preview",
        title: "Preview Calendar Event",
        description: "Validate a proposed event and render the exact bounded iCalendar payload Omni would write. No local or remote state changes.",
        annotations: Annotations {
            read_only_hint: true,
            destructive_hint: false,
            idempotent_hint: true,
            open_world_hint: false,
        },
        policy: Policy {
            side_effects: &[],
            cost: "None",
            recommended: ExecutorPolicy::Allow,
        },
    });

pub static CALENDAR_EVENT_CREATE: ToolDef<CalendarEventCreateInput, CalendarEventCreateOutput> =
    ToolDef::new(ToolInfo {
        name: "calendar_event_create",
        title: "Create Calendar Event",
        description: "Create and locally track an event in the configured CalDAV calendar. Content-hash deduplication makes identical repeated calls no-ops. This external calendar mutation requires approval.",
        annotations: Annotations {
            read_only_hint: false,
            destructive_hint: false,
            idempotent_hint: true,
            open_world_hint: true,
        },
        policy: Policy {
            side_effects: &[
                "Creates an event in the configured external CalDAV calendar",
                "Writes a local tracked-event record",
            ],
            cost: "No paid API expected; one CalDAV discovery/write sequence",
            recommended: ExecutorPolicy::RequireApproval,
        },
    });

pub static CALENDAR_EVENT_UPDATE: ToolDef<CalendarEventUpdateInput, CalendarEventUpdateOutput> =
    ToolDef::new(ToolInfo {
        name: "calendar_event_update",
        title: "Update Calendar Event",
        description: "Patch one active Omni-tracked event and overwrite its external CalDAV representation. Null clears an optional field. This consequential external mutation requires approval.",
        annotations: Annotations {
            read_only_hint: false,
            destructive_hint: true,
            idempotent_hint: true,
            open_world_hint: true,
        },
        policy: Policy {
            side_effects: &[
                "Overwrites an event in the configured external CalDAV calendar",
                "Updates local tracked-event identity and content",
            ],
            cost: "No paid API expected; one CalDAV discovery/write sequence when changed",
            recommended: ExecutorPolicy::RequireApproval,
        },
    });

pub static CALENDAR_EVENT_DELETE: ToolDef<CalendarEventDeleteInput, CalendarEventDeleteOutput> =
    ToolDef::new(ToolInfo {
        name: "calendar_event_delete",
        title: "Delete Calendar Event",
        description: "Delete one Omni-tracked event from the external CalDAV calendar and retain a cancelled local tombstone to prevent accidental recreation. Requires approval.",
        annotations: Annotations {
            read_only_hint: false,
            destructive_hint: true,
            idempotent_hint: true,
            open_world_hint: true,
        },
        policy: Policy {
            side_effects: &[
                "Deletes an external CalDAV event",
                "Marks the local tracked event cancelled",
            ],
            cost: "No paid API expected; one CalDAV discovery/delete sequence",
            recommended: ExecutorPolicy::RequireApproval,
        },
    });

/// Every calendar tool, in serving order.
pub static TOOLS: [&dyn ToolDefinition; 7] = [
    &CALENDAR_EVENTS_LIST,
    &CALENDAR_EVENT_GET,
    &CALENDAR_STATUS,
    &CALENDAR_EVENT_PREVIEW,
    &CALENDAR_EVENT_CREATE,
    &CALENDAR_EVENT_UPDATE,
    &CALENDAR_EVENT_DELETE,
];

#[derive(JsonSchema)]
#[schemars(rename_all = "lowercase")]
pub enum Status {
    Active,
    Cancelled,
    All,
}

#[derive(JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct CalendarEventsListInput {
    #[schemars(description = "Zero-based result offset", extend("default" = 0))]
    pub cursor: Option<u64>,
    #[schemars(range(min = 1, max = 100), extend("default" = 25))]
    pub limit: Option<u64>,
    #[schemars(length(min = 1, max = 200))]
    pub query: Option<String>,
    #[schemars(pattern(r#"^\d{4}-\d{2}-\d{2}$"#))]
    pub from: Option<String>,
    #[schemars(pattern(r#"^\d{4}-\d{2}-\d{2}$"#))]
    pub through: Option<String>,
    #[schemars(extend("default" = "active"))]
    pub status: Option<Status>,
}

#[derive(JsonSchema)]
#[schemars(rename_all = "lowercase")]
pub enum Frequency {
    Daily,
    Weekly,
    Monthly,
}

#[derive(JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct Recurrence {
    pub frequency: Frequency,
    #[schemars(pattern(r#"^\d{4}-\d{2}-\d{2}$"#))]
    pub until: String,
}

#[derive(JsonSchema)]
#[schemars(rename_all = "lowercase")]
pub enum CalendarEventsListItemStatus {
    Active,
    Cancelled,
}

#[derive(JsonSchema)]
#[schemars(rename_all = "camelCase", deny_unknown_fields)]
pub struct CalendarEventsListItem {
    pub event_hash: String,
    pub calendar_event_id: String,
    pub source_email_id: String,
    pub title: String,
    pub start_date: String,
    pub start_time: Option<String>,
    pub end_date: Option<String>,
    pub end_time: Option<String>,
    pub all_day: bool,
    pub location: Option<String>,
    pub time_zone: Option<String>,
    pub description: Option<String>,
    pub duration: Option<String>,
    pub reminder_minutes: Option<f64>,
    pub recurrence: Option<Recurrence>,
    pub created_at: f64,
    pub status: CalendarEventsListItemStatus,
}

#[derive(JsonSchema)]
#[schemars(rename_all = "camelCase", deny_unknown_fields)]
pub struct CalendarEventsListOutput {
    pub items: Vec<CalendarEventsListItem>,
    pub next_cursor: Option<f64>,
    pub total: f64,
}

#[derive(JsonSchema)]
#[schemars(rename_all = "camelCase", deny_unknown_fields)]
pub struct CalendarEventGetInput {
    #[schemars(length(min = 1, max = 1000))]
    pub event_hash: String,
}

#[derive(JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct CalendarEventGetOutput {
    pub event: CalendarEventsListItem,
}

#[derive(JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct CalendarStatusInput {}

#[derive(JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct Tracked {
    pub active: f64,
    pub cancelled: f64,
    pub total: f64,
}

#[derive(JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct CalendarStatusOutput {
    pub configured: bool,
    #[schemars(transform = Literal("icloud"), transform = nullable)]
    pub provider: Option<Lit>,
    pub tracked: Tracked,
}

#[derive(JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct EventRecurrence {
    pub frequency: Frequency,
    #[schemars(pattern(r#"^\d{4}-\d{2}-\d{2}$"#))]
    pub until: String,
}

#[derive(JsonSchema)]
#[schemars(rename_all = "camelCase", deny_unknown_fields)]
pub struct Event {
    #[schemars(length(min = 1, max = 200))]
    pub title: String,
    #[schemars(pattern(r#"^\d{4}-\d{2}-\d{2}$"#))]
    pub start_date: String,
    #[schemars(pattern(r#"^([01]\d|2[0-3]):[0-5]\d$"#))]
    pub start_time: Option<String>,
    #[schemars(pattern(r#"^\d{4}-\d{2}-\d{2}$"#))]
    pub end_date: Option<String>,
    #[schemars(pattern(r#"^([01]\d|2[0-3]):[0-5]\d$"#))]
    pub end_time: Option<String>,
    pub all_day: bool,
    #[schemars(length(min = 1, max = 300))]
    pub location: Option<String>,
    #[schemars(length(max = 100))]
    pub time_zone: Option<String>,
    #[schemars(length(max = 2000))]
    pub description: Option<String>,
    #[schemars(pattern(r#"^P(?=\d|T\d)(?:\d+D)?(?:T(?:\d+H)?(?:\d+M)?(?:\d+S)?)?$"#))]
    pub duration: Option<String>,
    #[schemars(range(max = 40320))]
    pub reminder_minutes: Option<u64>,
    pub recurrence: Option<EventRecurrence>,
}

#[derive(JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct CalendarEventPreviewInput {
    pub event: Event,
}

#[derive(JsonSchema)]
#[schemars(rename_all = "camelCase", deny_unknown_fields)]
pub struct CalendarEventPreviewOutput {
    pub event_hash: String,
    pub duplicate_tracked_event: bool,
    pub i_calendar: String,
}

pub type CalendarEventCreateInput = CalendarEventPreviewInput;

#[derive(JsonSchema)]
#[schemars(rename_all = "snake_case")]
pub enum CalendarEventCreateStatus {
    Created,
    AlreadyExists,
    Reconciled,
}

#[derive(JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct CalendarEventCreateOutput {
    pub status: CalendarEventCreateStatus,
    pub event: CalendarEventsListItem,
}

#[derive(JsonSchema)]
#[schemars(rename_all = "camelCase", deny_unknown_fields)]
pub struct Changes {
    #[schemars(length(min = 1, max = 200))]
    pub title: Option<String>,
    #[schemars(pattern(r#"^\d{4}-\d{2}-\d{2}$"#))]
    pub start_date: Option<String>,
    #[schemars(pattern(r#"^([01]\d|2[0-3]):[0-5]\d$"#), transform = nullable)]
    pub start_time: Option<String>,
    #[schemars(pattern(r#"^\d{4}-\d{2}-\d{2}$"#), transform = nullable)]
    pub end_date: Option<String>,
    #[schemars(pattern(r#"^([01]\d|2[0-3]):[0-5]\d$"#), transform = nullable)]
    pub end_time: Option<String>,
    pub all_day: Option<bool>,
    #[schemars(length(min = 1, max = 300), transform = nullable)]
    pub location: Option<String>,
    #[schemars(length(max = 100), transform = nullable)]
    pub time_zone: Option<String>,
    #[schemars(length(max = 2000), transform = nullable)]
    pub description: Option<String>,
    #[schemars(pattern(r#"^P(?=\d|T\d)(?:\d+D)?(?:T(?:\d+H)?(?:\d+M)?(?:\d+S)?)?$"#), transform = nullable)]
    pub duration: Option<String>,
    #[schemars(range(max = 40320), transform = nullable)]
    pub reminder_minutes: Option<u64>,
    #[schemars(transform = nullable)]
    pub recurrence: Option<EventRecurrence>,
}

#[derive(JsonSchema)]
#[schemars(rename_all = "camelCase", deny_unknown_fields)]
pub struct CalendarEventUpdateInput {
    #[schemars(length(min = 1, max = 1000))]
    pub event_hash: String,
    pub changes: Changes,
}

#[derive(JsonSchema)]
#[schemars(rename_all = "lowercase")]
pub enum CalendarEventUpdateStatus {
    Updated,
    Unchanged,
}

#[derive(JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct CalendarEventUpdateOutput {
    pub status: CalendarEventUpdateStatus,
    pub event: CalendarEventsListItem,
}

pub type CalendarEventDeleteInput = CalendarEventGetInput;

#[derive(JsonSchema)]
#[schemars(rename_all = "snake_case")]
pub enum CalendarEventDeleteStatus {
    Deleted,
    AlreadyDeleted,
}

#[derive(JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct CalendarEventDeleteOutput {
    pub status: CalendarEventDeleteStatus,
    pub event: CalendarEventsListItem,
}
