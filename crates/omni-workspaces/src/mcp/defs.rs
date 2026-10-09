//! The workspace tools' contracts: metadata, policy and the schema types
//! their input and output schemas derive from (`omni_mcp_kit::schema`). These
//! types describe the wire format only; the handlers decode and encode with
//! their own types. After changing anything here run `cargo xtask mcp-golden`
//! and review the snapshot diff.

use omni_mcp_kit::schema::{Lit, Literal, positive, uuid};
use omni_mcp_kit::{Annotations, ExecutorPolicy, Policy, ToolDef, ToolDefinition, ToolInfo};
use schemars::JsonSchema;

pub static WORKSPACES_LIST: ToolDef<WorkspacesListInput, WorkspacesListOutput> = ToolDef::new(
    ToolInfo {
        name: "workspaces_list",
        title: "List Workspaces",
        description: "List Omni's durable personal workspaces with compact definitions and current subject, approval, and papercut counts. Internal agent prompts are excluded.",
        annotations: Annotations {
            read_only_hint: true,
            destructive_hint: false,
            idempotent_hint: true,
            open_world_hint: false,
        },
        policy: Policy {
            side_effects: &[],
            cost: "none",
            recommended: ExecutorPolicy::Allow,
        },
    },
);

pub static WORKSPACE_GET: ToolDef<WorkspaceGetInput, WorkspaceGetOutput> = ToolDef::new(ToolInfo {
    name: "workspace_get",
    title: "Get Workspace",
    description: "Get a workspace overview or one subject dossier. Subject results include bounded current artifacts, messages, sources, actions, email scope, and open papercuts.",
    annotations: Annotations {
        read_only_hint: true,
        destructive_hint: false,
        idempotent_hint: true,
        open_world_hint: false,
    },
    policy: Policy {
        side_effects: &[],
        cost: "none",
        recommended: ExecutorPolicy::Allow,
    },
});

pub static WORKSPACE_SEARCH: ToolDef<WorkspaceSearchInput, WorkspaceSearchOutput> = ToolDef::new(
    ToolInfo {
        name: "workspace_search",
        title: "Search Workspaces",
        description: "Search subject titles and summaries, current artifacts, recent messages, and recent sources across one or all workspaces. Results contain bounded snippets.",
        annotations: Annotations {
            read_only_hint: true,
            destructive_hint: false,
            idempotent_hint: true,
            open_world_hint: false,
        },
        policy: Policy {
            side_effects: &[],
            cost: "none",
            recommended: ExecutorPolicy::Allow,
        },
    },
);

pub static WORKSPACE_MESSAGE: ToolDef<WorkspaceMessageInput, WorkspaceMessageOutput> = ToolDef::new(
    ToolInfo {
        name: "workspace_message",
        title: "Message Workspace",
        description: "Send a user message to a workspace agent, optionally continuing an existing subject. This queues paid model and web-research work and may create reviewable action proposals, but does not approve them.",
        annotations: Annotations {
            read_only_hint: false,
            destructive_hint: false,
            idempotent_hint: false,
            open_world_hint: true,
        },
        policy: Policy {
            side_effects: &[
                "Queues a workspace agent run",
                "Persists the message and resulting dossier revisions, sources, and proposals",
            ],
            cost: "variable paid model and web-search cost",
            recommended: ExecutorPolicy::RequireApproval,
        },
    },
);

pub static WORKSPACE_SUBJECT_SET_STATUS: ToolDef<
    WorkspaceSubjectSetStatusInput,
    WorkspaceSubjectSetStatusOutput,
> = ToolDef::new(ToolInfo {
    name: "workspace_subject_set_status",
    title: "Set Workspace Subject Status",
    description: "Set a workspace subject to active, paused, completed, or archived. This is a local preference/state change and does not run the workspace agent.",
    annotations: Annotations {
        read_only_hint: false,
        destructive_hint: false,
        idempotent_hint: true,
        open_world_hint: false,
    },
    policy: Policy {
        side_effects: &["Updates local workspace subject state"],
        cost: "none",
        recommended: ExecutorPolicy::Allow,
    },
});

