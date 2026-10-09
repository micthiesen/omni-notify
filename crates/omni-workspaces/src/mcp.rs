//! Workspace MCP tools, in serving order. Each tool's contract (descriptions, schemas,
//! annotations, policy) is declared in [`defs`].

pub mod defs;

use std::future::Future;

use omni_api::workspaces::{
    WorkspaceActionStatus, WorkspaceActionType, WorkspaceArtifactKind, WorkspaceDefinition,
    WorkspaceEmailScope, WorkspaceMessageRole, WorkspacePapercutCategory,
    WorkspacePapercutResolution, WorkspacePapercutStatus, WorkspaceSourceKind, WorkspaceSubject,
    WorkspaceSubjectStatus,
};
use omni_mcp_kit::{
    McpTool, ToolContext, ToolDef, ToolError, ToolMetaError, paginate, truncate_utf16, typed_tool,
};
use omni_runtime::ports::CalendarEventInput;
use omni_tasks::RunNowError;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::entities::{ActionRow, ArtifactRevisionRow, PapercutRow, SubjectRow};
use crate::persistence::SubjectUpsert;
use crate::service::WorkspaceService;
use crate::text::{js_len, js_prefix, js_trim};

/// Every workspace tool in `createWorkspaceTools` order.
pub fn tools(service: &WorkspaceService) -> Result<Vec<McpTool>, ToolMetaError> {
    Ok(vec![
        tool(service, &defs::WORKSPACES_LIST, workspaces_list)?,
        tool(service, &defs::WORKSPACE_GET, workspace_get)?,
        tool(service, &defs::WORKSPACE_SEARCH, workspace_search)?,
        tool(service, &defs::WORKSPACE_MESSAGE, workspace_message)?,
        tool(
            service,
            &defs::WORKSPACE_SUBJECT_SET_STATUS,
            workspace_subject_set_status,
        )?,
        tool(
            service,
            &defs::WORKSPACE_ACTIONS_LIST,
            workspace_actions_list,
        )?,
        tool(
            service,
            &defs::WORKSPACE_ACTION_APPROVE,
            workspace_action_approve,
        )?,
        tool(
            service,
            &defs::WORKSPACE_ACTION_REJECT,
            workspace_action_reject,
        )?,
        tool(
            service,
            &defs::WORKSPACE_PAPERCUTS_LIST,
            workspace_papercuts_list,
        )?,
        tool(
            service,
            &defs::WORKSPACE_PAPERCUT_RESOLVE,
            workspace_papercut_resolve,
        )?,
    ])
}

fn tool<S, R, I, O, F, Fut>(
    service: &WorkspaceService,
    def: &'static ToolDef<S, R>,
    f: F,
) -> Result<McpTool, ToolMetaError>
where
    S: JsonSchema,
    R: JsonSchema,
    I: for<'de> Deserialize<'de> + Send + 'static,
    O: Serialize + Send + 'static,
    F: Fn(WorkspaceService, I) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Result<O, ToolError>> + Send + 'static,
{
    let service = service.clone();
    typed_tool(def, move |input: I, _cx: ToolContext| {
        f(service.clone(), input)
    })
}

fn execute(error: impl std::error::Error) -> ToolError {
    ToolError::execute_from(&error)
}

/// Trims `value`: the schema checks the raw length, the trimmed value
/// must still satisfy the minimum.
fn trimmed(field: &str, value: &str, min: usize) -> Result<String, ToolError> {
    let value = js_trim(value);
    if js_len(value) < min {
        return Err(ToolError::input(format!(
            "{field}: Too small: expected string to have >={min} characters"
        )));
    }
    Ok(value.to_owned())
}

fn trimmed_opt(
    field: &str,
    value: Option<String>,
    min: usize,
) -> Result<Option<String>, ToolError> {
    value.map(|v| trimmed(field, &v, min)).transpose()
}

fn require_workspace<'a>(
    service: &'a WorkspaceService,
    workspace_id: &str,
) -> Result<&'a WorkspaceDefinition, ToolError> {
    service
        .definition(workspace_id)
        .ok_or_else(|| ToolError::execute(format!("Unknown workspace \"{workspace_id}\"")))
}