pub static WORKSPACE_ACTIONS_LIST: ToolDef<WorkspaceActionsListInput, WorkspaceActionsListOutput> =
    ToolDef::new(ToolInfo {
        name: "workspace_actions_list",
        title: "List Workspace Actions",
        description: "List reviewable workspace action proposals, optionally filtered by workspace, subject, or status. Payloads are parsed into typed JSON-compatible values.",
        annotations: Annotations {
            read_only_hint: true,
            destructive_hint: false,
            idempotent_hint: true,
            open_world_hint: false,
        },
        policy: Policy {
            side_effects: &[],
            cost: "none",
            recommended: ExecutorPolicy::Allow,
        },
    });

pub static WORKSPACE_ACTION_APPROVE: ToolDef<
    WorkspaceActionApproveInput,
    WorkspaceActionApproveOutput,
> = ToolDef::new(ToolInfo {
    name: "workspace_action_approve",
    title: "Approve Workspace Action",
    description: "Approve and execute one pending or failed workspace proposal. Email-scope approvals broaden local email ingestion; calendar-event approvals write to the configured external CalDAV calendar. Executor approval is required.",
    annotations: Annotations {
        read_only_hint: false,
        destructive_hint: false,
        idempotent_hint: true,
        open_world_hint: true,
    },
    policy: Policy {
        side_effects: &[
            "Approves a durable workspace proposal",
            "May broaden email ingestion scope or create an external calendar event",
        ],
        cost: "no direct monetary cost; may perform a CalDAV network request",
        recommended: ExecutorPolicy::RequireApproval,
    },
});

pub static WORKSPACE_ACTION_REJECT: ToolDef<
    WorkspaceActionRejectInput,
    WorkspaceActionRejectOutput,
> = ToolDef::new(ToolInfo {
    name: "workspace_action_reject",
    title: "Reject Workspace Action",
    description: "Reject one pending workspace proposal without performing its proposed external effect.",
    annotations: Annotations {
        read_only_hint: false,
        destructive_hint: false,
        idempotent_hint: true,
        open_world_hint: false,
    },
    policy: Policy {
        side_effects: &["Marks a pending local action proposal rejected"],
        cost: "none",
        recommended: ExecutorPolicy::Allow,
    },
});

pub static WORKSPACE_PAPERCUTS_LIST: ToolDef<
    WorkspacePapercutsListInput,
    WorkspacePapercutsListOutput,
> = ToolDef::new(ToolInfo {
    name: "workspace_papercuts_list",
    title: "List Workspace Papercuts",
    description: "List structured workspace friction reports, optionally filtered by workspace and resolution status. Internal deduplication fingerprints are excluded.",
    annotations: Annotations {
        read_only_hint: true,
        destructive_hint: false,
        idempotent_hint: true,
        open_world_hint: false,
    },
    policy: Policy {
        side_effects: &[],
        cost: "none",
        recommended: ExecutorPolicy::Allow,
    },
});

pub static WORKSPACE_PAPERCUT_RESOLVE: ToolDef<
    WorkspacePapercutResolveInput,
    WorkspacePapercutResolveOutput,
> = ToolDef::new(ToolInfo {
    name: "workspace_papercut_resolve",
    title: "Resolve Workspace Papercut",
    description: "Mark one workspace papercut addressed or dismissed with a durable local resolution note.",
    annotations: Annotations {
        read_only_hint: false,
        destructive_hint: false,
        idempotent_hint: true,
        open_world_hint: false,
    },
    policy: Policy {
        side_effects: &["Updates a local papercut's resolution state"],
        cost: "none",
        recommended: ExecutorPolicy::Allow,
    },
});

/// Every workspace tool, in serving order.
pub static TOOLS: [&dyn ToolDefinition; 10] = [
    &WORKSPACES_LIST,
    &WORKSPACE_GET,
    &WORKSPACE_SEARCH,
    &WORKSPACE_MESSAGE,
    &WORKSPACE_SUBJECT_SET_STATUS,
    &WORKSPACE_ACTIONS_LIST,
    &WORKSPACE_ACTION_APPROVE,
    &WORKSPACE_ACTION_REJECT,
    &WORKSPACE_PAPERCUTS_LIST,
    &WORKSPACE_PAPERCUT_RESOLVE,
];

#[derive(JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct WorkspacesListInput {}

#[derive(JsonSchema)]
#[schemars(rename_all = "kebab-case")]
pub enum Kind {
    Markdown,
    Structured,
    EvidenceLedger,
    Timeline,
    Collection,
}

#[derive(JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct Artifact {
    pub key: String,
    pub title: String,
    pub kind: Kind,
}

#[derive(JsonSchema)]
#[schemars(rename_all = "camelCase", deny_unknown_fields)]
pub struct Workspace {
    pub id: String,
    pub title: String,
    pub description: String,
    pub subject_label: String,
    pub subject_label_plural: String,
    pub scheduled_runs: bool,
    pub artifacts: Vec<Artifact>,
    pub active_subject_count: u64,
    pub pending_action_count: u64,
    pub open_papercut_count: u64,
}

#[derive(JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct WorkspacesListOutput {
    pub workspaces: Vec<Workspace>,
}

#[derive(JsonSchema)]
#[schemars(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkspaceGetInput {
    #[schemars(length(min = 1, max = 100))]
    pub workspace_id: String,
    #[schemars(length(min = 1, max = 200))]
    pub subject_id: Option<String>,
    #[schemars(range(min = 1, max = 100), extend("default" = 30))]
    pub message_limit: Option<u64>,
    #[schemars(range(min = 1, max = 100), extend("default" = 30))]
    pub source_limit: Option<u64>,
    #[schemars(range(min = 1, max = 100), extend("default" = 30))]
    pub revision_limit: Option<u64>,
    #[schemars(range(min = 1, max = 100), extend("default" = 30))]
    pub action_limit: Option<u64>,
    #[schemars(range(min = 200, max = 10000), extend("default" = 4000))]
    pub max_content_chars: Option<u64>,
}

#[derive(JsonSchema)]
#[schemars(rename_all = "lowercase")]
pub enum Status {
    Active,
    Paused,
    Completed,
    Archived,
}

#[derive(JsonSchema)]
#[schemars(rename_all = "camelCase", deny_unknown_fields)]
pub struct Subject {
    pub workspace_id: String,
    pub subject_id: String,
    pub title: String,
    pub status: Status,
    pub summary: String,
    pub created_at: f64,
    pub updated_at: f64,
    #[schemars(skip_serializing_if = "Option::is_none")]
    pub last_researched_at: Option<f64>,
}

#[derive(JsonSchema)]
#[schemars(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkspaceGetArtifact {
    pub revision_id: String,
    pub workspace_id: String,
    pub subject_id: String,
    pub artifact_key: String,
    pub kind: Kind,
    pub content: String,
    pub content_truncated: bool,
    pub summary: String,
    pub created_at: f64,
    pub run_id: Option<String>,
}

#[derive(JsonSchema)]
#[schemars(rename_all = "lowercase")]
pub enum Role {
    User,
    Assistant,
    System,
}

#[derive(JsonSchema)]
#[schemars(rename_all = "camelCase", deny_unknown_fields)]
pub struct Message {
    pub message_id: String,
    pub workspace_id: String,
    pub subject_id: Option<String>,
    pub role: Role,
    pub text: String,
    pub text_truncated: bool,
    pub created_at: f64,
    pub run_id: Option<String>,
}

#[derive(JsonSchema)]
#[schemars(rename_all = "lowercase")]
pub enum SourceKind {
    Web,
    Email,
}

#[derive(JsonSchema)]
#[schemars(rename_all = "camelCase", deny_unknown_fields)]
pub struct Source {
    pub source_id: String,
    pub workspace_id: String,
    pub subject_id: String,
    pub kind: SourceKind,
    pub title: String,
    pub url: Option<String>,
    pub excerpt: String,
    pub excerpt_truncated: bool,
    pub email_id: Option<String>,
    pub created_at: f64,
    pub run_id: Option<String>,
}

#[derive(JsonSchema)]
#[schemars(rename_all = "snake_case")]
pub enum Type {
    EmailScope,
    CalendarEvent,
}

#[derive(JsonSchema)]
#[schemars(rename_all = "lowercase")]
pub enum ActionStatus {
    Pending,
    Approved,
    Rejected,
    Failed,
}

#[derive(JsonSchema)]
#[schemars(rename_all = "camelCase", deny_unknown_fields)]
pub struct PayloadSenders {
    pub senders: Vec<String>,
    pub domains: Vec<String>,
    pub subject_keywords: Vec<String>,
    pub body_keywords: Vec<String>,
}