async fn require_subject(
    service: &WorkspaceService,
    workspace_id: &str,
    subject_id: &str,
) -> Result<SubjectRow, ToolError> {
    require_workspace(service, workspace_id)?;
    service
        .repo()
        .get_subject(workspace_id, subject_id)
        .await
        .map_err(execute)?
        .ok_or_else(|| {
            ToolError::execute(format!(
                "Unknown subject \"{subject_id}\" in workspace \"{workspace_id}\""
            ))
        })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ArtifactSummary {
    key: String,
    title: String,
    kind: WorkspaceArtifactKind,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct WorkspaceSummary {
    id: String,
    title: String,
    description: String,
    subject_label: String,
    subject_label_plural: String,
    scheduled_runs: bool,
    artifacts: Vec<ArtifactSummary>,
    active_subject_count: usize,
    pending_action_count: usize,
    open_papercut_count: usize,
}

async fn summarize(
    service: &WorkspaceService,
    definition: &WorkspaceDefinition,
) -> Result<WorkspaceSummary, ToolError> {
    let repo = service.repo();
    let subjects = repo.list_subjects(&definition.id).await.map_err(execute)?;
    let pending = repo
        .list_actions(&definition.id, None)
        .await
        .map_err(execute)?
        .iter()
        .filter(|a| a.status == WorkspaceActionStatus::Pending)
        .count();
    let open = repo
        .list_papercuts(Some(&definition.id), Some(WorkspacePapercutStatus::Open))
        .await
        .map_err(execute)?
        .len();
    Ok(WorkspaceSummary {
        id: definition.id.clone(),
        title: definition.title.clone(),
        description: definition.description.clone(),
        subject_label: definition.subject_label.clone(),
        subject_label_plural: definition.subject_label_plural.clone(),
        scheduled_runs: crate::definitions::scheduled_runs(definition),
        artifacts: definition
            .artifacts
            .iter()
            .map(|a| ArtifactSummary {
                key: a.key.clone(),
                title: a.title.clone(),
                kind: a.kind,
            })
            .collect(),
        active_subject_count: subjects
            .iter()
            .filter(|s| s.status == WorkspaceSubjectStatus::Active)
            .count(),
        pending_action_count: pending,
        open_papercut_count: open,
    })
}

/// A stored action payload as typed JSON (excess keys dropped), or a marker.
pub fn parse_action_payload(payload: &str) -> Value {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Payload {
        Email(WorkspaceEmailScope),
        Calendar(CalendarEventInput),
    }
    let parsed = serde_json::from_str::<Value>(payload)
        .ok()
        .and_then(|v| serde_json::from_value::<Payload>(v).ok());
    let value = match parsed {
        Some(Payload::Email(scope)) => serde_json::to_value(scope),
        Some(Payload::Calendar(event)) => serde_json::to_value(event),
        None => return json!({ "unavailable": "Stored action payload is invalid" }),
    };
    value.unwrap_or_else(|_| json!({ "unavailable": "Stored action payload is invalid" }))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ActionOut {
    action_id: String,
    workspace_id: String,
    subject_id: String,
    #[serde(rename = "type")]
    action_type: WorkspaceActionType,
    status: WorkspaceActionStatus,
    title: String,
    description: String,
    payload: Value,
    created_at: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    resolved_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<String>,
    run_id: Option<String>,
}

fn action_out(action: &ActionRow) -> ActionOut {
    ActionOut {
        action_id: action.action_id.clone(),
        workspace_id: action.workspace_id.clone(),
        subject_id: action.subject_id.clone(),
        action_type: action.action_type,
        status: action.status,
        title: action.title.clone(),
        description: action.description.clone(),
        payload: parse_action_payload(&action.payload),
        created_at: action.created_at,
        resolved_at: action.resolved_at,
        result: action.result.clone(),
        run_id: action.run_id.clone(),
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PapercutOut {
    papercut_id: String,
    workspace_id: String,
    subject_id: Option<String>,
    run_id: Option<String>,
    category: WorkspacePapercutCategory,
    title: String,
    detail: String,
    related_tool: Option<String>,
    occurrences: i64,
    first_seen_at: i64,
    last_seen_at: i64,
    status: WorkspacePapercutStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    resolution: Option<String>,
}

/// Papercuts without their internal deduplication fingerprint.
fn papercut_out(p: &PapercutRow) -> PapercutOut {
    PapercutOut {
        papercut_id: p.papercut_id.clone(),
        workspace_id: p.workspace_id.clone(),
        subject_id: p.subject_id.clone(),
        run_id: p.run_id.clone(),
        category: p.category,
        title: p.title.clone(),
        detail: p.detail.clone(),
        related_tool: p.related_tool.clone(),
        occurrences: p.occurrences,
        first_seen_at: p.first_seen_at,
        last_seen_at: p.last_seen_at,
        status: p.status,
        resolution: p.resolution.clone(),
    }
}

#[derive(Deserialize)]
struct EmptyInput {}

#[derive(Serialize)]
struct WorkspacesListOut {
    workspaces: Vec<WorkspaceSummary>,
}

async fn workspaces_list(
    service: WorkspaceService,
    _input: EmptyInput,
) -> Result<WorkspacesListOut, ToolError> {
    let mut workspaces = Vec::new();
    for definition in service.definitions() {
        workspaces.push(summarize(&service, definition).await?);
    }
    Ok(WorkspacesListOut { workspaces })
}

fn default_30() -> usize {
    30
}

fn default_4000() -> usize {
    4_000
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WorkspaceGetInput {
    workspace_id: String,
    #[serde(default)]
    subject_id: Option<String>,
    #[serde(default = "default_30")]
    message_limit: usize,
    #[serde(default = "default_30")]
    source_limit: usize,
    #[serde(default = "default_30")]
    revision_limit: usize,
    #[serde(default = "default_30")]
    action_limit: usize,
    #[serde(default = "default_4000")]
    max_content_chars: usize,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ArtifactOut {
    revision_id: String,
    workspace_id: String,
    subject_id: String,
    artifact_key: String,
    kind: WorkspaceArtifactKind,
    content: String,
    content_truncated: bool,
    summary: String,
    created_at: i64,
    run_id: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct MessageOut {
    message_id: String,
    workspace_id: String,
    subject_id: Option<String>,
    role: WorkspaceMessageRole,
    text: String,
    text_truncated: bool,
    created_at: i64,
    run_id: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SourceOut {
    source_id: String,
    workspace_id: String,
    subject_id: String,
    kind: WorkspaceSourceKind,
    title: String,
    url: Option<String>,
    excerpt: String,
    excerpt_truncated: bool,
    email_id: Option<String>,
    created_at: i64,
    run_id: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct EmailScopeOut {
    #[serde(flatten)]
    scope: WorkspaceEmailScope,
    updated_at: i64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct WorkspaceGetOut {
    workspace: WorkspaceSummary,
    subjects: Vec<WorkspaceSubject>,
    subjects_truncated: bool,
    subject: Option<WorkspaceSubject>,
    artifacts: Vec<ArtifactOut>,
    artifact_revisions: Vec<ArtifactOut>,
    messages: Vec<MessageOut>,
    sources: Vec<SourceOut>,
    actions: Vec<ActionOut>,
    email_scope: Option<EmailScopeOut>,
    papercuts: Vec<PapercutOut>,
    papercuts_truncated: bool,
}

fn artifact_out(item: &ArtifactRevisionRow, max: usize) -> ArtifactOut {
    let (content, content_truncated) = truncate_utf16(&item.content, max);
    ArtifactOut {
        revision_id: item.revision_id.clone(),
        workspace_id: item.workspace_id.clone(),
        subject_id: item.subject_id.clone(),
        artifact_key: item.artifact_key.clone(),
        kind: item.kind,
        content,
        content_truncated,
        summary: item.summary.clone(),
        created_at: item.created_at,
        run_id: item.run_id.clone(),
    }
}

async fn workspace_get(
    service: WorkspaceService,
    input: WorkspaceGetInput,
) -> Result<WorkspaceGetOut, ToolError> {
    let workspace_id = trimmed("workspaceId", &input.workspace_id, 1)?;
    let subject_id = trimmed_opt("subjectId", input.subject_id, 1)?;
    let max = input.max_content_chars;
    let definition = require_workspace(&service, &workspace_id)?.clone();
    let subject = match &subject_id {
        Some(subject_id) => Some(require_subject(&service, &workspace_id, subject_id).await?),
        None => None,
    };
    let repo = service.repo();
    let (mut messages, mut sources, mut artifacts, mut artifact_revisions) =
        (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    let mut email_scope = None;
    if let Some(subject_id) = &subject_id {
        messages = repo
            .list_messages(&workspace_id, Some(subject_id), input.message_limit)
            .await
            .map_err(execute)?
            .iter()
            .map(|m| {
                let (text, text_truncated) = truncate_utf16(&m.text, max);
                MessageOut {
                    message_id: m.message_id.clone(),
                    workspace_id: m.workspace_id.clone(),
                    subject_id: m.subject_id.clone(),
                    role: m.role,
                    text,
                    text_truncated,
                    created_at: m.created_at,
                    run_id: m.run_id.clone(),
                }
            })
            .collect();
        sources = repo
            .list_sources(&workspace_id, subject_id, input.source_limit)
            .await
            .map_err(execute)?
            .iter()
            .map(|s| {
                let (excerpt, excerpt_truncated) = truncate_utf16(&s.excerpt, max);
                SourceOut {
                    source_id: s.source_id.clone(),
                    workspace_id: s.workspace_id.clone(),
                    subject_id: s.subject_id.clone(),
                    kind: s.kind,
                    title: s.title.clone(),
                    url: s.url.clone(),
                    excerpt,
                    excerpt_truncated,
                    email_id: s.email_id.clone(),
                    created_at: s.created_at,
                    run_id: s.run_id.clone(),
                }
            })
            .collect();
        email_scope = repo
            .get_email_scope(&workspace_id, subject_id)
            .await
            .map_err(execute)?
            .map(|s| EmailScopeOut {
                scope: s.scope(),
                updated_at: s.updated_at,
            });
        artifacts = repo
            .latest_artifacts(&workspace_id, subject_id)
            .await
            .map_err(execute)?
            .iter()
            .map(|a| artifact_out(a, max))
            .collect();
        artifact_revisions = repo
            .list_artifact_revisions(&workspace_id, subject_id, None)
            .await
            .map_err(execute)?
            .iter()
            .take(input.revision_limit)
            .map(|a| artifact_out(a, max))
            .collect();
    }
    let subjects = repo.list_subjects(&workspace_id).await.map_err(execute)?;
    let papercuts: Vec<PapercutRow> = repo
        .list_papercuts(Some(&workspace_id), Some(WorkspacePapercutStatus::Open))
        .await
        .map_err(execute)?
        .into_iter()
        .filter(|p| match (&subject_id, p.subject_id.as_deref()) {
            (None, _) | (_, None | Some("")) => true,
            (Some(wanted), Some(own)) => own == wanted,
        })
        .collect();
    let actions = repo
        .list_actions(&workspace_id, subject_id.as_deref())
        .await
        .map_err(execute)?
        .iter()
        .take(input.action_limit)
        .map(action_out)
        .collect();
    Ok(WorkspaceGetOut {
        workspace: summarize(&service, &definition).await?,
        subjects_truncated: subjects.len() > 100,
        subjects: subjects.iter().take(100).map(SubjectRow::view).collect(),
        subject: subject.map(|s| s.view()),
        artifacts,
        artifact_revisions,
        messages,
        sources,
        actions,
        email_scope,
        papercuts_truncated: papercuts.len() > 100,
        papercuts: papercuts.iter().take(100).map(papercut_out).collect(),
    })
}

fn default_cursor() -> usize {
    0
}

fn default_25() -> usize {
    25
}

fn default_400() -> usize {
    400
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SearchInput {
    query: String,
    #[serde(default)]
    workspace_id: Option<String>,
    #[serde(default = "default_cursor")]
    cursor: usize,
    #[serde(default = "default_25")]
    limit: usize,
    #[serde(default = "default_400")]
    max_snippet_chars: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
enum ResourceType {
    Subject,
    Artifact,
    Message,
    Source,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SearchMatch {
    workspace_id: String,
    subject_id: String,
    resource_type: ResourceType,
    resource_id: String,
    title: String,
    snippet: String,
    updated_at: i64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SearchOut {
    matches: Vec<SearchMatch>,
    next_cursor: Option<usize>,
    total: usize,
}

fn utf16_lower(s: &str) -> Vec<u16> {
    s.to_lowercase().encode_utf16().collect()
}

fn find_u16(haystack: &[u16], needle: &[u16]) -> Option<usize> {
    if needle.is_empty() {
        return Some(0);
    }
    haystack.windows(needle.len()).position(|w| w == needle)
}

/// The window around the first case-insensitive match.
pub fn search_snippet(value: &str, query: &str, max_chars: usize) -> String {
    let length = js_len(value);
    if length <= max_chars {
        return value.to_owned();
    }
    let Some(index) = find_u16(&utf16_lower(value), &utf16_lower(query)) else {
        return js_prefix(value, max_chars);
    };
    let half = (i64::try_from(max_chars).unwrap_or(i64::MAX)
        - i64::try_from(js_len(query)).unwrap_or(i64::MAX))
    .div_euclid(2);
    let start = usize::try_from((i64::try_from(index).unwrap_or(0) - half).max(0)).unwrap_or(0);
    let end = length.min(start + max_chars);
    format!(
        "{}{}{}",
        if start > 0 { "…" } else { "" },
        omni_core::js::utf16_slice(value, start, end),
        if end < length { "…" } else { "" }
    )
}

fn contains_ci(haystack: &str, query_lower: &[u16]) -> bool {
    find_u16(&utf16_lower(haystack), query_lower).is_some()
}

async fn workspace_search(
    service: WorkspaceService,
    input: SearchInput,
) -> Result<SearchOut, ToolError> {
    let query = trimmed("query", &input.query, 2)?;
    let workspace_id = trimmed_opt("workspaceId", input.workspace_id, 1)?;
    let max = input.max_snippet_chars;
    let definitions: Vec<WorkspaceDefinition> = match &workspace_id {
        Some(id) => vec![require_workspace(&service, id)?.clone()],
        None => service.definitions().to_vec(),
    };
    let query_lower = utf16_lower(&query);
    let repo = service.repo();
    let mut matches = Vec::new();
    let mut add = |hit: SearchMatch, haystack: &str| {
        if contains_ci(haystack, &query_lower) {
            matches.push(SearchMatch {
                snippet: search_snippet(haystack, &query, max),
                ..hit
            });
        }
    };
    for definition in &definitions {
        for subject in repo.list_subjects(&definition.id).await.map_err(execute)? {
            let sid = subject.subject_id.clone();
            add(
                SearchMatch {
                    workspace_id: definition.id.clone(),
                    subject_id: sid.clone(),
                    resource_type: ResourceType::Subject,
                    resource_id: sid.clone(),
                    title: subject.title.clone(),
                    snippet: String::new(),
                    updated_at: subject.updated_at,
                },
                &format!("{}\n{}", subject.title, subject.summary),
            );
            for artifact in repo
                .latest_artifacts(&definition.id, &sid)
                .await
                .map_err(execute)?
            {
                add(
                    SearchMatch {
                        workspace_id: definition.id.clone(),
                        subject_id: sid.clone(),
                        resource_type: ResourceType::Artifact,
                        resource_id: artifact.revision_id.clone(),
                        title: artifact.artifact_key.clone(),
                        snippet: String::new(),
                        updated_at: artifact.created_at,
                    },
                    &format!("{}\n{}", artifact.summary, artifact.content),
                );
            }
            for message in repo
                .list_messages(&definition.id, Some(&sid), 100)
                .await
                .map_err(execute)?
            {
                add(
                    SearchMatch {
                        workspace_id: definition.id.clone(),
                        subject_id: sid.clone(),
                        resource_type: ResourceType::Message,
                        resource_id: message.message_id.clone(),
                        title: format!("{} message", message.role.as_str()),
                        snippet: String::new(),
                        updated_at: message.created_at,
                    },
                    &message.text,
                );
            }
            for source in repo
                .list_sources(&definition.id, &sid, 100)
                .await
                .map_err(execute)?
            {
                add(
                    SearchMatch {
                        workspace_id: definition.id.clone(),
                        subject_id: sid.clone(),
                        resource_type: ResourceType::Source,
                        resource_id: source.source_id.clone(),
                        title: source.title.clone(),
                        snippet: String::new(),
                        updated_at: source.created_at,
                    },
                    &format!("{}\n{}", source.title, source.excerpt),
                );
            }
        }
    }
    matches.sort_by_key(|row| std::cmp::Reverse(row.updated_at));
    let page = paginate(matches, input.cursor, input.limit);
    Ok(SearchOut {
        matches: page.items,
        next_cursor: page.next_cursor,
        total: page.total,
    })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct MessageInput {
    workspace_id: String,
    #[serde(default)]
    subject_id: Option<String>,
    message: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct MessageQueued {
    workspace_id: String,
    subject_id: Option<String>,
    run_id: String,
    queued: bool,
}

fn run_now_message(error: RunNowError) -> ToolError {
    ToolError::execute(match error {
        error @ (RunNowError::NotFound { .. }
        | RunNowError::AlreadyRunning { .. }
        | RunNowError::ManualInputUnsupported { .. }) => error.to_string(),
        other => omni_core::error::chain_message(&other),
    })
}

async fn workspace_message(
    service: WorkspaceService,
    input: MessageInput,
) -> Result<MessageQueued, ToolError> {
    let workspace_id = trimmed("workspaceId", &input.workspace_id, 1)?;
    let subject_id = trimmed_opt("subjectId", input.subject_id, 1)?;
    let message = trimmed("message", &input.message, 1)?;
    let task_name = require_workspace(&service, &workspace_id)?
        .task_name
        .clone();
    if let Some(subject_id) = &subject_id {
        require_subject(&service, &workspace_id, subject_id).await?;
    }
    let mut run_input = json!({ "message": message });
    if let Some(subject_id) = &subject_id {
        run_input["subjectId"] = Value::String(subject_id.clone());
    }
    let run_id = service
        .tasks()
        .run_now(&task_name, Some(run_input))
        .map_err(run_now_message)?;
    Ok(MessageQueued {
        workspace_id,
        subject_id,
        run_id,
        queued: true,
    })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SetStatusInput {
    workspace_id: String,
    subject_id: String,
    status: WorkspaceSubjectStatus,
}

#[derive(Serialize)]
struct SubjectOut {
    subject: WorkspaceSubject,
}

/// Keeps the subject's stored `updatedAt`.
async fn workspace_subject_set_status(
    service: WorkspaceService,
    input: SetStatusInput,
) -> Result<SubjectOut, ToolError> {
    let workspace_id = trimmed("workspaceId", &input.workspace_id, 1)?;
    let subject_id = trimmed("subjectId", &input.subject_id, 1)?;
    let subject = require_subject(&service, &workspace_id, &subject_id).await?;
    let updated = service
        .repo()
        .upsert_subject(SubjectUpsert {
            workspace_id: subject.workspace_id,
            subject_id: subject.subject_id,
            title: subject.title,
            status: input.status,
            summary: subject.summary,
            created_at: Some(subject.created_at),
            updated_at: Some(subject.updated_at),
            last_researched_at: subject.last_researched_at,
        })
        .await
        .map_err(execute)?;
    Ok(SubjectOut {
        subject: updated.view(),
    })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ActionsListInput {
    #[serde(default)]
    workspace_id: Option<String>,
    #[serde(default)]
    subject_id: Option<String>,
    #[serde(default)]
    status: Option<WorkspaceActionStatus>,
    #[serde(default = "default_cursor")]
    cursor: usize,
    #[serde(default = "default_25")]
    limit: usize,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ActionsOut {
    actions: Vec<ActionOut>,
    next_cursor: Option<usize>,
    total: usize,
}

async fn workspace_actions_list(
    service: WorkspaceService,
    input: ActionsListInput,
) -> Result<ActionsOut, ToolError> {
    let workspace_id = trimmed_opt("workspaceId", input.workspace_id, 1)?;
    let subject_id = trimmed_opt("subjectId", input.subject_id, 1)?;
    if subject_id.is_some() && workspace_id.is_none() {
        return Err(ToolError::execute(
            "workspaceId is required when subjectId is provided",
        ));
    }
    let definitions: Vec<WorkspaceDefinition> = match &workspace_id {
        Some(id) => vec![require_workspace(&service, id)?.clone()],
        None => service.definitions().to_vec(),
    };
    let mut values = Vec::new();
    for definition in &definitions {
        values.extend(
            service
                .repo()
                .list_actions(&definition.id, subject_id.as_deref())
                .await
                .map_err(execute)?,
        );
    }
    values.retain(|a| input.status.is_none_or(|s| a.status == s));
    values.sort_by_key(|row| std::cmp::Reverse(row.created_at));
    let page = paginate(values, input.cursor, input.limit);
    Ok(ActionsOut {
        actions: page.items.iter().map(action_out).collect(),
        next_cursor: page.next_cursor,
        total: page.total,
    })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ActionIdInput {
    action_id: String,
}

#[derive(Serialize)]
struct ActionResult {
    action: ActionOut,
}

/// Executor approval is required by policy; this is the user's authorization.
async fn workspace_action_approve(
    service: WorkspaceService,
    input: ActionIdInput,
) -> Result<ActionResult, ToolError> {
    let action = service
        .approve_action(&input.action_id)
        .await
        .map_err(execute)?;
    Ok(ActionResult {
        action: action_out(&action),
    })
}

async fn workspace_action_reject(
    service: WorkspaceService,
    input: ActionIdInput,
) -> Result<ActionResult, ToolError> {
    let action = service
        .reject_action(&input.action_id)
        .await
        .map_err(execute)?;
    Ok(ActionResult {
        action: action_out(&action),
    })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PapercutsListInput {
    #[serde(default)]
    workspace_id: Option<String>,
    #[serde(default)]
    status: Option<WorkspacePapercutStatus>,
    #[serde(default = "default_cursor")]
    cursor: usize,
    #[serde(default = "default_25")]
    limit: usize,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PapercutsOut {
    papercuts: Vec<PapercutOut>,
    next_cursor: Option<usize>,
    total: usize,
}

async fn workspace_papercuts_list(
    service: WorkspaceService,
    input: PapercutsListInput,
) -> Result<PapercutsOut, ToolError> {
    let workspace_id = trimmed_opt("workspaceId", input.workspace_id, 1)?;
    if let Some(id) = &workspace_id {
        require_workspace(&service, id)?;
    }
    let rows = service
        .repo()
        .list_papercuts(workspace_id.as_deref(), input.status)
        .await
        .map_err(execute)?;
    let page = paginate(rows, input.cursor, input.limit);
    Ok(PapercutsOut {
        papercuts: page.items.iter().map(papercut_out).collect(),
        next_cursor: page.next_cursor,
        total: page.total,
    })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ResolveInput {
    papercut_id: String,
    status: WorkspacePapercutResolution,
    resolution: String,
}

#[derive(Serialize)]
struct PapercutResult {
    papercut: PapercutOut,
}

async fn workspace_papercut_resolve(
    service: WorkspaceService,
    input: ResolveInput,
) -> Result<PapercutResult, ToolError> {
    let resolution = trimmed("resolution", &input.resolution, 1)?;
    let papercut = service
        .repo()
        .resolve_papercut(&input.papercut_id, input.status.into(), &resolution)
        .await
        .map_err(execute)?
        .ok_or_else(|| {
            ToolError::execute(format!(
                "Unknown workspace papercut \"{}\"",
                input.papercut_id
            ))
        })?;
    Ok(PapercutResult {
        papercut: papercut_out(&papercut),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snippets_center_on_the_match() {
        let value = format!("{}needle{}", "a".repeat(200), "b".repeat(200));
        let snippet = search_snippet(&value, "NEEDLE", 100);
        assert!(snippet.starts_with('…') && snippet.ends_with('…'));
        assert!(snippet.contains("needle"));
        assert_eq!(js_len(&snippet), 102);
        assert_eq!(search_snippet("short", "x", 100), "short");
        assert_eq!(search_snippet(&"c".repeat(150), "zz", 100), "c".repeat(100));
    }

    #[test]
    fn payloads_parse_to_typed_json() {
        assert_eq!(
            parse_action_payload(
                r#"{"senders":["a@b"],"domains":[],"subjectKeywords":[],"bodyKeywords":[],"x":1}"#
            ),
            json!({"senders":["a@b"],"domains":[],"subjectKeywords":[],"bodyKeywords":[]})
        );
        assert_eq!(
            parse_action_payload(r#"{"title":"T","startDate":"2026-09-01","allDay":true}"#),
            json!({"title":"T","startDate":"2026-09-01","allDay":true})
        );
        assert_eq!(
            parse_action_payload("not json"),
            json!({"unavailable": "Stored action payload is invalid"})
        );
    }
}