#[derive(JsonSchema)]
#[schemars(rename_all = "camelCase", deny_unknown_fields)]
pub struct PayloadTitle {
    pub title: String,
    pub start_date: String,
    #[schemars(skip_serializing_if = "Option::is_none")]
    pub end_date: Option<String>,
    #[schemars(skip_serializing_if = "Option::is_none")]
    pub start_time: Option<String>,
    #[schemars(skip_serializing_if = "Option::is_none")]
    pub end_time: Option<String>,
    #[schemars(skip_serializing_if = "Option::is_none")]
    pub location: Option<String>,
    #[schemars(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[schemars(skip_serializing_if = "Option::is_none")]
    pub time_zone: Option<String>,
    pub all_day: bool,
    #[schemars(skip_serializing_if = "Option::is_none")]
    pub reminder_minutes: Option<f64>,
}

#[derive(JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct PayloadUnavailable {
    #[schemars(transform = Literal("Stored action payload is invalid"))]
    pub unavailable: Lit,
}

#[derive(JsonSchema)]
#[schemars(untagged)]
pub enum Payload {
    Senders(PayloadSenders),
    Title(PayloadTitle),
    Unavailable(PayloadUnavailable),
}

#[derive(JsonSchema)]
#[schemars(rename_all = "camelCase", deny_unknown_fields)]
pub struct Action {
    pub action_id: String,
    pub workspace_id: String,
    pub subject_id: String,
    pub r#type: Type,
    pub status: ActionStatus,
    pub title: String,
    pub description: String,
    pub payload: Payload,
    pub created_at: f64,
    #[schemars(skip_serializing_if = "Option::is_none")]
    pub resolved_at: Option<f64>,
    #[schemars(skip_serializing_if = "Option::is_none")]
    pub result: Option<String>,
    pub run_id: Option<String>,
}

#[derive(JsonSchema)]
#[schemars(rename_all = "camelCase", deny_unknown_fields)]
pub struct EmailScope {
    pub senders: Vec<String>,
    pub domains: Vec<String>,
    pub subject_keywords: Vec<String>,
    pub body_keywords: Vec<String>,
    pub updated_at: f64,
}

#[derive(JsonSchema)]
#[schemars(rename_all = "kebab-case")]
pub enum Category {
    MissingCapability,
    PoorSourceData,
    IntegrationFriction,
    WorkflowGap,
    PromptProblem,
    UiGap,
}

#[derive(JsonSchema)]
#[schemars(rename_all = "lowercase")]
pub enum PapercutStatus {
    Open,
    Addressed,
    Dismissed,
}

#[derive(JsonSchema)]
#[schemars(rename_all = "camelCase", deny_unknown_fields)]
pub struct Papercut {
    pub papercut_id: String,
    pub workspace_id: String,
    pub subject_id: Option<String>,
    pub run_id: Option<String>,
    pub category: Category,
    pub title: String,
    pub detail: String,
    pub related_tool: Option<String>,
    #[schemars(transform = positive)]
    pub occurrences: u64,
    pub first_seen_at: f64,
    pub last_seen_at: f64,
    pub status: PapercutStatus,
    #[schemars(skip_serializing_if = "Option::is_none")]
    pub resolution: Option<String>,
}

#[derive(JsonSchema)]
#[schemars(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkspaceGetOutput {
    pub workspace: Workspace,
    pub subjects: Vec<Subject>,
    pub subjects_truncated: bool,
    pub subject: Option<Subject>,
    pub artifacts: Vec<WorkspaceGetArtifact>,
    pub artifact_revisions: Vec<WorkspaceGetArtifact>,
    pub messages: Vec<Message>,
    pub sources: Vec<Source>,
    pub actions: Vec<Action>,
    pub email_scope: Option<EmailScope>,
    pub papercuts: Vec<Papercut>,
    pub papercuts_truncated: bool,
}

#[derive(JsonSchema)]
#[schemars(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkspaceSearchInput {
    #[schemars(length(min = 2, max = 300))]
    pub query: String,
    #[schemars(length(min = 1, max = 100))]
    pub workspace_id: Option<String>,
    #[schemars(range(max = 4999), extend("default" = 0))]
    pub cursor: Option<u64>,
    #[schemars(range(min = 1, max = 100), extend("default" = 25))]
    pub limit: Option<u64>,
    #[schemars(range(min = 100, max = 1000), extend("default" = 400))]
    pub max_snippet_chars: Option<u64>,
}

#[derive(JsonSchema)]
#[schemars(rename_all = "lowercase")]
pub enum ResourceType {
    Subject,
    Artifact,
    Message,
    Source,
}

#[derive(JsonSchema)]
#[schemars(rename_all = "camelCase", deny_unknown_fields)]
pub struct Matche {
    pub workspace_id: String,
    pub subject_id: String,
    pub resource_type: ResourceType,
    pub resource_id: String,
    pub title: String,
    pub snippet: String,
    pub updated_at: f64,
}

#[derive(JsonSchema)]
#[schemars(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkspaceSearchOutput {
    pub matches: Vec<Matche>,
    pub next_cursor: Option<u64>,
    pub total: u64,
}

#[derive(JsonSchema)]
#[schemars(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkspaceMessageInput {
    #[schemars(length(min = 1, max = 100))]
    pub workspace_id: String,
    #[schemars(length(min = 1, max = 200))]
    pub subject_id: Option<String>,
    #[schemars(length(min = 1, max = 20000))]
    pub message: String,
}

#[derive(JsonSchema)]
#[schemars(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkspaceMessageOutput {
    pub workspace_id: String,
    pub subject_id: Option<String>,
    pub run_id: String,
    #[schemars(transform = Literal(true))]
    pub queued: Lit,
}

#[derive(JsonSchema)]
#[schemars(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkspaceSubjectSetStatusInput {
    #[schemars(length(min = 1, max = 100))]
    pub workspace_id: String,
    #[schemars(length(min = 1, max = 200))]
    pub subject_id: String,
    pub status: Status,
}

#[derive(JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct WorkspaceSubjectSetStatusOutput {
    pub subject: Subject,
}

#[derive(JsonSchema)]
#[schemars(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkspaceActionsListInput {
    #[schemars(length(min = 1, max = 100))]
    pub workspace_id: Option<String>,
    #[schemars(length(min = 1, max = 200))]
    pub subject_id: Option<String>,
    pub status: Option<ActionStatus>,
    #[schemars(range(max = 4999), extend("default" = 0))]
    pub cursor: Option<u64>,
    #[schemars(range(min = 1, max = 100), extend("default" = 25))]
    pub limit: Option<u64>,
}

#[derive(JsonSchema)]
#[schemars(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkspaceActionsListOutput {
    pub actions: Vec<Action>,
    pub next_cursor: Option<u64>,
    pub total: u64,
}

#[derive(JsonSchema)]
#[schemars(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkspaceActionApproveInput {
    #[schemars(transform = uuid)]
    pub action_id: String,
}

#[derive(JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct WorkspaceActionApproveOutput {
    pub action: Action,
}

pub type WorkspaceActionRejectInput = WorkspaceActionApproveInput;

pub type WorkspaceActionRejectOutput = WorkspaceActionApproveOutput;

#[derive(JsonSchema)]
#[schemars(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkspacePapercutsListInput {
    #[schemars(length(min = 1, max = 100))]
    pub workspace_id: Option<String>,
    pub status: Option<PapercutStatus>,
    #[schemars(range(max = 4999), extend("default" = 0))]
    pub cursor: Option<u64>,
    #[schemars(range(min = 1, max = 100), extend("default" = 25))]
    pub limit: Option<u64>,
}

#[derive(JsonSchema)]
#[schemars(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkspacePapercutsListOutput {
    pub papercuts: Vec<Papercut>,
    pub next_cursor: Option<u64>,
    pub total: u64,
}

#[derive(JsonSchema)]
#[schemars(rename_all = "lowercase")]
pub enum WorkspacePapercutResolveStatus {
    Addressed,
    Dismissed,
}

#[derive(JsonSchema)]
#[schemars(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkspacePapercutResolveInput {
    #[schemars(transform = uuid)]
    pub papercut_id: String,
    pub status: WorkspacePapercutResolveStatus,
    #[schemars(length(min = 1, max = 2000))]
    pub resolution: String,
}

#[derive(JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct WorkspacePapercutResolveOutput {
    pub papercut: Papercut,
}
